//! Layout verbs and their semantics.
//!
//! Verbs are the keyboard-triggerable operations from the Layout Model spec
//! (``Alt+hjkl`` focus, ``Alt+Shift+hjkl`` move, ``cycle-width``, ``split``,
//! ``kill-pane``, ``spawn-agent``, ...). Each verb is a pure mutation of the layout tree that
//! must preserve the invariants (no implicit resize, no gaps, no row reorder).
//! Any I/O (PTY spawn/kill) is the caller's job; here we only change structure.

use crate::model::Layout;
use crate::viewport::{follow_focus_scroll, Viewport};
use crate::width::Width;
use crate::{FollowScroll, LayoutError, LayoutResult, PaneId, RowId};

/// A single user-initiated layout action. Represents one keypress verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    MovePaneLeft,
    MovePaneRight,
    MovePaneUp,
    MovePaneDown,
    CycleWidth,
    SplitBelow,
    KillPane,
    /// Close a specific pane by id (e.g. its process exited), collapsing the
    /// layout exactly like `KillPane` does for the focused pane.
    ClosePane(PaneId),
    NewColumn,
    NewRow,
    SpawnAgent,
    ScrollViewport(i32),
    JumpToColumn(usize),
}

impl Layout {
    /// Number of columns in the focused row.
    fn focused_col_count(&self) -> usize {
        self.focused_row().map(|r| r.columns.len()).unwrap_or(0)
    }

    /// Absolute x-center of the focused column (for row-crossing navigation).
    fn focused_x_center(&self, vw: u16) -> i32 {
        self.focused_range(vw)
            .map(|(s, e)| ((s + e) / 2) as i32)
            .unwrap_or(0)
    }

    /// Apply a verb, keeping the layout consistent, and return the new
    /// follow-scroll position for the focused row.
    pub fn apply(
        &mut self,
        action: Action,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        match action {
            Action::FocusLeft => self.focus_left(viewport, follow),
            Action::FocusRight => self.focus_right(viewport, follow),
            Action::FocusUp => self.focus_up(viewport, follow),
            Action::FocusDown => self.focus_down(viewport, follow),
            Action::MovePaneLeft => self.move_pane(-1, viewport, follow),
            Action::MovePaneRight => self.move_pane(1, viewport, follow),
            Action::MovePaneUp => self.move_pane_vertical(-1, viewport, follow),
            Action::MovePaneDown => self.move_pane_vertical(1, viewport, follow),
            Action::CycleWidth => Ok(self.apply_cycle_width()),
            Action::SplitBelow => self.apply_split_below(),
            Action::KillPane => self.apply_kill_pane(viewport, follow),
            Action::ClosePane(pid) => self.apply_close_pane(pid, viewport, follow),
            Action::NewColumn => Ok(self.apply_new_column(viewport, follow)),
            Action::NewRow => Ok(self.apply_new_row(viewport, follow)),
            Action::SpawnAgent => Ok(self.apply_new_column(viewport, follow)),
            Action::ScrollViewport(d) => Ok(self.apply_scroll(d, viewport)),
            Action::JumpToColumn(n) => self.apply_jump(n, viewport, follow),
        }
    }
}
impl Layout {
    fn focused_scroll(&self) -> i32 {
        self.focused_row().map(|r| r.scroll_x).unwrap_or(0)
    }

