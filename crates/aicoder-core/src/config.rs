use std::str::FromStr;

use anyhow::Context;
use serde::Deserialize;

use crate::error::AicoderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Ollama,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Ollama => "ollama",
        }
    }
}

impl FromStr for ProviderKind {
    type Err = AicoderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            _ => Err(AicoderError::Config(format!("unknown provider: {s}"))),
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileConfig {
    default: Option<DefaultConfig>,
    providers: Option<ProvidersConfig>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct DefaultConfig {
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ProvidersConfig {
    anthropic: Option<AnthropicConfig>,
    ollama: Option<OllamaConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AnthropicConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub timeout_secs: Option<u64>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            api_base: Some("https://api.anthropic.com/v1".to_string()),
            timeout_secs: Some(60),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct OllamaConfig {
    pub api_base: Option<String>,
    pub timeout_secs: Option<u64>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            api_base: Some("http://localhost:11434".to_string()),
            timeout_secs: Some(180),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub anthropic_api_key: Option<String>,
    pub anthropic_api_base: String,
    pub anthropic_timeout_secs: u64,
    pub ollama_api_base: String,
    pub ollama_timeout_secs: u64,
}

impl RuntimeConfig {
    pub fn load(
        provider_override: Option<&str>,
        model_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let file_cfg = read_file_config().context("failed reading config file")?;

        let provider = if let Some(override_value) = provider_override {
            ProviderKind::from_str(override_value)?
        } else if let Ok(env_provider) = std::env::var("AICODER_PROVIDER") {
            ProviderKind::from_str(&env_provider)?
        } else if let Some(defaults) = &file_cfg.default {
            match defaults.provider.as_deref() {
                Some("ollama") => ProviderKind::Ollama,
                _ => ProviderKind::Anthropic,
            }
        } else {
            ProviderKind::Anthropic
        };

        let model = if let Some(override_value) = model_override {
            override_value.to_string()
        } else if let Ok(env_model) = std::env::var("AICODER_MODEL") {
            env_model
        } else if let Some(defaults) = &file_cfg.default {
            defaults.model.clone().unwrap_or_else(|| default_model(provider))
        } else {
            default_model(provider)
        };

        let anthropic = file_cfg
            .providers
            .as_ref()
            .and_then(|providers| providers.anthropic.clone())
            .unwrap_or_default();

        let ollama = file_cfg
            .providers
            .as_ref()
            .and_then(|providers| providers.ollama.clone())
            .unwrap_or_default();

        Ok(Self {
            provider,
            model,
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .or(anthropic.api_key.clone()),
            anthropic_api_base: anthropic
                .api_base
                .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
            anthropic_timeout_secs: anthropic.timeout_secs.unwrap_or(60),
            ollama_api_base: ollama
                .api_base
                .unwrap_or_else(|| "http://localhost:11434".to_string()),
            ollama_timeout_secs: ollama.timeout_secs.unwrap_or(180),
        })
    }
}

fn default_model(provider: ProviderKind) -> String {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-20250514".to_string(),
        ProviderKind::Ollama => "qwen2.5-coder".to_string(),
    }
}

fn read_file_config() -> anyhow::Result<FileConfig> {
    let Some(config_path) = resolve_config_path() else {
        return Ok(FileConfig::default());
    };

    if !config_path.exists() {
        return Ok(FileConfig::default());
    }

    let raw = std::fs::read_to_string(&config_path)?;
    let parsed: FileConfig = toml::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("invalid TOML in {}: {e}", config_path.display()))?;
    Ok(parsed)
}

/// Returns the config file that is actually in use, if any exists.
pub fn config_path() -> Option<std::path::PathBuf> {
    resolve_config_path()
}

fn resolve_config_path() -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let home_path = std::path::PathBuf::from(home)
            .join(".config")
            .join("aicoder")
            .join("config.toml");
        if home_path.exists() {
            return Some(home_path);
        }
    }

    let xdg_path = dirs::config_dir()?.join("aicoder").join("config.toml");
    if xdg_path.exists() {
        Some(xdg_path)
    } else {
        None
    }
}
