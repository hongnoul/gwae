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

    #[test]
    fn conf_path_respects_kitty_config_directory() {
        let dir = kitty_conf_path().expect("a path on any machine");
        assert!(dir.ends_with("kitty.conf"), "{dir:?}");
    }
}
