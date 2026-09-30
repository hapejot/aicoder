//! tmux popup helper.
//!
//! Helix has no popup API, but when it runs inside tmux we can hand the
//! rendering off to `tmux display-popup`. The current process writes the
//! selection to a temp file and re-launches itself inside the popup, so the
//! streamed answer appears there while Helix stays responsive.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

/// Returns true when the process runs inside a tmux session.
pub fn in_tmux() -> bool {
    env::var_os("TMUX").is_some()
}

/// Creates a unique temp file holding `content`.
pub fn write_temp(content: &str, suffix: &str) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = env::temp_dir().join(format!("aicoder-{}-{nanos}{suffix}", std::process::id()));
    fs::write(&path, content).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(path)
}

/// Launches `args` in a tmux popup.
///
/// The popup is opened without `-E`, so it stays visible after the command
/// finishes and closes on the next key press. The tmux client is spawned
/// detached so the caller (and therefore Helix) is not blocked while the
/// answer streams in.
pub fn launch_popup(args: &[String], width: &str, height: &str, title: &str) -> Result<()> {
    let exe = env::current_exe().context("failed resolving own executable path")?;
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    Command::new("tmux")
        .arg("display-popup")
        .args(["-w", width])
        .args(["-h", height])
        .args(["-T", title])
        // Start in our cwd so relative --file paths keep resolving.
        .arg("-d")
        .arg(&cwd)
        .arg("--")
        .arg(&exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to run `tmux display-popup` (is tmux installed?)")?;

    Ok(())
}

/// Removes a temp file, ignoring errors.
pub fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
}
