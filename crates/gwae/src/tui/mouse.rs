//! Mouse routing + hit-testing + SGR encoding (verbatim move from `tui/mod.rs`).

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::PaneView;
use gwae_layout::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseRole {
    /// Forward verbatim to the child as an SGR mouse report.
    Forward,
    /// Drive gwae's own drag-to-copy selection.
    Select,
    /// Scroll the pane's history directly (`ScrollBack`).
    Wheel,
    /// Handled locally, or ignored.
    Local,
}

/// Decide what a mouse event does inside the pane under the cursor.
pub(crate) fn mouse_role(kind: MouseEventKind, modifiers: KeyModifiers, child_wants_mouse: bool) -> MouseRole {
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let selecting = matches!(
        kind,
        MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::Drag(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Left)
    );
    if selecting {
        // A reporting child owns the drag, unless Shift is held: the xterm
        // convention for "let the multiplexer select instead of the app".
        if child_wants_mouse && !shift {
            return MouseRole::Forward;
        }
        return MouseRole::Select;
    }
    if is_wheel(kind) {
        let horizontal = matches!(
            kind,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
        );
        // A reporting child owns the wheel (jcode scrolls its own
        // transcript, vim its own buffer) - except vertical Shift+wheel,
        // the escape hatch that scrolls gwae's history instead.
        // Horizontal flicks always stay with a reporting child: only the
        // child knows wide content.
        if child_wants_mouse && (!shift || horizontal) {
            return MouseRole::Forward;
        }
        return MouseRole::Wheel;
    }
    // Anything else (right/middle buttons, moves): a reporting child owns
    // it, otherwise gwae handles it locally or ignores it.
    if child_wants_mouse {
        return MouseRole::Forward;
    }
    MouseRole::Local
}

/// True for the four wheel kinds (vertical notches and horizontal flicks).
pub(crate) fn is_wheel(kind: MouseEventKind) -> bool {
    matches!(
        kind,
        MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    )
}

/// One notch of wheel travel in scrollback rows: line-by-line like jcode's
/// transcript, not a page jump. Small enough to keep precise positioning.
pub(crate) const WHEEL_SCROLL_LINES: i32 = 3;

/// The scrollback delta for one wheel notch: up/left goes back into history,
/// down/right comes forward. Horizontal flicks scroll history too when no
/// reporting child owns them; a reporting child keeps all of its own wheel
/// (see `mouse_role`), so this mapping only runs for plain panes.
pub(crate) fn wheel_scroll_delta(kind: MouseEventKind) -> i32 {
    match kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => WHEEL_SCROLL_LINES,
        _ => -WHEEL_SCROLL_LINES,
    }
}

/// The arrow keys a full-screen child (vim, less) expects for one wheel
/// notch: it owns its own scrolling and keeps no scrollback of ours, so the
/// wheel becomes the keys it would get natively. Mirrors the `ScrollBack`
/// arm, which does the same translation for the keyboard route.
pub(crate) fn wheel_alt_screen_keys(kind: MouseEventKind) -> &'static [u8] {
    match kind {
        MouseEventKind::ScrollUp => b"\x1b[A",
        _ => b"\x1b[B]",
    }
}

/// The pane whose on-screen rect contains `(x, y)`, plus the cell coordinates
/// *inside* that pane's grid. Only panes in the focused strip are visible, so
/// only those can be hit.
pub(crate) fn pane_at(views: &[PaneView], x: u16, y: u16) -> Option<(PaneId, u16, u16)> {
    views.iter().find_map(|v| {
        let r = v.rect;
        if x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h {
            let gx = (x - r.x) as i32 + v.col_x0 as i32 + v.h_scroll;
            if gx < 0 || gx >= v.grid_cols as i32 {
                return None;
            }
            Some((v.pid, gx as u16, y - r.y))
        } else {
            None
        }
    })
}

