//! OS and terminal identity: the facts every kitty-gated stage reads.
//!
//! Moved out of `latency.rs` so focus, bindings, and keyboard stages reuse
//! one probe instead of each growing their own.

use std::path::PathBuf;

/// The kitty config file kitty itself would load.
pub fn kitty_conf_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("KITTY_CONFIG_DIRECTORY") {
        return Some(PathBuf::from(dir).join("kitty.conf"));
    }
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(base.join("kitty/kitty.conf"))
}

/// True when running under kitty, so kitty-specific advice is relevant.
pub fn in_kitty() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM")
            .map(|t| t.contains("kitty"))
            .unwrap_or(false)
}

/// True when running under ghostty, so ghostty-specific advice is relevant.
pub fn in_ghostty() -> bool {
    std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var("TERM_PROGRAM")
            .map(|t| t.eq_ignore_ascii_case("ghostty"))
            .unwrap_or(false)
}

/// The ghostty config files ghostty itself would load, in load order.
///
/// On macOS ghostty reads both the XDG path and the Application Support
/// path; either may carry the setting, so callers check both.
pub fn ghostty_conf_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
    };
    if let Some(base) = base {
        out.push(base.join("ghostty/config"));
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            out.push(
                PathBuf::from(home)
                    .join("Library/Application Support/com.mitchellh.ghostty/config"),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_detection_agrees_with_env() {
        // Without kitty markers this must be false: telling an Alacritty
        // user to edit kitty.conf implies gwae does not know the machine.
        if std::env::var_os("KITTY_WINDOW_ID").is_none()
            && !std::env::var("TERM").unwrap_or_default().contains("kitty")
        {
            assert!(!in_kitty());
        }
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn conf_path_respects_kitty_config_directory() {
        let dir = kitty_conf_path().expect("a path on any machine");
        assert!(dir.ends_with("kitty.conf"), "{dir:?}");
    }
}
