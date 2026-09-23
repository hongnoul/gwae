//! `[theme]` manual overrides: per-key colors layered on the retro default.
//!
//! ```toml
//! [theme]
//! accent = "#ff00ff"  # RGB hex
//! running = 12        # 256-color index
//! text = "default"    # the terminal's own color for this key
//! ```
//!
//! Every key is optional; an unset key keeps the [`Palette::RETRO`] value.
//! No presets, no picker, no onboarding flow: this is a hand-edit escape
//! hatch, and the palette is re-read on config reload so a save repaints the
//! running session.

use super::palette::Palette;
use gwae_term::CColor;
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;

/// A color as written in the config: a 256-color index (`12`), a hex RGB
/// string (`"#00ffff"`, with or without the `#`), or the literal
/// `"default"` (the terminal's own color for that key).
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
        f.write_str("a 256-color index (0-255), a hex RGB string like \"#00ffff\", or \"default\"")
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
                "color must be \"default\" or a 6-digit hex RGB like \"#00ffff\"",
            ));
        }
        let r = u8::from_str_radix(&hex[0..2], 16).map_err(de::Error::custom)?;
        let g = u8::from_str_radix(&hex[2..4], 16).map_err(de::Error::custom)?;
        let b = u8::from_str_radix(&hex[4..6], 16).map_err(de::Error::custom)?;
        Ok(Color(CColor::Rgb(r, g, b)))
    }

    fn visit_string<E>(self, s: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&s)
    }
}

/// The `[theme]` table: optional per-key overrides on [`Palette::RETRO`].
///
/// A bare `theme = "nord"` string (the retired preset shorthand) parses as
/// "no overrides", and a retired `preset` key inside the table is ignored:
/// old configs keep loading with the retro default. Any other unknown key is
/// a parse error, so a typo fails loudly at startup (and on live reload)
/// rather than painting a silently wrong chrome.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThemeConfig {
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
    /// Override: failed status tint.
    pub failed: Option<Color>,
}

/// The table form, with the retired `preset` key accepted and dropped.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeTable {
    #[allow(dead_code)]
    preset: Option<de::IgnoredAny>,
    base: Option<Color>,
    surface: Option<Color>,
    overlay: Option<Color>,
    accent: Option<Color>,
    text: Option<Color>,
    label: Option<Color>,
    running: Option<Color>,
    idle: Option<Color>,
    /// Retired: the `Done` status was removed (success at a prompt is
    /// `idle`). Accepted and dropped so old configs keep loading.
    #[allow(dead_code)]
    done: Option<de::IgnoredAny>,
    failed: Option<Color>,
}

impl<'de> Deserialize<'de> for ThemeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ThemeVisitor;
        impl<'de> Visitor<'de> for ThemeVisitor {
            type Value = ThemeConfig;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a [theme] table of color overrides")
            }
            fn visit_str<E>(self, _v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // Retired preset shorthand (`theme = "nord"`): no overrides.
                Ok(ThemeConfig::default())
            }
            fn visit_string<E>(self, _v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(ThemeConfig::default())
            }
            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let t = ThemeTable::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ThemeConfig {
                    base: t.base,
                    surface: t.surface,
                    overlay: t.overlay,
                    accent: t.accent,
                    text: t.text,
                    label: t.label,
                    running: t.running,
                    idle: t.idle,
                    failed: t.failed,
                })
            }
        }
        deserializer.deserialize_any(ThemeVisitor)
    }
}

impl ThemeConfig {
    /// Resolve to a concrete [`Palette`]: start from [`Palette::RETRO`], then
    /// layer every override that was actually written in the config on top.
    pub fn resolve(&self) -> Palette {
        let mut p = Palette::RETRO;
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
        if let Some(c) = self.failed {
            p.failed = c.color();
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_keeps_retro() {
        #[derive(Deserialize)]
        struct W {
            #[serde(default)]
            theme: ThemeConfig,
        }
        let w: W = toml::from_str("").unwrap();
        assert_eq!(w.theme.resolve(), Palette::RETRO);
    }

    #[test]
    fn overrides_replace_only_the_named_keys() {
        #[derive(Deserialize)]
        struct W {
            theme: ThemeConfig,
        }
        let w: W =
            toml::from_str("[theme]\naccent = \"#ff00ff\"\nrunning = 12\ntext = \"default\"\n")
                .unwrap();
        let p = w.theme.resolve();
        assert_eq!(p.accent, CColor::Rgb(0xff, 0x00, 0xff));
        assert_eq!(p.running, CColor::Idx(12));
        assert_eq!(p.text, CColor::Default);
        assert_eq!(p.base, Palette::RETRO.base, "rest of retro is kept");
        assert_eq!(p.failed, Palette::RETRO.failed);
    }

    #[test]
    fn color_accepts_index_hex_and_default() {
        #[derive(Deserialize)]
        struct W {
            c: Color,
        }
        let w: W = toml::from_str("c = 12").unwrap();
        assert_eq!(w.c.color(), CColor::Idx(12));
        let w: W = toml::from_str("c = \"#00ffff\"").unwrap();
        assert_eq!(w.c.color(), CColor::Rgb(0x00, 0xff, 0xff));
        let w: W = toml::from_str("c = \"00ffff\"").unwrap();
        assert_eq!(w.c.color(), CColor::Rgb(0x00, 0xff, 0xff));
        let w: W = toml::from_str("c = \"default\"").unwrap();
        assert_eq!(w.c.color(), CColor::Default);
    }

    #[test]
    fn retired_preset_is_silently_ignored() {
        #[derive(Deserialize)]
        struct W {
            theme: ThemeConfig,
        }
        // Bare string shorthand and the old `preset` table key both mean
        // "no overrides": old configs keep loading on retro.
        let w: W = toml::from_str("theme = \"nord\"").unwrap();
        assert_eq!(w.theme.resolve(), Palette::RETRO);
        let w: W = toml::from_str("[theme]\npreset = \"nord\"\n").unwrap();
        assert_eq!(w.theme.resolve(), Palette::RETRO);
    }

    #[test]
    fn retired_done_key_is_silently_ignored() {
        // The `Done` status was removed (success at a prompt is `idle`),
        // but a config that still tints it must keep loading rather than
        // failing the whole file at startup.
        #[derive(Deserialize)]
        struct W {
            theme: ThemeConfig,
        }
        let w: W = toml::from_str("[theme]\ndone = \"#00ff00\"\nidle = 11\n").unwrap();
        let p = w.theme.resolve();
        assert_eq!(p.idle, CColor::Idx(11), "live keys still apply");
        assert_eq!(p.failed, Palette::RETRO.failed);
    }

    #[test]
    fn unknown_theme_key_is_a_parse_error() {
        let r = toml::from_str::<ThemeConfig>("[theme]\nacccent = \"#ff0000\"\n");
        assert!(r.is_err(), "typos must fail loudly, not paint silently");
    }
}
