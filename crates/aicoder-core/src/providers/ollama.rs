use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    config::RuntimeConfig,
    error::AicoderError,
    models::{build_edit_prompt, extract_updated_content, GenerateRequest, GenerateResponse, RawRequest},
    providers::{AiProvider, ChunkSink, ProviderHealth},
};

pub struct OllamaProvider {
    client: reqwest::Client,
    api_base: String,
}

impl OllamaProvider {
    pub fn new(cfg: &RuntimeConfig) -> Result<Self, AicoderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.ollama_timeout_secs))
            .build()?;

        Ok(Self {
            client,
            api_base: cfg.ollama_api_base.clone(),
        })
    }
}

#[derive(Deserialize, Default)]
struct OllamaTags {
    #[serde(default)]
    models: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
}

#[async_trait]
impl AiProvider for OllamaProvider {
    async fn health(&self) -> ProviderHealth {
        let url = format!("{}/api/tags", self.api_base.trim_end_matches('/'));
        match self
            .client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => match resp.json::<OllamaTags>().await {
                Ok(tags) => ProviderHealth::up(format!("reachable, {} model(s) installed", tags.models.len())),
                Err(_) => ProviderHealth::up("reachable"),
            },
            Ok(resp) => ProviderHealth::down(format!("unexpected status {}", resp.status())),
            Err(e) if e.is_timeout() => ProviderHealth::down("timed out"),
            Err(e) => ProviderHealth::down(format!("unreachable: {e}")),
        }
    }

    async fn generate_edit(&self, req: &GenerateRequest) -> Result<GenerateResponse, AicoderError> {
        let payload = OllamaRequest {
            model: req.model.clone(),
            prompt: build_edit_prompt(req),
            stream: false,
            system: None,
        };

        let url = format!("{}/api/generate", self.api_base.trim_end_matches('/'));
        let response = self.client.post(url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AicoderError::Provider(format!(
                "ollama returned {status}: {body}"
            )));
        }

        let parsed: OllamaResponse = response.json().await?;
        Ok(GenerateResponse {
            updated_content: extract_updated_content(&parsed.response),
        })
    }

    async fn complete_stream(
        &self,
        req: &RawRequest,
        sink: ChunkSink<'_>,
    ) -> Result<String, AicoderError> {
        let payload = OllamaRequest {
            model: req.model.clone(),
            prompt: req.prompt.clone(),
            stream: true,
            system: req.system.clone(),
        };

        let url = format!("{}/api/generate", self.api_base.trim_end_matches('/'));
        let response = self.client.post(url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AicoderError::Provider(format!(
                "ollama returned {status}: {body}"
            )));
        }

        let mut full_text = String::new();
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(item) = stream.next().await {
            let bytes = item?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Ollama streams newline-delimited JSON objects.
            while let Some(idx) = buffer.find('\n') {
                let line: String = buffer.drain(..idx + 1).collect();
                if let Some(delta) = parse_ndjson_response(&line) {
                    sink(&delta);
                    full_text.push_str(&delta);
                }
            }
        }

        if let Some(delta) = parse_ndjson_response(&buffer) {
            sink(&delta);
            full_text.push_str(&delta);
        }

        Ok(full_text)
    }
}

/// Extracts the `response` field from one NDJSON line of the Ollama stream.
fn parse_ndjson_response(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let text = value.get("response")?.as_str()?;
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_ndjson_response;

    #[test]
    fn parses_response_chunk() {
        let line = "{\"model\":\"x\",\"response\":\"fn \",\"done\":false}";
        assert_eq!(parse_ndjson_response(line).as_deref(), Some("fn "));
    }

    #[test]
    fn ignores_empty_and_final_chunk() {
        assert!(parse_ndjson_response("").is_none());
        assert!(parse_ndjson_response("{\"response\":\"\",\"done\":true}").is_none());
    }
}
