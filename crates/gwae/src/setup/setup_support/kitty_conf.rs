//! Parse and comment-preserving edit of kitty's `key value` config format.
//!
//! kitty's format is whitespace-separated, one setting per line, `#`
//! comments, last assignment wins. This module reads one key and writes one
//! key without disturbing anything else, mirroring what
//! `agent::set_scalar_text` does for gwae's own TOML.

/// Read one `key value` setting out of a kitty config's text.
///
/// The last assignment wins, matching kitty's own precedence. Commented
/// lines and bare keys with no value are ignored.
pub fn get(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let mut parts = t.split_whitespace();
        if parts.next() == Some(key) {
            let v: Vec<&str> = parts.collect();
            if !v.is_empty() {
                found = Some(v.join(" "));
            }
        }
    }
    found
}

/// Set `key value` in a kitty config's text, preserving comments and order.
///
/// An existing assignment (the last one, which is what kitty honors) is
/// replaced in place; otherwise the line is appended. Commented-out lines
/// are never touched: uncommenting a user's disabled setting would be
/// setup pretending to know better.
pub fn set(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} {value}");
    let mut out: Vec<String> = Vec::new();
    // Index of the last live assignment, the one kitty honors.
    let mut target: Option<usize> = None;
    let rows: Vec<&str> = text.lines().collect();
    for (i, raw) in rows.iter().enumerate() {
        let t = raw.trim();
        if t.starts_with('#') {
            continue;
        }
        let mut parts = t.split_whitespace();
        if parts.next() == Some(key) && !parts.clone().collect::<Vec<_>>().is_empty() {
            target = Some(i);
        }
    }
    for (i, raw) in rows.iter().enumerate() {
        if Some(i) == target {
            // Preserve the line's leading indent.
            let indent_len = raw.len() - raw.trim_start().len();
            out.push(format!("{}{line}", &raw[..indent_len]));
        } else {
            out.push(raw.to_string());
        }
    }
    if target.is_none() {
        if !out.is_empty() && !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// The diff between what the file says and what setup wants, for the
/// confirm screen. `None` when the setting already matches.
pub fn diff_line(text: &str, key: &str, want: &str) -> Option<String> {
    match get(text, key) {
        Some(cur) if cur == want => None,
        Some(cur) => Some(format!("{key}: {cur} -> {want}")),
        None => Some(format!("{key}: (unset) -> {want}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_takes_the_last_live_assignment() {
        let text = "# a comment\nrepaint_delay 10\nfont_size 13\nrepaint_delay 1\n";
        assert_eq!(get(text, "repaint_delay").as_deref(), Some("1"));
        assert_eq!(get(text, "font_size").as_deref(), Some("13"));
        assert_eq!(get(text, "missing"), None);
    }

    #[test]
    fn commented_and_bare_lines_are_not_settings() {
        let text = "#repaint_delay 1\nrepaint_delay\nallow_remote_control\n";
        assert_eq!(get(text, "repaint_delay"), None);
        assert_eq!(get(text, "allow_remote_control"), None);
    }

    #[test]
    fn write_replaces_the_last_assignment_and_keeps_comments() {
        let before = "# mine\nallow_remote_control no\nlisten_on unix:/tmp/a\nallow_remote_control ask\n";
        let after = set(before, "allow_remote_control", "yes");
        assert!(after.contains("# mine"), "{after}");
        assert_eq!(after.matches("allow_remote_control").count(), 2, "{after}");
        assert!(after.contains("allow_remote_control yes"), "{after}");
        // Reading back honors last-wins.
        assert_eq!(get(&after, "allow_remote_control").as_deref(), Some("yes"));
    }

    #[test]
    fn write_appends_when_absent() {
        let after = set("font_size 13\n", "allow_remote_control", "yes");
        assert!(after.contains("font_size 13"), "{after}");
        assert!(after.contains("allow_remote_control yes"), "{after}");
    }

    #[test]
    fn diff_is_none_when_already_correct() {
        assert_eq!(diff_line("allow_remote_control yes\n", "allow_remote_control", "yes"), None);
        assert!(diff_line("allow_remote_control no\n", "allow_remote_control", "yes").is_some());
        assert!(diff_line("", "allow_remote_control", "yes").is_some());
    }
}
