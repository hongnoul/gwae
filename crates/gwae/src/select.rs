//! Selection geometry inside a PTY pane.
//!
//! gwae captures the mouse (for click-to-focus, and to forward events to a
//! child that asked for mouse reporting), which takes native selection away
//! from the host terminal. Clipboard I/O (copy/paste, OSC 52, image) has been
//! removed — the host terminal's native clipboard (Cmd+V / Ctrl+V, native
//! selection) is used instead.

use gwae_term::{Cell, TermGrid};

/// One cell address inside a pane's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    /// Grid row (0 = top of the visible pane).
    pub y: u16,
    /// Grid column.
    pub x: u16,
}

impl Point {
    pub fn new(x: u16, y: u16) -> Self {
        Point { x, y }
    }
}

/// An in-progress or completed selection inside one pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<P: Copy + Eq> {
    pub pane: P,
    pub anchor: Point,
    pub cursor: Point,
    /// True while the button is still held.
    pub dragging: bool,
}

impl<P: Copy + Eq> Selection<P> {
    /// The selection ends in document order (start <= end).
    pub fn ends(&self) -> (Point, Point) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// True when the selection covers no cell at all (a plain click).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Whether `(x, y)` in `pane`'s grid is inside the selection. The range is
    /// inclusive of both ends, matching how terminals highlight a drag: the
    /// cell under the press and the cell under the release are both selected.
    pub fn contains(&self, pane: P, x: u16, y: u16) -> bool {
        if pane != self.pane {
            return false;
        }
        let (s, e) = self.ends();
        let p = Point::new(x, y);
        p >= s && p <= e
    }
}

/// Extract the selected text from a grid.
///
/// Lines are joined with `\n`, trailing blanks on each line are trimmed (a
/// terminal grid is space-padded to the full width, and pasting that padding
/// back is never what anyone wants), and wide-glyph continuation cells are
/// skipped so a CJK character is copied once, not twice.
#[allow(dead_code)]
pub fn selected_text<G: TermGrid>(grid: &G, sel: &Selection<impl Copy + Eq>) -> String {
    let size = grid.size();
    let (start, end) = sel.ends();
    let mut out = String::new();
    let mut first = true;
    for y in start.y..=end.y.min(size.rows.saturating_sub(1)) {
        let x0 = if y == start.y { start.x } else { 0 };
        let x1 = if y == end.y {
            end.x
        } else {
            size.cols.saturating_sub(1)
        };
        let mut line = String::new();
        let mut x = x0;
        while x <= x1.min(size.cols.saturating_sub(1)) {
            let cell: Cell = grid.cell(x, y);
            if cell.width != 0 {
                cell.push_codepoints(&mut line);
            }
            x += 1;
        }
        let line = line.trim_end();
        if !first {
            out.push('\n');
        }
        out.push_str(line);
        first = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::{Size, Vt100Grid};

    fn sel(anchor: (u16, u16), cursor: (u16, u16)) -> Selection<u8> {
        Selection {
            pane: 1,
            anchor: Point::new(anchor.0, anchor.1),
            cursor: Point::new(cursor.0, cursor.1),
            dragging: false,
        }
    }

    fn grid(lines: &[&str]) -> Vt100Grid {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        g.feed(lines.join("\r\n").as_bytes());
        g
    }

    #[test]
    fn ends_are_ordered_regardless_of_drag_direction() {
        let forward = sel((2, 0), (5, 1));
        let backward = sel((5, 1), (2, 0));
        assert_eq!(forward.ends(), backward.ends());
        assert_eq!(forward.ends().0, Point::new(2, 0));
    }

    #[test]
    fn contains_spans_whole_intermediate_rows() {
        let s = sel((5, 0), (2, 2));
        assert!(!s.contains(2, 5, 0));
        assert!(!s.contains(1, 4, 0));
        assert!(s.contains(1, 5, 0));
        assert!(s.contains(1, 0, 1));
        assert!(s.contains(1, 19, 1));
        assert!(s.contains(1, 2, 2));
        assert!(!s.contains(1, 3, 2));
    }

    #[test]
    fn single_cell_selection_is_empty() {
        assert!(sel((3, 1), (3, 1)).is_empty());
        assert!(!sel((3, 1), (4, 1)).is_empty());
    }

    #[test]
    fn selected_text_trims_grid_padding_and_spans_rows() {
        let g = grid(&["hello world", "second line"]);
        let s = sel((0, 0), (5, 1));
        assert_eq!(selected_text(&g, &s), "hello world\nsecond");
    }

    #[test]
    fn selected_text_takes_a_partial_single_row() {
        let g = grid(&["hello world"]);
        assert_eq!(selected_text(&g, &sel((6, 0), (10, 0))), "world");
    }

    #[test]
    fn selected_text_skips_wide_glyph_continuation_cells() {
        let g = grid(&["日本語"]);
        assert_eq!(selected_text(&g, &sel((0, 0), (5, 0))), "日本語");
    }
}
