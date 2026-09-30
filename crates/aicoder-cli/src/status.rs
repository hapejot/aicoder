//! `aicoder status` — reports the effective configuration and probes the
//! provider endpoints so integration problems surface before the first edit.

use aicoder_core::{
    config::{self, ProviderKind, RuntimeConfig},
    providers::{build_provider, ProviderHealth},
};
use anyhow::Result;

use crate::popup;

/// Renders the status report.
pub async fn run_status(
    provider_override: Option<&str>,
    model_override: Option<&str>,
    no_probe: bool,
    all: bool,
) -> Result<()> {
    let cfg = RuntimeConfig::load(provider_override, model_override)?;

    println!("aicoder {}", env!("CARGO_PKG_VERSION"));

    println!("\nconfig");
    match config::config_path() {
        Some(path) => println!("  file            {}", path.display()),
        None => println!("  file            <none> (built-in defaults in use)"),
    }
    println!(
        "  provider        {} ({})",
        cfg.provider.as_str(),
        source_of(provider_override, "AICODER_PROVIDER")
    );
    println!(
        "  model           {} ({})",
        cfg.model,
        source_of(model_override, "AICODER_MODEL")
    );

    println!("\nanthropic");
    println!("  api_base        {}", cfg.anthropic_api_base);
    println!("  timeout         {}s", cfg.anthropic_timeout_secs);
    println!("  api_key         {}", describe_key(cfg.anthropic_api_key.as_deref()));

    println!("\nollama");
    println!("  api_base        {}", cfg.ollama_api_base);
    println!("  timeout         {}s", cfg.ollama_timeout_secs);

    println!("\nenvironment");
    println!("  tmux            {}", yes_no(popup::in_tmux()));

    println!("\nconnections");
    if no_probe {
        println!("  (skipped: --no-probe)");
        return Ok(());
    }

    let targets: Vec<ProviderKind> = if all {
        vec![ProviderKind::Anthropic, ProviderKind::Ollama]
    } else {
        vec![cfg.provider]
    };

    let mut all_ok = true;
    for kind in targets {
        let active = if kind == cfg.provider { " (active)" } else { "" };
        let health = probe(kind, &cfg).await;
        all_ok &= health.ok;
        println!(
            "  {:<15} {} {}{}",
            kind.as_str(),
            if health.ok { "ok" } else { "FAIL" },
            health.detail,
            active
        );
    }

    if !all_ok {
        println!("\nOne or more connections are unavailable.");
    }

    Ok(())
}

async fn probe(kind: ProviderKind, cfg: &RuntimeConfig) -> ProviderHealth {
    match build_provider(kind, cfg) {
        Ok(provider) => provider.health().await,
        Err(e) => ProviderHealth::down(format!("not configured: {e}")),
    }
}

fn source_of(flag: Option<&str>, env_key: &str) -> &'static str {
    if flag.is_some() {
        "cli flag"
    } else if std::env::var_os(env_key).is_some() {
        "environment"
    } else if config::config_path().is_some() {
        "config file or default"
    } else {
        "default"
    }
}

fn describe_key(key: Option<&str>) -> String {
    match key {
        Some(k) if k.len() > 8 => format!("set (…{})", &k[k.len() - 4..]),
        Some(_) => "set".to_string(),
        None => "not set".to_string(),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
