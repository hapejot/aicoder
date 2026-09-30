#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub file_path: String,
    pub original_content: String,
    pub instruction: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub updated_content: String,
}

/// A free-form prompt sent to the model without any edit-specific wrapping.
#[derive(Debug, Clone)]
pub struct RawRequest {
    pub system: Option<String>,
    pub prompt: String,
    pub model: String,
    pub max_tokens: u32,
}

impl RawRequest {
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            system: None,
            prompt: prompt.into(),
            model: model.into(),
            max_tokens: 2048,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

/// Removes surrounding markdown code fences from a model answer.
///
/// Handles ```lang ... ``` as well as answers where the model emitted the
/// fence but forgot the closing one.
pub fn strip_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let mut lines: Vec<&str> = trimmed.lines().collect();
    // Drop opening fence (possibly with a language tag).
    lines.remove(0);
    // Drop closing fence if present.
    if lines.last().is_some_and(|l| l.trim_end().starts_with("```")) {
        lines.pop();
    }
    lines.join("\n").trim_matches('\n').to_string()
}

pub fn build_edit_prompt(req: &GenerateRequest) -> String {
    format!(
        "You are an expert coding assistant.\n\
Task: Edit the file according to the user instruction.\n\
Return ONLY the complete updated file content wrapped in <updated_file>...</updated_file>.\n\
Do not include explanations.\n\n\
File path: {}\n\n\
Instruction:\n{}\n\n\
Current file content:\n```\n{}\n```\n",
        req.file_path, req.instruction, req.original_content
    )
}

pub fn extract_updated_content(raw: &str) -> String {
    let start_tag = "<updated_file>";
    let end_tag = "</updated_file>";

    if let (Some(start), Some(end)) = (raw.find(start_tag), raw.find(end_tag)) {
        let inner = &raw[start + start_tag.len()..end];
        return inner.trim_matches('\n').to_string();
    }

    raw.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{extract_updated_content, strip_fences};

    #[test]
    fn strips_fences_with_language_tag() {
        let raw = "```rust\nfn main() {}\n```";
        assert_eq!(strip_fences(raw), "fn main() {}");
    }

    #[test]
    fn strips_unclosed_fence() {
        let raw = "```\nlet x = 1;";
        assert_eq!(strip_fences(raw), "let x = 1;");
    }

    #[test]
    fn leaves_plain_code_untouched() {
        assert_eq!(strip_fences("  let x = 1;  "), "let x = 1;");
    }

    #[test]
    fn extracts_tagged_content() {
        let raw = "before\n<updated_file>\nfn main() {}\n</updated_file>\nafter";
        let got = extract_updated_content(raw);
        assert_eq!(got, "fn main() {}");
    }

    #[test]
    fn falls_back_to_full_text_when_no_tags() {
        let got = extract_updated_content("  fn x() {}  ");
        assert_eq!(got, "fn x() {}");
    }
}
