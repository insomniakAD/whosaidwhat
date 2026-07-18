//! macOS audio capture backend: microphone (cpal) + system audio
//! (ScreenCaptureKit), written as two separate tracks.
//!
//! Design decisions (evidence in docs/00-architecture.md §Capture):
//!
//! - **Two tracks, not one mix.** The mic track carries the local speaker; the
//!   system track carries everyone remote. Keeping them separate makes
//!   diarization strictly easier (the local user never needs clustering — they
//!   ARE the mic track) and avoids echo double-capture: when the user is on
//!   speakers, the mic picks up remote voices that the system tap also
//!   records; with separate tracks the transcriber can prefer the clean
//!   system track for remote speech instead of needing AEC.
//!   This mirrors what Hyprnote ships (its mic input explicitly filters out
//!   its own system-audio tap device) and what Meetily's 48 kHz pipeline does.
//!
//! - **ScreenCaptureKit for system audio (v1).** SCK delivers 48 kHz system
//!   audio on macOS 13+ under the Screen Recording TCC grant and has the only
//!   maintained safe Rust binding (`screencapturekit` crate). Core Audio
//!   process taps (macOS 14.4+, `CATapDescription`) are the better long-term
//!   path — per-process capture and a quieter "System Audio Recording Only"
//!   permission — but Rust bindings for taps were unverified at build time;
//!   the seam here (`SystemAudioSource`) is where that swap lands.
//!
//! - **WAV containers, finalized on stop.** Recordings must survive crashes;
//!   `hound` writers are flushed per buffer and the header finalized in
//!   `stop()`. 48 kHz f32 as captured; the pipeline downmixes/resamples to
//!   16 kHz mono for ASR/diarization (whisper.cpp and pyannote-family models
//!   both require 16 kHz mono — see `crate::pipeline::load_wav_16k_mono` for the
//!   Rust path and `pipeline/wsw/transcribe.py` for the Python one).
//!
//! Permissions (Info.plist): `NSMicrophoneUsageDescription` (mic) and the
//! Screen Recording grant (SCK). Both are user-facing one-time grants; macOS
//! 15 re-prompts for Screen Recording roughly monthly for unused apps.

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::capture::session::{RecorderBackend, SavedRecording};
use crate::detect::state::MeetingApp;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("microphone stream error: {0}")]
    Mic(String),
    #[error("system-audio stream error: {0}")]
    SystemAudio(String),
    #[error("recording already in progress")]
    Busy,
    #[error("no recording in progress")]
    NotRecording,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wav error: {0}")]
    Wav(String),
}

/// Seam for the system-audio implementation (SCK now, Core Audio taps later).
pub trait SystemAudioSource: Send {
    /// Start delivering 48 kHz f32 interleaved stereo buffers to `sink`.
    fn start(&mut self, sink: Box<dyn FnMut(&[f32]) + Send>) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
}

/// The production recorder: one WAV per track.
///
/// File layout per recording:
///   {out_dir}/{stem}.system.wav  — remote participants (SCK)
///   {out_dir}/{stem}.mic.wav     — local user (cpal)
pub struct MacRecorder<S: SystemAudioSource> {
    system_source: S,
    /// Exposed to the detector so it can switch to per-process mic checks
    /// while we hold the input device open.
    self_recording: Arc<AtomicBool>,
    active: Option<ActiveRecording>,
}

struct ActiveRecording {
    stem_path: PathBuf,
    started: std::time::Instant,
    mic_stream: Option<cpal::Stream>,
    mic_writer: Arc<std::sync::Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    sys_writer: Arc<std::sync::Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
}

impl<S: SystemAudioSource> MacRecorder<S> {
    pub fn new(system_source: S, self_recording: Arc<AtomicBool>) -> Self {
        MacRecorder { system_source, self_recording, active: None }
    }

    fn wav_spec(channels: u16, sample_rate: u32) -> hound::WavSpec {
        hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }
    }
}

impl<S: SystemAudioSource> RecorderBackend for MacRecorder<S> {
    type Error = CaptureError;

    fn start(&mut self, _app: &MeetingApp, out_dir: &str, file_stem: &str) -> Result<(), CaptureError> {
        if self.active.is_some() {
            return Err(CaptureError::Busy);
        }
        std::fs::create_dir_all(out_dir)?;
        let stem_path = PathBuf::from(out_dir).join(file_stem);

        // --- Microphone via cpal (default input, native config) ---
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| CaptureError::Mic("no default input device".into()))?;
        let config = device
            .default_input_config()
            .map_err(|e| CaptureError::Mic(e.to_string()))?;
        let mic_rate = config.sample_rate().0;
        let mic_channels = config.channels();

        let mic_writer = Arc::new(std::sync::Mutex::new(Some(
            hound::WavWriter::create(
                stem_path.with_extension("mic.wav"),
                Self::wav_spec(mic_channels, mic_rate),
            )
            .map_err(|e| CaptureError::Wav(e.to_string()))?,
        )));

