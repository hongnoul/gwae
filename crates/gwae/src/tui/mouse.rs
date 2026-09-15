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
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

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

}
