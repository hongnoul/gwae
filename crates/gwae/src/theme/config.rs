//! Theme TOML parsing: presets, overrides, and legacy keys.

use super::palette::Palette;
use gwae_term::CColor;
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;

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
#[cfg(test)]
mod tests {
    use super::*;

    use super::*;

    
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
}
