//! The chrome palette: every color strimux itself paints.
//!
//! Panes are real PTYs that paint themselves, so strimux only themes its own
//! *chrome*: the uncovered background, the skeleton frames, the focus accent,
//! the placeholder big-label, the HUD/minimap surface and text, and the four
//! pane-status tints. That is the whole surface, so one flat [`Palette`]
//! covers it.
//!
//! A palette comes from one of three places, in increasing specificity:
//!
//! 1. A built-in preset named by `theme = "catppuccin-mocha"` (see
//!    [`Palette::preset`] for the list). Default: `catppuccin-mocha`.
//! 2. `theme = "terminal"`, which derives the palette from the host
//!    terminal's own ANSI 0-15 colors, so strimux matches whatever the
//!    terminal is already themed as.
//! 3. Per-key overrides in the `[theme]` table (and the legacy top-level
//!    `background` / `focus_color` / `skeleton_color` keys), layered on top.

use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;
use strimux_layout::PaneStatus;
use strimux_term::CColor;

/// Every color strimux paints as chrome.
///
/// Status tints are stored at full intensity; the muted variants used for
/// minimap tiles are derived with [`Palette::muted`] rather than stored, so a
/// preset only has to name ten colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The empty (uncovered) background behind the panes.
    pub base: CColor,
    /// Background of the HUD and centered minimap panels.
    pub surface: CColor,
    /// Skeleton frames around unfocused boxes.
    pub overlay: CColor,
    /// The 1-cell accent frame around the focused box, and focus highlights.
    pub accent: CColor,
    /// Text drawn in the HUD and minimap.
    pub text: CColor,
    /// The big block-font `strip.column` label in placeholder boxes.
    pub label: CColor,
    /// Pane status: running (OSC 133 command in flight).
    pub running: CColor,
    /// Pane status: idle / wants attention.
    pub idle: CColor,
    /// Pane status: last command succeeded.
    pub done: CColor,
    /// Pane status: last command exited non-zero.
    pub failed: CColor,
}

