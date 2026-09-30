use async_trait::async_trait;

use crate::{
    config::{ProviderKind, RuntimeConfig},
    error::AicoderError,
    models::{GenerateRequest, GenerateResponse, RawRequest},
};

mod anthropic;
mod ollama;

/// Callback invoked for every streamed text delta.
pub type ChunkSink<'a> = &'a mut (dyn FnMut(&str) + Send);

/// Result of a lightweight connectivity probe against a provider endpoint.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub ok: bool,
    pub detail: String,
}

impl ProviderHealth {
    pub fn up(detail: impl Into<String>) -> Self {
        Self { ok: true, detail: detail.into() }
    }

    pub fn down(detail: impl Into<String>) -> Self {
        Self { ok: false, detail: detail.into() }
    }
}

#[async_trait]
pub trait AiProvider {
    async fn generate_edit(&self, req: &GenerateRequest) -> Result<GenerateResponse, AicoderError>;

    /// Performs a cheap read-only request to verify the endpoint is reachable.
    async fn health(&self) -> ProviderHealth;

    /// Sends a free-form prompt and streams the answer.
    ///
    /// Every text delta is handed to `sink`; the full answer is returned.
    async fn complete_stream(
        &self,
        req: &RawRequest,
        sink: ChunkSink<'_>,
    ) -> Result<String, AicoderError>;

    /// Sends a free-form prompt and returns the complete answer.
    async fn complete(&self, req: &RawRequest) -> Result<String, AicoderError> {
        let mut noop = |_: &str| {};
        self.complete_stream(req, &mut noop).await
    }
}

pub fn build_provider(
    provider: ProviderKind,
    cfg: &RuntimeConfig,
) -> Result<Box<dyn AiProvider + Send + Sync>, AicoderError> {
    match provider {
        ProviderKind::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(cfg)?)),
        ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(cfg)?)),
    }
}
