//! TOML configuration (ADR-008).
//!
//! Loaded from `$XDG_CONFIG_HOME/gwae/gwae.toml` (or
//! `$HOME/.config/gwae/gwae.toml`). The schema is intentionally small in
//! M0 and grows with the layout. `docs/CONFIG.md` is generated from the doc
//! comments here.

#[cfg(test)]
use crate::keys;
use crate::theme::{Palette, ThemeSpec};
use gwae_layout::Width;
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

/// A color as written in the config. Re-exported from [`crate::theme`] under
/// its historical name so existing `background = ...` handling is unchanged.
pub use crate::theme::Color as Background;

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
    /// The agent harness command that `;` (spawn-agent) launches. Empty (the
    /// default) means "not chosen yet": `;` then runs the agent gateway, which
    /// offers the harnesses found on PATH and writes the choice back here.
    pub default_agent: String,
    /// Extra agent commands to offer in the `;` picker, on top of the ones
    /// gwae knows and the ones it finds by scanning `PATH`. Use this to
    /// teach it a harness with a name it cannot guess, or a wrapper script.
    /// Entries that are not installed are simply not shown.
    pub agents: Vec<String>,
    /// Number of equal-width panes on screen at first launch. Default: 1 (a
    /// single quarter-width pane; the skeleton's placeholder boxes show the
    /// rest of the container).
    pub startup_panes: usize,
    /// The chrome color theme. Either a built-in preset name
    /// (`theme = "tokyo-night"`) or a `[theme]` table with a `preset` plus
    /// per-key overrides. `theme = "terminal"` derives every color from the
    /// host terminal's own ANSI 0-15 palette. Default: `catppuccin-mocha`.
    /// See `docs/CONFIG.md` for the full key list.
    pub theme: ThemeSpec,
    /// Color of the empty (uncovered) background behind the panes. Accepts a
    /// 256-color index (`236`), a hex RGB (`"#1e1e2e"`), or `"default"`.
    ///
    /// Legacy alias for `theme.base`; when set it overrides the theme.
    pub background: Option<Background>,
    /// Color of the 1-cell accent frame drawn around the focused box. Accepts
    /// a 256-color index (`196`), a hex RGB (`"#ff0000"`), or `"default"`.
    ///
    /// Legacy alias for `theme.accent`; when set it overrides the theme.
    pub focus_color: Option<Background>,
    /// Color of the skeleton frames around unfocused boxes. Accepts the same
    /// forms as `background`.
    ///
    /// Legacy alias for `theme.overlay`; when set it overrides the theme.
    pub skeleton_color: Option<Background>,
    /// The minimap: a small bottom-right grid showing each strip (row) and its
    /// panes (columns), with the focused strip and column highlighted.
    pub minimap: Minimap,
    /// Cowsay art drawn in empty placeholder boxes, under the big cell
    /// identifier.
    pub cowsay: Cowsay,
    /// Draw the big `strip.pane` identifier in empty placeholder boxes.
    /// Default `false`: empty boxes stay a bare skeleton. Set to `true` to
    /// bring the address labels back.
    pub cell_labels: bool,
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
            default_agent: String::new(),
            agents: Vec::new(),
            startup_panes: 1,
            // Colors all live in the theme now; the Catppuccin Mocha defaults
            // come from `Palette::default()`. These legacy keys stay unset
            // unless the user writes them, so they only ever *override*.
            theme: ThemeSpec::default(),
            background: None,
            focus_color: None,
            skeleton_color: None,
            minimap: Minimap::default(),
            cowsay: Cowsay::default(),
            cell_labels: false,
            input_poll_ms: default_input_poll_ms(),
        }
    }
}

