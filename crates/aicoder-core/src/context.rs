//! Lightweight, dependency-free source context extraction.
//!
//! Used by the `explain` and `gen` commands to build a useful prompt from a
//! selection inside an editor. The heuristics deliberately avoid a full parser:
//! they work on indentation and brace matching, which is good enough for the
//! short snippets an editor selection produces and keeps the binary small.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Marker inserted at the cursor position for the `gen` command.
pub const CURSOR_MARKER: &str = "<<<CURSOR>>>";

/// Maximum number of sibling files scanned when resolving symbol definitions.
const MAX_SIBLING_FILES: usize = 40;
/// Maximum number of referenced symbol definitions included in a prompt.
const MAX_SYMBOL_DEFS: usize = 6;
/// Maximum number of lines kept per included definition.
const MAX_DEF_LINES: usize = 24;
/// Maximum number of characters read from README/AGENTS.md.
const MAX_DOC_CHARS: usize = 2000;

/// Syntax traits of a language, used by the heuristic scope finder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// Display name used in prompts, e.g. `rust`.
    pub name: &'static str,
    /// Line prefixes that mark an import/use statement.
    pub import_prefixes: &'static [&'static str],
    /// Keywords that start a definition (function, type, impl, ...).
    pub def_keywords: &'static [&'static str],
    /// Whether blocks are delimited by braces (otherwise: indentation).
    pub braces: bool,
    /// Line comment token, used to annotate generated context.
    pub line_comment: &'static str,
}

const RUST: Language = Language {
    name: "rust",
    import_prefixes: &["use ", "extern crate ", "#["],
    def_keywords: &[
        "fn ", "struct ", "enum ", "trait ", "impl ", "mod ", "macro_rules!", "type ", "const ",
        "static ",
    ],
    braces: true,
    line_comment: "//",
};

const PYTHON: Language = Language {
    name: "python",
    import_prefixes: &["import ", "from "],
    def_keywords: &["def ", "class ", "async def "],
    braces: false,
    line_comment: "#",
};

const JS_TS: Language = Language {
    name: "typescript",
    import_prefixes: &["import ", "const ", "require(", "export "],
    def_keywords: &[
        "function ", "class ", "interface ", "type ", "enum ", "const ", "let ", "var ",
    ],
    braces: true,
    line_comment: "//",
};

const GO: Language = Language {
    name: "go",
    import_prefixes: &["import ", "package "],
    def_keywords: &["func ", "type ", "var ", "const "],
    braces: true,
    line_comment: "//",
};

const C_LIKE: Language = Language {
    name: "c",
    import_prefixes: &["#include", "#define", "using "],
    def_keywords: &["struct ", "class ", "enum ", "typedef ", "void ", "int ", "static "],
    braces: true,
    line_comment: "//",
};

const GENERIC: Language = Language {
    name: "text",
    import_prefixes: &["import ", "use ", "#include"],
    def_keywords: &["fn ", "def ", "function ", "class ", "struct "],
    braces: true,
    line_comment: "//",
};

/// Guesses the language from a file extension.
pub fn detect_language(path: &Path) -> Language {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => RUST,
        "py" | "pyi" => PYTHON,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => JS_TS,
        "go" => GO,
        "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "java" | "cs" => C_LIKE,
        _ => GENERIC,
    }
}

/// A resolved source scope (function, impl block, class, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// 0-based inclusive start line.
    pub start_line: usize,
    /// 0-based inclusive end line.
    pub end_line: usize,
    /// The scope's source text.
    pub text: String,
    /// The signature line that opened the scope.
    pub signature: String,
}

