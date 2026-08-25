//! Layout verbs and their semantics.
//!
//! Verbs are the keyboard-triggerable operations from the Layout Model spec
//! (``Alt+hjkl`` focus, ``Alt+Shift+hjkl`` move, ``cycle-width``, ``split``,
//! ``kill-pane``, ``spawn-agent``, ...). Each verb is a pure mutation of the layout tree that
//! must preserve the invariants (no implicit resize, no gaps, no row reorder).
//! Any I/O (PTY spawn/kill) is the caller's job; here we only change structure.

use crate::model::Layout;
use crate::viewport::{follow_focus_scroll, scroll_stops, snap_scroll, Viewport};
use crate::width::{Preset, Width};
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
    /// Toggle the focused column between full viewport width and 1/4.
    ToggleFullWidth,
    SplitBelow,
    KillPane,
    /// Close a specific pane by id (e.g. its process exited), collapsing the
    /// layout exactly like `KillPane` does for the focused pane.
    ClosePane(PaneId),
    NewColumn,
    NewRow,
    SpawnAgent,
    /// Spawn an agent on a brand-new strip below the focused one.
    SpawnAgentRow,
    ScrollViewport(i32),
    JumpToColumn(usize),
    /// Jump focus directly to a pane anywhere in the grid (smart-jump: the
    /// caller picks the pane, e.g. the next one whose status needs attention).
    FocusPane(PaneId),
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
            Action::ToggleFullWidth => Ok(self.apply_toggle_full_width(viewport, follow)),
            Action::SplitBelow => self.apply_split_below(),
            Action::KillPane => self.apply_kill_pane(viewport, follow),
            Action::ClosePane(pid) => self.apply_close_pane(pid, viewport, follow),
            Action::NewColumn => Ok(self.apply_new_column(viewport, follow)),
            Action::NewRow => Ok(self.apply_new_row(viewport, follow)),
            Action::SpawnAgent => Ok(self.apply_new_column(viewport, follow)),
            Action::SpawnAgentRow => Ok(self.apply_new_row(viewport, follow)),
            Action::ScrollViewport(d) => Ok(self.apply_scroll(d, viewport)),
            Action::JumpToColumn(n) => self.apply_jump(n, viewport, follow),
            Action::FocusPane(pid) => self.apply_focus_pane(pid, viewport, follow),
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

    /// Point `focus.pane` at the focused column's remembered pane.
    ///
    /// Vertical focus is *per column*, not per strip: stepping sideways off a
    /// stack and back returns to the pane you were on rather than the top of
    /// the column.
    fn restore_column_focus(&mut self) {
        self.focus.pane = self.column_focus(self.focus.row, self.focus.column);
        self.clamp_focus_pane();
    }

    fn focus_left(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.column == 0 {
            return Ok(self.focused_scroll());
        }
        self.focus.column -= 1;
        self.restore_column_focus();
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn focus_right(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
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

    fn focus_up(&mut self, viewport: Viewport, follow: FollowScroll) -> LayoutResult<i32> {
        if self.focus.pane > 0 {
            self.focus.pane -= 1;
            self.remember_focus();
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
            self.remember_focus();
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
            // niri-style dynamic strips: moving past the last strip creates a
            // fresh empty one, but only when the current strip has something
            // in it. That keeps a chain of empty strips from piling up.
            if self.row_is_empty(self.focus.row) {
                return Ok(self.focused_scroll());
            }
            self.new_row(format!("strip {}", self.rows.len() + 1));
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
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }

    fn move_pane_vertical(
        &mut self,
        dy: i32,
        viewport: Viewport,
        follow: FollowScroll,
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
        let Some(t) = target.filter(|t| *t < panes_len) else {
            // At the top/bottom of the stack the pane leaves the strip
            // entirely and lands on the neighboring one, niri-style
            // "move window to workspace".
            return self.move_pane_across_row(dy, viewport, follow);
        };
        if let Some(row) = self.row_mut(row) {
            if let Some(c) = row.columns.get_mut(col) {
                c.panes.swap(p, t);
            }
        }
        self.focus.pane = t;
        self.remember_focus();
        Ok(self.focused_scroll())
    }

    /// Carry the focused pane to the strip above/below as a column of its own,
    /// creating a new strip past the end (only from a non-empty one) and
    /// discarding the source strip if the move emptied it.
    fn move_pane_across_row(
        &mut self,
        dy: i32,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let from = self.focus.row;
        let col = self.focus.column;
        let pane_idx = self.focus.pane;
        let Some(pid) = self.focused_pane_id() else {
            return Ok(self.focused_scroll());
        };
        let idx = self
            .rows
            .iter()
            .position(|r| r.id == from)
            .ok_or(LayoutError::UnknownRow(from))?;
        let ti = if dy < 0 {
            idx.checked_sub(1)
        } else {
            Some(idx + 1)
        };
        let Some(ti) = ti else {
            return Ok(self.focused_scroll());
        };
        // A lone pane on its strip has nowhere new to go: moving it "past the
        // end" would just recreate the same one-pane strip one slot down.
        let lone = self
            .row(from)
            .map(|r| r.columns.len() == 1 && r.columns[0].panes.len() == 1)
            .unwrap_or(false);
        if ti >= self.rows.len() {
            if lone {
                return Ok(self.focused_scroll());
            }
            self.new_row(format!("strip {}", self.rows.len() + 1));
        }
        let target_id = self.rows[ti].id;
        let width = self
            .row(from)
            .and_then(|r| r.columns.get(col))
            .map(|c| c.width)
            .unwrap_or(Width::DEFAULT);
        // Remember the x-position before detaching; afterwards the column may
        // be gone and the center would read as 0.
        let x_center = self.focused_x_center(viewport.cols);
        // Detach from the source strip, compacting away an emptied column.
        if let Some(r) = self.row_mut(from) {
            if let Some(c) = r.columns.get_mut(col) {
                if pane_idx < c.panes.len() {
                    c.panes.remove(pane_idx);
                }
            }
            if r.columns
                .get(col)
                .map(|c| c.panes.is_empty())
                .unwrap_or(false)
            {
                r.columns.remove(col);
            }
        }
        // Land beside the column nearest the old x-position on the target.
        let at = if self.row_is_empty(target_id) {
            0
        } else {
            self.nearest_column(target_id, x_center, viewport.cols) + 1
        };
        let landed = self.insert_column(target_id, at, width, vec![pid]);
        self.focus.row = target_id;
        self.focus.column = landed;
        self.focus.pane = 0;
        // An emptied source strip disappears, exactly as when focus leaves it.
        if self.row_is_empty(from) {
            self.rows.retain(|r| r.id != from);
        }
        self.gc_row_focus();
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
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

    /// Toggle the focused column between `Full` and `Quarter` width. Any
    /// other width (preset or fixed cells) goes to `Full` first, so the
    /// binding always has an obvious first effect.
    fn apply_toggle_full_width(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
        let (row, col) = (self.focus.row, self.focus.column);
        if let Some(r) = self.row_mut(row) {
            if let Some(c) = r.columns.get_mut(col) {
                c.width = if c.width == Width::Preset(Preset::Full) {
                    Width::Preset(Preset::Quarter)
                } else {
                    Width::Preset(Preset::Full)
                };
            }
        }
        self.refocus_scroll(viewport, follow);
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
        self.remember_focus();
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
    /// columns compact leftward (no gaps, invariant 5) and focus **keeps its
    /// slot**: the column that slides in from the right takes the focus, and
    /// only when nothing remains to the right does focus fall left.
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
        // Keep the surviving column's memory pointed at the *same* pane:
        // removing an entry above it shifts every later index down one.
        if let Some(c) = self.row_mut(row).and_then(|r| r.columns.get_mut(col)) {
            if c.focus > pane_idx {
                c.focus -= 1;
            }
        }
        // Was the closed pane the focused one? Remember before indices shift.
        let was_focused =
            self.focus.row == row && self.focus.column == col && self.focus.pane == pane_idx;
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
        // A row with zero columns disappears, even when the focus is standing
        // on it: killing the last pane of a strip shifts focus to the nearest
        // surviving strip above (falling back to below) instead of parking on
        // an empty husk.
        let row_emptied = self.row_is_empty(row);
        let row_index = self.rows.iter().position(|r| r.id == row);
        // No panes anywhere: the grid is meaningless, so reset to a default
        // layout rather than leaving an empty husk behind.
        if self.panes.is_empty() {
            *self = Layout::default();
            return Ok(0);
        }
        if row_emptied {
            let was_focused_row = self.focus.row == row;
            self.rows.retain(|r| r.id != row);
            self.gc_row_focus();
            if self.rows.is_empty() {
                *self = Layout::default();
                return Ok(0);
            }
            if was_focused_row {
                // Prefer the strip above; if the removed strip was the first
                // one, take the strip that slid up into its place.
                let idx = row_index
                    .map(|i| i.saturating_sub(1))
                    .unwrap_or(0)
                    .min(self.rows.len() - 1);
                self.focus.row = self.rows[idx].id;
                if let Some((c, pane)) = self.remembered_focus(self.focus.row) {
                    self.focus.column = c;
                    self.focus.pane = pane;
                } else {
                    self.focus.column = 0;
                    self.focus.pane = self.column_focus(self.focus.row, 0);
                }
                self.clamp_focus_pane();
                self.remember_focus();
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
                    // Stay in the same screen position: the column that slid
                    // in from the right takes the closed slot. Only when
                    // nothing is left to the right does focus fall leftward.
                    let cols = self.row(row).map(|r| r.columns.len()).unwrap_or(0);
                    self.focus.column = if col < cols {
                        col
                    } else {
                        col.saturating_sub(1)
                    };
                    self.focus.pane = self.column_focus(row, self.focus.column);
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
            self.remember_focus();
            self.refocus_scroll(viewport, follow);
        } else {
            // Focus was on another strip; still GC stale row_focus entries
            self.gc_row_focus();
        }
        Ok(self.focused_scroll())
    }

    fn apply_new_column(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
        let pane = self.alloc_pane();
        // Spawn immediately to the right of the focused column (not at the far
        // end of the strip) so a new agent/terminal appears next to the work it
        // came from, and take focus there.
        let at = self.focus.column + 1;
        let col = self.insert_column(self.focus.row, at, Width::DEFAULT, vec![pane]);
        self.focus.column = col;
        self.focus.pane = 0;
        self.remember_focus();
        // The new column may be off-screen (e.g. whatever fixed width it has).
        // Follow-scroll so the freshly spawned pane is immediately in view.
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
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        self.focused_scroll()
    }

    fn apply_scroll(&mut self, delta: i32, viewport: Viewport) -> i32 {
        // Quantized scrolling: a manual scroll pages to the next/previous
        // stop (column boundary, or the end stop that pins the last column to
        // the right edge) rather than panning by cells. Stops never pass the
        // strip extent, so scrolling can never reveal background on the right.
        if delta == 0 {
            return self.focused_scroll();
        }
        let row = self.focus.row;
        let stops = scroll_stops(self, row, viewport.cols);
        let cur = self.focused_scroll();
        let next = if delta > 0 {
            stops.iter().copied().find(|b| *b > cur)
        } else {
            stops.iter().rev().copied().find(|b| *b < cur)
        };
        // Off-stop (e.g. stale state): snap toward the requested direction.
        let target = next.unwrap_or_else(|| snap_scroll(self, row, viewport.cols, cur));
        if let Some(r) = self.row_mut(row) {
            r.scroll_x = target;
        }
        self.focused_scroll()
    }

    /// The largest valid `scroll_x` for the focused row at this viewport:
    /// `max(0, strip_end - viewport_cols)`.
    pub fn max_scroll(&self, viewport: Viewport) -> i32 {
        self.column_x_ranges(self.focus.row, viewport.cols)
            .and_then(|r| r.last().map(|(_, e)| *e as i32 - viewport.cols as i32))
            .unwrap_or(0)
            .max(0)
    }

    /// Re-snap every row's stored scroll onto a valid stop for `viewport`.
    /// Call after external geometry changes (e.g. a terminal resize): a scroll
    /// that was valid at the old width can overshoot the strip, or land
    /// between column boundaries, at the new one. Snapping (which also clamps,
    /// since stops never exceed `max_scroll`) keeps the paint stable and never
    /// reveals background at the right edge.
    pub fn clamp_scrolls(&mut self, viewport: Viewport) {
        let row_ids: Vec<_> = self.rows.iter().map(|r| r.id).collect();
        for id in row_ids {
            let cur = self.row(id).map(|r| r.scroll_x).unwrap_or(0);
            let snapped = snap_scroll(self, id, viewport.cols, cur);
            if let Some(row) = self.row_mut(id) {
                row.scroll_x = snapped;
            }
        }
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
        self.restore_column_focus();
        self.remember_focus();
        Ok(self.focused_scroll())
    }

    /// Smart-jump: focus a pane anywhere in the grid by id, crossing strips
    /// if needed and following the focus with the scroll. Leaving an empty
    /// strip drops it, exactly like directional row-crossing does.
    fn apply_focus_pane(
        &mut self,
        pid: PaneId,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let Some((row, column, pane)) = self.locate_pane(pid) else {
            return Ok(self.focused_scroll());
        };
        let from = self.focus.row;
        self.remember_focus();
        self.focus = crate::model::Focus { row, column, pane };
        if from != row && self.row_is_empty(from) {
            self.rows.retain(|r| r.id != from);
            self.gc_row_focus();
        }
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        Ok(self.focused_scroll())
    }
}
