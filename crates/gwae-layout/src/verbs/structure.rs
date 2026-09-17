//! Structure verbs: split, kill, spawn, widths, scroll, and jumps.

use crate::model::Layout;
use crate::viewport::{scroll_stops, snap_scroll, Viewport};
use crate::width::{Preset, Width};
use crate::{FollowScroll, LayoutError, LayoutResult, PaneId, RowId};

impl Layout {
    /// discarding the source strip if the move emptied it.
    pub(super) fn move_pane_across_row(
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
            self.new_row();
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

    pub(super) fn apply_cycle_width(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
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
        // Widening the rightmost visible column pushes it past the viewport
        // edge; re-run follow-scroll so the new width is on screen on this
        // very frame instead of only after the next focus change.
        self.refocus_scroll(viewport, follow);
        self.focused_scroll()
    }

    /// Toggle the focused column between `Full` and `Quarter` width. Any
    /// other width (preset or fixed cells) goes to `Full` first, so the
    /// binding always has an obvious first effect.
    pub(super) fn apply_toggle_full_width(
        &mut self,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> i32 {
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

    pub(super) fn apply_split_below(&mut self) -> LayoutResult<i32> {
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
    pub(super) fn apply_kill_pane(
        &mut self,
        viewport: Viewport,
        follow: FollowScroll,
    ) -> LayoutResult<i32> {
        let row = self.focus.row;
        let col = self.focus.column;
        let pane_idx = self.focus.pane;
        self.remove_pane_at(row, col, pane_idx, viewport, follow)
    }

    /// Close a pane wherever it lives (used when its process exits). A pane
    /// that is already gone from the layout is a no-op, so a late `Exited`
    /// message after an explicit kill can never remove the wrong pane.
    pub(super) fn apply_close_pane(
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
    pub(super) fn remove_pane_at(
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

    pub(super) fn apply_new_column(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
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

    pub(super) fn apply_new_row(&mut self, viewport: Viewport, follow: FollowScroll) -> i32 {
        let row = self.new_row();
        let pane = self.alloc_pane();
        self.add_column(row, Width::DEFAULT, vec![pane]);
        self.focus.row = row;
        self.focus.column = 0;
        self.focus.pane = 0;
        self.remember_focus();
        self.refocus_scroll(viewport, follow);
        self.focused_scroll()
    }

    pub(super) fn apply_scroll(&mut self, delta: i32, viewport: Viewport) -> i32 {
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

    /// Smart-jump: focus a pane anywhere in the grid by id, crossing strips
    /// if needed and following the focus with the scroll. Leaving an empty
    /// strip drops it, exactly like directional row-crossing does.
    pub(super) fn apply_focus_pane(
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