        let mw = mic_writer.clone();
        let mic_stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    if let Some(w) = mw.lock().unwrap().as_mut() {
                        for &s in data {
                            // Dropped samples on write error beat a panic in
                            // the realtime callback; stop() surfaces problems.
                            let _ = w.write_sample(s);
                        }
                    }
                },
                move |err| {
                    tracing::error!("mic stream error: {err}");
                },
                None,
            )
            .map_err(|e| CaptureError::Mic(e.to_string()))?;
        mic_stream.play().map_err(|e| CaptureError::Mic(e.to_string()))?;

        // --- System audio via the SystemAudioSource seam (SCK: 48kHz stereo) ---
        let sys_writer = Arc::new(std::sync::Mutex::new(Some(
            hound::WavWriter::create(
                stem_path.with_extension("system.wav"),
                Self::wav_spec(2, 48_000),
            )
            .map_err(|e| CaptureError::Wav(e.to_string()))?,
        )));
        let sw = sys_writer.clone();
        self.system_source.start(Box::new(move |data: &[f32]| {
            if let Some(w) = sw.lock().unwrap().as_mut() {
                for &s in data {
                    let _ = w.write_sample(s);
                }
            }
        }))?;

        self.self_recording.store(true, Ordering::Relaxed);
        self.active = Some(ActiveRecording {
            stem_path,
            started: std::time::Instant::now(),
            mic_stream: Some(mic_stream),
            mic_writer,
            sys_writer,
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<SavedRecording, CaptureError> {
        let mut rec = self.active.take().ok_or(CaptureError::NotRecording)?;
        self.self_recording.store(false, Ordering::Relaxed);

        // Order matters: stop sources first so no callback writes to a
        // finalized WAV header.
        drop(rec.mic_stream.take()); // cpal stream stops on drop
        self.system_source.stop()?;

        let mut mic_written = false;
        if let Some(w) = rec.mic_writer.lock().unwrap().take() {
            w.finalize().map_err(|e| CaptureError::Wav(e.to_string()))?;
            mic_written = true;
        }
        if let Some(w) = rec.sys_writer.lock().unwrap().take() {
            w.finalize().map_err(|e| CaptureError::Wav(e.to_string()))?;
        }

        let system_path = rec.stem_path.with_extension("system.wav").display().to_string();
        let mic_path = mic_written
            .then(|| rec.stem_path.with_extension("mic.wav").display().to_string());
        Ok(SavedRecording {
            system_path,
            mic_path,
            duration_ms: rec.started.elapsed().as_millis() as u64,
        })
    }

    fn is_recording(&self) -> bool {
        self.active.is_some()
    }
}

/// ScreenCaptureKit-backed system audio (macOS 13+).
///
/// Uses the `screencapturekit` crate: an `SCStream` configured with
/// `captures_audio(true)` and a minimal video config (SCK requires a stream;
/// audio-only capture works by ignoring video buffers). Buffers arrive as
/// `CMSampleBuffer`s on the audio output type and are converted to f32.
pub struct SckSystemAudio {
    stream: Option<screencapturekit::stream::SCStream>,
}

impl SckSystemAudio {
    pub fn new() -> Self {
        SckSystemAudio { stream: None }
    }
}

impl SystemAudioSource for SckSystemAudio {
    fn start(&mut self, sink: Box<dyn FnMut(&[f32]) + Send>) -> Result<(), CaptureError> {
        use screencapturekit::shareable_content::SCShareableContent;
        use screencapturekit::stream::configuration::SCStreamConfiguration;
        use screencapturekit::stream::content_filter::SCContentFilter;
        use screencapturekit::stream::output_trait::SCStreamOutputTrait;
        use screencapturekit::stream::output_type::SCStreamOutputType;
        use screencapturekit::stream::SCStream;

        // The SCK output handler is invoked on SCK's own dispatch queue, so the
        // trait method takes &self and the handler must be Send + Sync + 'static.
        // The FnMut sink therefore lives behind a Mutex (interior mutability),
        // and a scratch buffer is reused to interleave channels without
        // per-callback allocation.
        struct AudioOut {
            sink: std::sync::Mutex<Box<dyn FnMut(&[f32]) + Send>>,
            scratch: std::sync::Mutex<Vec<f32>>,
        }
        impl SCStreamOutputTrait for AudioOut {
            fn did_output_sample_buffer(
                &self,
                sample: screencapturekit::cm::CMSampleBuffer,
                of_type: SCStreamOutputType,
            ) {
                if !matches!(of_type, SCStreamOutputType::Audio) {
                    return;
                }
                let Some(buffers) = sample.audio_buffer_list() else { return };
                // AudioBuffer exposes raw bytes; SCK audio is native-endian f32.
                let channels: Vec<&[u8]> = buffers.iter().map(|b| b.data()).collect();
                if channels.is_empty() {
                    return;
                }
                // SCK delivers PLANAR audio: one AudioBuffer per channel. The
                // WAV writer expects interleaved frames, so zip the channels
                // (L,R,L,R,...). Writing each planar buffer straight through
                // would scramble the file into [all-L][all-R].
                let frames = channels.iter().map(|c| c.len() / 4).min().unwrap_or(0);
                let mut interleaved = self.scratch.lock().unwrap();
                interleaved.clear();
                interleaved.reserve(frames * channels.len());
                for f in 0..frames {
                    for ch in &channels {
                        let i = f * 4;
                        interleaved.push(f32::from_ne_bytes([
                            ch[i], ch[i + 1], ch[i + 2], ch[i + 3],
                        ]));
                    }
                }
                if let Ok(mut sink) = self.sink.lock() {
                    (sink)(&interleaved);
                }
            }
        }

        let content = SCShareableContent::get()
            .map_err(|e| CaptureError::SystemAudio(format!("shareable content: {e:?}")))?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| CaptureError::SystemAudio("no display".into()))?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();
        let config = SCStreamConfiguration::new()
            .with_captures_audio(true)
            .with_sample_rate(48_000)
            .with_channel_count(2);

        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(
            AudioOut { sink: std::sync::Mutex::new(sink), scratch: std::sync::Mutex::new(Vec::new()) },
            SCStreamOutputType::Audio,
        );
        stream
            .start_capture()
            .map_err(|e| CaptureError::SystemAudio(format!("start_capture: {e:?}")))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if let Some(stream) = self.stream.take() {
            stream
                .stop_capture()
                .map_err(|e| CaptureError::SystemAudio(format!("stop_capture: {e:?}")))?;
        }
        Ok(())
    }
}
