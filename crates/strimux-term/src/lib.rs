//! strimux-term: the emulator facade.
//!
//! Isolates the terminal-emulation crate behind a single `TermGrid` trait so
//! swapping the backend touches exactly one crate. M0/M1 uses `vt100` for the
//! hosted grid (ADR-004 remains open for `alacritty_terminal` vs `wezterm-term`).

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
/// combining diacritics after U+10EEEE. Capacity matches vt100's six
/// codepoints per cell (one base + five combining).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub combining: [char; MAX_COMBINING],
    pub style: Style,
    pub width: u8,
}

/// Maximum combining codepoints stored per cell (vt100 keeps 6 total).
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

/// The rectangle of the grid (in cells).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// A described region of damage to be re-rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// Trait boundary for a hosted terminal emulator grid.
pub trait TermGrid {
    fn size(&self) -> Size;
    fn resize(&mut self, size: Size);
    fn feed(&mut self, bytes: &[u8]) -> Vec<Damage>;
    fn cell(&self, x: u16, y: u16) -> Cell;
    /// The most recent window title set by the child (OSC 0/2), if any.
    fn title(&self) -> &str;
}

// --- vt100-backed grid (M0/M1 implementation) ---

/// Map a vt100 color into our own.
fn map_color(c: vt100::Color) -> CColor {
    match c {
        vt100::Color::Default => CColor::Default,
        vt100::Color::Idx(i) => CColor::Idx(i),
        vt100::Color::Rgb(r, g, b) => CColor::Rgb(r, g, b),
    }
}

/// A `TermGrid` backed by the `vt100` parser.
pub struct Vt100Grid {
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
    /// How many scrollback rows are currently scrolled into view.
    scrollback_offset: usize,
}

impl Vt100Grid {
    pub fn new(size: Size) -> Self {
        let parser = vt100::Parser::new(size.rows, size.cols, 10_000);
        Vt100Grid {
            parser,
            rows: size.rows,
            cols: size.cols,
            scrollback_offset: 0,
        }
    }

    /// True when the child has taken over the alternate screen (a full-screen
    /// app like vim or less). Such apps own scrolling themselves, so wheel
    /// events must be forwarded to them instead of moving our scrollback.
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// True when the child asked for mouse reporting (any xterm mouse mode).
    pub fn wants_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// The number of scrollback rows currently scrolled into view.
    pub fn scrollback_offset(&self) -> usize {
        self.scrollback_offset
    }

    /// Scroll the view by `delta` rows (positive = back into history).
    /// Returns true when the visible offset actually changed.
    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let want = (self.scrollback_offset as i64 + delta as i64).max(0) as usize;
        self.parser.set_scrollback(want);
        let now = self.parser.screen().scrollback();
        let changed = now != self.scrollback_offset;
        self.scrollback_offset = now;
        changed
    }

    /// Jump back to the live bottom of the buffer.
    pub fn scroll_to_bottom(&mut self) -> bool {
        if self.scrollback_offset == 0 {
            return false;
        }
        self.parser.set_scrollback(0);
        self.scrollback_offset = 0;
        true
    }
}

impl TermGrid for Vt100Grid {
    fn size(&self) -> Size {
        Size {
            cols: self.cols,
            rows: self.rows,
        }
    }

    fn resize(&mut self, size: Size) {
        self.parser.set_size(size.rows, size.cols);
        self.rows = size.rows;
        self.cols = size.cols;
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<Damage> {
        // vt100 does not report incremental damage, so we re-render the whole grid.
        self.parser.process(bytes);
        // vt100 shifts the scrollback offset itself when new lines scroll the
        // buffer (so a scrolled-back view stays pinned to the same content);
        // adopt its value so our cached offset never drifts.
        self.scrollback_offset = self.parser.screen().scrollback();
        vec![Damage {
            x: 0,
            y: 0,
            w: self.cols,
            h: self.rows,
        }]
    }

    fn cell(&self, x: u16, y: u16) -> Cell {
        match self.parser.screen().cell(y, x) {
            Some(c) => {
                // vt100 hands back the whole cluster; the first codepoint is
                // the base glyph and the rest are combining marks (accents,
                // variation selectors, Kitty placeholder diacritics) that must
                // be carried through or composed text and Kitty images break.
                let contents = c.contents();
                let mut cps = contents.chars();
                let ch = cps.next().unwrap_or(' ');
                let mut combining = NO_COMBINING;
                for (slot, cp) in combining.iter_mut().zip(cps) {
                    *slot = cp;
                }
                let style = Style {
                    fg: map_color(c.fgcolor()),
                    bg: map_color(c.bgcolor()),
                    bold: c.bold(),
                    underline: c.underline(),
                    inverse: c.inverse(),
                };
                let width = if c.is_wide() {
                    2
                } else if c.is_wide_continuation() {
                    0
                } else {
                    1
                };
                Cell {
                    ch,
                    combining,
                    style,
                    width,
                }
            }
            None => Cell::default(),
        }
    }

    fn title(&self) -> &str {
        self.parser.screen().title()
    }
}

/// A blank, fixed-size grid used as a test double and during startup.
pub struct NullGrid {
    size: Size,
}

impl NullGrid {
    pub fn new(size: Size) -> Self {
        NullGrid { size }
    }
}

impl TermGrid for NullGrid {
    fn size(&self) -> Size {
        self.size
    }
    fn resize(&mut self, size: Size) {
        self.size = size;
    }
    fn feed(&mut self, _bytes: &[u8]) -> Vec<Damage> {
        vec![Damage {
            x: 0,
            y: 0,
            w: self.size.cols,
            h: self.size.rows,
        }]
    }
    fn cell(&self, _x: u16, _y: u16) -> Cell {
        Cell::default()
    }