/// Resolve `(x, y)` inside `pid`'s view, clamping a point that has wandered
/// outside the pane's rect to its nearest edge cell.
///
/// This is what makes a drag that leaves the pane behave like a native
/// selection: dragging off the right edge selects to end of line, dragging
/// below the last row selects to the bottom, instead of the selection simply
/// freezing at the last in-bounds position.
pub(crate) fn clamped_pane_point(
    views: &[PaneView],
    pid: PaneId,
    x: u16,
    y: u16,
) -> Option<(PaneId, u16, u16)> {
    let v = views.iter().find(|v| v.pid == pid)?;
    let r = v.rect;
    let sx = x.clamp(r.x, r.x + r.w.saturating_sub(1));
    let sy = y.clamp(r.y, r.y + r.h.saturating_sub(1));
    let gx = ((sx - r.x) as i32 + v.col_x0 as i32 + v.h_scroll)
        .clamp(0, v.grid_cols.saturating_sub(1) as i32) as u16;
    Some((pid, gx, sy - r.y))
}

/// Encode a mouse event as an SGR (1006) report for a child that asked for
/// mouse reporting, with coordinates translated into the pane's own grid
/// (1-based, as the protocol requires).
pub(crate) fn sgr_mouse_report(ev: &MouseEvent, gx: u16, gy: u16) -> Option<Vec<u8>> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match ev.kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) => (button(b), true),
        MouseEventKind::Drag(b) => (button(b) + 32, false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if ev.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if ev.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let final_byte = if release { 'm' } else { 'M' };
    Some(
        format!(
            "\x1b[<{};{};{}{}",
            code,
            gx as u32 + 1,
            gy as u32 + 1,
            final_byte
        )
        .into_bytes(),
    )
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pty::{spawn_pane, PaneMsg, PtyPane};
    use super::super::render::tests::{no_cow, no_map};
    use super::super::render::{focused_pane_views, render_frame};
    use crate::geometry::CellPixels;
    use crate::select::{self, Selection};
    use crate::theme::Palette;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use gwae_layout::{Layout, PaneId};
    use gwae_term::TermGrid;
    use std::collections::HashMap;
    use std::sync::mpsc::channel;

    #[test]
    fn wheel_scrolls_a_plain_pane_but_reaches_a_reporting_child() {
        use MouseEventKind::*;
        let plain = KeyModifiers::NONE;
        // A plain shell keeps no mouse of its own: the wheel scrolls gwae's
        // history directly (vertical and horizontal alike), like jcode.
        for kind in [ScrollUp, ScrollDown, ScrollLeft, ScrollRight] {
            assert_eq!(
                mouse_role(kind, plain, false),
                MouseRole::Wheel,
                "{kind:?} over a plain pane should scroll history"
            );
        }
        // A child that asked for mouse reporting owns the wheel (jcode
        // scrolls its own transcript, vim its own buffer).
        for kind in [ScrollUp, ScrollDown, ScrollLeft, ScrollRight] {
            assert_eq!(
                mouse_role(kind, plain, true),
                MouseRole::Forward,
                "{kind:?} must reach a reporting child"
            );
        }
        // Shift+vertical wheel is the escape hatch: even a reporting child
        // yields to the multiplexer's scroll. Horizontal flicks always stay
        // with the child (only it knows wide content).
        assert_eq!(
            mouse_role(ScrollUp, KeyModifiers::SHIFT, true),
            MouseRole::Wheel,
            "Shift+wheel lets the multiplexer scroll past a reporting child"
        );
        assert_eq!(
            mouse_role(ScrollLeft, KeyModifiers::SHIFT, true),
            MouseRole::Forward,
            "Shift+horizontal wheel stays with the child"
        );
    }

    #[test]
    fn wheel_helpers_map_notches_to_deltas_and_arrow_keys() {
        // Up/left go back into history, down/right come forward, by exactly
        // one step constant so keyboard, wheel and e2e agree on the stride.
        assert_eq!(
            wheel_scroll_delta(MouseEventKind::ScrollUp),
            WHEEL_SCROLL_LINES
        );
        assert_eq!(
            wheel_scroll_delta(MouseEventKind::ScrollLeft),
            WHEEL_SCROLL_LINES
        );
        assert_eq!(
            wheel_scroll_delta(MouseEventKind::ScrollDown),
            -WHEEL_SCROLL_LINES
        );
        assert_eq!(
            wheel_scroll_delta(MouseEventKind::ScrollRight),
            -WHEEL_SCROLL_LINES
        );
        // A full-screen child gets the arrows it expects, matching the
        // `ScrollBack` arm's translation for the keyboard route.
        assert_eq!(wheel_alt_screen_keys(MouseEventKind::ScrollUp), b"\x1b[A");
        assert_eq!(
            wheel_alt_screen_keys(MouseEventKind::ScrollDown),
            b"\x1b[B]"
        );
    }



    #[test]
    fn left_drag_selects_but_a_reporting_child_keeps_its_mouse() {
        let plain = KeyModifiers::NONE;
        // No mouse reporting: left press/drag/release drive our selection.
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            assert_eq!(mouse_role(kind, plain, false), MouseRole::Select);
            // A child that asked for mouse reporting owns them instead, so
            // clicking inside vim or an agent TUI behaves natively.
            assert_eq!(mouse_role(kind, plain, true), MouseRole::Forward);
            // ...unless Shift is held: the xterm convention for "let the
            // multiplexer select instead of the app".
            assert_eq!(
                mouse_role(kind, KeyModifiers::SHIFT, true),
                MouseRole::Select
            );
        }
        // The wheel is never a selection: plain panes scroll their history
        // (`Wheel`), and a reporting child owns the event (`Forward`).
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, plain, false),
            MouseRole::Wheel
        );
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, plain, true),
            MouseRole::Forward
        );
        // Shift+vertical wheel is the escape hatch: even a reporting child
        // yields to the multiplexer's scroll. Horizontal flicks always stay
        // with the child (only it knows wide content).
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, KeyModifiers::SHIFT, true),
            MouseRole::Wheel
        );
        assert_eq!(
            mouse_role(MouseEventKind::ScrollLeft, KeyModifiers::SHIFT, true),
            MouseRole::Forward
        );
        // Right-drag is not our selection either.
        assert_eq!(
            mouse_role(MouseEventKind::Drag(MouseButton::Right), plain, false),
            MouseRole::Local
        );
    }


    #[test]
    fn sgr_mouse_report_encodes_wheel_and_buttons() {
        let ev = |kind| MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // Coordinates are 1-based in the protocol.
        assert_eq!(
            sgr_mouse_report(&ev(MouseEventKind::ScrollUp), 4, 2).unwrap(),
            b"\x1b[<64;5;3M".to_vec()
        );
        assert_eq!(
            sgr_mouse_report(&ev(MouseEventKind::ScrollDown), 0, 0).unwrap(),
            b"\x1b[<65;1;1M".to_vec()
        );
        // Release uses the lowercase final byte.
        assert_eq!(
            sgr_mouse_report(&ev(MouseEventKind::Up(MouseButton::Left)), 0, 0).unwrap(),
            b"\x1b[<0;1;1m".to_vec()
        );
        // Modifiers add their bits.
        let shifted = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::SHIFT,
        };
        assert_eq!(
            sgr_mouse_report(&shifted, 0, 0).unwrap(),
            b"\x1b[<4;1;1M".to_vec()
        );
    }

    #[test]
    fn mouse_hit_test_maps_screen_cell_to_pane_grid() {
        use gwae_layout::{Preset, Width};
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        for _ in 0..2 {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Half), vec![p]);
        }
        let panes = HashMap::new();
        let views = focused_pane_views(&layout, 80, 24, 0, &panes, false);
        assert_eq!(views.len(), 2);
        // A click in the left half hits the left pane at its own grid column.
        let (pid, gx, gy) = pane_at(&views, 5, 3).expect("hit left pane");
        assert_eq!(pid, views[0].pid);
        assert_eq!((gx, gy), (5, 3));
        // A click in the right half hits the right pane, and the grid column
        // is relative to that pane, not the screen.
        let (pid, gx, gy) = pane_at(&views, 45, 7).expect("hit right pane");
        assert_eq!(pid, views[1].pid);
        assert_eq!((gx, gy), (45 - views[1].rect.x, 7));
        // Past the last pane's right edge there is nothing to hit.
        assert!(pane_at(&views, 79, 3).is_some());
        assert!(pane_at(&views, 200, 3).is_none());
        assert!(pane_at(&views, 5, 200).is_none());
    }

    #[test]
    fn drag_outside_a_pane_clamps_to_its_edges() {
        use gwae_layout::{Preset, Width};
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        for _ in 0..2 {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Half), vec![p]);
        }
        let panes = HashMap::new();
        let views = focused_pane_views(&layout, 80, 24, 0, &panes, false);
        let left = views[0].pid;
        let r = views[0].rect;
        // Inside the pane the clamp is a no-op: same answer as `pane_at`.
        assert_eq!(clamped_pane_point(&views, left, 5, 3), Some((left, 5, 3)));
        // Dragging right, past the pane into its neighbour, still extends the
        // left pane's selection to its last column instead of freezing.
        let (pid, gx, gy) = clamped_pane_point(&views, left, 200, 3).unwrap();
        assert_eq!(pid, left);
        assert_eq!(gx, r.w - 1);
        assert_eq!(gy, 3);
        // Dragging below the last row clamps to the bottom row.
        let (_, _, gy) = clamped_pane_point(&views, left, 5, 200).unwrap();
        assert_eq!(gy, r.h - 1);
        // Dragging above/left of the pane clamps to its first cell.
        assert_eq!(clamped_pane_point(&views, left, 0, 0), Some((left, 0, 0)));
        // A pane that is not on screen cannot be resolved at all.
        let gone: PaneId = 9999;
        assert_eq!(clamped_pane_point(&views, gone, 5, 3), None);
    }

    #[test]
    fn selection_highlight_inverts_exactly_the_dragged_cells() {
        let layout = Layout::new(1);
        let pid = *layout
            .focused_row()
            .and_then(|r| r.columns.first())
            .and_then(|c| c.panes.first())
            .unwrap();
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let (tx, _rx) = channel::<PaneMsg>();
        let mut pane = spawn_pane(pid, "sleep 30", 80, 24, tx, None, CellPixels::default())
            .expect("spawn pane");
        pane.grid.feed(b"hello world\r\nsecond line");
        panes.insert(pid, pane);
        let (cols, rows) = (80u16, 24u16);
        let sel = Selection {
            pane: pid,
            anchor: select::Point::new(0, 0),
            cursor: select::Point::new(4, 0),
            dragging: true,
        };
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &Palette::default(),
            &no_map(),
            &no_cow(),
            false,
            Some(&sel),
        );
        // Content is inset 1 cell inside the column frame, so grid (0,0)
        // lands at screen (1,1).
        let at = |x: u16, y: u16| out[(y + 1) as usize * cols as usize + (x + 1) as usize];
        // "hello" is inverted, both ends inclusive; the space after is not.
        for x in 0..=4u16 {
            assert!(at(x, 0).style.inverse, "cell {x} should be highlighted");
        }
        assert!(
            !at(5, 0).style.inverse,
            "past the drag end, not highlighted"
        );
        assert!(!at(0, 1).style.inverse, "other rows untouched");
        // The text itself is unchanged: highlighting only restyles.
        assert_eq!(at(0, 0).ch, 'h');
        assert_eq!(at(4, 0).ch, 'o');
    }

}
