mod assist;
mod commands;
mod popup;
mod status;

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use aicoder_core::{
    config::RuntimeConfig,
    editor::{
        chunking::split_into_structural_chunks,
        diff::render_unified_diff,
        file_ops::write_with_backup_atomic,
        patch::{apply_patch_ops, parse_patch_ops},
    },
    models::GenerateRequest,
    providers::{build_provider, AiProvider},
};
use anyhow::{Context, Result};
use clap::Parser;
use commands::{Cli, Command};

enum EditOutcome {
    Applied,
    Skipped,
    Aborted,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Status { no_probe, all } => {
            status::run_status(
                cli.provider.as_deref(),
                cli.model.as_deref(),
                no_probe,
                all,
            )
            .await?;
        }
        Command::Edit { file, prompt } => {
            let _ = run_edit(
                &file,
                &prompt,
                cli.provider.as_deref(),
                cli.model.as_deref(),
                cli.dry_run,
                cli.chunk_lines,
                cli.retrieval_chunks,
            )
            .await?;
        }
        Command::EditMany {
            files_from,
            globs,
            files,
            prompt,
        } => {
            run_edit_many(
                files_from.as_deref(),
                &globs,
                &files,
                &prompt,
                cli.provider.as_deref(),
                cli.model.as_deref(),
                cli.dry_run,
                cli.chunk_lines,
                cli.retrieval_chunks,
            )
            .await?;
        }
        Command::Explain {
            file,
            line,
            lang,
            max_lines,
            no_docs,
            popup,
            popup_width,
            popup_height,
            stdin_file,
        } => {
            assist::run_explain(
                file.as_deref(),
                line,
                &lang,
                max_lines,
                no_docs,
                assist::PopupOptions {
                    enabled: popup,
                    width: &popup_width,
                    height: &popup_height,
                },
                stdin_file.as_deref(),
                cli.provider.as_deref(),
                cli.model.as_deref(),
            )
            .await?;
        }
        Command::Gen {
            file,
            line,
            column,
            radius,
        } => {
            assist::run_gen(
                file.as_deref(),
                line,
                column,
                radius,
                cli.provider.as_deref(),
                cli.model.as_deref(),
            )
            .await?;
        }
    }

    Ok(())
}

