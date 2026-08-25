//! strimux-layout/src/minimap.rs

use crate::{Layout, PaneStatus};

/// A single block in the minimap: one (strip, column, pane) tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapCell {
    /// x in cells from the minimap's left edge.
    pub x: u16,
    /// y in cells: one row per strip (0 = first strip).
    pub y: u16,
    /// width in cells, proportional to the column's real width share.
    pub w: u16,
    /// OSC 133 status of this pane (for the health tint).
    pub status: PaneStatus,
    /// True when this tile's strip is the focused row.
    pub focus_row: bool,
    /// True when this tile is the focused pane (strip + column + pane).
    pub focus_col: bool,
}

/// The computed minimap geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minimap {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<MinimapCell>,
}

/// Distribute `budget` cells among weights `ws` proportionally (largest
/// remainder), so each column keeps ≥ the width share implied by its size.
fn allocate(ws: &[u16], _total: u64, budget: u16) -> Vec<u16> {
    let n = ws.len();
    if n == 0 {
        return Vec::new();
    }
    let budget = budget as u64;
    // Largest-remainder apportionment of the integer cells.
    let mut out = vec![0u64; n];
    let total: u64 = ws.iter().map(|w| *w as u64).sum::<u64>().max(1);
    let mut given = 0u64;
    // Floor allocation.
    let mut remainders: Vec<(u64, usize)> = Vec::with_capacity(n);
    for (i, w) in ws.iter().enumerate() {
        let scaled = budget * (*w as u64) / total;
        out[i] = scaled;
        given += scaled;
        remainders.push((budget * (*w as u64) % total, i));
    }
    // Hand out the leftover cells to the largest remainders (gives equal-width
    // columns a deterministic tie-break by order).
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut extra = budget.saturating_sub(given);
    for (_, i) in remainders {
        if extra == 0 {
            break;
        }
        if out[i] < budget {
            out[i] += 1;
            extra -= 1;
        }
    }
    out.into_iter().map(|w| w as u16).collect()
}

/// Split `w` cells into `p` parts as evenly as possible (left-weighted), so a
/// column stack of panes maps to contiguous same-height tiles.
fn split(w: u16, p: usize) -> Vec<u16> {
    let w = w as u64;
    let p = p.max(1) as u64;
    let base = w / p;
    let rem = (w % p) as usize;
    (0..p as usize)
        .map(|i| (base + (i < rem) as u64) as u16)
        .collect()
}

