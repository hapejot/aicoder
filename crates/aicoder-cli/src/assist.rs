//! Editor-facing commands: `explain` and `gen`.
//!
//! Both read from stdin and write to stdout so they can be used as Helix pipe
//! commands (`:pipe-to` / `:pipe`).

use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use aicoder_core::{
    config::RuntimeConfig,
    context::{
        detect_language, enclosing_scope, extract_imports, insert_cursor_marker, locate_selection,
        nearby_lines, project_docs, referenced_symbol_defs, Language, CURSOR_MARKER,
    },
    models::{strip_fences, RawRequest},
    providers::build_provider,
};
use anyhow::{Context, Result};

use crate::popup;

/// Reads all of stdin as UTF-8.
pub fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("failed reading stdin")?;
    Ok(buf)
}

/// Loads the file the selection came from, if it exists.
fn load_file(file: Option<&Path>) -> Option<(PathBuf, String)> {
    let path = file?;
    let content = std::fs::read_to_string(path).ok()?;
    Some((path.to_path_buf(), content))
}

/// Converts a 1-based CLI line number to a 0-based index.
fn zero_based(line: Option<usize>) -> Option<usize> {
    line.map(|l| l.saturating_sub(1))
}

/// Renders a labelled context section, skipping empty bodies.
fn section(out: &mut String, title: &str, body: &str) {
    if body.trim().is_empty() {
        return;
    }
    out.push_str(&format!("## {title}\n{}\n\n", body.trim_end()));
}

/// Builds the shared context block used by both commands.
fn build_context(
    file: Option<&Path>,
    content: Option<&str>,
    lang: &Language,
    focus_line: Option<usize>,
    selection: &str,
    include_docs: bool,
) -> String {
    let mut ctx = String::new();

    if let Some(path) = file {
        section(
            &mut ctx,
            "Datei",
            &format!("{} (Sprache: {})", path.display(), lang.name),
        );
    }

    let Some(content) = content else {
        return ctx;
    };

    let imports = extract_imports(content, lang);
    if !imports.is_empty() {
        section(&mut ctx, "Imports", &imports.join("\n"));
    }

    if let Some(scope) = focus_line.and_then(|line| enclosing_scope(content, line, lang)) {
        section(
                &mut ctx,
            &format!("Umschliessender Block ({})", scope.signature),
            &scope.text,
        );
    }

    if let (Some(path), false) = (file, selection.trim().is_empty()) {
        let defs = referenced_symbol_defs(selection, path, content, lang);
        if !defs.is_empty() {
            let body = defs
                .iter()
                .map(|d| format!("// {} aus {}\n{}", d.name, d.file.display(), d.snippet))
                .collect::<Vec<_>>()
                .join("\n\n");
            section(&mut ctx, "Definitionen referenzierter Symbole", &body);
        }
    }

    if let Some((doc_path, doc)) = file.filter(|_| include_docs).and_then(project_docs) {
        section(
            &mut ctx,
            &format!("Projektdoku ({})", doc_path.display()),
            &doc,
        );
    }

    ctx
}

/// Popup configuration for [`run_explain`].
#[derive(Debug, Clone, Copy)]
pub struct PopupOptions<'a> {
    pub enabled: bool,
    pub width: &'a str,
    pub height: &'a str,
}

/// Builds the argument list for the `explain` process running in the popup.
#[allow(clippy::too_many_arguments)]
fn explain_child_args(
    file: Option<&Path>,
    line: Option<usize>,
    answer_lang: &str,
    max_lines: usize,
    no_docs: bool,
    stdin_file: &Path,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["explain".to_string()];
    if let Some(path) = file {
        args.push("--file".into());
        args.push(path.display().to_string());
    }
    if let Some(line) = line {
        args.push("--line".into());
        args.push(line.to_string());
    }
    args.push("--lang".into());
    args.push(answer_lang.to_string());
    args.push("--max-lines".into());
    args.push(max_lines.to_string());
    if no_docs {
        args.push("--no-docs".into());
    }
    if let Some(provider) = provider_override {
        args.push("--provider".into());
        args.push(provider.to_string());
    }
    if let Some(model) = model_override {
        args.push("--model".into());
        args.push(model.to_string());
    }
    args.push("--stdin-file".into());
    args.push(stdin_file.display().to_string());
    args
}

