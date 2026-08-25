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

fn default_input_poll_ms() -> u64 {
    2
}

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
    /// The minimap: a small bottom-right grid showing each strip (row) and its
    /// panes (columns), with the focused strip and column highlighted.
    pub minimap: Minimap,
    /// Capture the mouse so the wheel scrolls *inside* the pane under the
    /// cursor (its scrollback) instead of reaching the host terminal, where it
    /// walks the shell's previous/next prompt history. Disable to hand the
    /// wheel back to the host terminal entirely.
    pub mouse: bool,
    /// Rows of pane scrollback moved per wheel notch.
    pub scroll_lines: u16,
    /// Milliseconds to wait in `event::poll` before checking PTY output and
    /// repainting. Lower values reduce perceived typing and backspace latency
    /// at the cost of more frequent wakeups. Default is 2ms (from 10ms) for
    /// low latency with modest CPU cost. Valid range 1..50. Use 1 for minimum
    /// possible input latency (backspace/delete will feel instant).
    #[serde(default = "default_input_poll_ms")]
    pub input_poll_ms: u64,
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
            // Catppuccin Mocha defaults: base #1e1e2e, sapphire #74c7ec, overlay0 #6c7086.
            background: Background(CColor::Rgb(0x1e, 0x1e, 0x2e)),
            focus_color: Background(CColor::Rgb(0x74, 0xc7, 0xec)),
            skeleton: true,
            skeleton_color: Background(CColor::Rgb(0x6c, 0x70, 0x86)),
            minimap: Minimap::default(),
            mouse: true,
            scroll_lines: 3,
            input_poll_ms: default_input_poll_ms(),
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

/// Where and how the minimap/status chrome is shown.
///
/// The bottom reserved status row (`reserved` / `reserved_quasimode`) has been
/// removed. Status is shown via the centered Alt HUD/minimap and (optionally)
/// `overlay` / `edge_ticks`. Legacy values `reserved` and
/// `reserved_quasimode` parse as `off` for compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MinimapMode {
    /// Classic bottom-right overlay (pre-redesign).
    Overlay,
    /// Single-cell ticks on the outer frame (no box).
    EdgeTicks,
    /// No minimap/status chrome at all. Hold ⌥/Alt to see the centered HUD
    /// (attention hint + cheat-sheet) and centered minimap.
    #[default]
    Off,
}

impl<'de> Deserialize<'de> for MinimapMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = MinimapMode;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("overlay, edge_ticks, off (plus legacy reserved / reserved_quasimode as off)")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v.to_ascii_lowercase().as_str() {
                    "overlay" => Ok(MinimapMode::Overlay),
                    "edge_ticks" | "edgeticks" => Ok(MinimapMode::EdgeTicks),
                    "off" => Ok(MinimapMode::Off),
                    // legacy bottom-row modes → off
                    "reserved" | "reserved_quasimode" => Ok(MinimapMode::Off),
                    _ => Err(de::Error::unknown_variant(v, &["overlay", "edge_ticks", "off"])),
                }
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&v)
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// The minimap widget: which strips (rows) and panes (columns) exist and
/// which is focused.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Minimap {
    /// Draw the minimap at all (kill-switch, kept for backward compat).
    pub show: bool,
    /// Presentation mode. `overlay` is the legacy bottom-right overlay;
    /// `edge_ticks` are single-cell frame ticks; `off` (default) shows only
    /// the centered Alt HUD/minimap on ⌥ hold. Legacy `reserved` /
    /// `reserved_quasimode` parse as `off` and the bottom row is reclaimed.
    pub mode: MinimapMode,
    /// Maximum width (in cells) of the minimap. Used for `overlay` and the
    /// centered minimap shown while holding Option/Alt.
    pub max_width: u16,
    /// Maximum number of strips (rows) shown; extra strips are cut off. Used
    /// for `overlay` and the centered minimap shown while holding Option/Alt.
    pub max_rows: u16,
    /// Draw the one-line status summary (above the map or in the HUD).
    pub show_counts: bool,
    /// If non-zero, a centered HUD (attention hint + cheat-sheet) is shown
    /// at startup and on attention transitions. `0` disables it. The numeric
    /// value is kept for backward compat (any non-zero enables).
    pub hud_on_attention_ms: u16,
}

impl Minimap {
    /// How many bottom rows are reserved for chrome. No longer used: the
    /// bottom status row has been removed, so this always returns 0.
    pub fn chrome_rows(&self) -> u16 {
        0
    }

    /// Whether the chrome strip should actually paint content this frame.
    /// With the bottom row removed this is true only for overlay-style
    /// chrome; the centered Alt HUD/minimap is gated elsewhere.
    pub fn should_paint(&self, _alt_held: bool, _has_attention: bool) -> bool {
        if !self.show {
            return false;
        }
        match self.mode {
            MinimapMode::Off => false,
            MinimapMode::Overlay | MinimapMode::EdgeTicks => true,
        }
    }
}

impl Default for Minimap {
    fn default() -> Self {
        Minimap {
            show: true,
            mode: MinimapMode::default(),
            max_width: 32,
            max_rows: 6,
            show_counts: true,
            hud_on_attention_ms: 2500,
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
    fn mouse_keys_parse() {
        let cfg = parse("mouse = false\nscroll_lines = 7\n");
        assert!(!cfg.mouse);
        assert_eq!(cfg.scroll_lines, 7);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let cfg = parse("");
        assert_eq!(cfg.startup_panes, 1);
        // Catppuccin Mocha defaults: background base (#1e1e2e), focus sapphire
        // (#74c7ec), skeleton overlay0 (#6c7086).
        assert_eq!(cfg.background, Background(CColor::Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(cfg.focus_color, Background(CColor::Rgb(0x74, 0xc7, 0xec)));
        assert!(cfg.skeleton, "skeleton frames on by default");
        assert!(cfg.mouse, "mouse captured by default");
        assert_eq!(cfg.scroll_lines, 3);
        assert_eq!(
            cfg.skeleton_color,
            Background(CColor::Rgb(0x6c, 0x70, 0x86))
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
