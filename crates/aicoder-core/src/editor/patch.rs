use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOp {
    pub find: String,
    pub replace: String,
}

pub fn parse_patch_ops(raw: &str) -> Vec<PatchOp> {
    let root = extract_optional_root(raw, "patches").unwrap_or(raw);
    let mut out = Vec::new();
    let mut rest = root;

    while let Some(start) = rest.find("<patch>") {
        let after_start = &rest[start + "<patch>".len()..];
        let Some(end) = after_start.find("</patch>") else {
            break;
        };
        let block = &after_start[..end];

        let find = extract_tag(block, "find").unwrap_or_default();
        let replace = extract_tag(block, "replace").unwrap_or_default();
        if !find.is_empty() {
            out.push(PatchOp { find, replace });
        }

        rest = &after_start[end + "</patch>".len()..];
    }

    out
}

pub fn apply_patch_ops(original: &str, ops: &[PatchOp]) -> Result<String> {
    let mut current = original.to_string();

    for (idx, op) in ops.iter().enumerate() {
        if op.find.is_empty() {
            return Err(anyhow!("patch {} has empty <find>", idx + 1));
        }

        let Some(pos) = current.find(&op.find) else {
            return Err(anyhow!(
                "patch {} could not be applied: <find> snippet not found",
                idx + 1
            ));
        };

        current.replace_range(pos..pos + op.find.len(), &op.replace);
    }

    Ok(current)
}

fn extract_optional_root<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)?;
    let end = text.find(&close)?;
    Some(&text[start + open.len()..end])
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)?;
    let end = text[start + open.len()..].find(&close)? + start + open.len();
    let inner = text[start + open.len()..end].trim();
    Some(strip_cdata(inner).to_string())
}

fn strip_cdata(s: &str) -> &str {
    let cdata_start = "<![CDATA[";
    let cdata_end = "]]>";
    if let Some(start) = s.find(cdata_start) {
        let after = &s[start + cdata_start.len()..];
        if let Some(end) = after.find(cdata_end) {
            return &after[..end];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_patch_ops() {
        let raw = "<patches><patch><find><![CDATA[a]]></find><replace><![CDATA[b]]></replace></patch></patches>";
        let ops = parse_patch_ops(raw);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].find, "a");
        assert_eq!(ops[0].replace, "b");
    }

    #[test]
    fn applies_patch_ops() {
        let ops = vec![PatchOp {
            find: "hello".to_string(),
            replace: "hi".to_string(),
        }];
        let out = apply_patch_ops("hello world", &ops).unwrap();
        assert_eq!(out, "hi world");
    }

    #[test]
    fn fails_if_find_missing() {
        let ops = vec![PatchOp {
            find: "x".to_string(),
            replace: "y".to_string(),
        }];
        assert!(apply_patch_ops("abc", &ops).is_err());
    }
}
