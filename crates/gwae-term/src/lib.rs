//! gwae-term: the emulator facade.
//!
//! Isolates the terminal-emulation crate behind a single `TermGrid` trait so
//! swapping the backend touches exactly one crate. The Alacritty core reflows
//! primary-screen text and scrollback when pane widths change.

mod compat;

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
    /// Cursor position inside the grid (row, col), 0-based.
    fn cursor_position(&self) -> (u16, u16);
    /// Whether the child asked to hide the cursor (DECTCEM).
    fn hide_cursor(&self) -> bool;
    /// Scrollback rows currently pulled into view (0 = live).
    fn scrollback_offset(&self) -> usize;
    /// Visible pane text (trimmed, no trailing blank lines).
    fn visible_text(&self) -> String {
        let size = self.size();
        let mut out = String::new();
        for y in 0..size.rows {
            let mut line = String::new();
            for x in 0..size.cols {
                let c = self.cell(x, y);
                if c.width != 0 {
                    c.push_codepoints(&mut line);
                }
            }
            let line = line.trim_end();
            if y > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        out.trim_end_matches('\n').to_string()
    }
    /// Full session text including scrollback (when available). Default is
    /// visible only; `Vt100Grid` overrides to include its 10k scrollback.
    fn session_text(&mut self) -> String {
        self.visible_text()
    }
}

// --- Reflowing Alacritty-backed grid ---

use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{Color, NamedColor, Processor},
};
use std::sync::mpsc;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// Keep title events inside the facade. The host still owns PTY query replies,
/// clipboard access, and graphics passthrough, just as with the previous core.
struct TitleListener(mpsc::Sender<String>);

impl EventListener for TitleListener {
    fn send_event(&self, event: Event) {
        let title = match event {
            Event::Title(title) => title,
            Event::ResetTitle => String::new(),
            _ => return,
        };
        let _ = self.0.send(title);
    }
}

fn map_color(c: Color) -> CColor {
    match c {
        Color::Indexed(i) => CColor::Idx(i),
        Color::Spec(rgb) => CColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Named(named) => {
            let n = named as u16;
            if n <= NamedColor::BrightWhite as u16 {
                CColor::Idx(n as u8)
            } else if (NamedColor::DimBlack as u16..=NamedColor::DimWhite as u16).contains(&n) {
                CColor::Idx((n - NamedColor::DimBlack as u16) as u8)
            } else {
                CColor::Default
            }
        }
    }
}

fn map_cell(c: &alacritty_terminal::term::cell::Cell) -> Cell {
    let mut combining = NO_COMBINING;
    for (slot, &cp) in combining.iter_mut().zip(c.zerowidth().unwrap_or_default()) {
        *slot = cp;
    }
    Cell {
        ch: c.c,
        combining,
        style: Style {
            fg: map_color(c.fg),
            bg: map_color(c.bg),
            bold: c.flags.contains(Flags::BOLD),
            underline: c.flags.intersects(Flags::ALL_UNDERLINES),
            inverse: c.flags.contains(Flags::INVERSE),
        },
        width: if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
            0
        } else if c.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        },
    }
}

/// A terminal grid with primary-screen reflow and 10,000 rows of scrollback.
/// Full-screen applications retain the standard non-reflowing alternate screen
/// and redraw for the new PTY dimensions on SIGWINCH.
pub struct TerminalGrid {
    term: Term<TitleListener>,
    parser: Processor,
    legacy_modes: compat::LegacyCsiNormalizer,
    titles: mpsc::Receiver<String>,
    title: String,
    size: Size,
}

/// Source-compatible name for callers of the original facade implementation.
pub type Vt100Grid = TerminalGrid;

impl TerminalGrid {
    pub fn new(size: Size) -> Self {
        let size = Self::nonzero_size(size);
        let (tx, titles) = mpsc::channel();
        Self {
            term: Term::new(Config::default(), &size, TitleListener(tx)),
            parser: Processor::new(),
            legacy_modes: compat::LegacyCsiNormalizer::default(),
            titles,
            title: String::new(),
            size,
        }
    }