impl Config {
    /// The default config file path for this user.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("gwae/gwae.toml");
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/gwae/gwae.toml")
    }

    /// The fully resolved chrome palette: the named preset (or the default
    /// Catppuccin Mocha), then the `[theme]` per-key overrides, then the
    /// legacy top-level `background` / `focus_color` / `skeleton_color` keys,
    /// which win so that pre-theme config files keep behaving exactly as they
    /// did.
    pub fn palette(&self) -> Palette {
        let (p, bad) = self.palette_checked();
        if let Some(name) = bad {
            tracing::warn!(
                "unknown theme {name:?}; using catppuccin-mocha. Available: {}",
                Palette::NAMES.join(", ")
            );
        }
        p
    }

    /// The configured theme name, for display. `"catppuccin-mocha"` when the
    /// config does not name one, since that is the preset actually used.
    pub fn theme_name(&self) -> String {
        match self.theme.0.preset {
            Some(n) => n.as_str().to_string(),
            None => "catppuccin-mocha (default)".to_string(),
        }
    }

    /// As [`Config::palette`], but also returns the offending name when the
    /// configured theme was not recognized, so callers that can actually show
    /// the user something (`gwae doctor`) can report it rather than
    /// dropping it into a log nobody reads.
    pub fn palette_checked(&self) -> (Palette, Option<String>) {
        let (mut p, bad) = self.theme.0.resolve();
        if let Some(c) = self.background {
            p.base = c.color();
        }
        if let Some(c) = self.focus_color {
            p.accent = c.color();
        }
        if let Some(c) = self.skeleton_color {
            p.overlay = c.color();
        }
        (p, bad)
    }

    /// Adopt the appearance settings from `new`, keeping everything that
    /// cannot safely change while the TUI is running.
    ///
    /// Live reload only re-reads the file; it does not re-run startup. So
    /// settings that were *consumed once* at launch are deliberately kept:
    /// `startup_panes` (the panes already exist). `default_agent` is kept too,
    /// but only because nothing in the TUI reads it: the agent gateway loads
    /// the file itself in the new pane, so an edited value applies to the next
    /// agent pane regardless. Everything that is read afresh
    /// every frame - colors, skeleton, minimap, scroll behavior - is adopted,
    /// which is exactly the set a user edits when tweaking a theme.
    pub fn adopt_appearance(&mut self, new: Config) {
        let Config {
            startup_panes,
            default_agent,
            ..
        } = self.clone();
        *self = Config {
            startup_panes,
            default_agent,
            ..new
        };
    }

    /// Load config from `path`, falling back to defaults if the file is
    /// missing or unparseable (with a warning).
    pub fn load(path: &std::path::Path) -> Config {
        match Config::load_checked(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("ignoring config {path:?}: {e}");
                Config::default()
            }
        }
    }

    /// Load config from `path`, returning the parse error instead of
    /// swallowing it.
    ///
    /// A missing file is not an error: it yields the defaults, same as
    /// [`Config::load`]. Live reload uses this so it can tell the user their
    /// edit is broken rather than silently reverting their theme to the
    /// defaults, which would look like the reload itself had misbehaved.
    pub fn load_checked(path: &std::path::Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| e.to_string()),
            Err(_) => Ok(Config::default()),
        }
    }

    /// The config file's last-modified time, or `None` when there is no file
    /// (or the filesystem does not report one).
    ///
    /// Used to detect edits without a filesystem-watch dependency: one `stat`
    /// per poll is cheap next to the render loop's existing work.
    pub fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(path).ok()?.modified().ok()
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
                f.write_str(
                    "overlay, edge_ticks, off (plus legacy reserved / reserved_quasimode as off)",
                )
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
                    _ => Err(de::Error::unknown_variant(
                        v,
                        &["overlay", "edge_ticks", "off"],
                    )),
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

/// Cowsay art in empty placeholder boxes.
///
/// The default messages are *keybinding hints*, so an empty grid documents
/// itself: a new user sees how to put something in the box they are looking
/// at. Replace `messages` to say anything else (fortunes, reminders, ...).
///
/// Which box gets which message is chosen by hashing the cell's coordinates,
/// never randomly, so a given box always says the same thing. That keeps the
/// frame diff stable, so idle gwae does not repaint every frame.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Cowsay {
    /// Draw the cow at all. Off by default; the hint list below is still
    /// populated so `enabled = true` alone restores the cheat-sheet.
    pub enabled: bool,
    /// The pool of messages. Each empty box picks one by position. An empty
    /// list disables the cow just like `enabled = false`.
    pub messages: Vec<String>,
}

