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

/// A single cell: a character and its style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            style: Style::default(),
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
}

impl Vt100Grid {
    pub fn new(size: Size) -> Self {
        let parser = vt100::Parser::new(size.rows, size.cols, 10_000);
        Vt100Grid {
            parser,
            rows: size.rows,
            cols: size.cols,
        }
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
                let ch = c.contents().chars().next().unwrap_or(' ');
                let style = Style {
                    fg: map_color(c.fgcolor()),
                    bg: map_color(c.bgcolor()),
                    bold: c.bold(),
                    underline: c.underline(),
                    inverse: c.inverse(),
                };
                Cell { ch, style }
            }
            None => Cell::default(),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
