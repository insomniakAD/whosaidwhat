//! OpenAI-compatible chat client for the local oMLX server.
//!
//! oMLX (https://github.com/jundot/omlx) serves `/v1/chat/completions` and
//! `/v1/models` on `http://localhost:8000` by default, with models discovered
//! from `~/.omlx/models`. We speak only the OpenAI-compatible surface — the
//! narrowest, most portable contract — so swapping oMLX for mlx-omni-server,
//! llama.cpp's server, or LM Studio is a base-URL change.
//!
//! Non-standard parameters (Qwen3.6's `chat_template_kwargs` for
//! enable_thinking/preserve_thinking) ride in `extra`, flattened into the
//! request JSON, because they are server-specific and must not leak into the
//! typed API.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("request failed: {0}")]
    Transport(String),
    #[error("server returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("empty response (no choices)")]
    EmptyResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: content.into() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Server-specific extensions (e.g. {"chat_template_kwargs":
    /// {"enable_thinking": false}}), flattened into the JSON body.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    pub message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponseMessage {
    pub content: Option<String>,
    /// oMLX surfaces Qwen reasoning traces here when thinking is enabled.
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct ModelList {
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub id: String,
}

/// Thin async client. One instance per server; cheap to clone (reqwest pools).
#[derive(Clone)]
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    base_url: String,
}

impl OpenAiCompatClient {
    /// `base_url` like "http://localhost:8000/v1" (no trailing slash).
    pub fn new(base_url: impl Into<String>) -> Self {
        OpenAiCompatClient {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn chat(&self, request: &ChatRequest) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status: status.as_u16(), body });
        }

        let parsed: ChatResponse =
            resp.json().await.map_err(|e| LlmError::Transport(e.to_string()))?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .filter(|c| !c.is_empty())
            .ok_or(LlmError::EmptyResponse)
    }

    /// List models the server has loaded/discovered — used at startup to
    /// verify the configured summarization model actually exists locally.
    pub async fn models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status: status.as_u16(), body });
        }
        let parsed: ModelList =
            resp.json().await.map_err(|e| LlmError::Transport(e.to_string()))?;
        Ok(parsed.data.into_iter().map(|m| m.id).collect())
    }

    /// Cheap health probe (is the oMLX server up?).
    pub async fn healthy(&self) -> bool {
        self.models().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_extra_flattened() {
        let mut extra = HashMap::new();
        extra.insert(
            "chat_template_kwargs".to_string(),
            serde_json::json!({"enable_thinking": false}),
        );
        let req = ChatRequest {
            model: "Qwen3.6-35B-A3B-oQ4e-mtp".into(),
            messages: vec![ChatMessage::system("s"), ChatMessage::user("u")],
            temperature: Some(0.7),
            top_p: Some(0.8),
            max_tokens: None,
            presence_penalty: Some(1.5),
            extra,
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["chat_template_kwargs"]["enable_thinking"], false);
        assert!(v.get("max_tokens").is_none(), "None fields must be omitted");
        assert_eq!(v["messages"][0]["role"], "system");
    }

    #[test]
    fn response_parses_with_and_without_reasoning() {
        let with: ChatResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"hi","reasoning_content":"..."}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
        )
        .unwrap();
        assert_eq!(with.choices[0].message.content.as_deref(), Some("hi"));

        let without: ChatResponse =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"hi"}}]}"#).unwrap();
        assert!(without.choices[0].message.reasoning_content.is_none());
    }
}
