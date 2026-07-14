//! App configuration with sane local-first defaults. Serialized to
//! `~/Library/Application Support/com.whosaidwhat.app/config.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::capture::session::RecordPolicy;
use crate::llm::router::RouterConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where recordings and the database live.
    pub data_dir: PathBuf,
    /// Prompt / Auto / Manual (see capture::session).
    pub record_policy: RecordPolicyConfig,
    /// oMLX routing (base URL, model, fallbacks).
    pub inference: RouterConfig,
    /// Whisper GGUF/GGML model path (e.g. ggml-large-v3-turbo-q5_0.bin).
    pub whisper_model: PathBuf,
    /// Whisper language hint ("en", "auto", ...).
    pub language: String,
    /// sherpa-onnx model paths for diarization.
    pub diarize_segmentation_model: PathBuf,
    pub diarize_embedding_model: PathBuf,
    /// 0 = let clustering pick the speaker count.
    pub expected_speakers: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecordPolicyConfig {
    Prompt,
    Auto,
    Manual,
}

impl From<RecordPolicyConfig> for RecordPolicy {
    fn from(c: RecordPolicyConfig) -> Self {
        match c {
            RecordPolicyConfig::Prompt => RecordPolicy::Prompt,
            RecordPolicyConfig::Auto => RecordPolicy::Auto,
            RecordPolicyConfig::Manual => RecordPolicy::Manual,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        let data_dir = home.join("Library/Application Support/com.whosaidwhat.app");
        let models = data_dir.join("models");
        Config {
            record_policy: RecordPolicyConfig::Prompt,
            inference: RouterConfig::default(),
            whisper_model: models.join("ggml-large-v3-turbo-q5_0.bin"),
            language: "auto".into(),
            diarize_segmentation_model: models.join("sherpa-onnx-pyannote-segmentation-3-0.onnx"),
            diarize_embedding_model: models.join("3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common.onnx"),
            expected_speakers: 0,
            data_dir,
        }
    }
}

impl Config {
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("whosaidwhat.sqlite")
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    pub fn load_or_default(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).expect("config serializes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_json() {
        let c = Config::default();
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.record_policy, RecordPolicyConfig::Prompt);
        assert_eq!(back.language, "auto");
    }

    #[test]
    fn partial_config_fills_defaults() {
        let back: Config = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
        assert_eq!(back.language, "en");
        assert_eq!(back.record_policy, RecordPolicyConfig::Prompt);
    }
}
