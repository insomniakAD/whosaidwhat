//! Post-recording pipeline: saved WAVs → diarized transcript → DB → summary.
//!
//! Audio flow (rationale in docs/00-architecture.md):
//!
//! ```text
//! {stem}.mic.wav ────► resample 16k mono ─► ASR ──────────────┐ (speaker = self)
//!                                                             ├─► interleave ─► segments (DB)
//! {stem}.system.wav ─► resample 16k mono ─┬► ASR ─────────────┤
//!                                         └► diarization ─────┘ (attribute_speakers)
//!                                                                    │
//!                                                             chunk + MapReduce (oMLX)
//!                                                                    │
//!                                                             summaries (DB, versioned)
//! ```
//!
//! Resampling is plain decimation-free linear interpolation here — meeting
//! speech through a 48k→16k linear resampler measures within noise for ASR
//! purposes and avoids a DSP dependency; swap in `rubato` if artifacts show.

pub mod worker;

use std::path::Path;

use crate::asr::Transcriber;
use crate::db::{DbError, Store};
use crate::diarize::merge::{attribute_speakers, coalesce_turns, interleave};
use crate::diarize::Diarizer;
use crate::llm::chunk::Turn;
use crate::llm::extract::{
    parse_action_items, parse_timestamps, quote_snippet, resolve_segment, SegmentSpan,
    ACTION_ITEMS_PROMPT,
};
use crate::llm::router::{InferenceRouter, SamplingProfile};
use crate::llm::summarize::{summarize, SummarizeConfig};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("audio read failed: {0}")]
    Audio(String),
    #[error(transparent)]
    Asr(#[from] crate::asr::AsrError),
    #[error(transparent)]
    Diarize(#[from] crate::diarize::DiarizeError),
    #[error(transparent)]
    Llm(#[from] crate::llm::client::LlmError),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// Max silence gap when merging consecutive same-speaker turns.
const COALESCE_GAP_MS: u64 = 1_500;
/// Display name for the mic-track owner until the user sets their name.
const SELF_SPEAKER: &str = "Me";

/// Read a WAV (any rate/channels) and produce 16 kHz mono f32.
pub fn load_wav_16k_mono(path: &Path) -> Result<Vec<f32>, PipelineError> {
    let mut reader = hound::WavReader::open(path).map_err(|e| PipelineError::Audio(e.to_string()))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| PipelineError::Audio(e.to_string()))?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .map_err(|e| PipelineError::Audio(e.to_string()))?
        }
    };

    // Downmix to mono.
    let mono: Vec<f32> = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    Ok(resample_linear(&mono, spec.sample_rate, 16_000))
}

