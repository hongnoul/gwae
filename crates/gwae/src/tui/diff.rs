//! Frame diff painting to the host terminal (verbatim move from `tui/mod.rs`).

use crossterm::cursor;
use gwae_term::{CColor, Cell};

fn crossterm_color(c: CColor) -> crossterm::style::Color {
    match c {
        CColor::Default => crossterm::style::Color::Reset,
        CColor::Idx(i) => crossterm::style::Color::AnsiValue(i),
        CColor::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
    }
}

/// Diff and paint `out` vs `last` into `buf`. Returns true if anything changed.
///
/// Wide (two-column) characters need care to avoid shearing the row:
///  - width-0 continuation cells are skipped, because the wide glyph printed
///    just before them already covers that column; printing their placeholder
///    space would shift everything after it one column right.
///  - every run starts with an explicit `MoveTo`, and a run is cut right after
///    any non-single-width cell, so even if the host terminal disagrees with
///    the emulator about a glyph's width (a classic emoji problem) the drift
///    is bounded to that one glyph instead of shearing the rest of the row.
///
/// Attributes are reset per run, not per row: SGR attributes (bold, underline,
/// reverse) have no "set exactly these" form, only additive codes, so a run
/// that doesn't reset first would inherit whatever the previous run enabled.
/// The observed failure was a popup row with underlined entries painting an
/// underline across every cell to its right ("line overflow"), which then
/// stuck because the diff buffer believed those cells were already blank.
///
/// Runs also stop at any glyph whose *host* width (per `unicode-width`)
/// disagrees with the width the emulator recorded. East-Asian text (Hangul,
/// CJK) and ambiguous-width symbols are the common case: the host advances the
/// cursor two columns where the emulator assumed one (or vice versa), and a
/// long merged run then paints past the pane's right edge, wraps at the screen
/// margin, and stains the rows below with the run's background ("highlight
/// overflow"). Cutting the run and re-issuing an explicit `MoveTo` bounds any
/// disagreement to the offending glyph.
pub(crate) fn paint(buf: &mut Vec<u8>, out: &[Cell], last: &[Cell], cols: u16, rows: u16) -> bool {
    use crossterm::queue;
    use crossterm::style::{
        Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor, SetUnderlineColor,
    };
    let cc = cols as usize;
    let mut dirty = false;
    for y in 0..rows as usize {
        let row_eq = last.get(y * cc..(y + 1) * cc) == Some(&out[y * cc..(y + 1) * cc]);
        if row_eq {
            continue;
        }
        dirty = true;
        // Group cells into style runs and print each run.
        let mut x = 0usize;
        while x < cc {
            let cell = out[y * cc + x];
            if cell.width == 0 {
                // Continuation of a wide char; the glyph already covers it.
                x += 1;
                continue;
            }
            let style = cell.style;
            let mut run = String::new();
            cell.push_codepoints(&mut run);
            let mut end = x + 1;
            if cell.width == 1 && host_width_agrees(cell) {
                while end < cc && out[y * cc + end].style == style {
                    let next = out[y * cc + end];
                    if next.width != 1 || !host_width_agrees(next) {
                        break;
                    }
                    next.push_codepoints(&mut run);
                    end += 1;
                }
            }
            let _ = queue!(
                buf,
                cursor::MoveTo(x as u16, y as u16),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(crossterm_color(style.fg)),
                SetBackgroundColor(crossterm_color(style.bg)),
            );
            if style.bold {
                let _ = queue!(buf, SetAttribute(Attribute::Bold));
            }
            if style.underline {
                let _ = queue!(buf, SetAttribute(Attribute::Underlined));
            }
            if style.underline_color != CColor::Default {
                let _ = queue!(
                    buf,
                    SetUnderlineColor(crossterm_color(style.underline_color))
                );
            }
            if style.inverse {
                let _ = queue!(buf, SetAttribute(Attribute::Reverse));
            }
            let _ = queue!(buf, Print(run));
            x = end;
        }
    }
    dirty
}

