//! Ghostty's `key = value` config format: read one key.
//!
//! Ghostty config is one setting per line, `#` comments, `=` separated,
//! last assignment wins. Mirrors `kitty_conf` for kitty's space-separated
//! format; both exist so focus advice can check what the terminal will
//! actually load.

/// Read one `key = value` setting out of a ghostty config's text.
///
/// The last assignment wins, matching ghostty's own precedence. Commented
/// lines and lines without `=` are ignored.
pub fn get(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                found = Some(v.trim().to_string());
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
        let text = "# c\ntheme = Dark\nmacos-non-native-fullscreen = false\nmacos-non-native-fullscreen = true\n";
        assert_eq!(
            get(text, "macos-non-native-fullscreen").as_deref(),
            Some("true")
        );
        assert_eq!(get(text, "theme").as_deref(), Some("Dark"));
        assert_eq!(get(text, "missing"), None);
    }

    #[test]
    fn commented_and_bare_lines_are_not_settings() {
        let text = "#macos-non-native-fullscreen = true\nmacos-non-native-fullscreen\n";
        assert_eq!(get(text, "macos-non-native-fullscreen"), None);
    }
}