/// Returns the indentation width of a line, tabs counted as one column.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Checks whether a trimmed line looks like a definition header.
fn is_definition_line(line: &str, lang: &Language) -> bool {
    let mut rest = line.trim_start();
    // Skip visibility / modifier prefixes so `pub async fn x` still matches.
    for modifier in [
        "pub(crate) ", "pub(super) ", "pub ", "export default ", "export ", "async ", "public ",
        "private ", "protected ", "static ", "final ", "unsafe ", "extern ", "default ",
    ] {
        if let Some(stripped) = rest.strip_prefix(modifier) {
            rest = stripped.trim_start();
        }
    }
    lang.def_keywords.iter().any(|kw| rest.starts_with(kw))
}

/// Finds the definition block enclosing `line` (0-based).
///
/// Returns `None` when the line is at top level or no header could be found.
pub fn enclosing_scope(content: &str, line: usize, lang: &Language) -> Option<Scope> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let line = line.min(lines.len() - 1);
    let target_indent = indent_of(lines[line]);

    // Walk upwards for the closest definition header at a smaller indentation.
    let mut start = None;
    for idx in (0..=line).rev() {
        let candidate = lines[idx];
        if candidate.trim().is_empty() {
            continue;
        }
        let candidate_indent = indent_of(candidate);
        if (idx == line || candidate_indent < target_indent || candidate_indent == 0)
            && is_definition_line(candidate, lang)
        {
            start = Some(idx);
            break;
        }
    }
    let start = start?;

    let end = if lang.braces {
        scope_end_by_braces(&lines, start)
    } else {
        scope_end_by_indent(&lines, start)
    };

    Some(Scope {
        start_line: start,
        end_line: end,
        text: lines[start..=end].join("\n"),
        signature: lines[start].trim().to_string(),
    })
}

/// Finds a block end by counting braces from the header line.
fn scope_end_by_braces(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut seen_open = false;

    for (offset, line) in lines[start..].iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if seen_open && depth <= 0 {
            return start + offset;
        }
        // Single-line declarations such as `type Foo = Bar;`.
        if !seen_open && line.trim_end().ends_with(';') {
            return start + offset;
        }
    }
    lines.len() - 1
}

/// Finds a block end by indentation (Python-style languages).
fn scope_end_by_indent(lines: &[&str], start: usize) -> usize {
    let base = indent_of(lines[start]);
    let mut end = start;
    for (offset, line) in lines[start + 1..].iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) <= base {
            return end;
        }
        end = start + 1 + offset;
    }
    end
}

/// Collects import/use statements from the top of a file.
///
/// Multi-line statements such as `use std::{\n  fmt,\n};` are kept intact.
pub fn extract_imports(content: &str, lang: &Language) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut depth: i32 = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        if current.is_empty() {
            let starts_import = lang.import_prefixes.iter().any(|p| trimmed.starts_with(p));
            if !starts_import {
                // Stop at the first real code line; blanks and comments are skipped.
                if trimmed.is_empty() || trimmed.starts_with(lang.line_comment) {
                    continue;
                }
                if is_definition_line(line, lang) {
                    break;
                }
                continue;
            }
        }

        depth += trimmed.matches(['{', '(']).count() as i32;
        depth -= trimmed.matches(['}', ')']).count() as i32;
        current.push(trimmed.to_string());

        let complete = depth <= 0 && (trimmed.ends_with(';') || !lang.braces || depth == 0);
        if complete && depth <= 0 {
            out.push(current.join("\n"));
            current.clear();
            depth = 0;
        }
    }

    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

/// A symbol definition found for an identifier referenced in the selection.
#[derive(Debug, Clone)]
pub struct SymbolDef {
    pub name: String,
    pub file: PathBuf,
    pub snippet: String,
}

/// Extracts identifier-like tokens from a snippet.
pub fn identifiers(snippet: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in snippet.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out.retain(|token| token.len() >= 3 && !token.chars().next().unwrap().is_ascii_digit());
    out.sort();
    out.dedup();
    out
}

