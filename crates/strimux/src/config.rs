//! TOML configuration (ADR-008).
//!
//! Loaded from `$XDG_CONFIG_HOME/strimux/strimux.toml` (or
//! `$HOME/.config/strimux/strimux.toml`). The schema is intentionally small in
//! M0 and grows with the layout. `docs/CONFIG.md` is generated from the doc
//! comments here.

use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;
use strimux_layout::Width;
use strimux_term::CColor;

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
    /// Logical grid content width (cells) of every pane, decoupled from the
    /// visible column width. Long lines up to this width do not wrap and can
    /// be revealed with horizontal pane scroll (Alt+Left/Right). `0` (the
    /// default) follows the visible column width so lines wrap normally and
    /// there is no horizontal overflow to manage in a pane.
    pub content_width: u16,
    /// The agent harness command that `;` (spawn-agent) launches.
    pub default_agent: String,
    /// Number of equal-width panes on screen at first launch. Default: 1 (a
    /// single quarter-width pane; the skeleton's placeholder boxes show the
    /// rest of the container).
    pub startup_panes: usize,
    /// Color of the empty (uncovered) background behind the panes. Accepts a
    /// 256-color index (`236`), a hex RGB (`"#1e1e2e"`), or `"default"`.
    pub background: Background,
    /// Color of the 1-cell accent frame drawn around the focused box. Accepts
    /// a 256-color index (`196`), a hex RGB (`"#ff0000"`), or `"default"`.
    pub focus_color: Background,
    /// Draw the skeleton: a 1-cell frame around every column box (full strip
    /// height), so the layout's structure is always visible. The focused box's
    /// frame uses `focus_color` instead of `skeleton_color`.
    pub skeleton: bool,
    /// Color of the skeleton frames around unfocused boxes. Accepts the same
    /// forms as `background`. Default: white.
    pub skeleton_color: Background,
    /// Draw each box's tmux-style `strip.cell` address (e.g. `1.2`) inline in
    /// the top frame row of its skeleton box. Requires `skeleton`.
    pub cell_labels: bool,
    /// The minimap: a small bottom-right grid showing each strip (row) and its
    /// panes (columns), with the focused strip and column highlighted.
    pub minimap: Minimap,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_column_width: Width::DEFAULT,
            scroll_margin: 2,
            center_focus: false,
            content_width: 0,
            default_agent: "jcode".to_string(),
            startup_panes: 1,
            background: Background::default(),
            focus_color: Background(CColor::Rgb(0xff, 0x00, 0x00)),
            skeleton: true,
            skeleton_color: Background(CColor::Rgb(0xff, 0xff, 0xff)),
            cell_labels: true,
            minimap: Minimap::default(),
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

/// The minimap widget: which strips (rows) and panes (columns) exist and
/// which is focused. Shown bottom-right; rows of the map are strips, the width
/// of each tile is proportional to the column's width share.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Minimap {
    /// Draw the minimap at all.
    pub show: bool,
    /// Maximum width (in cells) of the minimap block. The map shrinks to fit
    /// the panel if the panel is narrower.
    pub max_width: u16,
    /// Maximum number of strips (rows) shown; extra strips are cut off.
    pub max_rows: u16,
}

impl Default for Minimap {
    fn default() -> Self {
        Minimap {
            show: true,
            max_width: 32,
            max_rows: 6,
        }
    }
}

/// The empty (uncovered) background color behind the panes. Wraps a `CColor`
/// and parses it from TOML as either a 256-color index, a hex RGB string, or
/// the literal `"default"` (the terminal's own background).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Background(pub CColor);

impl Default for Background {
    fn default() -> Self {
        Background(CColor::Default)
    }
}

impl Background {
    /// Resolve to the color used when painting uncovered background cells.
    pub fn color(self) -> CColor {
        self.0
    }
}

impl<'de> Deserialize<'de> for Background {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(BackgroundVisitor)
    }
}

struct BackgroundVisitor;

impl<'de> Visitor<'de> for BackgroundVisitor {
    type Value = Background;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a 256-color index (0-255), a hex RGB string like \"#1e1e2e\", or \"default\"")
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let i = v.min(255) as u8;
        Ok(Background(CColor::Idx(i)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let i = v.clamp(0, 255) as u8;
        Ok(Background(CColor::Idx(i)))
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if s.eq_ignore_ascii_case("default") {
            return Ok(Background(CColor::Default));
        }
        let hex = s.trim_start_matches('#');
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(de::Error::custom(
                "background string must be \"default\" or a 6-digit hex RGB like \"#1e1e2e\"",
            ));
        }
        let r = u8::from_str_radix(&hex[0..2], 16).map_err(de::Error::custom)?;
        let g = u8::from_str_radix(&hex[2..4], 16).map_err(de::Error::custom)?;
        let b = u8::from_str_radix(&hex[4..6], 16).map_err(de::Error::custom)?;
        Ok(Background(CColor::Rgb(r, g, b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> Config {
        toml::from_str(toml).expect("config parses")
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let cfg = parse("");
        assert_eq!(cfg.startup_panes, 1);
        assert_eq!(cfg.background, Background::default());
        assert_eq!(cfg.focus_color, Background(CColor::Rgb(0xff, 0, 0)));
        assert!(cfg.skeleton, "skeleton frames on by default");
        assert_eq!(
            cfg.skeleton_color,
            Background(CColor::Rgb(0xff, 0xff, 0xff))
        );
    }

    #[test]
    fn focus_color_parses() {
        let cfg = parse("focus_color = 36");
        assert_eq!(cfg.focus_color, Background(CColor::Idx(36)));
        let cfg = parse("focus_color = \"#ff0000\"");
        assert_eq!(cfg.focus_color, Background(CColor::Rgb(0xff, 0, 0)));
        let cfg = parse("focus_color = \"default\"");
        assert_eq!(cfg.focus_color, Background::default());
    }

    #[test]
    fn skeleton_parses() {
        let cfg = parse("cell_labels = false");
        assert!(!cfg.cell_labels);
        assert!(parse("").cell_labels, "cell labels on by default");
        let cfg = parse("skeleton = false");
        assert!(!cfg.skeleton);
        let cfg = parse("skeleton_color = \"#333333\"");
        assert_eq!(
            cfg.skeleton_color,
            Background(CColor::Rgb(0x33, 0x33, 0x33))
        );
    }

    #[test]
    fn background_index_parses() {
        let cfg = parse("background = 235");
        assert_eq!(cfg.background, Background(CColor::Idx(235)));
    }

    #[test]
    fn background_hex_parses() {
        let cfg = parse("background = \"#1e1e2e\"");
        assert_eq!(cfg.background, Background(CColor::Rgb(0x1e, 0x1e, 0x2e)));
        // Leading '#' is optional.
        let cfg = parse("background = '1e1e2e'");
        assert_eq!(cfg.background, Background(CColor::Rgb(0x1e, 0x1e, 0x2e)));
    }

    #[test]
    fn background_default_literal() {
        let cfg = parse("background = \"default\"");
        assert_eq!(cfg.background, Background(CColor::Default));
    }

    #[test]
    fn startup_panes_parses() {
        let cfg = parse("startup_panes = 2");
        assert_eq!(cfg.startup_panes, 2);
    }

    #[test]
    fn sample_user_config() {
        let cfg = parse("startup_panes = 2\nbackground = 235");
        assert_eq!(cfg.startup_panes, 2);
        assert_eq!(cfg.background, Background(CColor::Idx(235)));
    }
}
