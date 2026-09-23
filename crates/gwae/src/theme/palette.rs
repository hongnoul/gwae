//! Chrome palette: the enforced retro default plus manual overrides.
//!
//! gwae paints its own chrome colors rather than inheriting the terminal
//! scheme: true-black panels with high-contrast functional colors (white
//! focus, blue running, amber idle, red failed). Any key can be
//! overridden under `[theme]` in the config file (see `theme::ThemeConfig`);
//! there are no presets and no picker.

use gwae_layout::PaneStatus;
use gwae_term::CColor;

/// Every color gwae paints as chrome.
///
/// Status tints are stored at full intensity and painted as-is on minimap
/// tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The empty (uncovered) background behind the panes.
    pub base: CColor,
    /// Background of the HUD and centered minimap panels.
    pub surface: CColor,
    /// Skeleton frames around unfocused boxes. Dim by design: the
    /// focused ring is bold white, so unfocused chrome recedes to a
    /// darker gray and the contrast delta reads at a glance.
    pub overlay: CColor,
    /// The 1-cell accent frame around the focused box, and focus highlights.
    pub accent: CColor,
    /// Text drawn in the HUD and minimap.
    pub text: CColor,
    /// Secondary text in pickers (origins, help lines, empty states).
    /// Brighter than the unfocused frame chrome so it stays readable
    /// while frames recede.
    pub label: CColor,
    /// Pane status: running (OSC 133 command in flight).
    pub running: CColor,
    /// Pane status: idle / wants attention.
    pub idle: CColor,
    /// Pane status: failed (last command exited non-zero).
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
        Palette::RETRO
    }
}

impl Palette {
    /// The enforced default: true-black panels with high-contrast functional
    /// colors (bold white focus ring, dim gray unfocused chrome, blue running,
    /// amber idle, red failed). Every entry is an explicit RGB
    /// value, so the chrome reads the same whatever the host terminal is
    /// themed as.
    pub const RETRO: Palette = Palette {
        base: rgb(0x000000),
        surface: rgb(0x000000),
        overlay: rgb(0x3f3f3f),
        accent: rgb(0xffffff),
        text: rgb(0xffffff),
        label: rgb(0x808080),
        running: rgb(0x0090ff),
        idle: rgb(0xffb000),
        failed: rgb(0xff0000),
    };

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

    /// Full-intensity tint for a pane status. `Plain` (no claim) reuses
    /// the neutral skeleton overlay so it never reads as healthy or sick.
    pub fn status(&self, s: PaneStatus) -> CColor {
        use PaneStatus as S;
        match s {
            S::Plain => self.overlay,
            S::Running => self.running,
            S::Idle => self.idle,
            S::Failed => self.failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_is_retro() {
        let p = Palette::default();
        assert_eq!(p, Palette::RETRO);
        // True-black panels, white text: the chrome is self-contained and
        // never inherits the terminal scheme.
        assert_eq!(p.base, CColor::Rgb(0, 0, 0));
        assert_eq!(p.surface, CColor::Rgb(0, 0, 0));
        assert_eq!(p.text, CColor::Rgb(0xff, 0xff, 0xff));
        // Functional colors are explicit RGB, one distinct hue per meaning.
        for c in [
            p.overlay, p.accent, p.label, p.running, p.idle, p.failed,
        ] {
            assert!(matches!(c, CColor::Rgb(..)), "{c:?} is not an RGB color");
        }
        assert_ne!(p.accent, p.running, "focus and running must differ");
        assert_ne!(p.idle, p.failed, "idle and failed must differ");
    }

    #[test]
    fn muted_scales_rgb_and_passes_the_rest_through() {
        assert_eq!(
            Palette::muted(CColor::Rgb(0xff, 0xff, 0xff)),
            CColor::Rgb(0x99, 0x99, 0x99)
        );
        assert_eq!(Palette::muted(CColor::Idx(6)), CColor::Idx(6));
        assert_eq!(Palette::muted(CColor::Default), CColor::Default);
    }
}
