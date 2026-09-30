use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::{
    config::RuntimeConfig,
    error::AicoderError,
    models::{build_edit_prompt, extract_updated_content, GenerateRequest, GenerateResponse, RawRequest},
    providers::{AiProvider, ChunkSink, ProviderHealth},
};

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
}

impl AnthropicProvider {
    pub fn new(cfg: &RuntimeConfig) -> Result<Self, AicoderError> {
        let api_key = cfg
            .anthropic_api_key
            .clone()
            .ok_or_else(|| AicoderError::Config("ANTHROPIC_API_KEY is not set".to_string()))?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(cfg.anthropic_timeout_secs))
            .build()?;

        Ok(Self {
            client,
            api_key,
            api_base: cfg.anthropic_api_base.clone(),
        })
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn health(&self) -> ProviderHealth {
        let url = format!("{}/models?limit=1", self.api_base);
        match self
            .client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => ProviderHealth::up("reachable, API key accepted"),
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || resp.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                ProviderHealth::down(format!("reachable, but API key rejected ({})", resp.status()))
            }
            Ok(resp) => ProviderHealth::down(format!("unexpected status {}", resp.status())),
            Err(e) if e.is_timeout() => ProviderHealth::down("timed out"),
            Err(e) => ProviderHealth::down(format!("unreachable: {e}")),
        }
    }

    async fn generate_edit(&self, req: &GenerateRequest) -> Result<GenerateResponse, AicoderError> {
        let payload = AnthropicRequest {
            model: req.model.clone(),
            max_tokens: 8192,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: build_edit_prompt(req),
            }],
            system: None,
            stream: false,
        };

        let url = format!("{}/messages", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AicoderError::Provider(format!(
                "anthropic returned {status}: {body}"
            )));
        }

        let parsed: AnthropicResponse = response.json().await?;
        let mut full_text = String::new();
        for item in parsed.content {
            if item.kind == "text" {
                if let Some(text) = item.text {
                    full_text.push_str(&text);
                }
            }
        }

        if full_text.trim().is_empty() {
            return Err(AicoderError::Provider(
                "anthropic response did not contain text content".to_string(),
            ));
        }

        Ok(GenerateResponse {
            updated_content: extract_updated_content(&full_text),
        })
    }

    async fn complete_stream(
        &self,
        req: &RawRequest,
        sink: ChunkSink<'_>,
    ) -> Result<String, AicoderError> {
        let payload = AnthropicRequest {
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: req.prompt.clone(),
            }],
            system: req.system.clone(),
            stream: true,
        };

        let url = format!("{}/messages", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AicoderError::Provider(format!(
                "anthropic returned {status}: {body}"
            )));
        }

        let mut full_text = String::new();
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(item) = stream.next().await {
            let bytes = item?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // SSE events are separated by a blank line; process complete ones.
            while let Some(idx) = buffer.find("\n\n") {
                let event: String = buffer.drain(..idx + 2).collect();
                if let Some(delta) = parse_sse_text_delta(&event) {
                    sink(&delta);
                    full_text.push_str(&delta);
                }
            }
        }

        if let Some(delta) = parse_sse_text_delta(&buffer) {
            sink(&delta);
            full_text.push_str(&delta);
        }

        Ok(full_text)
    }
}

/// Extracts the `text_delta` payload from a single SSE event block.
fn parse_sse_text_delta(event: &str) -> Option<String> {
    for line in event.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("content_block_delta") {
            continue;
        }
        if let Some(text) = value
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
        {
            return Some(text.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_sse_text_delta;

    #[test]
    fn parses_text_delta_event() {
        let event = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hallo\"}}\n\n";
        assert_eq!(parse_sse_text_delta(event).as_deref(), Some("Hallo"));
    }

    #[test]
    fn ignores_non_delta_events() {
        let event = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n";
        assert!(parse_sse_text_delta(event).is_none());
    }
}
