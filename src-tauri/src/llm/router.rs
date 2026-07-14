//! Inference routing: which engine serves which task.
//!
//! Task 5's separation of concerns, made executable (rationale with evidence
//! in docs/05-inference-routing.md):
//!
//! | Task            | Engine                          | Why not oMLX?                          |
//! |-----------------|---------------------------------|----------------------------------------|
//! | Summarization   | oMLX (OpenAI-compat HTTP)       | — (this IS oMLX's job)                 |
//! | Chat-with-notes | oMLX (same model, same server)  | —                                      |
//! | Transcription   | whisper.cpp in-process (Metal)  | needs word timing; LLM audio models    |
//! |                 |                                 | cap at 30 s/request (Gemma 4) and      |
//! |                 |                                 | oMLX's audio endpoints are optional    |
//! | Diarization     | sherpa-onnx in-process (ONNX)   | not an LLM task at all — segmentation  |
//! |                 |                                 | + embeddings + clustering              |
//!
//! The router also owns model fallback: if the preferred summarization model
//! is not in `/v1/models`, it degrades to the first available model rather
//! than failing the whole pipeline (a meeting summary from a smaller local
//! model beats no summary; the substitution is recorded in the DB's summary
//! provenance columns).

use super::client::{ChatMessage, ChatRequest, LlmError, OpenAiCompatClient};
use std::collections::HashMap;

/// Sampling profiles, from the official Qwen3.6 model card recommendations:
/// non-thinking temperature=0.7 / top_p=0.8 / presence_penalty=1.5 for general
/// text; temperature=0.6 / top_p=0.95 / presence_penalty=0.0 for strictly
/// formatted output. (https://huggingface.co/Qwen/Qwen3.6-35B-A3B)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingProfile {
    /// Extraction / outline stages: keep it factual and format-stable.
    Strict,
    /// Final prose rewrite: allow a little voice.
    Prose,
}

impl SamplingProfile {
    pub fn apply(self, req: &mut ChatRequest) {
        match self {
            SamplingProfile::Strict => {
                req.temperature = Some(0.6);
                req.top_p = Some(0.95);
                req.presence_penalty = Some(0.0);
            }
            SamplingProfile::Prose => {
                req.temperature = Some(0.7);
                req.top_p = Some(0.8);
                req.presence_penalty = Some(1.5);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// e.g. "http://localhost:8000/v1"
    pub omlx_base_url: String,
    /// Preferred summarization model id as oMLX names it,
    /// e.g. "Qwen3.6-35B-A3B-oQ4e-mtp".
    pub summarize_model: String,
    /// Ordered fallbacks (e.g. ["gemma-4-12b-it-4bit", ...]); after these,
    /// any model the server reports is accepted.
    pub fallback_models: Vec<String>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        RouterConfig {
            omlx_base_url: "http://localhost:8000/v1".into(),
            summarize_model: "Qwen3.6-35B-A3B-oQ4e-mtp".into(),
            fallback_models: vec![],
        }
    }
}

/// What the router actually decided, for provenance recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedModel {
    pub model: String,
    /// False when the preferred model was unavailable and a fallback ran.
    pub preferred: bool,
}

pub struct InferenceRouter {
    client: OpenAiCompatClient,
    config: RouterConfig,
}

impl InferenceRouter {
    pub fn new(config: RouterConfig) -> Self {
        InferenceRouter { client: OpenAiCompatClient::new(config.omlx_base_url.clone()), config }
    }

    pub fn client(&self) -> &OpenAiCompatClient {
        &self.client
    }

    /// Resolve the summarization model against what the server actually has.
    pub async fn resolve_summarize_model(&self) -> Result<RoutedModel, LlmError> {
        let available = self.client.models().await?;
        Ok(pick_model(&self.config.summarize_model, &self.config.fallback_models, &available)
            .ok_or(LlmError::EmptyResponse)?)
    }

    /// One summarization-stage call: system prompt + user content, with
    /// thinking disabled (single-shot summarization gains nothing from
    /// reasoning traces and they cost latency; `preserve_thinking` only
    /// matters for multi-turn — oMLX manages it server-side).
    pub async fn summarize_stage(
        &self,
        routed: &RoutedModel,
        system_prompt: &str,
        user_content: &str,
        profile: SamplingProfile,
    ) -> Result<String, LlmError> {
        let mut extra = HashMap::new();
        extra.insert(
            "chat_template_kwargs".to_string(),
            serde_json::json!({ "enable_thinking": false }),
        );
        let mut req = ChatRequest {
            model: routed.model.clone(),
            messages: vec![ChatMessage::system(system_prompt), ChatMessage::user(user_content)],
            temperature: None,
            top_p: None,
            max_tokens: None,
            presence_penalty: None,
            extra,
        };
        profile.apply(&mut req);
        self.client.chat(&req).await
    }
}

/// Pure model-selection logic (unit-tested; no I/O).
pub fn pick_model(
    preferred: &str,
    fallbacks: &[String],
    available: &[String],
) -> Option<RoutedModel> {
    if available.iter().any(|m| m == preferred) {
        return Some(RoutedModel { model: preferred.to_string(), preferred: true });
    }
    for fb in fallbacks {
        if available.iter().any(|m| m == fb) {
            return Some(RoutedModel { model: fb.clone(), preferred: false });
        }
    }
    available.first().map(|m| RoutedModel { model: m.clone(), preferred: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avail(models: &[&str]) -> Vec<String> {
        models.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn preferred_model_wins_when_present() {
        let picked = pick_model("qwen", &avail(&["gemma"]), &avail(&["gemma", "qwen"])).unwrap();
        assert_eq!(picked, RoutedModel { model: "qwen".into(), preferred: true });
    }

    #[test]
    fn fallback_order_respected() {
        let picked =
            pick_model("qwen", &avail(&["a", "b"]), &avail(&["c", "b", "a"])).unwrap();
        assert_eq!(picked.model, "a");
        assert!(!picked.preferred);
    }

    #[test]
    fn any_model_beats_none() {
        let picked = pick_model("qwen", &[], &avail(&["something-else"])).unwrap();
        assert_eq!(picked.model, "something-else");
        assert!(!picked.preferred);
    }

    #[test]
    fn empty_server_yields_none() {
        assert!(pick_model("qwen", &avail(&["a"]), &[]).is_none());
    }

    #[test]
    fn profiles_match_qwen_card() {
        let mut req = ChatRequest {
            model: "m".into(),
            messages: vec![],
            temperature: None,
            top_p: None,
            max_tokens: None,
            presence_penalty: None,
            extra: HashMap::new(),
        };
        SamplingProfile::Strict.apply(&mut req);
        assert_eq!(req.temperature, Some(0.6));
        assert_eq!(req.top_p, Some(0.95));
        SamplingProfile::Prose.apply(&mut req);
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.presence_penalty, Some(1.5));
    }
}
