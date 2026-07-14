//! whisper.cpp transcription via whisper-rs (Metal on Apple Silicon).
//!
//! Model recommendation for the 64 GB M-series target: `large-v3-turbo`
//! (GGUF/GGML q5 ≈ 1.7 GB, near-large-v3 accuracy at ~6x decode speed) or
//! `large-v3` q5 (~2.9 GB) when accuracy matters more than latency. Both are
//! rounding errors next to the ~22 GB summarization model.
//!
//! whisper-rs segment timestamps are in centiseconds (10 ms units) — converted
//! to ms here so nothing downstream ever sees centiseconds.

#![cfg(feature = "asr-whisper")]

use super::{AsrError, AsrSegment, Transcriber};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

pub struct WhisperTranscriber {
    state: WhisperState,
    /// Language hint ("en", "auto", ...). "auto" enables detection.
    language: String,
    threads: i32,
}

impl WhisperTranscriber {
    pub fn new(model_path: &str, language: &str) -> Result<Self, AsrError> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true); // Metal, when built with the `metal` feature
        let ctx = WhisperContext::new_with_params(model_path, params)
            .map_err(|e| AsrError::ModelLoad(format!("{model_path}: {e}")))?;
        let state = ctx.create_state().map_err(|e| AsrError::ModelLoad(e.to_string()))?;
        // Leave 2 performance cores for capture + UI; whisper saturates fast.
        let threads = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8) as i32 - 2)
            .max(2);
        Ok(WhisperTranscriber { state, language: language.to_string(), threads })
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&mut self, samples_16k_mono: &[f32]) -> Result<Vec<AsrSegment>, AsrError> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(Some(&self.language));
        params.set_token_timestamps(true);
        // Meeting audio: suppress non-speech artifacts, keep segments tight.
        params.set_suppress_blank(true);
        params.set_no_context(false);

        self.state
            .full(params, samples_16k_mono)
            .map_err(|e| AsrError::Inference(e.to_string()))?;

        let n = self.state.full_n_segments().map_err(|e| AsrError::Inference(e.to_string()))?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let text = self
                .state
                .full_get_segment_text(i)
                .map_err(|e| AsrError::Inference(e.to_string()))?;
            let t0 = self
                .state
                .full_get_segment_t0(i)
                .map_err(|e| AsrError::Inference(e.to_string()))?;
            let t1 = self
                .state
                .full_get_segment_t1(i)
                .map_err(|e| AsrError::Inference(e.to_string()))?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            out.push(AsrSegment {
                // centiseconds → milliseconds
                start_ms: (t0.max(0) as u64) * 10,
                end_ms: (t1.max(0) as u64) * 10,
                text: trimmed.to_string(),
            });
        }
        Ok(out)
    }
}
