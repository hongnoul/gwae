//! Focus navigation verbs: arrows across columns, strips, and rows.

use super::Action;
use crate::model::Layout;
use crate::viewport::{follow_focus_scroll, scroll_stops, snap_scroll, Viewport};
use crate::width::{Preset, Width};
use crate::{FollowScroll, LayoutError, LayoutResult, PaneId, RowId};

impl Layout {
    /// Number of columns in the focused row.
    pub(super) fn focused_col_count(&self) -> usize {
        self.focused_row().map(|r| r.columns.len()).unwrap_or(0)
    }

    /// Absolute x-center of the focused column (for row-crossing navigation).
    pub(super) fn focused_x_center(&self, vw: u16) -> i32 {
        self.focused_range(vw)
            .map(|(s, e)| ((s + e) / 2) as i32)
            .unwrap_or(0)
    }

    pub(super) fn focused_scroll(&self) -> i32 {
        self.focused_row().map(|r| r.scroll_x).unwrap_or(0)
    }

    pub(super) fn refocus_scroll(&mut self, viewport: Viewport, follow: FollowScroll) {
        let scroll = follow_focus_scroll(
            self,
            self.focus.row,
            self.focus.column,
            viewport.cols,
            follow,
        );
        if let Some(row) = self.row_mut(self.focus.row) {
            row.scroll_x = scroll;
        }
    }

    /// Clamp `focus.pane` into the currently focused column, so moving focus
    /// into a shallower column never leaves a stale (out-of-range) pane index.
    pub(super) fn clamp_focus_pane(&mut self) {
        let max = self
            .focused_row()
            .and_then(|r| r.columns.get(self.focus.column))
            .map(|c| c.panes.len().max(1))
            .unwrap_or(1);
        self.focus.pane = self.focus.pane.min(max - 1);
    }

    /// Point `focus.pane` at the focused column's remembered pane.
    ///
    /// Vertical focus is *per column*, not per strip: stepping sideways off a
    /// stack and back returns to the pane you were on rather than the top of
    /// the column.
    pub(super) fn restore_column_focus(&mut self) {
        self.focus.pane = self.column_focus(self.focus.row, self.focus.column);
        self.clamp_focus_pane();
    }

    pub(super) fn focus_left(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.column == 0 {
            return Ok(self.focused_scroll());
        }
        self.focus.column -= 1;
        self.restore_column_focus();
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    pub(super) fn focus_right(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        let count = self.focused_col_count();
        if self.focus.column + 1 >= count {
            return Ok(self.focused_scroll());
        }
        self.focus.column += 1;
        self.restore_column_focus();
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    pub(super) fn focus_up(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.pane > 0 {
            self.focus.pane -= 1;
            self.remember_focus();
            return Ok(self.focused_scroll());
        }
        self.cross_row(-1, viewport, follow)
    }

    pub(super) fn focus_down(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        let max = self
            .focused_row()
            .and_then(|r| r.columns.get(self.focus.column))
            .map(|c| c.panes.len())
            .unwrap_or(1);
        if self.focus.pane + 1 < max {
            self.focus.pane += 1;
            self.remember_focus();
            return Ok(self.focused_scroll());
        }
        self.cross_row(1, viewport, follow)
    }

    pub(super) fn cross_row(
        &mut self,
        delta: i32,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let idx = self
            .rows
            .iter()
            .position(|r| r.id == self.focus.row)
            .ok_or(LayoutError::UnknownRow(self.focus.row))?;
        let ti = if delta < 0 {
            idx.checked_sub(1)
        } else {
            Some(idx + 1)
        };
        let Some(ti) = ti else {
            return Ok(self.focused_scroll());
        };
        if ti >= self.rows.len() {
            // niri-style dynamic strips: moving past the last strip creates a
            // fresh empty one, but only when the current strip has something
            // in it. That keeps a chain of empty strips from piling up.
            if self.row_is_empty(self.focus.row) {
                return Ok(self.focused_scroll());
            }
            self.new_row();
        }
        let target_id = self.rows[ti].id;
        let from = self.focus.row;
        // Remember where we were on the source strip.
        self.remember_focus();
        // Restore remembered focus for the target strip if we have one;
        // otherwise fall back to the nearest column to the x-center heuristic.
        if let Some((col, pane)) = self.remembered_focus(target_id) {
            self.focus.row = target_id;
            self.focus.column = col;
            self.focus.pane = pane;
            self.clamp_focus_pane();
        } else {
            let x_center = self.focused_x_center(viewport.cols);
            let col = self.nearest_column(target_id, x_center, viewport.cols);
            self.focus.row = target_id;
            self.focus.column = col;
            self.restore_column_focus();
        }
        // Leaving an empty strip behind drops it, so only the strip you are
        // standing on can ever be empty.
        if from != target_id && self.row_is_empty(from) {
            self.rows.retain(|r| r.id != from);
            self.gc_row_focus();
        }
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    /// A strip with no columns: nothing lives on it yet.
    pub fn row_is_empty(&self, row: RowId) -> bool {
        self.row(row).map(|r| r.columns.is_empty()).unwrap_or(true)
    }

    pub(super) fn nearest_column(&self, row: RowId, x: i32, vw: u16) -> usize {
        let ranges = self.column_x_ranges(row, vw).unwrap_or_default();
        if ranges.is_empty() {
            return 0;
        }
        ranges
            .iter()
            .enumerate()
            .min_by_key(|(_, (s, e))| {
                let sz = *s as i32;
                let ez = *e as i32;
                if x < sz {
                    sz - x
                } else if x >= ez {
                    x - (ez - 1)
                } else {
                    0
                }
            })
            .map(|(i, _)| i)
            .unwrap_or(0)
    }
}