    fn title(&self) -> &str {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_scrollback_moves_view_and_returns() {
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 3 });
        for i in 0..10 {
            g.feed(format!("line{i}\r\n").as_bytes());
        }
        // The live view shows the tail.
        assert_eq!(g.cell(0, 0).ch, 'l');
        assert_eq!(g.scrollback_offset(), 0);
        // Scrolling back reveals older lines.
        assert!(g.scroll_by(3));
        assert_eq!(g.scrollback_offset(), 3);
        let scrolled: String = (0..6).map(|x| g.cell(x, 0).ch).collect();
        assert_eq!(scrolled, "line5 ");
        // Scrolling forward past live is clamped at the bottom.
        assert!(g.scroll_by(-99));
        assert_eq!(g.scrollback_offset(), 0);
        assert!(!g.scroll_by(-1));
    }

    #[test]
    fn scroll_to_bottom_snaps_back_to_live() {
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 3 });
        for i in 0..10 {
            g.feed(format!("line{i}\r\n").as_bytes());
        }
        g.scroll_by(5);
        assert!(g.scroll_to_bottom());
        assert_eq!(g.scrollback_offset(), 0);
        assert!(!g.scroll_to_bottom());
    }

    #[test]
    fn alt_screen_and_mouse_modes_are_reported() {
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 3 });
        assert!(!g.alternate_screen());
        assert!(!g.wants_mouse());
        g.feed(b"\x1b[?1049h");
        assert!(g.alternate_screen());
        g.feed(b"\x1b[?1000h");
        assert!(g.wants_mouse());
        g.feed(b"\x1b[?1000l\x1b[?1049l");
        assert!(!g.alternate_screen());
        assert!(!g.wants_mouse());
    }

    #[test]
    fn vt100_feed_writes_cells() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        let dmg = g.feed(b"hello");
        assert_eq!(
            dmg,
            vec![Damage {
                x: 0,
                y: 0,
                w: 20,
                h: 5
            }]
        );
        assert_eq!(g.size(), Size { cols: 20, rows: 5 });
        assert_eq!(g.cell(0, 0).ch, 'h');
        assert_eq!(g.cell(4, 0).ch, 'o');
        assert_eq!(g.cell(5, 0).ch, ' ');
    }

    #[test]
    fn vt100_resize_changes_size() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        g.resize(Size { cols: 30, rows: 8 });
        assert_eq!(g.size(), Size { cols: 30, rows: 8 });
    }

    #[test]
    fn vt100_style_flags_survive() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        g.feed(b"\x1b[1mbold\x1b[0m");
        let c = g.cell(0, 0);
        assert_eq!(c.ch, 'b');
        assert!(c.style.bold);
        let c2 = g.cell(5, 0);
        assert_eq!(c2.ch, ' ');
        assert!(!c2.style.bold);
    }

    #[test]
    fn null_grid_is_blank() {
        let g = NullGrid::new(Size { cols: 10, rows: 10 });
        assert_eq!(g.cell(3, 4), Cell::default());
        assert_eq!(g.size(), Size { cols: 10, rows: 10 });
    }

    #[test]
    fn vt100_reports_wide_char_widths() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        // "你" is a two-column CJK glyph followed by an ASCII 'a'.
        g.feed("你a".as_bytes());
        let head = g.cell(0, 0);
        assert_eq!(head.ch, '你');
        assert_eq!(head.width, 2);
        // The cell under its right half is a zero-width continuation.
        let cont = g.cell(1, 0);
        assert_eq!(cont.width, 0);
        // Ordinary characters land after the full glyph width.
        let a = g.cell(2, 0);
        assert_eq!(a.ch, 'a');
        assert_eq!(a.width, 1);
    }

    #[test]
    fn vt100_tracks_osc_title() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        assert_eq!(g.title(), "");
        // OSC 2 (window title) terminated by BEL.
        g.feed(b"\x1b]2;my session\x07");
        assert_eq!(g.title(), "my session");
        // OSC 0 (icon + window title) terminated by ST (ESC \).
        g.feed(b"\x1b]0;other title\x1b\\");
        assert_eq!(g.title(), "other title");
        // A later title completely replaces the previous one.
        g.feed(b"\x1b]0;\x1b\\");
        assert_eq!(g.title(), "");
    }
}