/// `aicoder explain`: streams a short explanation of the piped selection.
#[allow(clippy::too_many_arguments)]
pub async fn run_explain(
    file: Option<&Path>,
    line: Option<usize>,
    answer_lang: &str,
    max_lines: usize,
    no_docs: bool,
    popup_opts: PopupOptions<'_>,
    stdin_file: Option<&Path>,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<()> {
    let selection = match stdin_file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("failed reading selection file {}", path.display()))?;
            popup::cleanup(path);
            text
        }
        None => read_stdin()?,
    };

    if selection.trim().is_empty() {
        anyhow::bail!("no code on stdin: select code before running explain");
    }

    // When asked for a popup, re-launch ourselves inside tmux and return.
    if popup_opts.enabled && stdin_file.is_none() {
        if !popup::in_tmux() {
            eprintln!("warning: --popup requires tmux; falling back to stdout");
        } else {
            let temp = popup::write_temp(&selection, ".txt")?;
            let args = explain_child_args(
                file,
                line,
                answer_lang,
                max_lines,
                no_docs,
                &temp,
                provider_override,
                model_override,
            );
            let title = file
                .map(|p| format!(" aicoder explain: {} ", p.display()))
                .unwrap_or_else(|| " aicoder explain ".to_string());
            let result = popup::launch_popup(
                &args,
                popup_opts.width,
                popup_opts.height,
                &title,
            );
            if result.is_err() {
                popup::cleanup(&temp);
            }
            return result;
        }
    }

    let loaded = load_file(file);
    let content = loaded.as_ref().map(|(_, c)| c.as_str());
    let lang = file
        .map(detect_language)
        .unwrap_or_else(|| detect_language(Path::new("x.txt")));

    let focus_line = zero_based(line)
        .or_else(|| content.and_then(|c| locate_selection(c, &selection)));

    let ctx = build_context(file, content, &lang, focus_line, &selection, !no_docs);

    let prompt = format!(
        "{ctx}## Markierter Code\n```{}\n{}\n```\n\n\
         Was macht der Code und wozu? Antworte auf {answer_lang}, maximal {max_lines} Zeilen, \
         ohne Code-Fences, ohne Wiederholung des Codes.",
        lang.name,
        selection.trim_end()
    );

    let cfg = RuntimeConfig::load(provider_override, model_override)?;
    let provider = build_provider(cfg.provider, &cfg).context("failed to initialize provider")?;

    let req = RawRequest::new(&cfg.model, prompt)
        .with_system("Du bist ein praeziser Code-Erklaerer. Fasse dich kurz.")
        .with_max_tokens(1024);

    let stdout = io::stdout();
    let mut sink = move |delta: &str| {
        let mut handle = stdout.lock();
        let _ = handle.write_all(delta.as_bytes());
        let _ = handle.flush();
    };

    let answer = provider
        .complete_stream(&req, &mut sink)
        .await
        .context("provider request failed")?;

    if answer.trim().is_empty() {
        anyhow::bail!("model returned an empty explanation");
    }
    if !answer.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// `aicoder gen`: turns the piped instruction into bare code on stdout.
pub async fn run_gen(
    file: Option<&Path>,
    line: Option<usize>,
    column: usize,
    radius: usize,
    provider_override: Option<&str>,
    model_override: Option<&str>,
) -> Result<()> {
    let instruction = read_stdin()?;
    if instruction.trim().is_empty() {
        anyhow::bail!("no instruction on stdin: select the instruction text before running gen");
    }

    let loaded = load_file(file);
    let content = loaded.as_ref().map(|(_, c)| c.as_str());
    let lang = file
        .map(detect_language)
        .unwrap_or_else(|| detect_language(Path::new("x.txt")));

    let focus_line = zero_based(line)
        .or_else(|| content.and_then(|c| locate_selection(c, &instruction)))
        .unwrap_or(0);

    let mut ctx = build_context(file, content, &lang, Some(focus_line), &instruction, false);

    if let Some(content) = content {
        let marked = insert_cursor_marker(content, focus_line, column);
        section(
            &mut ctx,
            "Umgebung mit Cursor-Marker",
            &nearby_lines(&marked, focus_line, radius),
        );
    }

    let prompt = format!(
        "{ctx}## Anweisung\n{}\n\n\
         Erzeuge den Code, der an der Stelle {CURSOR_MARKER} eingefuegt wird.\n\
         Gib NUR Code aus, keine Fences, kein Text, keine Erklaerung, \
         im Stil und in der Einrueckung der Datei ({}).",
        instruction.trim(),
        lang.name
    );

    let cfg = RuntimeConfig::load(provider_override, model_override)?;
    let provider = build_provider(cfg.provider, &cfg).context("failed to initialize provider")?;

    let req = RawRequest::new(&cfg.model, prompt)
        .with_system(
            "Du bist ein Code-Generator. Du antwortest ausschliesslich mit reinem Quellcode.",
        )
        .with_max_tokens(2048);

    let raw = provider
        .complete(&req)
        .await
        .context("provider request failed")?;

    let code = strip_fences(&raw);
    if code.trim().is_empty() {
        anyhow::bail!("model returned no code");
    }

    print!("{code}");
    if !code.ends_with('\n') {
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_child_args_for_popup() {
        let args = explain_child_args(
            Some(Path::new("src/main.rs")),
            Some(12),
            "Deutsch",
            8,
            true,
            Path::new("/tmp/sel.txt"),
            Some("ollama"),
            None,
        );
        assert_eq!(args[0], "explain");
        assert!(args.windows(2).any(|w| w == ["--file", "src/main.rs"]));
        assert!(args.windows(2).any(|w| w == ["--line", "12"]));
        assert!(args.windows(2).any(|w| w == ["--provider", "ollama"]));
        assert!(args.windows(2).any(|w| w == ["--stdin-file", "/tmp/sel.txt"]));
        assert!(args.contains(&"--no-docs".to_string()));
        // No --model override was requested.
        assert!(!args.contains(&"--model".to_string()));
    }

    #[test]
    fn omits_optional_child_args() {
        let args = explain_child_args(
            None,
            None,
            "English",
            5,
            false,
            Path::new("/tmp/sel.txt"),
            None,
            None,
        );
        assert!(!args.contains(&"--file".to_string()));
        assert!(!args.contains(&"--line".to_string()));
        assert!(!args.contains(&"--no-docs".to_string()));
    }
}
