//! Facade grid boundary: size, damage, and the `TermGrid` trait.

use crate::cell::Cell;

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
