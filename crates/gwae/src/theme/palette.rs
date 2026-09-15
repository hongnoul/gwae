//! Chrome palette: presets and color derivation.

use gwae_layout::PaneStatus;
use gwae_term::CColor;
use std::fmt;

/// Every color gwae paints as chrome.
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

pub(super) const fn rgb(hex: u32) -> CColor {
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
    /// Catppuccin Mocha - gwae's default, and the palette the pre-theme
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

    /// White phosphor CRT: soft white chrome on true black, grayscale status.
    pub const WHITE_PHOSPHOR: Palette = Palette {
        base: rgb(0x000000),
        surface: rgb(0x000000),
        overlay: rgb(0x50504c),
        accent: rgb(0xd8d8d0),
        text: rgb(0xd8d8d0),
        label: rgb(0x808078),
        running: rgb(0x909090),
        idle: rgb(0xd8d8d8),
        done: rgb(0xb0b0b0),
        failed: rgb(0xffffff),
    };

    /// The host terminal's own ANSI 0-15 colors.
    ///
    /// Nothing is hardcoded to an RGB value, so gwae inherits whatever the
    /// terminal is already themed as: change your terminal's scheme and
    /// gwae follows. `base`, `surface`, and `text` use [`CColor::Default`],
    /// preserving the terminal's native foreground/background pair rather
    /// than assuming ANSI black and white are its default colors.
    pub const TERMINAL: Palette = Palette {
        base: CColor::Default,
        surface: CColor::Default,
        overlay: CColor::Idx(8),
        accent: CColor::Idx(6),
        text: CColor::Default,
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
            "whitephosphor" | "monochrome" => Palette::WHITE_PHOSPHOR,
            "terminal" | "ansi" | "ansi16" => Palette::TERMINAL,
            _ => return None,
        })
    }

    /// The names of every built-in preset, in presentation order. Used by
    /// `gwae --list-themes` and by config error messages.
    pub const NAMES: &'static [&'static str] = &[
        "catppuccin-mocha",
        "catppuccin-latte",
        "tokyo-night",
        "gruvbox",
        "nord",
        "rose-pine",
        "dracula",
        "terminal",
        "white-phosphor",
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn terminal_preset_uses_native_surfaces_and_indexed_accents() {
        let p = Palette::TERMINAL;
        assert_eq!(p.surface, CColor::Default);
        assert_eq!(p.text, CColor::Default);
        // Nothing may be a hardcoded RGB, or it would not follow the host
        // terminal's scheme.
        for c in [
            p.overlay, p.accent, p.label, p.running, p.idle, p.done, p.failed,
        ] {
            assert!(matches!(c, CColor::Idx(_)), "{c:?} is not an ANSI index");
        }
        assert_eq!(
            p.base,
            CColor::Default,
            "base must not repaint the terminal"
        );
    }


}
