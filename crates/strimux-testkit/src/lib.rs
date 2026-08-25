//! strimux-testkit: scriptable harness for the binary and the emulator facade.
//!
//! Provides:
//!   - `FakeTerminal`: a `TermGrid` that records what was written, so tests can
//!     assert rendered frames without a real PTY.
//!   - `ScriptedPty`: an in-memory PTY stand-in (used once the binary exists).
//!   - a tiny snapshot runner for frame assertions.

pub use strimux_term::{Cell, Damage, NullGrid, Size, Style, TermGrid};

/// A `TermGrid` that records every written row so tests can snapshot frames.
#[derive(Debug, Clone, Default)]
pub struct FakeTerminal {
    size: Size,
    /// Rows of rendered text, used as the frame under test.
    pub rows: Vec<Vec<char>>,
    /// Accumulated raw bytes fed in.
    pub fed: Vec<u8>,
    /// Window title via OSC 0/2, so tests can exercise title forwarding.
    pub title: String,
}

impl FakeTerminal {
    pub fn new(size: Size) -> Self {
        let rows = vec![vec![' '; size.cols as usize]; size.rows as usize];
        FakeTerminal {
            size,
            rows,
            fed: Vec::new(),
            title: String::new(),
        }
    }

    /// Collapse the buffer into lines of text (for snapshot comparison).
    pub fn frame(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.iter().collect()).collect()
    }
}

impl TermGrid for FakeTerminal {
    fn size(&self) -> Size {
        self.size
    }
    fn resize(&mut self, size: Size) {
        self.size = size;
        self.rows = vec![vec![' '; size.cols as usize]; size.rows as usize];
    }
    fn feed(&mut self, bytes: &[u8]) -> Vec<Damage> {
        // The naive rect that a real emulator would report for the whole
        // screen; scripted tests override the rows directly.
        self.fed.extend_from_slice(bytes);
        vec![Damage {
            x: 0,
            y: 0,
            w: self.size.cols,
            h: self.size.rows,
        }]
    }
    fn cell(&self, x: u16, y: u16) -> Cell {
        let ch = self
            .rows
            .get(y as usize)
            .and_then(|r| r.get(x as usize))
            .copied()
            .unwrap_or(' ');
        Cell {
            ch,
            style: Style::default(),
        }
    }

    fn title(&self) -> &str {
        &self.title
    }
}
