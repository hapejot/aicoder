use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "aicoder")]
#[command(about = "Terminal AI coding assistant", long_about = None)]
pub struct Cli {
    #[arg(long, global = true)]
    pub provider: Option<String>,

    #[arg(long, global = true)]
    pub model: Option<String>,

    #[arg(long, global = true)]
    pub dry_run: bool,

    #[arg(long, global = true, default_value_t = 100)]
    pub chunk_lines: usize,

    #[arg(long, global = true, default_value_t = 6)]
    pub retrieval_chunks: usize,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Prints the effective configuration and probes provider connections.
    Status {
        /// Only print the configuration, do not contact any provider.
        #[arg(long)]
        no_probe: bool,

        /// Probe every known provider, not just the active one.
        #[arg(long)]
        all: bool,
    },

    Edit {
        file: std::path::PathBuf,
        prompt: String,
    },
    EditMany {
        #[arg(long = "files-from")]
        files_from: Option<std::path::PathBuf>,

        #[arg(long = "glob")]
        globs: Vec<String>,

        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,

        prompt: String,
    },

    /// Explains code read from stdin (for editor integration).
    ///
    /// stdin = selected code, stdout = streamed explanation.
    Explain {
        /// Path of the file the selection comes from.
        #[arg(long)]
        file: Option<std::path::PathBuf>,

        /// 1-based line number of the selection start.
        #[arg(long)]
        line: Option<usize>,

        /// Answer language, e.g. `Deutsch` or `English`.
        #[arg(long, default_value = "Deutsch")]
        lang: String,

        /// Maximum number of output lines requested from the model.
        #[arg(long, default_value_t = 8)]
        max_lines: usize,

        /// Do not include README/AGENTS.md in the context.
        #[arg(long)]
        no_docs: bool,

        /// Show the answer in a tmux popup instead of writing to stdout.
        #[arg(long)]
        popup: bool,

        /// Popup width, e.g. `80%` or `100`.
        #[arg(long, default_value = "80%")]
        popup_width: String,

        /// Popup height, e.g. `60%` or `20`.
        #[arg(long, default_value = "60%")]
        popup_height: String,

        /// Read the selection from a file instead of stdin (used internally
        /// when re-launching inside a tmux popup).
        #[arg(long, hide = true)]
        stdin_file: Option<std::path::PathBuf>,
    },

    /// Generates code from a free-text instruction read from stdin.
    ///
    /// stdin = instruction, stdout = bare code (no fences, no prose).
    Gen {
        /// Path of the file the code is generated for.
        #[arg(long)]
        file: Option<std::path::PathBuf>,

        /// 1-based cursor line.
        #[arg(long)]
        line: Option<usize>,

        /// 0-based cursor column.
        #[arg(long, default_value_t = 0)]
        column: usize,

        /// Number of context lines around the cursor.
        #[arg(long, default_value_t = 40)]
        radius: usize,
    },
}