pub fn build(layout: &Layout, map_w: u16, viewport_cols: u16) -> Minimap {
    let mut cells = Vec::new();
    let height = layout.rows.len().max(1) as u16;
    for (i, row) in layout.rows.iter().enumerate() {
        let y = i as u16;
        let focus_row = layout.focus.row == row.id;
        let width_cells: Vec<u16> = row
            .columns
            .iter()
            .map(|c| c.width.cells(viewport_cols))
            .collect();
        let total: u64 = width_cells.iter().map(|w| *w as u64).sum();
        let alloc = allocate(&width_cells, total, map_w);
        let mut x = 0u16;
        for (ci, w) in alloc.iter().enumerate() {
            if *w == 0 {
                continue;
            }
            let focus_col = focus_row && ci == layout.focus.column;
            let col = &row.columns[ci];
            let panes = col.panes.as_slice();
            let pw = split(*w, panes.len().max(1));
            let mut px = x;
            for (pi, pid) in panes.iter().enumerate() {
                let ww = pw.get(pi).copied().unwrap_or(0);
                if ww == 0 {
                    continue;
                }
                let status = layout
                    .panes
                    .get(pid)
                    .map(|p| p.status)
                    .unwrap_or(PaneStatus::Running);
                cells.push(MinimapCell {
                    x: px,
                    y,
                    w: ww,
                    status,
                    focus_row,
                    focus_col: focus_col
                        && pi == layout.focus.pane.min(panes.len().saturating_sub(1)),
                });
                px += ww;
            }
            x += *w;
        }
    }
    Minimap {
        width: map_w,
        height,
        cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PaneStatus, Width};

    fn single_row(panes_wide: usize) -> Layout {
        Layout::new(panes_wide)
    }

    #[test]
    fn single_row_allocates_widths_and_marks_focus() {
        let l = single_row(4);
        let m = build(&l, 40, 80);
        assert_eq!(m.height, 1);
        assert_eq!(m.width, 40);
        // 4 equal quarter-width columns -> 40/4 = 10 cells each, single pane.
        assert_eq!(m.cells.len(), 4, "one tile per pane");
        let total: u16 = m.cells.iter().map(|c| c.w).sum();
        assert_eq!(total, 40);
        // First pane is focused (column 0, pane 0).
        assert!(m.cells[0].focus_col);
        assert!(m.cells[0].focus_row);
        // Later columns share the row's focus but not the column focus.
        assert!(!m.cells[1].focus_col);
        // All tiles carry a status.
        assert!(m.cells.iter().all(|c| c.status == PaneStatus::Running));
    }

    #[test]
    fn split_below_subdivides_a_column() {
        let mut l = Layout::new(2);
        // Split the focused column below: column 0 now has 2 panes.
        l.apply(
            crate::verbs::Action::SplitBelow,
            crate::Viewport::new(80),
            crate::FollowScroll::default(),
        )
        .unwrap();
        let m = build(&l, 20, 80);
        // Two columns: col0 has 2 panes, col1 has 1 -> 3 tiles.
        assert_eq!(m.cells.len(), 3);
        // The focused pane is the new (second) pane in column 0.
        assert!(m.cells[1].focus_col, "focused pane is pane index 1");
        assert_eq!(l.focus.pane, 1);
    }

    #[test]
    fn multiple_rows_one_highlighted() {
        let mut l = Layout::default();
        // Strip 2: a single column.
        let r2 = l.new_row("second".to_string());
        let p = l.alloc_pane();
        l.add_column(r2, Width::Cells(40), vec![p]);
        // Strip 3: two columns with differing widths (40 vs 80 of 120 total).
        let r3 = l.new_row("third".to_string());
        let p2 = l.alloc_pane();
        l.add_column(r3, Width::Cells(40), vec![p2]);
        let p3 = l.alloc_pane();
        l.add_column(r3, Width::Cells(80), vec![p3]);
        // Focus stays on row 0.
        let m = build(&l, 24, 120);
        // 3 strips -> height 3.
        assert_eq!(m.height, 3);
        assert!(m.cells[0].focus_row, "first strip is focused");
        assert!(!m.cells.iter().any(|c| c.y > 0 && c.focus_row));
        // A single-column strip fills the whole row.
        let r2_cells: Vec<&MinimapCell> = m.cells.iter().filter(|c| c.y == 1).collect();
        let w2: u16 = r2_cells.iter().map(|c| c.w).sum();
        assert_eq!(w2, 24);
        // Strip 3: 40 + 80 = 120 -> columns get 8 and 16 of the 24 map width.
        let r3_cells: Vec<&MinimapCell> = m.cells.iter().filter(|c| c.y == 2).collect();
        assert_eq!(r3_cells.len(), 2);
        assert_eq!(r3_cells[0].w, 8);
        assert_eq!(r3_cells[1].w, 16);
        // Tiles tile contiguously left-to-right with no gaps/overlap.
        assert_eq!(r3_cells[0].x, 0);
        assert_eq!(r3_cells[1].x, 8);
    }

    #[test]
    fn status_is_reportable() {
        let mut l = Layout::new(1);
        let first = *l.panes.keys().next().unwrap();
        l.panes.get_mut(&first).unwrap().status = PaneStatus::Done;
        let m = build(&l, 10, 80);
        assert_eq!(m.cells[0].status, PaneStatus::Done);
    }
}