impl Default for Cowsay {
    fn default() -> Self {
        // Every hint names a binding that `tui::handle_key` actually
        // implements, spelled with the platform's own modifier name (`⌥` on
        // macOS, `Alt` elsewhere) via [`crate::keys`], so an empty box never
        // teaches a key that does nothing or a glyph the user's keyboard
        // doesn't have.
        Cowsay {
            enabled: false,
            messages: crate::binds::cowsay_hints(),
        }
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
}

impl Minimap {
    /// How many bottom rows are reserved for chrome. No longer used: the
    /// bottom status row has been removed, so this always returns 0.
    #[allow(dead_code)]
    pub fn chrome_rows(&self) -> u16 {
        0
    }

    /// Whether the chrome strip should actually paint content this frame.
    /// With the bottom row removed this is true only for overlay-style
    /// chrome; the centered Alt HUD/minimap is gated elsewhere.
    #[allow(dead_code)]
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::CColor;

    fn parse(toml: &str) -> Config {
        toml::from_str(toml).expect("config parses")
    }

    #[test]
    fn cowsay_defaults_to_keybinding_hints() {
        let cfg = parse("");
        assert!(!cfg.cowsay.enabled, "cow off by default");
        assert!(
            !cfg.cowsay.messages.is_empty(),
            "default messages must exist or the cow never draws"
        );
    }

    #[test]
    fn cowsay_defaults_name_the_platform_modifier() {
        // The hints are the only keybinding docs many users ever read, so they
        // must speak the local keyboard's vocabulary: `⌥` on macOS, `Alt`
        // elsewhere, never both and never the wrong one.
        let cfg = parse("");
        let m = keys::mod_key();
        // Chord hints must name the modifier. A few bindings are mouse or
        // key-range prose (`1-9`, `click`, `↑/↓`) and correctly have no
        // modifier to name.
        let chord_hints = cfg
            .cowsay
            .messages
            .iter()
            .filter(|msg| !msg.starts_with(['1', 'c', 'w', '←', '↵', '⇧']))
            .count();
        assert!(chord_hints > 0, "some hints are chords");
        for msg in &cfg.cowsay.messages {
            if msg.starts_with(['1', 'c', 'w', '←', '↵', '⇧', 'E', 'S']) {
                continue;
            }
            assert!(msg.contains(m), "hint {msg:?} does not mention {m:?}");
        }
        let other = if cfg!(target_os = "macos") {
            "Alt"
        } else {
            "⌥"
        };
        for msg in &cfg.cowsay.messages {
            assert!(
                !msg.contains(other),
                "hint {msg:?} uses the other platform's modifier name"
            );
        }
    }

    #[test]
    fn cowsay_defaults_do_not_teach_dead_keys() {
        // Regressions guarded: `⌥+c` ("new pane") was never implemented, and
        // the hints once told users to "press c"/"press ;" with no modifier at
        // all, which just types the letter into the focused pane.
        let cfg = parse("");
        let m = keys::mod_key();
        for msg in &cfg.cowsay.messages {
            assert!(
                !msg.contains(&format!("{m}+c")),
                "hint {msg:?} names the nonexistent new-pane binding"
            );
            assert!(
                !msg.to_lowercase().starts_with("press "),
                "hint {msg:?} omits the modifier"
            );
        }
    }

