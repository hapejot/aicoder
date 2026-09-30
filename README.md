# aicoder

Terminal-based AI coding assistant written in Rust.

## MVP status

- Anthropic provider implemented (default)
- Ollama provider implemented (optional)
- Single-file edit flow with mandatory diff preview
- Multi-file edit flow via file-list and glob pattern
- Interactive refine loop before applying changes
- Atomic write with timestamped backup
- Editor-facing `explain` and `gen` commands (stdin/stdout, streaming)

## Quick start

1. Export Anthropic key:

```bash
export ANTHROPIC_API_KEY="your-key"
```

2. Optional config at `~/.config/aicoder/config.toml`:

```toml
[default]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[providers.anthropic]
api_key = "optional-fallback-if-env-not-set"
api_base = "https://api.anthropic.com/v1"
timeout_secs = 60

[providers.ollama]
api_base = "http://localhost:11434"
timeout_secs = 180
```

3. Run:

```bash
cargo run -p aicoder-cli -- edit path/to/file.rs "Improve error handling"
```

Use `--provider ollama --model qwen2.5-coder` to route to local Ollama.

For larger files, use chunking (default: 500 lines per chunk):

```bash
cargo run -p aicoder-cli -- --chunk-lines 350 edit path/to/large.rs "Refactor error handling"
```

For very large files, the tool automatically switches to intent retrieval + patch mode:

- selects the most relevant chunks for your prompt
- asks the model for targeted find/replace patches
- applies patches to the original file

Tune retrieval breadth with:

```bash
cargo run -p aicoder-cli -- --chunk-lines 120 --retrieval-chunks 8 edit path/to/large.rs "Rename FooService to EngineService"
```

## Multi-file edit

Use a file list and/or glob patterns.

Example file list (`targets.txt`):

```txt
# one file per line
src/main.rs
src/lib.rs
crates/aicoder-core/src/config.rs
```

Run with file list:

```bash
cargo run -p aicoder-cli -- edit-many --files-from targets.txt "Add better error messages"
```

Run with glob:

```bash
cargo run -p aicoder-cli -- edit-many --glob "crates/**/*.rs" "Add module-level docs"
```

Combine sources:

```bash
cargo run -p aicoder-cli -- edit-many --files-from targets.txt --glob "examples/*.rs" --file README.md "Normalize wording"
```

## Helix integration (`explain` / `gen`)

Both commands read stdin and write stdout, so they work as Helix pipe commands.

### `aicoder explain`

stdin = the selected code, stdout = a streamed short explanation.

Context sent to the model:

- the selection plus file path and detected language
- the enclosing function/impl/class block (brace- and indentation-aware)
- definitions of symbols referenced in the selection (current file, then siblings)
- optionally `AGENTS.md` / `CLAUDE.md` / `README.md` found by walking up the tree

```bash
printf 'fn foo() {}' | aicoder explain --file src/main.rs --line 12
```

Options: `--lang Deutsch` (answer language), `--max-lines 8`, `--no-docs`.

#### tmux popup

With `--popup` the answer is rendered in a `tmux display-popup` instead of
stdout. The selection is handed to a detached child process, so Helix is never
blocked while the answer streams in. The popup has no `-E`, so it stays open
after the stream finishes and closes on the next key press.

```bash
printf 'fn foo() {}' | aicoder explain --file src/main.rs --line 12 --popup
```

Popup size is configurable: `--popup-width 80%`, `--popup-height 60%`.
Outside tmux the flag warns and falls back to stdout.

### `aicoder gen`

stdin = a free-text instruction, stdout = bare code (fences stripped),
so Helix can replace the selection with the result.

Context sent to the model: the file with a `<<<CURSOR>>>` marker at the cursor,
its imports, and the enclosing block.

```bash
printf 'parse the config file with serde' | aicoder gen --file src/main.rs --line 42 --column 4
```

Options: `--column 0`, `--radius 40` (context lines around the cursor).

### Keybindings

Add to `~/.config/helix/config.toml`:

```toml
[keys.select.space.a]
# Explain the selection in a tmux popup.
e = ":pipe-to aicoder explain --file %{buffer_name} --line %{cursor_line} --popup"
# Replace the selection with generated code.
g = ":pipe aicoder gen --file %{buffer_name} --line %{cursor_line}"
```

`:pipe` replaces the selection with the command output (used for `gen`), while
`:pipe-to` sends the selection to the command and discards stdout (used for
`explain`, which renders into the popup itself).

Without tmux, write the explanation to a file and open it in a split instead:

```toml
[keys.select.space.a]
e = [
  ":pipe-to sh -c 'aicoder explain --file %{buffer_name} --line %{cursor_line} > /tmp/aicoder-explain.md'",
  ":open /tmp/aicoder-explain.md",
]
```
