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
}
