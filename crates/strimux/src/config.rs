//! TOML configuration (ADR-008).
//!
//! Loaded from `$XDG_CONFIG_HOME/strimux/strimux.toml` (or
//! `$HOME/.config/strimux/strimux.toml`). The schema is intentionally small in
//! M0 and grows with the layout. `docs/CONFIG.md` is generated from the doc
//! comments here.

use serde::Deserialize;
use std::path::PathBuf;
use strimux_layout::Width;

/// The resolved view of the config file, with defaults filled in.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default width of a newly created column (default: Half).
    pub default_column_width: Width,
    /// Cells of context kept visible around the focused column when scrolling.
    pub scroll_margin: u16,
    /// Always center the focused column instead of scrolling minimally.
    pub center_focus: bool,
    /// The command `Alt+a` spawns (the default agent harness).
    pub default_agent: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_column_width: Width::DEFAULT,
            scroll_margin: 2,
            center_focus: false,
            default_agent: "claude".to_string(),
        }
    }
}

impl Config {
    /// The default config file path for this user.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("strimux/strimux.toml");
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/strimux/strimux.toml")
    }

    /// Load config from `path`, falling back to defaults if the file is
    /// missing or unparseable (with a warning).
    pub fn load(path: &std::path::Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("ignoring config {path:?}: {e}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }
}