    fn nonzero_size(size: Size) -> Size {
        // A wide glyph needs two columns. Reflowing it into a one-column
        // Alacritty grid cannot make progress, so retain a safe logical size
        // even if the host can show only a clipped sliver of the pane.
        Size {
            cols: size.cols.max(2),
            rows: size.rows.max(1),
        }
    }

    /// Full-screen apps own their scrolling instead of using our history.
    pub fn alternate_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// True when the child asked for any xterm mouse reporting mode.
    pub fn wants_mouse(&self) -> bool {
        self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// True when the child enabled bracketed paste (`DECSET 2004`).
    pub fn wants_bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn scrollback_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll by rows (positive = into history), clamping only at history bounds.
    /// Unlike the old parser, deep scrollback is safe beyond one screenful.
    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let before = self.scrollback_offset();
        // The core adds delta in i32 before clamping. Bound the request first
        // so even an extreme jump cannot overflow when already in history.
        let remaining = self.term.grid().history_size() - before;
        let delta = delta.clamp(-(before as i32), remaining as i32);
        self.term.scroll_display(Scroll::Delta(delta));
        self.scrollback_offset() != before
    }

    pub fn scroll_to_bottom(&mut self) -> bool {
        let before = self.scrollback_offset();
        self.term.scroll_display(Scroll::Bottom);
        before != 0
    }
}

impl TermGrid for TerminalGrid {
    fn size(&self) -> Size {
        self.size
    }

