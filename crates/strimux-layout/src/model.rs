//! Core data model for the 2D strip grid.
//!
//! A `Layout` is an ordered list of named `Row`s, each an infinite horizontal
//! strip of `Column`s. A `Column` holds a vertical stack of one or more
//! `Pane`s and keeps its own `Width`. Panes never shrink; the focused row's
//! `scroll_x` pans the viewport across it.

use crate::width::Width;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type PaneId = u64;
pub type RowId = u64;

/// Agent status from the OSC 133 shell-integration protocol. Purely advisory:
/// it colors the minimap and powers smart-jump, never the layout itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneStatus {
    Running,
    Idle,
    Done,
}

/// One PTY-backed pane. The layout only tracks its id and status; the actual
/// terminal surface lives in the `strimux` binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub status: PaneStatus,
}

/// A vertical stack of panes at a given x-position with a fixed width.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub panes: Vec<PaneId>,
    pub width: Width,
}

/// One infinite horizontal strip. Columns are ordered with no gaps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: RowId,
    pub name: String,
    pub columns: Vec<Column>,
    /// Horizontal viewport offset in cells.
    pub scroll_x: i32,
}

/// The focused `(row, column, pane)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
    pub row: RowId,
    pub column: usize,
    pub pane: usize,
}

/// The whole 2D grid of strips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub rows: Vec<Row>,
    pub focus: Focus,
    pub panes: HashMap<PaneId, Pane>,
    next_row: RowId,
    next_pane: PaneId,
}
impl Default for Layout {
    fn default() -> Self {
        Layout::new(Self::INITIAL_STRIPS)
    }
}

impl Layout {
    /// The default number of equal-width panes on screen at first launch.
    pub const INITIAL_STRIPS: usize = 4;

    /// Build a fresh layout with `strips` equal-width quarter panes in a single
    /// row, focused on the first. Panes keep a fixed `1/4` share of the
    /// viewport regardless of `strips`, so fewer `strips` than 4 leave the right
    /// side of the screen empty (uncovered background).
    pub fn new(strips: usize) -> Self {
        let mut layout = Layout {
            rows: Vec::new(),
            focus: Focus {
                row: 0,
                column: 0,
                pane: 0,
            },
            panes: HashMap::new(),
            next_row: 0,
            next_pane: 0,
        };
        let row = layout.new_row("main".to_string());
        // A single strip of `n` panes, each 1/4 of the viewport.
        let width = Width::Preset(crate::width::Preset::Quarter);
        for _ in 0..strips {
            let pane = layout.alloc_pane();
            layout.add_column(row, width, vec![pane]);
        }
        layout.focus.row = row;
        layout.focus.column = 0;
        layout
    }

    pub fn alloc_pane(&mut self) -> PaneId {
        let id = self.next_pane;
        self.next_pane += 1;
        self.panes.insert(
            id,
            Pane {
                id,
                status: PaneStatus::Running,
            },
        );
        id
    }

    pub fn new_row(&mut self, name: String) -> RowId {
        let id = self.next_row;
        self.next_row += 1;
        self.rows.push(Row {
            id,
            name,
            columns: Vec::new(),
            scroll_x: 0,
        });
        id
    }

    pub fn add_column(&mut self, row: RowId, width: Width, panes: Vec<PaneId>) -> usize {
        let row = self.row_mut(row).expect("row must exist");
        row.columns.push(Column { width, panes });
        row.columns.len() - 1
    }

    pub fn row(&self, id: RowId) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn row_mut(&mut self, id: RowId) -> Option<&mut Row> {
        self.rows.iter_mut().find(|r| r.id == id)
    }

    pub fn focused_row(&self) -> Option<&Row> {
        self.row(self.focus.row)
    }

    /// The id of the currently focused pane, if the focus points at one.
    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.focused_row()
            .and_then(|r| r.columns.get(self.focus.column))
            .and_then(|c| c.panes.get(self.focus.pane))
            .copied()
    }

    /// Locate a pane anywhere in the grid: `(row id, column index, pane index)`.
    pub fn locate_pane(&self, pid: PaneId) -> Option<(RowId, usize, usize)> {
        for row in &self.rows {
            for (ci, col) in row.columns.iter().enumerate() {
                if let Some(pi) = col.panes.iter().position(|p| *p == pid) {
                    return Some((row.id, ci, pi));
                }
            }
        }
        None
    }

    pub fn column_x_ranges(&self, row: RowId, viewport_cols: u16) -> Option<Vec<(u32, u32)>> {
        let row = self.row(row)?;
        // Each column renders at its own width: preset fractions are a *fixed
        // share of the viewport* (e.g. 1/4) regardless of how many columns
        // exist. Columns therefore keep their size as the strip grows and any
        // overflow extends past the right edge, where follow-focus scrolling
        // reveals it, rather than every column shrinking to fit.
        //
        // Positions accumulate in twelfths of a cell (exact for the preset
        // denominators 2/3/4) and each column *boundary* is rounded to the
        // nearest cell. Rounding boundaries instead of widths means the
        // rounding error never accumulates: four 1/4 columns tile the
        // viewport exactly instead of each rounding up and pushing the
        // rightmost column past the right edge.
        let mut x12 = 0u64;
        let mut prev = 0u32;
        let mut out = Vec::with_capacity(row.columns.len());
        for col in &row.columns {
            x12 += col.width.twelfths(viewport_cols);
            // Round-half-up to the nearest cell boundary, but never collapse
            // a column to zero width (`Width::cells` guarantees >= 1 too).
            let end = (((x12 + 6) / 12) as u32).max(prev + 1);
            out.push((prev, end));
            prev = end;
        }
        Some(out)
    }

    pub fn focused_range(&self, viewport_cols: u16) -> Option<(u32, u32)> {
        let ranges = self.column_x_ranges(self.focus.row, viewport_cols)?;
        ranges.get(self.focus.column).copied()
    }
}