/// Looks up definitions of symbols referenced in `selection`.
///
/// Searches the current file first, then sibling files with the same
/// extension. This is the cheap local stand-in for an LSP `definition` call.
pub fn referenced_symbol_defs(
    selection: &str,
    file: &Path,
    content: &str,
    lang: &Language,
) -> Vec<SymbolDef> {
    let wanted = identifiers(selection);
    if wanted.is_empty() {
        return Vec::new();
    }

    let mut sources: Vec<(PathBuf, String)> = vec![(file.to_path_buf(), content.to_string())];
    for sibling in sibling_files(file) {
        if let Ok(text) = fs::read_to_string(&sibling) {
            sources.push((sibling, text));
        }
    }

    let mut found: Vec<SymbolDef> = Vec::new();
    for name in &wanted {
        if found.len() >= MAX_SYMBOL_DEFS {
            break;
        }
        for (path, text) in &sources {
            if let Some(def) = find_definition(text, name, lang) {
                // Skip definitions that are the selection itself.
                if path == file && selection.contains(def.trim()) {
                    continue;
                }
                found.push(SymbolDef {
                    name: name.clone(),
                    file: path.clone(),
                    snippet: def,
                });
                break;
            }
        }
    }
    found
}

/// Finds the definition of `name` inside `text`, truncated to a few lines.
fn find_definition(text: &str, name: &str, lang: &Language) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        if !is_definition_line(line, lang) {
            continue;
        }
        if !declares_name(line, name) {
            continue;
        }
        let scope = enclosing_scope(text, idx, lang)?;
        let snippet: Vec<&str> = scope.text.lines().take(MAX_DEF_LINES).collect();
        let mut out = snippet.join("\n");
        if scope.text.lines().count() > MAX_DEF_LINES {
            out.push_str("\n// ... truncated ...");
        }
        return Some(out);
    }
    None
}

/// Checks whether a definition header declares exactly `name`.
fn declares_name(line: &str, name: &str) -> bool {
    let mut rest = line;
    while let Some(pos) = rest.find(name) {
        let before_ok = rest[..pos]
            .chars()
            .next_back()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        let after = &rest[pos + name.len()..];
        let after_ok = after
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        rest = after;
    }
    false
}

/// Lists files next to `file` that share its extension.
fn sibling_files(file: &Path) -> Vec<PathBuf> {
    let Some(dir) = file.parent() else {
        return Vec::new();
    };
    let ext = file.extension().and_then(|e| e.to_str());
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p != file)
        .filter(|p| ext.is_none() || p.extension().and_then(|e| e.to_str()) == ext)
        .collect();
    out.sort();
    out.truncate(MAX_SIBLING_FILES);
    out
}

