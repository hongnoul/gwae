//! Layout verbs and their semantics.
//!
//! Verbs are the keyboard-triggerable operations from the Layout Model spec
//! (``Alt+hjkl`` focus, ``Alt+Shift+hjkl`` move, ``cycle-width``, ``split``,
//! ``kill-pane``, ``spawn-agent``, ...). Each verb is a pure mutation of the layout tree that
//! must preserve the invariants (no implicit resize, no gaps, no row reorder).
//! Any I/O (PTY spawn/kill) is the caller's job; here we only change structure.

mod focus;
mod move_pane;
mod structure;

use crate::model::Layout;
use crate::viewport::Viewport;
use crate::{FollowScroll, LayoutResult, PaneId};

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
    ScrollViewport(i32),
    /// Jump focus directly to a pane anywhere in the grid (smart-jump: the
    /// caller picks the pane, e.g. the next one whose status needs attention).
    FocusPane(PaneId),
}

impl Layout {
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
            Action::CycleWidth => Ok(self.apply_cycle_width(viewport, follow)),
            Action::ToggleFullWidth => Ok(self.apply_toggle_full_width(viewport, follow)),
            Action::SplitBelow => self.apply_split_below(),
            Action::KillPane => self.apply_kill_pane(viewport, follow),
            Action::ClosePane(pid) => self.apply_close_pane(pid, viewport, follow),
            Action::NewColumn => Ok(self.apply_new_column(viewport, follow)),
            Action::NewRow => Ok(self.apply_new_row(viewport, follow)),
            Action::SpawnAgent => Ok(self.apply_new_column(viewport, follow)),
            Action::ScrollViewport(d) => Ok(self.apply_scroll(d, viewport)),
            Action::FocusPane(pid) => self.apply_focus_pane(pid, viewport, follow),
        }
    }
}
