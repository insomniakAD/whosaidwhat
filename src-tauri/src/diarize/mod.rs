//! Speaker diarization stage (native, in-process).
//!
//! Engine choice (full comparison in docs/04-diarization-evaluation.md):
//! sherpa-onnx running the pyannote `segmentation-3.0` ONNX export plus a
//! speaker-embedding ONNX model — pure ONNX Runtime, no Python, no HF gating
//! at runtime, macOS arm64 support, Rust bindings (`sherpa-rs`). FluidAudio's
//! CoreML/ANE pipeline is faster per watt but Swift-only; the trait is the
//! seam where a Swift sidecar could replace sherpa later.
//!
//! `merge` (dependency-free, unit-tested) turns (ASR segments + diarization
//! segments) into speaker-attributed turns — that logic is engine-agnostic.

pub mod merge;

#[cfg(feature = "diarize-sherpa")]
pub mod sherpa;

/// A span attributed to one (anonymous) speaker: "SPEAKER_00", "SPEAKER_01"...
/// Stable within a meeting, meaningless across meetings until the user names
/// them (the DB's `speakers` table owns naming + voiceprint matching).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    #[error("model load failed: {0}")]
    ModelLoad(String),
    #[error("inference failed: {0}")]
    Inference(String),
}

/// Contract: 16 kHz mono f32 PCM in, non-overlapping speaker segments out
/// (overlap resolved to the dominant speaker, pyannote "exclusive" style,
/// which is what transcript alignment needs).
pub trait Diarizer: Send {
    fn diarize(&mut self, samples_16k_mono: &[f32]) -> Result<Vec<SpeakerSegment>, DiarizeError>;
}
