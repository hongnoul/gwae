//! Chrome palette: fixed terminal-native colors.
//!
//! gwae has no theming. Every chrome color is the host terminal's own: the
//! default foreground/background pair plus ANSI 0-15 indices. Change the
//! terminal's scheme and gwae follows. There is no config key, no preset,
//! and no picker.

use gwae_layout::PaneStatus;
use gwae_term::CColor;

/// Every color gwae paints as chrome.
///
/// Status tints are stored at full intensity; the muted variants used for
/// minimap tiles are derived with [`Palette::muted`] rather than stored.
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
    /// Pane status: failed (last command exited non-zero).
    pub failed: CColor,
}

impl Default for Palette {
    fn default() -> Self {
        Palette::TERMINAL
    }
}

impl Palette {
    /// The only palette: the host terminal's own ANSI 0-15 colors.
    ///
    /// Nothing is hardcoded to an RGB value, so gwae inherits whatever the
    /// terminal is already themed as. `base`, `surface`, and `text` use
    /// [`CColor::Default`], preserving the terminal's native
    /// foreground/background pair rather than assuming ANSI black and white
    /// are its default colors.
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

    #[test]
    fn default_palette_is_terminal_native() {
        let p = Palette::default();
        assert_eq!(p, Palette::TERMINAL);
        assert_eq!(p.base, CColor::Default);
        assert_eq!(p.surface, CColor::Default);
        assert_eq!(p.text, CColor::Default);
        for c in [
            p.overlay, p.accent, p.label, p.running, p.idle, p.done, p.failed,
        ] {
            assert!(matches!(c, CColor::Idx(_)), "{c:?} is not an ANSI index");
        }
    }

    #[test]
    fn muted_passes_indexed_and_default_through() {
        assert_eq!(Palette::muted(CColor::Idx(6)), CColor::Idx(6));
        assert_eq!(Palette::muted(CColor::Default), CColor::Default);
    }
}
