//! strimux-term: the emulator facade.
//!
//! Isolates the terminal-emulation crate choice (ADR-004: `alacritty_terminal`
//! vs `wezterm-term`) behind a single `TermGrid` trait so swapping the backend
//! touches exactly one crate. The concrete implementation is decided and wired
//! in M0; until then this ships the trait boundary plus a no-op `NullGrid`
//! used by `strimux-testkit`.

/// A single cell on the screen: a character and an SGR style handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Index into the palette/style table, resolved by the renderer.
    pub style: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', style: 0 }
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
///
/// Implementations wrap a real emulator crate (`alacritty_terminal`,
/// `wezterm-term`) or a fake (`strimux-testkit::FakeTerminal`).
pub trait TermGrid {
    /// The grid's logical size (what the PTY is told via TIOCSWINSZ).
    fn size(&self) -> Size;
    /// Resize the grid to a new logical size.
    fn resize(&mut self, size: Size);
    /// Feed raw bytes from the PTY into the emulator.
    fn feed(&mut self, bytes: &[u8]) -> Vec<Damage>;
    /// Read the cell at a grid coordinate.
    fn cell(&self, x: u16, y: u16) -> Cell;
}

/// A trivially empty grid that renders spaces; used before the M0 emulator
/// spike and by `strimux-testkit` as the "no emulator" baseline.
#[derive(Debug, Default, Clone)]
pub struct NullGrid {
    size: Size,
}

impl TermGrid for NullGrid {
    fn size(&self) -> Size {
        self.size
    }
    fn resize(&mut self, size: Size) {
        self.size = size;
    }
    fn feed(&mut self, _bytes: &[u8]) -> Vec<Damage> {
        Vec::new()
    }
    fn cell(&self, _x: u16, _y: u16) -> Cell {
        Cell::default()
    }
}
