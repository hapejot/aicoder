use similar::{ChangeTag, TextDiff};

pub fn render_unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    out.push_str(&format!("--- {path}\n"));
    out.push_str(&format!("+++ {path}\n"));

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => {
                out.push('-');
                out.push_str(change.to_string().as_str());
            }
            ChangeTag::Insert => {
                out.push('+');
                out.push_str(change.to_string().as_str());
            }
            ChangeTag::Equal => {
                out.push(' ');
                out.push_str(change.to_string().as_str());
            }
        }
    }

    out
}