const fn rgb(hex: u32) -> CColor {
    CColor::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Default for Palette {
    fn default() -> Self {
        Palette::CATPPUCCIN_MOCHA
    }
}

impl Palette {
    /// Catppuccin Mocha - strimux's default, and the palette the pre-theme
    /// hardcoded colors were drawn from.
    pub const CATPPUCCIN_MOCHA: Palette = Palette {
        base: rgb(0x1e1e2e),
        surface: rgb(0x181825),
        overlay: rgb(0x6c7086),
        accent: rgb(0x74c7ec),
        text: rgb(0xa6adc8),
        label: rgb(0x585b70),
        running: rgb(0x89b4fa),
        idle: rgb(0xfab387),
        done: rgb(0xa6e3a1),
        failed: rgb(0xf38ba8),
    };

    /// Catppuccin Latte - the light-mode counterpart to Mocha.
    pub const CATPPUCCIN_LATTE: Palette = Palette {
        base: rgb(0xeff1f5),
        surface: rgb(0xe6e9ef),
        overlay: rgb(0x9ca0b0),
        accent: rgb(0x209fb5),
        text: rgb(0x4c4f69),
        label: rgb(0xbcc0cc),
        running: rgb(0x1e66f5),
        idle: rgb(0xfe640b),
        done: rgb(0x40a02b),
        failed: rgb(0xd20f39),
    };

    /// Tokyo Night (storm-ish dark).
    pub const TOKYO_NIGHT: Palette = Palette {
        base: rgb(0x1a1b26),
        surface: rgb(0x16161e),
        overlay: rgb(0x565f89),
        accent: rgb(0x7aa2f7),
        text: rgb(0xa9b1d6),
        label: rgb(0x3b4261),
        running: rgb(0x7aa2f7),
        idle: rgb(0xe0af68),
        done: rgb(0x9ece6a),
        failed: rgb(0xf7768e),
    };

    /// Gruvbox Dark.
    pub const GRUVBOX_DARK: Palette = Palette {
        base: rgb(0x282828),
        surface: rgb(0x1d2021),
        overlay: rgb(0x665c54),
        accent: rgb(0x83a598),
        text: rgb(0xebdbb2),
        label: rgb(0x504945),
        running: rgb(0x83a598),
        idle: rgb(0xfe8019),
        done: rgb(0xb8bb26),
        failed: rgb(0xfb4934),
    };

    /// Nord.
    pub const NORD: Palette = Palette {
        base: rgb(0x2e3440),
        surface: rgb(0x272c36),
        overlay: rgb(0x4c566a),
        accent: rgb(0x88c0d0),
        text: rgb(0xd8dee9),
        label: rgb(0x434c5e),
        running: rgb(0x81a1c1),
        idle: rgb(0xd08770),
        done: rgb(0xa3be8c),
        failed: rgb(0xbf616a),
    };

    /// Rosé Pine.
    pub const ROSE_PINE: Palette = Palette {
        base: rgb(0x191724),
        surface: rgb(0x1f1d2e),
        overlay: rgb(0x6e6a86),
        accent: rgb(0x9ccfd8),
        text: rgb(0xe0def4),
        label: rgb(0x403d52),
        running: rgb(0x31748f),
        idle: rgb(0xf6c177),
        done: rgb(0x9ccfd8),
        failed: rgb(0xeb6f92),
    };

    /// Dracula.
    pub const DRACULA: Palette = Palette {
        base: rgb(0x282a36),
        surface: rgb(0x21222c),
        overlay: rgb(0x6272a4),
        accent: rgb(0x8be9fd),
        text: rgb(0xf8f8f2),
        label: rgb(0x44475a),
        running: rgb(0xbd93f9),
        idle: rgb(0xffb86c),
        done: rgb(0x50fa7b),
        failed: rgb(0xff5555),
    };

    /// The host terminal's own ANSI 0-15 colors.
    ///
    /// Nothing is hardcoded to an RGB value, so strimux inherits whatever the
    /// terminal is already themed as: change your terminal's scheme and
    /// strimux follows. `base` stays [`CColor::Default`] so the terminal's
    /// real background shows through rather than being repainted as ANSI
    /// black (which is wrong on light themes).
    pub const TERMINAL: Palette = Palette {
        base: CColor::Default,
        surface: CColor::Idx(0),
        overlay: CColor::Idx(8),
        accent: CColor::Idx(6),
        text: CColor::Idx(7),
        label: CColor::Idx(8),
        running: CColor::Idx(12),
        idle: CColor::Idx(11),
        done: CColor::Idx(10),
        failed: CColor::Idx(9),
    };

    /// Look up a built-in preset by name. Names are matched case-insensitively
    /// and `-`, `_`, and ` ` are interchangeable, so `catppuccin-mocha`,
    /// `Catppuccin_Mocha`, and `catppuccin mocha` are the same theme.
    pub fn preset(name: &str) -> Option<Palette> {
        let norm: String = name
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .flat_map(|c| c.to_lowercase())
            .collect();
        Some(match norm.as_str() {
            "catppuccinmocha" | "mocha" | "catppuccin" => Palette::CATPPUCCIN_MOCHA,
            "catppuccinlatte" | "latte" => Palette::CATPPUCCIN_LATTE,
            "tokyonight" | "tokyo" => Palette::TOKYO_NIGHT,
            "gruvbox" | "gruvboxdark" => Palette::GRUVBOX_DARK,
            "nord" => Palette::NORD,
            "rosepine" | "rosépine" => Palette::ROSE_PINE,
            "dracula" => Palette::DRACULA,
            "terminal" | "ansi" | "ansi16" => Palette::TERMINAL,
            _ => return None,
        })
    }

    /// The names of every built-in preset, in presentation order. Used by
    /// `strimux --list-themes` and by config error messages.
    pub const NAMES: &'static [&'static str] = &[
        "catppuccin-mocha",
        "catppuccin-latte",
        "tokyo-night",
        "gruvbox",
        "nord",
        "rose-pine",
        "dracula",
        "terminal",
    ];

    /// The tint used for minimap tiles: the status color at 60% intensity, so
    /// a grid of tiles reads as a dim wash and the focused/summary row at full
    /// intensity stands out against it.
    ///
    /// Indexed and default colors have no components to scale, so they are
    /// returned unchanged rather than approximated.
    pub fn muted(c: CColor) -> CColor {
        match c {
            CColor::Rgb(r, g, b) => CColor::Rgb(
                ((r as u16 * 3) / 5) as u8,
                ((g as u16 * 3) / 5) as u8,
                ((b as u16 * 3) / 5) as u8,
            ),
            other => other,
        }
    }

    /// Full-intensity tint for a pane status.
    pub fn status(&self, s: PaneStatus) -> CColor {
        use PaneStatus as S;
        match s {
            S::Running => self.running,
            S::Idle => self.idle,
            S::Done => self.done,
            S::Failed => self.failed,
        }
    }

    /// Muted (60%) tint for a pane status, used for minimap tiles.
    pub fn status_muted(&self, s: PaneStatus) -> CColor {
        Palette::muted(self.status(s))
    }
}

