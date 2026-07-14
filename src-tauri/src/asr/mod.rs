//! Transcription stage (native, in-process).
//!
//! Engine choice (evidence in docs/05-inference-routing.md §Separation of
//! concerns): oMLX serves the summarization LLM, but transcription runs on a
//! different engine — whisper.cpp via the `whisper-rs` bindings, compiled with
//! the `metal` feature so encoding runs on the Apple GPU. The trait keeps the
//! engine swappable (mlx-whisper sidecar, FluidAudio/Parakeet CoreML helper)
//! without touching the pipeline.

pub mod whisper;

/// A transcribed span with timing. `start_ms`/`end_ms` are relative to the
/// start of the audio track that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct AsrSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("model load failed: {0}")]
    ModelLoad(String),
    #[error("inference failed: {0}")]
    Inference(String),
}

/// Contract: input is 16 kHz mono f32 PCM (the caller owns resampling; see
/// capture::macos for why capture stays at 48 kHz).
pub trait Transcriber: Send {
    fn transcribe(&mut self, samples_16k_mono: &[f32]) -> Result<Vec<AsrSegment>, AsrError>;
}
