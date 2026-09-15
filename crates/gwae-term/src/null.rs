//! Blank fixed-size grid used as a test double and during startup.

use crate::{
    cell::Cell,
    grid::{Damage, Size, TermGrid},
};

/// A blank, fixed-size grid used as a test double and during startup.
pub struct NullGrid {
    size: Size,
    cursor: (u16, u16),
    hide: bool,
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
    use crate::{Cell, Size};

    #[test]
    fn null_grid_is_blank() {
        let g = NullGrid::new(Size { cols: 10, rows: 10 });
        assert_eq!(g.cell(3, 4), Cell::default());
        assert_eq!(g.size(), Size { cols: 10, rows: 10 });
    }

}