    #[test]
    fn cowsay_section_parses() {
        // Exactly the shape documented in docs/CONFIG.md.
        let cfg = parse("[cowsay]\nenabled = false\nmessages = [\"a\", \"b\"]\n");
        assert!(!cfg.cowsay.enabled);
        assert_eq!(cfg.cowsay.messages, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn cowsay_partial_section_keeps_other_defaults() {
        // `#[serde(default)]`: naming only `enabled` must not wipe the
        // built-in hint list out from under the user.
        let cfg = parse("[cowsay]\nenabled = false\n");
        assert!(!cfg.cowsay.enabled);
        assert!(!cfg.cowsay.messages.is_empty(), "messages were cleared");
    }

    #[test]
    fn cowsay_is_adopted_on_live_reload() {
        // Cowsay is read afresh every frame, so editing it in the config file
        // must take effect without a restart, like the other appearance keys.
        let mut cfg = Config::default();
        let new = parse("[cowsay]\nenabled = false\nmessages = [\"z\"]\n");
        cfg.adopt_appearance(new);
        assert!(!cfg.cowsay.enabled, "cowsay.enabled not adopted");
        assert_eq!(cfg.cowsay.messages, vec!["z".to_string()]);
    }

    #[test]
    fn retired_mouse_keys_are_ignored_not_fatal() {
        // Old configs still on disk must keep loading: the keys are gone, so
        // they are simply not read rather than a parse error.
        let cfg = parse("mouse = false\nscroll_lines = 7\nstartup_panes = 2\n");
        assert_eq!(cfg.startup_panes, 2);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let cfg = parse("");
        assert_eq!(cfg.startup_panes, 1);
        // No color keys set: the palette is the default Catppuccin Mocha,
        // exactly the colors that used to be hardcoded.
        assert_eq!(cfg.palette(), Palette::CATPPUCCIN_MOCHA);
    }

    #[test]
    fn focus_color_parses() {
        let cfg = parse("focus_color = 36");
        assert_eq!(cfg.palette().accent, CColor::Idx(36));
        let cfg = parse("focus_color = \"#ff0000\"");
        assert_eq!(cfg.palette().accent, CColor::Rgb(0xff, 0, 0));
        let cfg = parse("focus_color = \"default\"");
        assert_eq!(cfg.palette().accent, CColor::Default);
    }

    #[test]
    fn skeleton_color_parses() {
        // `skeleton` is no longer a key: the frames are the only look. A
        // stale `skeleton = ...` in an old config is ignored, not an error.
        let _ = parse("skeleton = false");
        let cfg = parse("skeleton_color = \"#333333\"");
        assert_eq!(cfg.palette().overlay, CColor::Rgb(0x33, 0x33, 0x33));
    }

    #[test]
    fn background_index_parses() {
        let cfg = parse("background = 235");
        assert_eq!(cfg.palette().base, CColor::Idx(235));
    }

    #[test]
    fn background_hex_parses() {
        let cfg = parse("background = \"#1e1e2e\"");
        assert_eq!(cfg.palette().base, CColor::Rgb(0x1e, 0x1e, 0x2e));
        // Leading '#' is optional.
        let cfg = parse("background = '1e1e2e'");
        assert_eq!(cfg.palette().base, CColor::Rgb(0x1e, 0x1e, 0x2e));
    }

    #[test]
    fn background_default_literal() {
        let cfg = parse("background = \"default\"");
        assert_eq!(cfg.palette().base, CColor::Default);
    }

    #[test]
    fn theme_name_selects_a_preset() {
        let cfg = parse("theme = \"nord\"");
        assert_eq!(cfg.palette(), Palette::NORD);
    }

    #[test]
    fn theme_table_overrides_layer_on_the_preset() {
        let cfg = parse("[theme]\npreset = \"nord\"\naccent = \"#ff0000\"\n");
        let p = cfg.palette();
        assert_eq!(p.accent, CColor::Rgb(0xff, 0, 0));
        assert_eq!(p.base, Palette::NORD.base);
    }

    #[test]
    fn legacy_flat_keys_beat_the_theme() {
        // A pre-theme config that also names a preset: the explicit legacy
        // keys must still win, so upgrading gwae never changes an
        // existing user's colors.
        let cfg = parse("theme = \"nord\"\nbackground = \"#010203\"\n");
        let p = cfg.palette();
        assert_eq!(p.base, CColor::Rgb(1, 2, 3), "legacy background wins");
        assert_eq!(p.accent, Palette::NORD.accent, "rest comes from the preset");
    }

    #[test]
    fn unknown_theme_falls_back_to_the_default_palette() {
        let cfg = parse("theme = \"no-such-theme\"");
        assert_eq!(cfg.palette(), Palette::CATPPUCCIN_MOCHA);
    }

    #[test]
    fn terminal_theme_inherits_the_ansi_palette() {
        let cfg = parse("theme = \"terminal\"");
        assert_eq!(cfg.palette(), Palette::TERMINAL);
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
        assert_eq!(cfg.palette().base, CColor::Idx(235));
    }
}
