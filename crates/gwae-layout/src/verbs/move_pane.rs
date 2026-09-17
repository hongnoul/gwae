//! Move verbs: drag panes across columns and strips.

use crate::model::Layout;
use crate::viewport::Viewport;
use crate::{FollowScroll, LayoutResult};

impl Layout {
    pub(super) fn move_pane(
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

    pub(super) fn move_pane_vertical(
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
}
