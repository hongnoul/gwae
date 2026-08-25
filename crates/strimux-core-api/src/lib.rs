//! Stable boundary between the HMR host and the hot-swappable core.
//!
//! This crate is NEVER recompiled during a hot reload; only the core crate
//! is. Keep it small and free of heavy deps so a core rebuild (and therefore
//! a reload cycle) stays fast.
//!
//! All session state lives in the HOST and is borrowed in per call, so
//! swapping the core is lossless: focus and layout never reset.

/// A decoded key, free of terminal-specific encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Logical key codes, decoded by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    Other,
}

/// Commands the core asks the host to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Forward bytes to the focused pane.
    Input(Vec<u8>),
    /// Quit the whole session.
    Quit,
    /// Force a full repaint.
    Repaint,
    /// Scroll the viewport by a cell delta.
    Scroll(i32),
    /// Ask the host to hot-reload the core (dev keybinding).
    Reload,
    None,
}

/// A single composed cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub bold: bool,
    pub inverse: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: 15,
            bg: 0,
            bold: false,
            inverse: false,
        }
    }
}

/// The full frame to paint to the terminal.
#[derive(Debug, Clone)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}

impl Frame {
    pub fn new(cols: u16, rows: u16) -> Self {
        Frame {
            cols,
            rows,
            cells: vec![Cell::default(); (cols as usize) * (rows as usize)],
        }
    }
    pub fn put(&mut self, x: u16, y: u16, c: Cell) {
        if x < self.cols && y < self.rows {
            let i = (y as usize) * (self.cols as usize) + x as usize;
            self.cells[i] = c;
        }
    }
    pub fn text(&mut self, x: u16, y: u16, s: &str, c: Cell) {
        for (dx, ch) in s.chars().enumerate() {
            let mut cc = c;
            cc.ch = ch;
            self.put(x + dx as u16, y, cc);
        }
    }
}

/// Mutable session state owned by the HOST. Borrowed into core calls so a
/// reload is lossless.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub focus: usize,
    pub cols: u16,
    pub rows: u16,
    pub frames: u64,
    pub panes: Vec<String>, // demo: pane titles
}

/// The hot-swappable core. Stateless with respect to the session: it reads
/// and writes `SessionState` by reference. Implementations live in the
/// `strimux-core` cdylib and are swapped at runtime by the host.
pub trait StrimuxCore {
    /// Human/iteration identifier; shown in the status line so a reload is
    /// visibly confirmed. Bump it when you change core behavior.
    fn label(&self) -> &'static str;
    /// Advance the model for a decoded key; return what the host should do.
    fn handle_key(&mut self, state: &mut SessionState, key: Key) -> Cmd;
    /// Compose the frame into `frame` (already sized cols x rows).
    fn render(&mut self, state: &mut SessionState, frame: &mut Frame);
}

// Symbol names exported by the core cdylib.
pub const FACTORY: &[u8] = b"strimux_core_create\0";