async fn run_edit_many(
    files_from: Option<&Path>,
    globs: &[String],
    files: &[PathBuf],
    prompt: &str,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    dry_run: bool,
    chunk_lines: usize,
    retrieval_chunks: usize,
) -> Result<()> {
    let targets = resolve_targets(files_from, globs, files)?;
    if targets.is_empty() {
        anyhow::bail!(
            "no target files resolved. Use --files-from, --glob, or --file for at least one file"
        );
    }

    println!("Resolved {} target file(s).", targets.len());

    for file in targets {
        println!("\n=== Processing {} ===", file.display());
        match run_edit(
            &file,
            prompt,
            provider_override,
            model_override,
            dry_run,
            chunk_lines,
            retrieval_chunks,
        )
        .await
        {
            Ok(EditOutcome::Applied | EditOutcome::Skipped) => {}
            Ok(EditOutcome::Aborted) => {
                println!("Aborted by user.");
                return Ok(());
            }
            Err(err) => {
                println!("Failed for {}: {}", file.display(), err);
                let decision = ask_continue_after_error()?;
                if !decision {
                    println!("Aborted after error.");
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn resolve_targets(
    files_from: Option<&Path>,
    globs: &[String],
    files: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let mut set = BTreeSet::new();

    for path in files {
        if path.is_file() {
            set.insert(path.clone());
        }
    }

    if let Some(list_file) = files_from {
        let content = fs::read_to_string(list_file)
            .with_context(|| format!("failed reading file list {}", list_file.display()))?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let path = PathBuf::from(trimmed);
            if path.is_file() {
                set.insert(path);
            }
        }
    }

    for pattern in globs {
        let entries = glob::glob(pattern)
            .with_context(|| format!("invalid glob pattern: {pattern}"))?;
        for entry in entries {
            match entry {
                Ok(path) if path.is_file() => {
                    set.insert(path);
                }
                Ok(_) => {}
                Err(err) => {
                    return Err(anyhow::anyhow!("glob resolution error for {pattern}: {err}"));
                }
            }
        }
    }

    Ok(set.into_iter().collect())
}

async fn run_edit(
    file: &Path,
    prompt: &str,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    dry_run: bool,
    chunk_lines: usize,
    retrieval_chunks: usize,
) -> Result<EditOutcome> {
    let cfg = RuntimeConfig::load(provider_override, model_override)?;
    let provider = build_provider(cfg.provider, &cfg).context("failed to initialize provider")?;

    if !file.exists() {
        anyhow::bail!("file does not exist: {}", file.display());
    }

    let original_content = std::fs::read_to_string(file)
        .with_context(|| format!("failed reading {}", file.display()))?;

    let mut instruction = prompt.to_string();
    loop {
        println!(
            "Using provider={} model={}...",
            cfg.provider.as_str(),
            cfg.model
        );

        let line_count = original_content.lines().count();
        let updated_content = if line_count > chunk_lines {
            println!(
                "Large file detected ({} lines). Using intent retrieval + patch mode.",
                line_count
            );
            generate_retrieved_patch_edit(
                provider.as_ref(),
                file,
                &original_content,
                &instruction,
                &cfg.model,
                chunk_lines,
                retrieval_chunks,
            )
            .await?
        } else {
            generate_single_edit(provider.as_ref(), file, &original_content, &instruction, &cfg.model)
                .await?
        };

        let diff = render_unified_diff(&file.display().to_string(), &original_content, &updated_content);
        println!("\n{}", diff);

        if dry_run {
            println!("Dry-run enabled: no files were modified.");
            return Ok(EditOutcome::Skipped);
        }

        let choice = ask_choice()?;
        match choice.as_str() {
            "a" | "apply" => {
                let backup = write_with_backup_atomic(file, &updated_content)?;
                println!(
                    "Applied changes to {}\nBackup created at {}",
                    file.display(),
                    backup.display()
                );
                return Ok(EditOutcome::Applied);
            }
            "r" | "refine" => {
                print!("Enter refinement instruction: ");
                io::stdout().flush()?;
                let mut refine = String::new();
                io::stdin().read_line(&mut refine)?;
                instruction = format!("{}\n\nRefinement: {}", prompt, refine.trim());
            }
            "s" | "skip" => {
                println!("Skipped {}.", file.display());
                return Ok(EditOutcome::Skipped);
            }
            "q" | "quit" => {
                println!("Aborted.");
                return Ok(EditOutcome::Aborted);
            }
            _ => {
                println!("Unknown choice. Use a/apply, r/refine, s/skip, or q/quit.");
            }
        }
    }
}

async fn generate_single_edit(
    provider: &(dyn AiProvider + Send + Sync),
    file: &Path,
    original_content: &str,
    instruction: &str,
    model: &str,
) -> Result<String> {
    let req = GenerateRequest {
        file_path: file.display().to_string(),
        original_content: original_content.to_string(),
        instruction: instruction.to_string(),
        model: model.to_string(),
    };

    let resp = provider
        .generate_edit(&req)
        .await
        .context("provider request failed")?;

    Ok(resp.updated_content)
}

async fn generate_retrieved_patch_edit(
    provider: &(dyn AiProvider + Send + Sync),
    file: &Path,
    original_content: &str,
    instruction: &str,
    model: &str,
    chunk_lines: usize,
    retrieval_chunks: usize,
) -> Result<String> {
    if chunk_lines == 0 {
        anyhow::bail!("--chunk-lines must be greater than 0");
    }
    if retrieval_chunks == 0 {
        anyhow::bail!("--retrieval-chunks must be greater than 0");
    }

    let chunks = split_into_structural_chunks(original_content, chunk_lines);
    let selected = select_relevant_chunks(&chunks, instruction, retrieval_chunks);
    let retrieved_context = build_retrieved_context(&chunks, &selected);

    println!(
        "Using {} retrieved chunk(s) out of {} total chunks.",
        selected.len(),
        chunks.len()
    );

    let patch_instruction = format!(
        "{instruction}\n\n\
         You are given relevant chunks from a larger file.\n\
         Return ONLY patch operations in this exact format:\n\
         <patches>\n\
           <patch>\n\
             <find><![CDATA[EXACT OLD TEXT FROM ORIGINAL FILE]]></find>\n\
             <replace><![CDATA[NEW TEXT]]></replace>\n\
           </patch>\n\
         </patches>\n\n\
         Rules:\n\
         - <find> must exactly match existing text in the original file.\n\
         - Keep each patch small and precise.\n\
         - Do not return full file content.\n\
         - If no changes are needed, return <patches></patches>."
    );

    let req = GenerateRequest {
        file_path: format!("{} (retrieved patch mode)", file.display()),
        original_content: retrieved_context,
        instruction: patch_instruction,
        model: model.to_string(),
    };

    let resp = provider
        .generate_edit(&req)
        .await
        .context("provider request failed for retrieved patch mode")?;

    let ops = parse_patch_ops(&resp.updated_content);
    if ops.is_empty() {
        anyhow::bail!(
            "model returned no valid patches; try refining prompt or increase --retrieval-chunks"
        );
    }

    apply_patch_ops(original_content, &ops).context("failed applying returned patches")
}

fn select_relevant_chunks(chunks: &[String], instruction: &str, max_chunks: usize) -> Vec<usize> {
    let tokens = tokenize_instruction(instruction);
    let mut scored: Vec<(usize, usize)> = chunks
        .iter()
        .enumerate()
        .map(|(idx, chunk)| (idx, score_chunk(chunk, &tokens)))
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut selected: Vec<usize> = scored
        .into_iter()
        .filter(|(_, score)| *score > 0)
        .take(max_chunks.saturating_sub(1))
        .map(|(idx, _)| idx)
        .collect();

    if !chunks.is_empty() && !selected.contains(&0) {
        selected.push(0);
    }

    if selected.is_empty() {
        selected.extend(0..chunks.len().min(max_chunks));
    }

    selected.sort_unstable();
    selected.dedup();
    selected
}

fn build_retrieved_context(chunks: &[String], selected: &[usize]) -> String {
    let mut out = String::new();
    for idx in selected {
        if let Some(chunk) = chunks.get(*idx) {
            out.push_str(&format!("<chunk id=\"{}\">\n", idx + 1));
            out.push_str(chunk);
            if !chunk.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("</chunk>\n\n");
        }
    }
    out
}

fn tokenize_instruction(instruction: &str) -> Vec<String> {
    instruction
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 3)
        .collect()
}

fn score_chunk(chunk: &str, tokens: &[String]) -> usize {
    let lc = chunk.to_ascii_lowercase();
    tokens
        .iter()
        .map(|token| lc.matches(token).count())
        .sum()
}

fn ask_choice() -> Result<String> {
    print!("Action? [a]pply / [r]efine / [s]kip / [q]uit: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_ascii_lowercase())
}

fn ask_continue_after_error() -> Result<bool> {
    print!("Continue with remaining files? [y]es/[n]o: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}