/// A color as written in the config: a 256-color index (`235`), a hex RGB
/// string (`"#1e1e2e"`), or the literal `"default"` (the terminal's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub CColor);

impl Default for Color {
    fn default() -> Self {
        Color(CColor::Default)
    }
}

impl Color {
    /// The wrapped terminal color.
    pub fn color(self) -> CColor {
        self.0
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorVisitor)
    }
}

struct ColorVisitor;

impl<'de> Visitor<'de> for ColorVisitor {
    type Value = Color;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a 256-color index (0-255), a hex RGB string like \"#1e1e2e\", or \"default\"")
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Color(CColor::Idx(v.min(255) as u8)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Color(CColor::Idx(v.clamp(0, 255) as u8)))
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if s.eq_ignore_ascii_case("default") {
            return Ok(Color(CColor::Default));
        }
        let hex = s.trim_start_matches('#');
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(de::Error::custom(
                "color must be \"default\" or a 6-digit hex RGB like \"#1e1e2e\"",
            ));
        }
        let r = u8::from_str_radix(&hex[0..2], 16).map_err(de::Error::custom)?;
        let g = u8::from_str_radix(&hex[2..4], 16).map_err(de::Error::custom)?;
        let b = u8::from_str_radix(&hex[4..6], 16).map_err(de::Error::custom)?;
        Ok(Color(CColor::Rgb(r, g, b)))
    }
}

/// The `[theme]` table: a preset name plus per-key overrides.
///
/// ```toml
/// [theme]
/// preset = "tokyo-night"
/// accent = "#ff0000"     # everything else stays Tokyo Night
/// ```
///
/// `theme = "nord"` (a bare string instead of a table) is accepted as
/// shorthand for `[theme] preset = "nord"`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Name of the built-in preset to start from. Unknown names fall back to
    /// the default preset with a warning. Default: `catppuccin-mocha`.
    pub preset: Option<String2>,
    /// Override: the empty (uncovered) background behind the panes.
    pub base: Option<Color>,
    /// Override: background of the HUD and centered minimap panels.
    pub surface: Option<Color>,
    /// Override: skeleton frames around unfocused boxes.
    pub overlay: Option<Color>,
    /// Override: the accent frame around the focused box.
    pub accent: Option<Color>,
    /// Override: HUD and minimap text.
    pub text: Option<Color>,
    /// Override: the big block-font label in placeholder boxes.
    pub label: Option<Color>,
    /// Override: running status tint.
    pub running: Option<Color>,
    /// Override: idle / wants-attention status tint.
    pub idle: Option<Color>,
    /// Override: succeeded status tint.
    pub done: Option<Color>,
    /// Override: failed status tint.
    pub failed: Option<Color>,
}

/// A `Copy` fixed-capacity string, so [`ThemeConfig`] can stay `Copy` like the
/// rest of the config structs while still holding a preset name.
#[derive(Debug, Clone, Copy)]
pub struct String2 {
    buf: [u8; 32],
    len: u8,
}

