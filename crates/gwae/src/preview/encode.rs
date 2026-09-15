//! Preview ANSI encoding: run-length rows and color escapes.

use crate::theme::Palette;
use gwae_layout::width::{Preset, Width};
use super::paint::Cell;
use super::prefs::RESET;
use gwae_term::CColor;

pub(super) fn run_length(row: &[Cell]) -> String {
    let mut s = String::new();
    let mut cur: Option<(CColor, CColor)> = None;
    for c in row {
        if cur != Some((c.fg, c.bg)) {
            s.push_str(&fg(c.fg));
            s.push_str(&bg(c.bg));
            cur = Some((c.fg, c.bg));
        }
        s.push(c.ch);
    }
    s.push_str(RESET);
    s
}

/// SGR foreground, passing indexed/default colors through untouched so the
/// `terminal` preset previews as the user's own ANSI palette.
pub(super) fn fg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[38;5;{i}m"),
        CColor::Default => "\x1b[39m".to_string(),
    }
}

/// SGR background, with the same pass-through rule as [`fg`].
pub(super) fn bg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[48;5;{i}m"),
        CColor::Default => "\x1b[49m".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::paint::render;
    use super::super::prefs::{Prefs, H};

    /// The visible characters of a rendered preview, ANSI stripped.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for c2 in it.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// The mockup stays rectangular and inside its frame at every height the
    /// adaptive path can pick, including the shortest.
    
    #[test]
    fn crlf_mode_terminates_every_line() {
        let s = render(&Prefs::default(), true);
        assert_eq!(s.matches("\r\n").count(), H + 2);
        assert!(!s.replace("\r\n", "").contains('\n'));
    }

}
