//! sherpa-onnx diarization via the `sherpa-rs` crate.
//!
//! Models (both mirrored ungated on HuggingFace, verified fetchable):
//! - segmentation: `sherpa-onnx-pyannote-segmentation-3-0` (ONNX export of
//!   pyannote/segmentation-3.0, MIT) — https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0
//! - embedding: a 3D-Speaker/WeSpeaker ONNX model (int8 variants available).
//!
//! API-fidelity note: sherpa-rs 0.6.x exposes speaker diarization
//! (`sherpa_rs::diarize`); the exact constructor/config field names below
//! follow its documented examples but could not be compiled in the authoring
//! sandbox (crates.io blocked) — expect at most renames, not design changes,
//! on first macOS build.

#![cfg(feature = "diarize-sherpa")]

use super::{DiarizeError, Diarizer, SpeakerSegment};

pub struct SherpaDiarizer {
    inner: sherpa_rs::diarize::Diarize,
}

impl SherpaDiarizer {
    /// `num_speakers`: upper bound on distinct speakers; 0 lets clustering
    /// decide. (pyannote segmentation-3.0 resolves at most 3 concurrent
    /// speakers per 10 s window, but a meeting can have more overall.)
    pub fn new(
        segmentation_model: &str,
        embedding_model: &str,
        num_speakers: usize,
    ) -> Result<Self, DiarizeError> {
        let config = sherpa_rs::diarize::DiarizeConfig {
            num_clusters: if num_speakers > 0 { Some(num_speakers as i32) } else { None },
            ..Default::default()
        };
        let inner =
            sherpa_rs::diarize::Diarize::new(segmentation_model, embedding_model, config)
                .map_err(|e| DiarizeError::ModelLoad(e.to_string()))?;
        Ok(SherpaDiarizer { inner })
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&mut self, samples_16k_mono: &[f32]) -> Result<Vec<SpeakerSegment>, DiarizeError> {
        let segments = self
            .inner
            .compute(samples_16k_mono.to_vec(), None)
            .map_err(|e| DiarizeError::Inference(e.to_string()))?;

        let mut out: Vec<SpeakerSegment> = segments
            .into_iter()
            .map(|s| SpeakerSegment {
                start_ms: (s.start * 1000.0) as u64,
                end_ms: (s.end * 1000.0) as u64,
                speaker: format!("SPEAKER_{:02}", s.speaker),
            })
            .collect();

        // sherpa returns segments per speaker; downstream merge assumes
        // chronological order.
        out.sort_by_key(|s| s.start_ms);
        Ok(out)
    }
}