/// Linear-interpolation resampler (pure, unit-tested).
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Everything after the recorder saves its files. Returns the summary row id.
#[allow(clippy::too_many_arguments)]
pub async fn process_recording(
    store: &mut Store,
    router: &InferenceRouter,
    transcriber: &mut dyn Transcriber,
    diarizer: &mut dyn Diarizer,
    meeting_id: &str,
    mic_wav: Option<&Path>,
    system_wav: &Path,
    progress: &mut (dyn FnMut(&str, usize, usize) + Send),
) -> Result<i64, PipelineError> {
    // 1. Remote participants: ASR + diarization on the system track.
    progress("transcribe", 0, 1);
    let system_audio = load_wav_16k_mono(system_wav)?;
    let system_asr = transcriber.transcribe(&system_audio)?;
    progress("transcribe", 1, 1);

    progress("diarize", 0, 1);
    let speakers = diarizer.diarize(&system_audio)?;
    let remote = coalesce_turns(
        attribute_speakers(&system_asr, &speakers, "SPEAKER_XX"),
        COALESCE_GAP_MS,
    );
    progress("diarize", 1, 1);

    // 2. Local user: mic track needs no diarization — it IS the local speaker.
    let local = match mic_wav {
        Some(path) => {
            let mic_audio = load_wav_16k_mono(path)?;
            let mic_asr = transcriber.transcribe(&mic_audio)?;
            coalesce_turns(
                mic_asr
                    .into_iter()
                    .map(|seg| Turn {
                        speaker: SELF_SPEAKER.to_string(),
                        text: seg.text,
                        start_ms: seg.start_ms,
                        end_ms: seg.end_ms,
                    })
                    .collect(),
                COALESCE_GAP_MS,
            )
        }
        None => Vec::new(),
    };

    // 3. One chronological transcript → DB.
    let tagged: Vec<(Turn, &str)> = interleave(local, remote)
        .into_iter()
        .map(|t| {
            let source = if t.speaker == SELF_SPEAKER { "mic" } else { "system" };
            (t, source)
        })
        .collect();
    store.insert_transcript(meeting_id, &tagged)?;
    store.set_meeting_status(meeting_id, "transcribed")?;

    // 4. Summarize via oMLX; record provenance including fallback.
    let turns: Vec<Turn> = tagged.into_iter().map(|(t, _)| t).collect();
    let output = summarize(router, &turns, &SummarizeConfig::default(), progress).await?;
    let summary_id = store.insert_summary(
        meeting_id,
        "notes",
        &output.notes,
        &output.model.model,
        !output.model.preferred,
    )?;
    let outline_id = store.insert_summary(
        meeting_id,
        "outline",
        &output.outline,
        &output.model.model,
        !output.model.preferred,
    )?;

    // 5. Structured citations: every [mm:ss] marker the summarizer preserved,
    // resolved to a real segment (unresolvable markers are dropped — see
    // llm::extract). Quotes carry the cited words for hover previews.
    progress("cite", 0, 1);
    let segments = store.segments_for_meeting(meeting_id)?;
    let spans: Vec<SegmentSpan> = segments
        .iter()
        .map(|s| SegmentSpan { id: s.id, start_ms: s.start_ms, end_ms: s.end_ms })
        .collect();
    for (sid, content) in [(summary_id, output.notes.as_str()), (outline_id, output.outline.as_str())] {
        let citations: Vec<(i64, Option<String>)> = parse_timestamps(content)
            .into_iter()
            .filter_map(|ms| resolve_segment(ms, &spans))
            .map(|segment_id| {
                let quote = segments
                    .iter()
                    .find(|s| s.id == segment_id)
                    .map(|s| quote_snippet(&s.text));
                (segment_id, quote)
            })
            .collect();
        store.insert_citations(sid, &citations)?;
    }
    progress("cite", 1, 1);

    // 6. Action items: stage-4 extraction over the outline (the surface that
    // keeps timestamps). A failure here must not fail the meeting — the
    // summary is already stored — so it degrades to zero items with a log.
    progress("actions", 0, 1);
    match router
        .summarize_stage(&output.model, ACTION_ITEMS_PROMPT, &output.outline, SamplingProfile::Strict)
        .await
    {
        Ok(response) => {
            let mut rows: Vec<(Option<i64>, String)> = Vec::new();
            for item in parse_action_items(&response) {
                let speaker_id = match &item.owner {
                    Some(name) => store.speaker_in_meeting(meeting_id, name)?,
                    None => None,
                };
                rows.push((speaker_id, item.text));
            }
            store.insert_action_items(meeting_id, Some(summary_id), &rows)?;
        }
        Err(e) => {
            tracing::warn!("action-item extraction failed for {meeting_id}: {e} (summary kept)");
        }
    }
    progress("actions", 1, 1);

    store.set_meeting_status(meeting_id, "summarized")?;

    Ok(summary_id)
}

/// Load the user-configured engines and run [`process_recording`] — the one
/// entry point shared by the headless daemon (main.rs) and the Tauri shell
/// (shell.rs), so engine selection never drifts between the two.
///
/// ASR + diarization models load lazily per call; on 64 GB the ~2 GB whisper
/// + ~50 MB diarization models could stay resident, but cold-loading keeps
/// the idle footprint near zero and adds only seconds per meeting.
pub async fn run_with_default_engines(
    store: &mut Store,
    router: &InferenceRouter,
    meeting_id: &str,
    system_wav: &Path,
    mic_wav: Option<&Path>,
    progress: &mut (dyn FnMut(&str, usize, usize) + Send),
) -> anyhow::Result<i64> {
    let config_path = crate::config::Config::default().data_dir.join("config.json");
    let config = crate::config::Config::load_or_default(&config_path);

    #[cfg(feature = "asr-whisper")]
    let mut transcriber = crate::asr::whisper::WhisperTranscriber::new(
        &config.whisper_model.display().to_string(),
        &config.language,
    )?;
    #[cfg(not(feature = "asr-whisper"))]
    anyhow::bail!("built without an ASR engine (enable feature asr-whisper)");

    #[cfg(feature = "diarize-sherpa")]
    let mut diarizer = crate::diarize::sherpa::SherpaDiarizer::new(
        &config.diarize_segmentation_model.display().to_string(),
        &config.diarize_embedding_model.display().to_string(),
        config.expected_speakers,
    )?;
    #[cfg(not(feature = "diarize-sherpa"))]
    anyhow::bail!("built without a diarization engine (enable feature diarize-sherpa)");

    #[cfg(all(feature = "asr-whisper", feature = "diarize-sherpa"))]
    {
        let summary_id = process_recording(
            store,
            router,
            &mut transcriber,
            &mut diarizer,
            meeting_id,
            mic_wav.filter(|p| p.exists()),
            system_wav,
            progress,
        )
        .await?;
        Ok(summary_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_length_48k_to_16k_ratio() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48_000.0).sin()).collect();
        let out = resample_linear(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        // Monotone ramp stays monotone through linear interpolation.
        let ramp: Vec<f32> = (0..480).map(|i| i as f32).collect();
        let r = resample_linear(&ramp, 48_000, 16_000);
        assert!(r.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn resample_identity_and_empty() {
        let x = vec![1.0f32, 2.0, 3.0];
        assert_eq!(resample_linear(&x, 16_000, 16_000), x);
        assert!(resample_linear(&[], 48_000, 16_000).is_empty());
    }

}
