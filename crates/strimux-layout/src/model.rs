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
        // A single strip of `INITIAL_STRIPS` panes, each 1/4 of the viewport.
        let width = Width::Preset(crate::width::Preset::Quarter);
        for _ in 0..Layout::INITIAL_STRIPS {
            let pane = layout.alloc_pane();
            layout.add_column(row, width, vec![pane]);
        }
        layout.focus.row = row;
        layout.focus.column = 0;
        layout
    }
}

impl Layout {
    /// The default number of equal-width panes on screen at first launch.
    pub const INITIAL_STRIPS: usize = 4;

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

    pub fn column_x_ranges(&self, row: RowId, viewport_cols: u16) -> Option<Vec<(u32, u32)>> {
        let row = self.row(row)?;
        let viewport = viewport_cols as u32;
        // Reserve the exact cells of fixed-width columns first; preset-fraction
        // columns share whatever is left so the strip is contiguous and fills
        // the viewport exactly (no per-column rounding overflow, no gaps).
        let reserved: u32 = row
            .columns
            .iter()
            .filter(|c| matches!(c.width, Width::Cells(_)))
            .map(|c| c.width.cells(viewport_cols) as u32)
            .sum();
        let budget = viewport.saturating_sub(reserved);
        let n_preset = row.columns.iter().filter(|c| !matches!(c.width, Width::Cells(_))).count();
        // Ideal share per preset column, so they all sum to exactly `budget`.
        let base = if n_preset > 0 { budget / n_preset as u32 } else { 0 };
        let mut rem = if n_preset > 0 { budget % n_preset as u32 } else { 0 };
        // Give each preset column `base` cells, then hand out the remainder one
        // cell at a time (round-robin) so no column overflows the viewport.
        let mut widths = Vec::with_capacity(row.columns.len());
        for col in &row.columns {
            let w = match col.width {
                Width::Cells(_) => col.width.cells(viewport_cols) as u32,
                Width::Preset(_) => {
                    let mut w = base;
                    if rem > 0 {
                        w += 1;
                        rem -= 1;
                    }
                    w.max(1)
                }
            };
            widths.push(w);
        }
        let mut x = 0u32;
        let mut out = Vec::with_capacity(row.columns.len());
        for w in widths {
            out.push((x, x + w));
            x += w;
        }
        Some(out)
    }

    pub fn focused_range(&self, viewport_cols: u16) -> Option<(u32, u32)> {
        let ranges = self.column_x_ranges(self.focus.row, viewport_cols)?;
        ranges.get(self.focus.column).copied()
    }
}