    fn resize(&mut self, size: Size) {
        let size = Self::nonzero_size(size);
        let history_before = self.term.grid().history_size();
        let reflow = size.cols != self.size.cols && !self.alternate_screen();
        // Resize the retained grid itself, including history and the inactive
        // primary screen. Replaying output would lose cursor edits and modes.
        self.term.resize(size);
        if reflow {
            // The core anchors the bottom of the grid, even when the rows below
            // the cursor are unused. Don't hide newly wrapped text in history
            // while there's room to show it. Drop only default rows below the
            // cursor, then grow back to pull that text into the viewport. Work
            // on the active grid so the alternate screen isn't resized twice.
            let grid = self.term.grid_mut();
            let added_history = grid.history_size().saturating_sub(history_before);
            let blank = alacritty_terminal::term::cell::Cell::default();
            let spare_rows = ((grid.cursor.point.line.0 + 1)..size.rows as i32)
                .rev()
                .take_while(|&y| {
                    (0..size.cols as usize).all(|x| grid[Point::new(Line(y), Column(x))] == blank)
                })
                .count();
            let pull = added_history.min(spare_rows);
            if pull > 0 {
                grid.resize::<Color>(true, size.rows as usize - pull, size.cols as usize);
                grid.resize::<Color>(true, size.rows as usize, size.cols as usize);
            }
        }
        self.size = size;
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<Damage> {
        let bytes = self.legacy_modes.feed(bytes);
        self.parser.advance(&mut self.term, &bytes);
        // The compositor owns synchronized host frames. Do not retain an
        // inner child's DECSET 2026 buffer: an interrupted frame could otherwise
        // freeze its pane forever since we don't run Alacritty's timeout loop.
        // stop_sync uses the same parser, preserving partial UTF-8/CSI state.
        self.parser.stop_sync(&mut self.term);
        for title in self.titles.try_iter() {
            self.title = title;
        }
        // The renderer diffs cells, so keep the existing whole-grid damage API.
        vec![Damage {
            x: 0,
            y: 0,
            w: self.size.cols,
            h: self.size.rows,
        }]
    }

    fn cell(&self, x: u16, y: u16) -> Cell {
        if x >= self.size.cols || y >= self.size.rows {
            return Cell::default();
        }
        let line = Line(y as i32 - self.scrollback_offset() as i32);
        map_cell(&self.term.grid()[Point::new(line, Column(x as usize))])
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn cursor_position(&self) -> (u16, u16) {
        let cursor = self.term.grid().cursor.point;
        (cursor.line.0 as u16, cursor.column.0 as u16)
    }

    fn hide_cursor(&self) -> bool {
        !self.term.mode().contains(TermMode::SHOW_CURSOR)
    }

    fn scrollback_offset(&self) -> usize {
        self.scrollback_offset()
    }

    fn session_text(&mut self) -> String {
        // Read history directly without moving the viewport. Preserve repeated
        // lines and hard breaks, joining only terminal-generated soft wraps.
        let grid = self.term.grid();
        let mut out = String::new();
        for y in -(grid.history_size() as i32)..self.size.rows as i32 {
            let mut line = String::new();
            for x in 0..self.size.cols as usize {
                let c = &grid[Point::new(Line(y), Column(x))];
                if !c
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    line.push(c.c);
                    line.extend(c.zerowidth().unwrap_or_default());
                }
            }
            let last = &grid[Point::new(Line(y), Column(self.size.cols as usize - 1))];
            if last.flags.contains(Flags::WRAPLINE) {
                out.push_str(&line);
            } else {
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
        out.trim_end_matches('\n').to_string()
    }
}

/// A blank, fixed-size grid used as a test double and during startup.
pub struct NullGrid {
    size: Size,
    cursor: (u16, u16),
    hide: bool,
}

/// Streaming extractor for Kitty graphics APC sequences (`ESC _ G ... ESC \`).
///
/// The terminal core parses and *drops* APC sequences, so
/// a child's Kitty image transmissions die inside the mux and panes show
/// nothing where an image should be. The fix is passthrough: gwae scans
/// each pane's raw PTY output and forwards complete graphics sequences
/// verbatim to the host terminal.
///
/// This is safe to do out-of-band because modern emitters (ratatui-image,
/// jcode) use *virtual placements* (`U=1`) addressed by U+10EEEE placeholder
/// cells: the APC only carries pixel data + an image id, and on-screen
/// position comes entirely from where the placeholder cells are painted. The
/// grid keeps those placeholder cells (see `Cell::combining`), so images land
/// exactly inside their pane and are cropped by pane clipping for free.
///
/// The extractor is a byte-level state machine so it survives PTY chunk
/// boundaries (a 1 MB PNG arrives as hundreds of 4 KB reads, and a chunk can
/// split even the 3-byte `ESC _ G` introducer). Non-graphics APCs are
/// swallowed, and a sequence over [`KittyApcExtractor::MAX_SEQ`] is discarded
/// rather than buffered forever (Kitty itself chunks payloads at 4 KB, so a
/// bigger "sequence" means a malformed or hostile stream).
#[derive(Default)]
pub struct KittyApcExtractor {
    state: ApcState,
    seq: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum ApcState {
    /// Ordinary output.
    #[default]
    Ground,
    /// Seen ESC.
    Esc,
    /// Seen ESC `_` (APC opener), kind not yet known.
    ApcOpen,
    /// Inside a graphics APC (`ESC _ G`), buffering into `seq`.
    Graphics,
    /// Inside a graphics APC, seen ESC (maybe ST).
    GraphicsEsc,
    /// Inside a non-graphics or oversized APC, discarding until ST.
    Skip,
    /// Inside a discarded APC, seen ESC (maybe ST).
    SkipEsc,
}

impl KittyApcExtractor {
    /// Upper bound for one buffered APC sequence. Kitty chunks image payloads
    /// at 4096 bytes of base64, so well-formed sequences are tiny; the bound
    /// only exists so a malformed stream cannot grow the buffer unboundedly.
    pub const MAX_SEQ: usize = 64 * 1024;

    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `bytes`, returning every complete Kitty graphics APC sequence
    /// (introducer and ST terminator included) ready to write to the host.
    /// Partial sequences are carried across calls.
    pub fn extract(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.state {
                ApcState::Ground => {
                    if b == 0x1b {
                        self.state = ApcState::Esc;
                    }
                }
                ApcState::Esc => {
                    self.state = match b {
                        b'_' => ApcState::ApcOpen,
                        0x1b => ApcState::Esc,
                        _ => ApcState::Ground,
                    };
                }
                ApcState::ApcOpen => match b {
                    b'G' => {
                        self.seq.clear();
                        self.seq.extend_from_slice(b"\x1b_G");
                        self.state = ApcState::Graphics;
                    }
                    0x1b => self.state = ApcState::Esc,
                    _ => self.state = ApcState::Skip,
                },
                ApcState::Graphics => {
                    if b == 0x1b {
                        self.state = ApcState::GraphicsEsc;
                    } else if self.seq.len() >= Self::MAX_SEQ {
                        self.seq.clear();
                        self.state = ApcState::Skip;
                    } else {
                        self.seq.push(b);
                    }
                }
                ApcState::GraphicsEsc => {
                    if b == b'\\' {
                        self.seq.extend_from_slice(b"\x1b\\");
                        // Queries (a=q) are dropped: the host's reply would
                        // arrive on gwae's stdin, not the child's, so
                        // forwarding them can only desync both sides.
                        if is_graphics_query(&self.seq) {
                            self.seq.clear();
                        } else {
                            out.append(&mut self.seq);
                        }
                        self.state = ApcState::Ground;
                    } else {
                        // ESC inside a graphics payload is malformed (payloads
                        // are base64 + ASCII keys); drop the sequence and
                        // re-treat this byte from the ESC state.
                        self.seq.clear();
                        self.state = if b == 0x1b {
                            ApcState::Esc
                        } else if b == b'_' {
                            ApcState::ApcOpen
                        } else {
                            ApcState::Ground
                        };
                    }
                }
                ApcState::Skip => {
                    if b == 0x1b {
                        self.state = ApcState::SkipEsc;
                    }
                }
                ApcState::SkipEsc => {
                    self.state = match b {
                        b'\\' => ApcState::Ground,
                        0x1b => ApcState::SkipEsc,
                        _ => ApcState::Skip,
                    };
                }
            }
        }
        out
    }
}

/// Whether a complete graphics APC (`ESC _ G <controls> ; <payload> ESC \`) is
/// a capability query (`a=q`). Only the control section before any `;` is
/// inspected, so base64 payload bytes can never false-positive.
fn is_graphics_query(seq: &[u8]) -> bool {
    let body = seq.strip_prefix(b"\x1b_G").unwrap_or(seq);
    let controls = match body.iter().position(|&b| b == b';') {
        Some(i) => &body[..i],
        None => body,
    };
    controls
        .split(|&b| b == b',')
        .any(|kv| kv == b"a=q" || kv == b"a=+q")
}

impl NullGrid {
    pub fn new(size: Size) -> Self {
        NullGrid {
            size,
            cursor: (0, 0),
            hide: false,
        }
    }

    #[cfg(test)]
    pub fn set_cursor(&mut self, row: u16, col: u16, hide: bool) {
        self.cursor = (row, col);
        self.hide = hide;
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

    fn cursor_position(&self) -> (u16, u16) {
        self.cursor
    }

    fn hide_cursor(&self) -> bool {
        self.hide
    }

    fn scrollback_offset(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_text_reflows_losslessly_across_width_cycles() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 8 });
        g.feed(b"abcdefghijklmnopqr");
        for _ in 0..3 {
            g.resize(Size { cols: 10, rows: 8 });
            assert_eq!(g.visible_text(), "abcdefghij\nklmnopqr");
            g.resize(Size { cols: 20, rows: 8 });
            assert_eq!(g.visible_text(), "abcdefghijklmnopqr");
        }
        g.feed(b"st");
        assert_eq!(g.visible_text(), "abcdefghijklmnopqrst");
    }

    #[test]
    fn reflow_preserves_hard_breaks_unicode_styles_and_cursor_edits() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 8 });
        g.feed("\x1b[1;4;7;31;44mabcd你e\u{0301}fghijklmnop\x1b[0m\r\nsecond".as_bytes());
        let original = g.session_text();
        for cols in [5, 9, 30, 6, 20] {
            g.resize(Size { cols, rows: 8 });
            assert_eq!(g.session_text(), original, "width {cols}");
        }
        assert_eq!(g.visible_text(), original);
        assert_eq!(g.cell(4, 0).ch, '你');
        assert_eq!(g.cell(4, 0).width, 2);
        assert_eq!(g.cell(5, 0).width, 0);
        assert_eq!(g.cell(6, 0).combining[0], '\u{0301}');
        assert_eq!(
            g.cell(0, 0).style,
            Style {
                fg: CColor::Idx(1),
                bg: CColor::Idx(4),
                bold: true,
                underline: true,
                inverse: true,
            }
        );
        // Cursor remains attached to the last hard line, not an old grid cell.
        g.feed(b"\x1b[3DXYZ");
        assert!(g.session_text().ends_with("secXYZ"));
    }

    #[test]
    fn deep_history_and_repeated_lines_survive_width_and_height_cycles() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 6 });
        let mut expected = Vec::new();
        for i in 0..40 {
            let line = if i % 3 == 0 {
                "repeated".into()
            } else {
                format!("row{i:02}: abcdefghijk")
            };
            g.feed(format!("{line}\r\n").as_bytes());
            expected.push(line);
        }
        let expected = expected.join("\n");
        assert_eq!(g.session_text(), expected);
        for size in [
            Size { cols: 7, rows: 3 },
            Size { cols: 40, rows: 10 },
            Size { cols: 20, rows: 6 },
        ] {
            g.resize(size);
            assert_eq!(g.session_text(), expected, "size {size:?}");
            g.scroll_by(i32::MAX);
            assert!(g.scrollback_offset() > size.rows as usize);
            assert!(g.visible_text().starts_with("repeate"));
            let offset = g.scrollback_offset();
            assert_eq!(g.session_text(), expected);
            assert_eq!(
                g.scrollback_offset(),
                offset,
                "export must not move viewport"
            );
            g.scroll_to_bottom();
        }
    }

    #[test]
    fn alternate_screen_redraw_does_not_replace_primary_history() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 8 });
        g.feed(b"abcdefghijklmnopqr\r\nprimary");
        let primary = g.session_text();
        g.feed(b"\x1b[?1049h\x1b[?25l\x1b[2JALT OLD FRAME");
        g.resize(Size { cols: 10, rows: 4 });
        assert!(g.alternate_screen());
        assert!(g.hide_cursor());
        assert_eq!(g.scrollback_offset(), 0);
        assert!(!g.scroll_by(99));
        // A full-screen child redraws in response to the new PTY dimensions.
        g.feed(b"\x1b[2J\x1b[HNEW\x1b[4;10HX");
        assert_eq!(g.cell(9, 3).ch, 'X');
        g.feed(b"\x1b[?1049l\x1b[?25h");
        assert!(!g.alternate_screen());
        assert!(!g.hide_cursor());
        assert_eq!(g.session_text(), primary);
        g.resize(Size { cols: 20, rows: 8 });
        assert_eq!(g.session_text(), primary);
        g.feed(b"!");
        assert_eq!(g.session_text(), format!("{primary}!"));
    }

    #[test]
    fn resize_keeps_partial_escape_sequences_and_truecolor() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 8 });
        g.feed(b"\x1b[38;2;10;");
        g.resize(Size { cols: 10, rows: 4 });
        g.feed(b"20;30mhello");
        assert_eq!(g.visible_text(), "hello");
        assert_eq!(g.cell(0, 0).style.fg, CColor::Rgb(10, 20, 30));
        assert_eq!(g.cell(10, 0), Cell::default());
        assert_eq!(g.cell(0, 4), Cell::default());
    }

    #[test]
    fn child_sync_markers_cannot_stall_the_hosted_grid() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        g.feed(b"before\x1b[?2026h after");
        assert_eq!(g.visible_text(), "before after");
        // An interrupted child can leave sync enabled forever. Later output,
        // including a resize, must not wait for its missing end marker.
        g.resize(Size { cols: 10, rows: 5 });
        g.feed(b"!");
        assert_eq!(g.session_text(), "before after!");

        // Flushing at feed boundaries must keep partial UTF-8/CSI/OSC state.
        let frame = "\x1b[?2026h\x1b[31m你e\u{0301}\x1b]2;frame\x07\x1b[?2026l";
        for cut in 0..=frame.len() {
            let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
            g.feed(&frame.as_bytes()[..cut]);
            g.feed(&frame.as_bytes()[cut..]);
            assert_eq!(g.visible_text(), "你e\u{0301}", "chunk at byte {cut}");
            assert_eq!(g.title(), "frame");
            assert_eq!(g.cell(0, 0).style.fg, CColor::Idx(1));
        }
    }

    #[test]
    fn tiny_dimensions_are_normalized_and_wide_text_survives() {
        let mut g = TerminalGrid::new(Size::default());
        assert_eq!(g.size(), Size { cols: 2, rows: 1 });
        g.resize(Size { cols: 10, rows: 4 });
        g.feed("a你bc".as_bytes());
        g.resize(Size { cols: 1, rows: 4 });
        assert_eq!(g.size(), Size { cols: 2, rows: 4 });
        assert_eq!(g.session_text(), "a你bc");
        g.resize(Size { cols: 10, rows: 4 });
        assert_eq!(g.visible_text(), "a你bc");
    }

    #[test]
    fn scrollback_moves_view_and_returns() {
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
    fn new_output_keeps_a_scrolled_view_pinned_after_resize() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        for i in 0..30 {
            g.feed(format!("row{i:02}: abcdefghijk\r\n").as_bytes());
        }
        g.scroll_by(12);
        g.resize(Size { cols: 10, rows: 4 });
        let view = g.visible_text();
        let before = g.scrollback_offset();
        g.feed(b"new output\r\n");
        assert!(g.scrollback_offset() > before);
        assert_eq!(g.visible_text(), view);
        assert!(g.scroll_to_bottom());
        assert!(g.visible_text().contains("new output"));
    }

    #[test]
    fn deep_scrollback_is_reachable_and_clamped_to_history() {
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 3 });
        for i in 0..10 {
            g.feed(format!("line{i}\r\n").as_bytes());
        }
        for _ in 0..10 {
            g.scroll_by(3);
        }
        assert_eq!(g.scrollback_offset(), 8);
        assert_eq!(g.visible_text(), "line0\nline1\nline2");
        assert!(!g.scroll_by(i32::MAX));
        assert!(g.scroll_by(i32::MIN));
        assert_eq!(g.scrollback_offset(), 0);
    }

    #[test]
    fn shrinking_the_grid_keeps_deep_scrollback_anchored() {
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 10 });
        for i in 0..20 {
            g.feed(format!("line{i}\r\n").as_bytes());
        }
        g.scroll_by(8);
        assert_eq!(g.scrollback_offset(), 8);
        let top: String = (0..6).map(|x| g.cell(x, 0).ch).collect();
        g.resize(Size { cols: 10, rows: 3 });
        assert_eq!(g.scrollback_offset(), 15);
        assert_eq!((0..6).map(|x| g.cell(x, 0).ch).collect::<String>(), top);
        g.resize(Size { cols: 10, rows: 10 });
        assert_eq!(g.scrollback_offset(), 8);
        assert_eq!((0..6).map(|x| g.cell(x, 0).ch).collect::<String>(), top);
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
    fn legacy_modes_keep_child_output_off_the_primary_screen() {
        let enter = b"MAIN\x1b[?47;9;25h\x1b[2J\x1b[HALT";
        for cut in 0..=enter.len() {
            let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
            g.feed(&enter[..cut]);
            g.feed(&enter[cut..]);
            assert!(g.alternate_screen(), "chunk at byte {cut}");
            assert!(g.wants_mouse());
            assert_eq!(g.visible_text(), "ALT");
            g.resize(Size { cols: 10, rows: 5 });
            g.feed(b"\x1b[?47;9l");
            assert!(!g.alternate_screen());
            assert!(!g.wants_mouse());
            assert_eq!(g.visible_text(), "MAIN");
        }
    }

    #[test]
    fn bracketed_paste_mode_is_reported() {
        // gwae strips the host's paste markers when it decodes an
        // back. Getting it wrong either runs a multi-line paste line by line
        // (markers withheld from a shell that asked) or prints `[200~` as
        // literal text (markers sent to a program that never asked).
        let mut g = Vt100Grid::new(Size { cols: 10, rows: 3 });
        assert!(!g.wants_bracketed_paste(), "off until the child asks");
        g.feed(b"\x1b[?2004h");
        assert!(g.wants_bracketed_paste());
        g.feed(b"\x1b[?2004l");
        assert!(!g.wants_bracketed_paste(), "and off again when it stops");
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

    #[test]
    fn vt100_cell_keeps_combining_marks() {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        // e + U+0301 (combining acute): both codepoints must survive.
        g.feed("e\u{0301}x".as_bytes());
        let c = g.cell(0, 0);
        assert_eq!(c.ch, 'e');
        assert_eq!(c.combining[0], '\u{0301}');
        assert_eq!(c.combining[1], '\0');
        let mut s = String::new();
        c.push_codepoints(&mut s);
        assert_eq!(s, "e\u{0301}");
        // Kitty placeholder base + row/col diacritics survive the same way.
        let mut g2 = Vt100Grid::new(Size { cols: 20, rows: 5 });
        g2.feed("\u{10EEEE}\u{0305}\u{030D}".to_string().as_bytes());
        let p = g2.cell(0, 0);
        assert_eq!(p.ch, '\u{10EEEE}');
        assert_eq!(p.combining[0], '\u{0305}');
        assert_eq!(p.combining[1], '\u{030D}');
    }

    #[test]
    fn apc_extractor_passes_graphics_and_survives_chunking() {
        let mut e = KittyApcExtractor::new();
        let seq = b"\x1b_Gi=42,a=T,U=1,f=32,s=2,v=1;AAAA\x1b\\";
        // Whole sequence in one chunk, surrounded by ordinary output.
        let out = e.extract(b"hello\x1b_Gi=42,a=T,U=1,f=32,s=2,v=1;AAAA\x1b\\world");
        assert_eq!(out, seq.to_vec());
        // Split at every possible byte boundary, including mid-introducer
        // and mid-terminator.
        for cut in 1..seq.len() {
            let mut e = KittyApcExtractor::new();
            let mut out = e.extract(&seq[..cut]);
            out.extend(e.extract(&seq[cut..]));
            assert_eq!(out, seq.to_vec(), "split at {cut}");
        }
    }

    #[test]
    fn apc_extractor_swallows_non_graphics_and_queries() {
        let mut e = KittyApcExtractor::new();
        // Non-graphics APC: swallowed.
        assert!(e.extract(b"\x1b_Xsomething\x1b\\").is_empty());
        // Graphics query (a=q): dropped, the host reply cannot be routed back.
        assert!(e
            .extract(b"\x1b_Ga=q,i=1,f=24,s=1,v=1;AAAA\x1b\\")
            .is_empty());
        // State machine returns to ground: a following display APC still passes.
        let seq = b"\x1b_Gi=7,a=T;AAAA\x1b\\";
        assert_eq!(e.extract(seq), seq.to_vec());
    }

    #[test]
    fn apc_extractor_bounds_runaway_sequences() {
        let mut e = KittyApcExtractor::new();
        // An unterminated "graphics" stream larger than MAX_SEQ is discarded,
        // not buffered forever.
        let big = vec![b'A'; KittyApcExtractor::MAX_SEQ + 1024];
        assert!(e.extract(b"\x1b_G").is_empty());
        assert!(e.extract(&big).is_empty());
        // Terminate the (now discarded) sequence; nothing comes out.
        assert!(e.extract(b"\x1b\\").is_empty());
        // And the extractor still works afterwards.
        let seq = b"\x1b_Gi=7,a=T;AAAA\x1b\\";
        assert_eq!(e.extract(seq), seq.to_vec());
    }
}