    fn refocus_scroll(&mut self, viewport: Viewport, follow: FollowScroll) {
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
    fn clamp_focus_pane(&mut self) {
        let max = self
            .focused_row()
            .and_then(|r| r.columns.get(self.focus.column))
            .map(|c| c.panes.len().max(1))
            .unwrap_or(1);
        self.focus.pane = self.focus.pane.min(max - 1);
    }

    fn focus_left(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.column == 0 {
            return Ok(self.focused_scroll());
        }
        self.focus.column -= 1;
        self.clamp_focus_pane();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn focus_right(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        let count = self.focused_col_count();
        if self.focus.column + 1 >= count {
            return Ok(self.focused_scroll());
        }
        self.focus.column += 1;
        self.clamp_focus_pane();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn focus_up(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.pane > 0 {
            self.focus.pane -= 1;
            return Ok(self.focused_scroll());
        }
        self.cross_row(-1, viewport, follow)
    }

    fn focus_down(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        let max = self
            .focused_row()
            .and_then(|r| r.columns.get(self.focus.column))
            .map(|c| c.panes.len())
            .unwrap_or(1);
        if self.focus.pane + 1 < max {
            self.focus.pane += 1;
            return Ok(self.focused_scroll());
        }
        self.cross_row(1, viewport, follow)
    }

    fn cross_row(
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
            return Ok(self.focused_scroll());
        }
        let target_id = self.rows[ti].id;
        let x_center = self.focused_x_center(viewport.cols);
        let col = self.nearest_column(target_id, x_center, viewport.cols);
        self.focus.row = target_id;
        self.focus.column = col;
        self.focus.pane = 0;
        self.clamp_focus_pane();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn nearest_column(&self, row: RowId, x: i32, vw: u16) -> usize {
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
impl Layout {
    fn move_pane(
        &mut self,
        dx: i32,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let n = self.focused_col_count();
        let c = self.focus.column;
        let target = if dx < 0 {
            c.checked_sub(1)
        } else {
            Some(c + 1)
        };
        let Some(t) = target else {
            return Ok(self.focused_scroll());
        };
        if t >= n {
            return Ok(self.focused_scroll());
        }
        if let Some(row) = self.row_mut(self.focus.row) {
            row.columns.swap(c, t);
        }
        self.focus.column = t;
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn move_pane_vertical(
        &mut self,
        dy: i32,
        _viewport: Viewport,
        _follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let row = self.focus.row;
        let col = self.focus.column;
        let panes_len = self
            .row(row)
            .and_then(|r| r.columns.get(col))
            .map(|c| c.panes.len())
            .unwrap_or(0);
        let p = self.focus.pane;
        let target = if dy < 0 {
            p.checked_sub(1)
        } else {
            Some(p + 1)
        };
        let Some(t) = target else {
            return Ok(self.focused_scroll());
        };
        if t >= panes_len {
            return Ok(self.focused_scroll());
        }
        if let Some(row) = self.row_mut(row) {
            if let Some(c) = row.columns.get_mut(col) {
                c.panes.swap(p, t);
            }
        }
        self.focus.pane = t;
        Ok(self.focused_scroll())
    }

    fn apply_cycle_width(&mut self) -> i32 {
        let row = self.focus.row;
        let col = self.focus.column;
        if let Some(row) = self.row_mut(row) {
            if let Some(c) = row.columns.get_mut(col) {
                let w = c.width;
                c.width = match w {
                    Width::Preset(p) => Width::Preset(p.next()),
                    other => other,
                };
            }
        }
        self.focused_scroll()
    }

    fn apply_split_below(&mut self) -> LayoutResult<i32> {
        let pane = self.alloc_pane();
        let row = self.focus.row;
        let col = self.focus.column;
        let idx = self.focus.pane;
        if let Some(row) = self.row_mut(row) {
            if let Some(c) = row.columns.get_mut(col) {
                let insert = (idx + 1).min(c.panes.len());
                c.panes.insert(insert, pane);
                self.focus.pane = insert;
            }
        }
        Ok(self.focused_scroll())
    }
}
impl Layout {
    fn apply_kill_pane(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        let row = self.focus.row;
        let col = self.focus.column;
        let pane_idx = self.focus.pane;
        self.remove_pane_at(row, col, pane_idx, viewport, follow)
    }

    /// Close a pane wherever it lives (used when its process exits). A pane
    /// that is already gone from the layout is a no-op, so a late `Exited`
    /// message after an explicit kill can never remove the wrong pane.
    fn apply_close_pane(
        &mut self,
        pid: PaneId,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let Some((row, col, pane_idx)) = self.locate_pane(pid) else {
            return Ok(self.focused_scroll());
        };
        self.remove_pane_at(row, col, pane_idx, viewport, follow)
    }

    /// Remove the pane at `(row, col, pane_idx)` and collapse the layout:
    /// columns compact leftward (no gaps, invariant 5) and focus always
    /// **fills left first**, landing on the left neighbor when one exists.
    fn remove_pane_at(
        &mut self,
        row: RowId,
        col: usize,
        pane_idx: usize,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let removed = {
            let r = self.row_mut(row).ok_or(LayoutError::UnknownRow(row))?;
            let c = r
                .columns
                .get_mut(col)
                .ok_or(LayoutError::UnknownColumn(col))?;
            if pane_idx < c.panes.len() {
                Some(c.panes.remove(pane_idx))
            } else {
                None
            }
        };
        let Some(removed) = removed else {
            return Ok(self.focused_scroll());
        };
        self.panes.remove(&removed);
        // Was the closed pane the focused one? Remember before indices shift.
        let was_focused = self.focus.row == row && self.focus.column == col
            && self.focus.pane == pane_idx;
        // Clean up empty columns and rows; never leave gaps (invariant 5).
        let col_emptied = self
            .row(row)
            .and_then(|r| r.columns.get(col))
            .map(|c| c.panes.is_empty())
            .unwrap_or(false);
        if col_emptied {
            if let Some(r) = self.row_mut(row) {
                r.columns.remove(col);
            }
        }
        // A row with zero columns disappears.
        let row_emptied = self.row(row).map(|r| r.columns.is_empty()).unwrap_or(false);
        if row_emptied {
            self.rows.retain(|r| r.id != row);
            if self.rows.is_empty() {
                *self = Layout::default();
                return Ok(0);
            }
            if self.focus.row == row {
                self.focus.row = self.rows[0].id;
                self.focus.column = 0;
                self.focus.pane = 0;
                self.refocus_scroll(viewport, follow);
            }
            return Ok(self.focused_scroll());
        }
        if self.focus.row == row {
            if was_focused {
                // Fill left first: prefer the left neighbor column (or the
                // pane above within a stacked column) over the one that slid
                // into the closed slot from the right.
                if col_emptied {
                    self.focus.column = col.saturating_sub(1);
                    self.focus.pane = 0;
                } else {
                    self.focus.pane = pane_idx.saturating_sub(1);
                }
            } else if col_emptied && self.focus.column > col {
                // Compaction shifted the focused column one slot left.
                self.focus.column -= 1;
            } else if !col_emptied && self.focus.column == col && self.focus.pane > pane_idx {
                // Same for a pane above the focus within the same column.
                self.focus.pane -= 1;
            }
            let cols = self.row(row).map(|r| r.columns.len()).unwrap_or(1);
            self.focus.column = self.focus.column.min(cols.saturating_sub(1));
            self.clamp_focus_pane();
            self.refocus_scroll(viewport, follow);
        }
        Ok(self.focused_scroll())
    }

    fn apply_new_column(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
        let pane = self.alloc_pane();
        let col = self.add_column(self.focus.row, Width::DEFAULT, vec![pane]);
        self.focus.column = col;
        self.focus.pane = 0;
        // The new column is appended at the rightmost edge, which can be
        // off-screen (e.g. whatever fixed width it has). Follow-scroll so the
        // freshly spawned pane is immediately in view.
        self.refocus_scroll(viewport, follow);
        self.focused_scroll()
    }

    fn apply_new_row(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
        let row = self.new_row("row".to_string());
        let pane = self.alloc_pane();
        self.add_column(row, Width::DEFAULT, vec![pane]);
        self.focus.row = row;
        self.focus.column = 0;
        self.focus.pane = 0;
        self.refocus_scroll(viewport, follow);
        self.focused_scroll()
    }

    fn apply_scroll(&mut self, delta: i32, viewport: Viewport) -> i32 {
        // Clamp to the strip extent so scrolling never reveals background past
        // the last column's right edge.
        let max_scroll = self
            .column_x_ranges(self.focus.row, viewport.cols)
            .and_then(|r| r.last().map(|(_, e)| *e as i32 - viewport.cols as i32))
            .unwrap_or(0)
            .max(0);
        if let Some(r) = self.row_mut(self.focus.row) {
            r.scroll_x = (r.scroll_x + delta).clamp(0, max_scroll);
        }
        self.focused_scroll()
    }

    fn apply_jump(
        &mut self,
        n: usize,
        _viewport: Viewport,
        _follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let count = self.focused_col_count();
        if n >= count {
            return Ok(self.focused_scroll());
        }
        self.focus.column = n;
        self.clamp_focus_pane();
        Ok(self.focused_scroll())
    }
}
