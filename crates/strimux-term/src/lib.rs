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
    /// Cursor position inside the grid (row, col), 0-based.
    fn cursor_position(&self) -> (u16, u16);
    /// Whether the child asked to hide the cursor (DECTCEM).
    fn hide_cursor(&self) -> bool;
    /// Scrollback rows currently pulled into view (0 = live).
    fn scrollback_offset(&self) -> usize;
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

    fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    fn hide_cursor(&self) -> bool {
        self.parser.screen().hide_cursor()
    }

    fn scrollback_offset(&self) -> usize {
        self.scrollback_offset
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
/// vt100 (like every cell-grid emulator) parses and *drops* APC sequences, so
/// a child's Kitty image transmissions die inside the mux and panes show
/// nothing where an image should be. The fix is passthrough: strimux scans
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
                        // arrive on strimux's stdin, not the child's, so
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
        assert!(e.extract(b"\x1b_Ga=q,i=1,f=24,s=1,v=1;AAAA\x1b\\").is_empty());
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