impl String2 {
    fn new(s: &str) -> String2 {
        let bytes = s.as_bytes();
        let n = bytes.len().min(32);
        // Truncate on a char boundary so `as_str` is always valid UTF-8.
        let mut n = n;
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        let mut buf = [0u8; 32];
        buf[..n].copy_from_slice(&bytes[..n]);
        String2 { buf, len: n as u8 }
    }

    /// The stored name.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

impl<'de> Deserialize<'de> for String2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(String2::new(&s))
    }
}

impl ThemeConfig {
    /// Resolve to a concrete [`Palette`]: look up the preset, then layer every
    /// override that was actually written in the config on top.
    ///
    /// Returns the palette and, when the preset name was not recognized, the
    /// bad name so the caller can surface it.
    pub fn resolve(&self) -> (Palette, Option<String>) {
        let (mut p, bad) = match self.preset {
            Some(name) => match Palette::preset(name.as_str()) {
                Some(p) => (p, None),
                None => (Palette::default(), Some(name.as_str().to_string())),
            },
            None => (Palette::default(), None),
        };
        if let Some(c) = self.base {
            p.base = c.color();
        }
        if let Some(c) = self.surface {
            p.surface = c.color();
        }
        if let Some(c) = self.overlay {
            p.overlay = c.color();
        }
        if let Some(c) = self.accent {
            p.accent = c.color();
        }
        if let Some(c) = self.text {
            p.text = c.color();
        }
        if let Some(c) = self.label {
            p.label = c.color();
        }
        if let Some(c) = self.running {
            p.running = c.color();
        }
        if let Some(c) = self.idle {
            p.idle = c.color();
        }
        if let Some(c) = self.done {
            p.done = c.color();
        }
        if let Some(c) = self.failed {
            p.failed = c.color();
        }
        (p, bad)
    }
}

/// `theme` accepts either a bare preset name or the full `[theme]` table, so
/// the common case is one word and the power case is still one key.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThemeSpec(pub ThemeConfig);

