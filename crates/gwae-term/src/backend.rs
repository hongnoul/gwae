//! Alacritty-backed `TermGrid` with primary-screen reflow and scrollback.

use super::compat;
use crate::{
    cell::{CColor, Cell, Style, NO_COMBINING},
    grid::{Damage, Size, TermGrid},
};
// --- Reflowing Alacritty-backed grid ---

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{cell::Flags, Config, Term, TermMode},
    vte::{
        ansi::{Color, NamedColor, Processor},
        Params, Parser, Perform,
    },
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

/// Keep titles and parser-generated replies in one queue so deferred size
/// callbacks cannot overtake immediate PTY writes. Clipboard access and graphics
/// passthrough remain the host's responsibility.
struct GridListener(mpsc::Sender<Event>);

impl EventListener for GridListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(_)
            | Event::ResetTitle
            | Event::PtyWrite(_)
            | Event::TextAreaSizeRequest(_) => {}
            _ => return,
        }
        let _ = self.0.send(event);
    }
}

/// vte's ANSI handler supports window queries 14 and 18, but not 16. Use
/// its VT parser for this one extension too, so control-string payloads,
/// cancellations and partial sequences are never mistaken for queries.
#[derive(Default)]
struct CellSizeQuery(bool, bool);

impl Perform for CellSizeQuery {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.1 = !ignore
            && intermediates == b"?"
            && matches!(action, 'h' | 'l')
            && params.iter().any(|p| matches!(p, [47] | [1047] | [1049]));
        let mut params = params.iter();
        self.0 = !ignore
            && intermediates.is_empty()
            && action == 't'
            && params.next() == Some(&[16][..])
            && params.next().is_none();
    }

    fn terminated(&self) -> bool {
        self.0 || self.1
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.1 = !ignore && intermediates.is_empty() && byte == b'c';
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
            underline_color: c.underline_color().map(map_color).unwrap_or_default(),
            bold: c.flags.contains(Flags::BOLD),
            underline: c.flags.intersects(Flags::ALL_UNDERLINES),
            // The emulator has no overline flag: SGR 53 parses as nothing,
            // so mapped cells never carry it. It only flows gwae -> host.
            overline: false,
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
    term: Term<GridListener>,
    parser: Processor,
    cell_query_parser: Parser,
    legacy_modes: compat::LegacyCsiNormalizer,
    events: mpsc::Receiver<Event>,
    pty_replies: Vec<u8>,
    cell_width: u16,
    cell_height: u16,
    screen_epoch: u64,
    title: String,
    size: Size,
}

/// Source-compatible name for callers of the original facade implementation.
pub type Vt100Grid = TerminalGrid;

impl TerminalGrid {
    pub fn new(size: Size) -> Self {
        let size = Self::nonzero_size(size);
        let (tx, events) = mpsc::channel();
        Self {
            term: Term::new(Config::default(), &size, GridListener(tx)),
            parser: Processor::new(),
            cell_query_parser: Parser::new(),
            legacy_modes: compat::LegacyCsiNormalizer::default(),
            events,
            pty_replies: Vec::new(),
            cell_width: 0,
            cell_height: 0,
            screen_epoch: 0,
            title: String::new(),
            size,
        }
    }

    /// Set measured cell dimensions in pixels. Zero means unknown (the default).
    /// Measurements are retained across grid resizes.
    pub fn set_cell_size(&mut self, width: u16, height: u16) {
        self.cell_width = width;
        self.cell_height = height;
    }

