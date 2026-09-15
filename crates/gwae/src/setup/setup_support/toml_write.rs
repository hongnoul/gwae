//! The single writer for gwae's own TOML.
//!
//! All stages return `(key, toml_value)` pairs; one commit writes them once
//! through the comment-preserving scalar rewrite in [`crate::agent`], so
//! concurrent writers never clobber keys they do not own or comments the
//! user hand-wrote.

use std::path::Path;

/// Apply answered top-level keys to config `text`, preserving comments and
/// any keys setup never asked about. Pure.
pub fn apply_answers(text: &str, answers: &[(String, String)]) -> String {
    let mut out = if text.trim().is_empty() {
        "# gwae configuration\n".to_string()
    } else {
        text.to_string()
    };
    for (k, v) in answers {
        out = crate::agent::set_scalar_text(&out, k, v);
    }
    out
}

/// Write the answers to `path`, creating the file and its parent as needed.
pub fn save_answers(path: &Path, answers: &[(String, String)]) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = apply_answers(&existing, answers);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

/// Set `key = value` inside `[table]`, creating the table when missing.
pub fn set_table_scalar_text(text: &str, table: &str, key: &str, value: &str) -> String {
    let header = format!("[{table}]");
    let line = format!("{key} = {value}");
    let mut out: Vec<String> = Vec::new();
    let mut in_ours = false;
    let mut replaced = false;
    let mut seen_table = false;
    for raw in text.lines() {
        let t = raw.trim();
        if t.starts_with('[') {
            in_ours = t == header;
            seen_table |= in_ours;
            if in_ours {
                out.push(raw.to_string());
                continue;
            }
        }
        let is_key = in_ours
            && !replaced
            && t.strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);
        if is_key {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        if seen_table {
            // Insert right after the header so the key lands in the table.
            let at = out.iter().position(|l| l.trim() == header).unwrap() + 1;
            out.insert(at, line);
        } else {
            // A blank line before a new table, so the file stays readable
            // next to the spaced-out top-level keys already written above.
            if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
                out.push(String::new());
            }
            out.push(header);
            out.push(line);
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_replace_rather_than_duplicate() {
        let before = "# my notes\ntheme = \"nord\"\n";
        let after = apply_answers(before, &[("theme".into(), "\"dracula\"".into())]);
        assert_eq!(after.matches("theme").count(), 1, "{after:?}");
        assert!(after.contains("# my notes"), "{after:?}");
    }

    #[test]
    fn a_table_key_lands_inside_its_table() {
        let text = set_table_scalar_text("", "theme", "preset", "\"nord\"");
        let v: toml::Value = toml::from_str(&text).expect("still parses");
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn top_level_keys_never_fall_into_a_table() {
        let text = apply_answers("[theme]\npreset = \"nord\"\n", &[("startup_panes".into(), "2".into())]);
        let v: toml::Value = toml::from_str(&text).expect("still parses");
        assert_eq!(v["startup_panes"].as_integer(), Some(2));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }
}