impl<'de> Deserialize<'de> for ThemeSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Name(String),
            Table(ThemeConfig),
        }
        Ok(match Either::deserialize(deserializer)? {
            Either::Name(s) => ThemeSpec(ThemeConfig {
                preset: Some(String2::new(&s)),
                ..ThemeConfig::default()
            }),
            Either::Table(t) => ThemeSpec(t),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_matches_the_old_hardcoded_mocha_colors() {
        // These are the literals that used to be scattered through tui.rs;
        // the refactor must be a no-op for anyone who never sets a theme.
        let p = Palette::default();
        assert_eq!(p.base, CColor::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(p.surface, CColor::Rgb(0x18, 0x18, 0x25));
        assert_eq!(p.overlay, CColor::Rgb(0x6c, 0x70, 0x86));
        assert_eq!(p.accent, CColor::Rgb(0x74, 0xc7, 0xec));
        assert_eq!(p.text, CColor::Rgb(0xa6, 0xad, 0xc8));
        assert_eq!(p.label, CColor::Rgb(0x58, 0x5b, 0x70));
        assert_eq!(p.running, CColor::Rgb(0x89, 0xb4, 0xfa));
        assert_eq!(p.idle, CColor::Rgb(0xfa, 0xb3, 0x87));
        assert_eq!(p.done, CColor::Rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(p.failed, CColor::Rgb(0xf3, 0x8b, 0xa8));
    }

    #[test]
    fn muted_reproduces_the_old_hardcoded_tile_tints() {
        // The old code stored both intensities by hand; `muted` must derive
        // exactly the same values it used to hardcode.
        let p = Palette::default();
        assert_eq!(Palette::muted(p.running), CColor::Rgb(0x52, 0x6c, 0x96));
        assert_eq!(Palette::muted(p.idle), CColor::Rgb(0x96, 0x6b, 0x51));
        assert_eq!(Palette::muted(p.done), CColor::Rgb(0x63, 0x88, 0x60));
        assert_eq!(Palette::muted(p.failed), CColor::Rgb(0x91, 0x53, 0x64));
    }

    #[test]
    fn muted_passes_indexed_and_default_through() {
        assert_eq!(Palette::muted(CColor::Idx(6)), CColor::Idx(6));
        assert_eq!(Palette::muted(CColor::Default), CColor::Default);
    }

    #[test]
    fn preset_names_are_case_and_separator_insensitive() {
        assert_eq!(
            Palette::preset("tokyo-night"),
            Palette::preset("TokyoNight")
        );
        assert_eq!(
            Palette::preset("tokyo_night"),
            Palette::preset("tokyo night")
        );
        assert_eq!(Palette::preset("nord"), Some(Palette::NORD));
        assert_eq!(Palette::preset("no-such-theme"), None);
    }

    #[test]
    fn every_advertised_name_resolves() {
        for name in Palette::NAMES {
            assert!(
                Palette::preset(name).is_some(),
                "advertised preset {name} does not resolve"
            );
        }
    }

    #[test]
    fn terminal_preset_is_entirely_indexed() {
        let p = Palette::TERMINAL;
        // Nothing may be a hardcoded RGB, or it would not follow the host
        // terminal's scheme.
        for c in [
            p.surface, p.overlay, p.accent, p.text, p.label, p.running, p.idle, p.done, p.failed,
        ] {
            assert!(matches!(c, CColor::Idx(_)), "{c:?} is not an ANSI index");
        }
        assert_eq!(
            p.base,
            CColor::Default,
            "base must not repaint the terminal"
        );
    }

    #[test]
    fn bare_string_theme_selects_a_preset() {
        #[derive(Deserialize)]
        struct W {
            theme: ThemeSpec,
        }
        let w: W = toml::from_str(r#"theme = "nord""#).unwrap();
        assert_eq!(w.theme.0.resolve().0, Palette::NORD);
    }

    #[test]
    fn table_theme_layers_overrides_on_the_preset() {
        #[derive(Deserialize)]
        struct W {
            theme: ThemeSpec,
        }
        let w: W = toml::from_str(
            r##"
            [theme]
            preset = "nord"
            accent = "#ff0000"
            "##,
        )
        .unwrap();
        let (p, bad) = w.theme.0.resolve();
        assert!(bad.is_none());
        assert_eq!(p.accent, CColor::Rgb(0xff, 0, 0), "override applies");
        assert_eq!(p.base, Palette::NORD.base, "rest of the preset is kept");
    }

    #[test]
    fn unknown_preset_falls_back_and_reports_the_name() {
        let t = ThemeConfig {
            preset: Some(String2::new("nope")),
            ..ThemeConfig::default()
        };
        let (p, bad) = t.resolve();
        assert_eq!(p, Palette::default());
        assert_eq!(bad.as_deref(), Some("nope"));
    }

    #[test]
    fn overrides_apply_without_any_preset() {
        #[derive(Deserialize)]
        struct W {
            theme: ThemeSpec,
        }
        let w: W = toml::from_str("[theme]\nbase = 235\n").unwrap();
        let (p, _) = w.theme.0.resolve();
        assert_eq!(p.base, CColor::Idx(235));
        assert_eq!(p.accent, Palette::default().accent);
    }

    #[test]
    fn color_accepts_index_hex_and_default() {
        #[derive(Deserialize)]
        struct W {
            c: Color,
        }
        let w: W = toml::from_str("c = 235").unwrap();
        assert_eq!(w.c.color(), CColor::Idx(235));
        let w: W = toml::from_str(r##"c = "#1e1e2e""##).unwrap();
        assert_eq!(w.c.color(), CColor::Rgb(0x1e, 0x1e, 0x2e));
        let w: W = toml::from_str(r#"c = "1e1e2e""#).unwrap();
        assert_eq!(w.c.color(), CColor::Rgb(0x1e, 0x1e, 0x2e));
        let w: W = toml::from_str(r#"c = "default""#).unwrap();
        assert_eq!(w.c.color(), CColor::Default);
    }

    #[test]
    fn string2_truncates_on_a_char_boundary() {
        let s = String2::new(&"é".repeat(40));
        assert!(std::str::from_utf8(s.as_str().as_bytes()).is_ok());
        assert!(s.as_str().len() <= 32);
    }
}