    /// Drain parser-generated replies, in query order, for writing to the child PTY.
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pty_replies)
    }

    /// Changes on alternate-screen commands or RIS, even when two such
    /// commands arrive in one read and the final screen mode is unchanged.
    pub fn screen_epoch(&self) -> u64 {
        self.screen_epoch
    }

    /// Absolute graphics cursor advance, independent of DEC origin mode.
    pub fn set_graphics_cursor(&mut self, row: u16, col: u16) {
        let row = row.min(self.size.rows.saturating_sub(1));
        let col = col.min(self.size.cols.saturating_sub(1));
        let cursor = &mut self.term.grid_mut().cursor;
        cursor.point.line = Line(row as i32);
        cursor.point.column = Column(col as usize);
        cursor.input_needs_wrap = false;
    }

    fn window_size(&self) -> WindowSize {
        // Alacritty's pixel-size callback multiplies u16 values. Clamp each
        // measurement before invoking it, and recompute after every resize.
        WindowSize {
            num_lines: self.size.rows,
            num_cols: self.size.cols,
            cell_width: self.cell_width.min(u16::MAX / self.size.cols),
            cell_height: self.cell_height.min(u16::MAX / self.size.rows),
        }
    }

    fn process_events(&mut self) {
        let window_size = self.window_size();
        for event in self.events.try_iter() {
            match event {
                Event::Title(title) => self.title = title,
                Event::ResetTitle => self.title.clear(),
                Event::PtyWrite(reply) => self.pty_replies.extend_from_slice(reply.as_bytes()),
                Event::TextAreaSizeRequest(format) => {
                    self.pty_replies
                        .extend_from_slice(format(window_size).as_bytes());
                }
                _ => {}
            }
        }
    }

    fn feed_core(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        // The compositor owns synchronized host frames. Flush child frames
        // before emitting extension replies so earlier core replies stay first.
        self.parser.stop_sync(&mut self.term);
        self.process_events();
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
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let mut query = CellSizeQuery::default();
            let consumed = self
                .cell_query_parser
                .advance_until_terminated(&mut query, remaining);
            self.feed_core(&remaining[..consumed]);
            if query.1 {
                self.screen_epoch = self.screen_epoch.wrapping_add(1);
            }
            if query.0 {
                let size = self.window_size();
                self.pty_replies.extend_from_slice(
                    format!("\x1b[6;{};{}t", size.cell_height, size.cell_width).as_bytes(),
                );
            }
            remaining = &remaining[consumed..];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CColor, Cell, Damage, Size, Style, TermGrid};

    #[test]
    fn query_replies_preserve_order_at_every_byte_boundary() {
        let input =
            b"\x1b[14t\x1b[16t\x1b[18t\x1b[c\x1b[5n\x1b[3;7H\x1b[6nXY\x1b[6n\x1b[16t\x1b[14t";
        let expected =
            b"\x1b[4;120;160t\x1b[6;24;8t\x1b[8;5;20t\x1b[?6c\x1b[0n\x1b[3;7R\x1b[3;9R\x1b[6;24;8t\x1b[4;120;160t";
        // Includes a single coalesced feed and every possible two-chunk split.
        for split in 0..=input.len() {
            let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
            g.set_cell_size(8, 24);
            g.feed(&input[..split]);
            let mut replies = g.take_pty_replies();
            g.feed(&input[split..]);
            replies.extend(g.take_pty_replies());
            assert_eq!(replies, expected, "split at byte {split}");
            assert!(g.take_pty_replies().is_empty(), "replies must drain");
            assert_eq!(g.cursor_position(), (2, 8));
        }
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        g.set_cell_size(8, 24);
        let mut replies = Vec::new();
        for byte in input {
            g.feed(std::slice::from_ref(byte));
            replies.extend(g.take_pty_replies());
        }
        assert_eq!(replies, expected, "one byte per feed");
    }

    #[test]
    fn geometry_replies_use_current_measurements_and_resize() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        assert!(g.take_pty_replies().is_empty());
        g.feed(b"\x1b[14t\x1b[18t");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;0;0t\x1b[8;5;20t");
        g.set_cell_size(8, 24);
        g.feed(b"\x1b[14t\x1b[18t\x1b[1");
        // Already-generated replies must keep the old dimensions even when
        // the host drains them only after a resize and measurement update.
        g.resize(Size { cols: 30, rows: 8 });
        g.set_cell_size(10, 18);
        g.feed(b"4t\x1b[18t");
        assert_eq!(
            g.take_pty_replies(),
            b"\x1b[4;120;160t\x1b[8;5;20t\x1b[4;144;300t\x1b[8;8;30t"
        );
        g.resize(Size { cols: 10, rows: 3 });
        g.feed(b"\x1b[14t");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;54;100t");
        g.set_cell_size(0, 0);
        g.feed(b"\x1b[14t");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;0;0t");
    }

    #[test]
    fn pixel_callbacks_clamp_before_multiplying_and_reclamp_after_resize() {
        let mut g = TerminalGrid::new(Size { cols: 80, rows: 24 });
        g.set_cell_size(u16::MAX, u16::MAX);
        g.feed(b"\x1b[14t\x1b[18t");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;65520;65520t\x1b[8;24;80t");
        g.resize(Size::default());
        g.feed(b"\x1b[14t\x1b[18t");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;65535;65534t\x1b[8;1;2t");
    }

    #[test]
    fn dsr_uses_live_cursor_even_when_scrollback_is_visible() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        for _ in 0..20 {
            g.feed(b"history\r\n");
        }
        g.feed(b"\x1b[4;9H");
        g.scroll_by(10);
        assert!(g.scrollback_offset() > 0);
        g.feed(b"\x1b[6n\x1b[2;3H\x1b[6n");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;9R\x1b[2;3R");
    }

    #[test]
    fn queries_do_not_change_titles_or_title_stack_behavior() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        // Save the original None title so restoring it emits ResetTitle.
        g.feed(b"\x1b[22t");
        g.feed(b"\x1b]2;first\x07\x1b[22t\x1b[14t\x1b]0;second\x1b\\\x1b[18t");
        assert_eq!(g.title(), "second");
        assert_eq!(g.take_pty_replies(), b"\x1b[4;0;0t\x1b[8;5;20t");
        g.feed(b"\x1b[23t\x1b[5n");
        assert_eq!(g.title(), "first");
        assert_eq!(g.take_pty_replies(), b"\x1b[0n");
        g.feed(b"\x1b[23t");
        assert_eq!(g.title(), "");
        assert!(g.take_pty_replies().is_empty());
    }

    #[test]
    fn query_like_payload_in_osc_and_apc_does_not_reply() {
        // CSI-looking text and C1 bytes are opaque payload, not queries. A raw
        // ESC would instead terminate the string in the backend's VT parser.
        let input =
            b"\x1b]2;[14t [18t [6n [c \x9b14t\x07\x1b_Gpayload;[14t [18t [6n [c \x9b18t\x1b\\";
        for split in 0..=input.len() {
            let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
            g.feed(&input[..split]);
            assert!(g.take_pty_replies().is_empty(), "first chunk {split}");
            g.feed(&input[split..]);
            assert!(g.take_pty_replies().is_empty(), "second chunk {split}");
            assert_eq!(g.visible_text(), "");
            g.feed(b"\x1b[18t");
            assert_eq!(g.take_pty_replies(), b"\x1b[8;5;20t");
        }
    }

    #[test]
    fn sgr58_underline_color_and_resets_survive_every_split() {
        let input = b"\x1b[58;2;18;52;86mA\x1b[58;5;42mB\x1b[59mC\x1b[58:2::7:8:9mD\x1b[0mE";
        for split in 0..=input.len() {
            let mut g = TerminalGrid::new(Size { cols: 8, rows: 3 });
            g.feed(&input[..split]);
            g.feed(&input[split..]);
            for (col, expected) in [
                CColor::Rgb(18, 52, 86),
                CColor::Idx(42),
                CColor::Default,
                CColor::Rgb(7, 8, 9),
                CColor::Default,
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    g.cell(col as u16, 0).style.underline_color,
                    expected,
                    "split {split}, col {col}"
                );
            }
        }
    }

    #[test]
    fn screen_epoch_counts_alt_pairs_and_ris_at_every_split() {
        for input in [
            b"\x1b[?47h\x1b[?47l".as_slice(),
            b"\x1b[?1047h\x1b[?1047l".as_slice(),
            b"\x1b[?1049h\x1b[?1049l".as_slice(),
            b"\x1bc\x1bc".as_slice(),
        ] {
            for split in 0..=input.len() {
                let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
                assert_eq!(g.screen_epoch(), 0);
                g.feed(&input[..split]);
                g.feed(&input[split..]);
                assert_eq!(g.screen_epoch(), 2, "input {input:?}, split {split}");
                g.feed(b"plain ?1049h text\x1b[16t");
                assert_eq!(g.screen_epoch(), 2);
                assert_eq!(g.take_pty_replies(), b"\x1b[6;0;0t");
            }
        }
    }

    #[test]
    fn graphics_cursor_is_absolute_clamped_and_clears_pending_wrap() {
        let mut g = TerminalGrid::new(Size { cols: 8, rows: 5 });
        g.feed(b"\x1b[2;4r\x1b[?6h12345678");
        assert_eq!(g.cursor_position(), (1, 7));
        g.set_graphics_cursor(0, 0);
        assert_eq!(g.cursor_position(), (0, 0));
        g.feed(b"X");
        assert_eq!(g.cell(0, 0).ch, 'X');
        assert_eq!(g.cursor_position(), (0, 1));
        g.set_graphics_cursor(u16::MAX, u16::MAX);
        assert_eq!(g.cursor_position(), (4, 7));
        g.set_graphics_cursor(3, 0);
        g.feed(b"Y");
        assert_eq!(g.cell(0, 3).ch, 'Y');
        assert_eq!(g.cursor_position(), (3, 1));
    }

    #[test]
    fn csi_16t_uses_current_measured_cells() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        g.feed(b"\x1b[16t");
        assert_eq!(g.take_pty_replies(), b"\x1b[6;0;0t");
        g.set_cell_size(8, 24);
        for byte in b"\x1b[16" {
            g.feed(std::slice::from_ref(byte));
            assert!(g.take_pty_replies().is_empty());
        }
        g.resize(Size { cols: 30, rows: 8 });
        g.set_cell_size(10, 18);
        g.feed(b"t\x1b[14t");
        assert_eq!(g.take_pty_replies(), b"\x1b[6;18;10t\x1b[4;144;300t");
        g.set_cell_size(u16::MAX, u16::MAX);
        g.feed(b"\x1b[16t\x1b[14t");
        assert_eq!(
            g.take_pty_replies(),
            b"\x1b[6;8191;2184t\x1b[4;65528;65520t"
        );
    }

    #[test]
    fn tdf_picker_receives_font_size_without_advertising_unsupported_graphics() {
        // Exact query from tdf 0.5.0's pinned ratatui-image 8.0.1 picker.
        // Without CSI16 it returns NoCap before trying its ioctl fallback,
        // then silently uses 10x20 even though tdf renders at measured pixels.
        let input = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c\x1b[16t\x1b[5n";
        for split in 0..=input.len() {
            let mut g = TerminalGrid::new(Size { cols: 52, rows: 61 });
            g.set_cell_size(16, 34);
            g.feed(&input[..split]);
            let mut replies = g.take_pty_replies();
            g.feed(&input[split..]);
            replies.extend(g.take_pty_replies());
            assert_eq!(replies, b"\x1b[?6c\x1b[6;34;16t\x1b[0n", "split {split}");
        }
    }

    #[test]
    fn csi_16t_rejects_payloads_cancelled_and_nonplain_sequences() {
        let input = b"\x1b]2;[16t\x9b16t\x07\x1b_Gdata;[16t\x9b16t\x1b\\\x1bPdata;[16t\x1b\\\x1b[?16t\x1b[16 t\x1b[16:0t\x1b[16;0t\x1b[16\x18t\x1b[16\x1at";
        for split in 0..=input.len() {
            let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
            g.feed(&input[..split]);
            g.feed(&input[split..]);
            assert!(g.take_pty_replies().is_empty(), "split {split}");
            g.feed(b"\x1b[16t");
            assert_eq!(g.take_pty_replies(), b"\x1b[6;0;0t");
        }
    }

    #[test]
    fn synchronized_output_flush_preserves_reply_order() {
        let mut g = TerminalGrid::new(Size { cols: 20, rows: 5 });
        g.feed(b"\x1b[?2026h\x1b[14t\x1b[16t\x1b[2;3H\x1b[6n\x1b[18t\x1b[16t");
        assert_eq!(
            g.take_pty_replies(),
            b"\x1b[4;0;0t\x1b[6;0;0t\x1b[2;3R\x1b[8;5;20t\x1b[6;0;0t"
        );
    }

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
                overline: false,
                inverse: true,
                underline_color: CColor::Default,
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
}