/// Reads project documentation (`AGENTS.md`, `README.md`) near a file.
///
/// Walks upwards from the file's directory until a match is found.
pub fn project_docs(file: &Path) -> Option<(PathBuf, String)> {
    let mut dir = file.parent()?.to_path_buf();
    loop {
        for candidate in ["AGENTS.md", "CLAUDE.md", "README.md"] {
            let path = dir.join(candidate);
            if !path.is_file() {
                continue;
            }
            if let Ok(mut text) = fs::read_to_string(&path) {
                text.truncate(
                    text.char_indices()
                        .nth(MAX_DOC_CHARS)
                        .map(|(i, _)| i)
                        .unwrap_or(text.len()),
                );
                return Some((path, text));
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Inserts [`CURSOR_MARKER`] at the given 0-based line/column.
pub fn insert_cursor_marker(content: &str, line: usize, column: usize) -> String {
    let mut out: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    if out.is_empty() {
        return CURSOR_MARKER.to_string();
    }
    let line = line.min(out.len() - 1);
    let target = &out[line];
    let column = column.min(target.chars().count());
    let byte_idx = target
        .char_indices()
        .nth(column)
        .map(|(i, _)| i)
        .unwrap_or(target.len());
    out[line] = format!("{}{}{}", &target[..byte_idx], CURSOR_MARKER, &target[byte_idx..]);
    out.join("\n")
}

/// Returns `radius` lines above and below `line` as a numbered excerpt.
pub fn nearby_lines(content: &str, line: usize, radius: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let line = line.min(lines.len() - 1);
    let start = line.saturating_sub(radius);
    let end = (line + radius).min(lines.len() - 1);
    lines[start..=end]
        .iter()
        .enumerate()
        .map(|(offset, text)| format!("{:>5} | {}", start + offset + 1, text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Locates the first line of `selection` inside `content` (0-based).
pub fn locate_selection(content: &str, selection: &str) -> Option<usize> {
    let needle = selection.lines().next()?.trim();
    if needle.is_empty() {
        return None;
    }
    content
        .lines()
        .position(|line| line.trim() == needle)
        .or_else(|| content.lines().position(|line| line.contains(needle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_SRC: &str = "use std::fmt;\nuse serde::Serialize;\n\nfn helper(x: i32) -> i32 {\n    x + 1\n}\n\nimpl Foo {\n    fn bar(&self) -> i32 {\n        helper(self.x)\n    }\n}\n";

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(detect_language(Path::new("a.rs")).name, "rust");
        assert_eq!(detect_language(Path::new("a.py")).name, "python");
        assert_eq!(detect_language(Path::new("a.unknown")).name, "text");
    }

    #[test]
    fn extracts_imports() {
        let imports = extract_imports(RUST_SRC, &RUST);
        assert_eq!(imports, vec!["use std::fmt;", "use serde::Serialize;"]);
    }

    #[test]
    fn extracts_multiline_imports() {
        let src = "use std::{\n    fmt,\n    io::Write,\n};\n\nfn main() {}\n";
        let imports = extract_imports(src, &RUST);
        assert_eq!(imports.len(), 1);
        assert!(imports[0].contains("io::Write"));
        assert!(imports[0].ends_with("};"));
    }

    #[test]
    fn stops_at_first_definition() {
        let src = "use a::b;\n\nfn main() {}\n\nuse late::thing;\n";
        assert_eq!(extract_imports(src, &RUST), vec!["use a::b;"]);
    }

    #[test]
    fn finds_enclosing_function_by_braces() {
        // Line 9 (0-based) is inside `fn bar`.
        let scope = enclosing_scope(RUST_SRC, 9, &RUST).expect("scope");
        assert_eq!(scope.signature, "fn bar(&self) -> i32 {");
        assert!(scope.text.contains("helper(self.x)"));
        assert!(scope.text.trim_end().ends_with('}'));
    }

    #[test]
    fn finds_enclosing_block_by_indentation() {
        let src = "import os\n\ndef outer():\n    x = 1\n    return x\n\ndef other():\n    pass\n";
        let scope = enclosing_scope(src, 4, &PYTHON).expect("scope");
        assert_eq!(scope.signature, "def outer():");
        assert!(scope.text.contains("return x"));
        assert!(!scope.text.contains("other"));
    }

    #[test]
    fn resolves_referenced_symbol_in_same_file() {
        let defs = referenced_symbol_defs(
            "helper(self.x)",
            Path::new("/tmp/does-not-exist.rs"),
            RUST_SRC,
            &RUST,
        );
        assert!(defs.iter().any(|d| d.name == "helper"));
    }

    #[test]
    fn inserts_cursor_marker_at_position() {
        let out = insert_cursor_marker("abc\ndef", 1, 1);
        assert_eq!(out, format!("abc\nd{}ef", CURSOR_MARKER));
    }

    #[test]
    fn locates_selection_line() {
        assert_eq!(locate_selection(RUST_SRC, "    x + 1"), Some(4));
    }

    #[test]
    fn identifiers_skips_short_tokens() {
        let ids = identifiers("let ab = helper(1);");
        assert!(ids.contains(&"helper".to_string()));
        assert!(!ids.contains(&"ab".to_string()));
    }
}
