/// Splits source code into chunks that always end at a top-level structural boundary
/// (i.e., when brace depth returns to 0, so every chunk contains only complete items).
///
/// - `//` and `/* */` comments are ignored when counting braces.
/// - String literals `"..."` (with `\"` escape) are ignored.
/// - If a single top-level item exceeds `max_lines`, it forms its own oversized chunk
///   rather than being split mid-structure.
pub fn split_into_structural_chunks(content: &str, max_lines: usize) -> Vec<String> {
    if max_lines == 0 || content.is_empty() {
        return vec![content.to_string()];
    }

    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let safe_splits = find_safe_split_lines(&lines);
    group_chunks(&lines, &safe_splits, max_lines)
}

/// Returns the indices of lines after which brace depth is 0 and we are outside
/// any comment or string literal — these are safe chunk boundaries.
fn find_safe_split_lines(lines: &[&str]) -> Vec<usize> {
    let mut depth: i32 = 0;
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut escape_next = false;
    let mut safe = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            if escape_next {
                escape_next = false;
                i += 1;
                continue;
            }

            if in_block_comment {
                if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                    in_block_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }

            if in_string {
                match chars[i] {
                    '\\' => {
                        escape_next = true;
                        i += 1;
                    }
                    '"' => {
                        in_string = false;
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
                continue;
            }

            match chars[i] {
                '/' if i + 1 < len && chars[i + 1] == '/' => {
                    break; // rest of line is a comment
                }
                '/' if i + 1 < len && chars[i + 1] == '*' => {
                    in_block_comment = true;
                    i += 2;
                }
                '"' => {
                    in_string = true;
                    i += 1;
                }
                '{' => {
                    depth += 1;
                    i += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth < 0 {
                        depth = 0;
                    }
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }

        if depth == 0 && !in_block_comment && !in_string {
            safe.push(line_idx);
        }
    }

    safe
}

/// Groups lines into chunks. Each chunk ends at a safe split point and stays
/// within `max_lines` if possible. Oversized single items form their own chunk.
fn group_chunks(lines: &[&str], safe_splits: &[usize], max_lines: usize) -> Vec<String> {
    if safe_splits.is_empty() {
        // No structural boundaries found — fall back to line-based splitting.
        return lines
            .chunks(max_lines)
            .map(|chunk| chunk.concat())
            .collect();
    }

    let total_lines = lines.len();
    let mut chunks = Vec::new();
    let mut chunk_start = 0;

    while chunk_start < total_lines {
        let limit_line = chunk_start + max_lines;

        // Best split: last safe point within [chunk_start, limit_line).
        let best = safe_splits
            .iter()
            .filter(|&&s| s >= chunk_start && s < limit_line)
            .last()
            .copied();

        let chunk_end = match best {
            Some(last_safe) => last_safe + 1,
            None => {
                // No safe split fits within max_lines.
                // Use the next safe split (chunk will be oversized but structurally intact).
                safe_splits
                    .iter()
                    .find(|&&s| s >= chunk_start)
                    .map(|&s| s + 1)
                    .unwrap_or(total_lines)
            }
        };

        chunks.push(lines[chunk_start..chunk_end].concat());
        chunk_start = chunk_end;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_function_boundary() {
        let src = "fn a() {\n    let x = 1;\n}\nfn b() {\n    let y = 2;\n}\n";
        // With max_lines=3 the first function (3 lines) fits in one chunk,
        // the second function starts a new chunk.
        let chunks = split_into_structural_chunks(src, 3);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("fn a()"));
        assert!(chunks[1].contains("fn b()"));
    }

    #[test]
    fn does_not_split_inside_function() {
        let src = "fn big() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n}\nfn small() {}\n";
        // max_lines=3 — the big function has 5 lines and cannot be split,
        // so it forms one oversized chunk.
        let chunks = split_into_structural_chunks(src, 3);
        assert!(chunks.iter().any(|c| c.contains("fn big()")));
        // big() must be in one piece
        let big_chunk = chunks.iter().find(|c| c.contains("fn big()")).unwrap();
        assert!(big_chunk.contains("let a") && big_chunk.contains("let c"));
    }

    #[test]
    fn ignores_braces_in_string_literals() {
        let src = "fn a() {\n    let s = \"{not a brace}\";\n}\nfn b() {}\n";
        let chunks = split_into_structural_chunks(src, 3);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn ignores_braces_in_line_comments() {
        let src = "fn a() {\n    // { this should not count\n}\nfn b() {}\n";
        let chunks = split_into_structural_chunks(src, 3);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn handles_single_chunk_when_small() {
        let src = "fn a() {}\n";
        let chunks = split_into_structural_chunks(src, 500);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn ignores_braces_in_block_comments() {
        let src = "fn a() {\n    /* { not a brace */\n}\nfn b() {}\n";
        let chunks = split_into_structural_chunks(src, 3);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn multiple_small_fns_merge_into_one_chunk() {
        // 3 tiny fns, max_lines=10 -> all should be in one chunk
        let src = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let chunks = split_into_structural_chunks(src, 10);
        assert_eq!(chunks.len(), 1);
    }
}
