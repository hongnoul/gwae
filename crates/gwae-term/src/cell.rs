//! Facade value types: colors, styles, and cells.

/// A terminal color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CColor {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

/// An SGR style applied to a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: CColor,
    pub bg: CColor,
    /// SGR 58, also used as the Kitty Unicode-placeholder placement id.
    pub underline_color: CColor,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// A single cell: a character, its style, and its terminal column width.
///
/// `width` is the number of screen columns the glyph occupies when printed:
/// 1 for ordinary characters, 2 for wide (CJK/emoji) characters, and 0 for
/// the continuation cell that sits under the right half of a wide character.
/// The renderer must skip width-0 cells (the wide glyph already covers that
/// column); printing them as spaces shears every following cell one column
/// to the right.
///
/// `combining` carries the zero-width codepoints attached to `ch` (accents,
/// variation selectors, and Kitty image-placeholder diacritics), NUL-padded.
/// Dropping them breaks composed text (é as e+U+0301) and completely breaks
/// Kitty Unicode-placeholder images, whose row/column addressing lives in
/// combining diacritics after U+10EEEE. The facade retains one base and up to
/// five combining codepoints per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub combining: [char; MAX_COMBINING],
    pub style: Style,
    pub width: u8,
}

/// Maximum combining codepoints stored per facade cell.
pub const MAX_COMBINING: usize = 5;

/// A `combining` array holding no codepoints.
pub const NO_COMBINING: [char; MAX_COMBINING] = ['\0'; MAX_COMBINING];

impl Cell {
    /// Append every codepoint of this cell (base char plus combining marks)
    /// to `out`, in order.
    pub fn push_codepoints(&self, out: &mut String) {
        out.push(self.ch);
        for &c in &self.combining {
            if c == '\0' {
                break;
            }
            out.push(c);
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            combining: NO_COMBINING,
            style: Style::default(),
            width: 1,
        }
    }
}