/// Whether the host terminal is expected to advance the cursor by exactly the
/// column count the emulator recorded for this cell. Control/zero-width and
/// ambiguous- or wide-width glyphs that the emulator called single-width are
/// the disagreement cases; those cells are printed alone so any drift stays
/// bounded to one column instead of shearing (and wrapping) the whole row.
fn host_width_agrees(cell: Cell) -> bool {
    // Protocol-generated placeholders have a specified one-cell width. Their
    // explicit high-id mark distinguishes these from arbitrary unknown glyphs.
    if cell.ch == crate::graphics_host::PLACEHOLDER && cell.combining[2] != '\0' {
        return cell.width == 1;
    }
    use unicode_width::UnicodeWidthChar;
    match cell.ch.width() {
        Some(w) => w as u8 == cell.width,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::{CColor, Cell};

    #[test]
    fn paint_emits_combining_marks_with_base_glyph() {
        // A cell holding a Kitty image placeholder (U+10EEEE) with row/col
        // diacritics: the diacritics are what address the image, so they must
        // reach the host bytes right after the base char.
        let mut row = vec![Cell::default(); 3];
        row[0].ch = '\u{10EEEE}';
        row[0].combining[0] = '\u{0305}';
        row[0].combining[1] = '\u{030D}';
        // width() for U+10EEEE is None (unassigned plane), so the run is cut
        // and printed alone; that must not drop the combining marks.
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            3
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 3, 1));
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("\u{10EEEE}\u{0305}\u{030D}"),
            "combining marks split from base: {s:?}"
        );
    }

    #[test]
    fn paint_skips_wide_continuation_cells() {
        // Row: wide '你' (head width 2, then a width-0 continuation), then "ab".
        // If the continuation's placeholder space were printed, 'a' would land
        // one column too far right and shear the row.
        let mut row = vec![Cell::default(); 6];
        row[0] = Cell {
            ch: '你',
            width: 2,
            ..Cell::default()
        };
        row[1] = Cell {
            ch: ' ',
            width: 0,
            ..Cell::default()
        };
        row[2].ch = 'a';
        row[3].ch = 'b';
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            6
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 6, 1));
        let s = String::from_utf8(buf).unwrap();
        // The wide glyph is printed exactly once and the continuation's
        // placeholder space is never printed between it and 'a'.
        assert_eq!(s.matches('你').count(), 1);
        assert!(!s.contains("你 a"), "continuation cell was printed: {s:?}");
        // 'a' is re-positioned to its true column (x=2) with an explicit
        // MoveTo (CUP row 1, col 3 -> ESC[1;3H) rather than relying on the
        // host's cursor advance across the wide glyph.
        assert!(
            s.contains("\u{1b}[1;3H"),
            "missing MoveTo before 'a': {s:?}"
        );
    }

    #[test]
    fn paint_cuts_runs_at_width_ambiguous_glyphs() {
        // A highlighted (styled) row of Hangul: vt100 records each syllable as
        // a single-width cell, but the host renders it two columns wide. Merged
        // into one run, the run overshoots the right margin, wraps, and smears
        // its background down the screen ("highlight overflow"). Each such
        // glyph must therefore be printed as its own MoveTo-anchored run.
        let hl = gwae_term::Style {
            bg: CColor::Idx(238),
            ..gwae_term::Style::default()
        };
        let text = "\u{ac00}\u{b098}\u{b2e4}"; // 가나다
        let mut row = vec![
            Cell {
                style: hl,
                ..Cell::default()
            };
            4
        ];
        for (i, ch) in text.chars().enumerate() {
            row[i] = Cell {
                ch,
                style: hl,
                width: 1, // emulator's (wrong for this host) idea of the width
                ..Cell::default()
            };
        }
        let last = vec![Cell::default(); 4];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 4, 1));
        let s = String::from_utf8(buf).unwrap();
        // Never merged: no two ambiguous glyphs share a run.
        assert!(
            !s.contains("\u{ac00}\u{b098}"),
            "ambiguous glyphs merged into one run: {s:?}"
        );
        // Every glyph is re-anchored with an explicit absolute cursor move, so
        // a host/emulator width disagreement cannot drift past this cell.
        for (i, ch) in text.chars().enumerate() {
            let mv = format!("\x1b[1;{}H", i + 1);
            let at = s
                .find(&mv)
                .unwrap_or_else(|| panic!("no MoveTo for col {i}: {s:?}"));
            let g = s.find(ch).unwrap();
            assert!(at < g, "glyph {ch} printed before its MoveTo: {s:?}");
        }
    }

    #[test]
    fn paint_keeps_merging_plain_ascii_runs() {
        // The cut must be surgical: ordinary text still batches into one run.
        let mut row = vec![Cell::default(); 6];
        for (i, ch) in "hello".chars().enumerate() {
            row[i].ch = ch;
        }
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            6
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 6, 1));
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("hello"), "ascii run was split: {s:?}");
    }

    #[test]
    fn paint_resets_attributes_between_runs() {
        // Regression for the popup "line overflow": an underlined run followed
        // by a plain run on the same row. SGR attrs are additive, so without a
        // reset at the start of the second run the underline bleeds across the
        // rest of the row on the host terminal.
        let mut row = vec![Cell::default(); 4];
        row[0].ch = 'u';
        row[0].style.underline = true;
        row[1].ch = 'p';
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            4
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 4, 1));
        let s = String::from_utf8(buf).unwrap();
        // Underline (SGR 4) is enabled for the first run, and a full reset
        // (SGR 0) is emitted after it and before the plain run's text.
        let under = s.find("\u{1b}[4m").expect("underline never set");
        let reset_after = s[under..]
            .find("\u{1b}[0m")
            .expect("no attribute reset after underlined run");
        let plain = s.find('p').expect("plain run missing");
        assert!(
            under + reset_after < plain,
            "underline leaks into the plain run: {s:?}"
        );
    }
}
