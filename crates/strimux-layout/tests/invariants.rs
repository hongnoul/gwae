//! Property tests for the layout core invariants (see docs/LAYOUT-SPEC.md).
//!
//! Invariants exercised:
//!   1. Structural/movement verbs never change a column's width (no implicit resize).
//!   2. Rows never auto-relocate or reorder.
//!   3. Focus is always within the focused row's column bounds.

use proptest::prelude::*;
use strimux_layout::{Action, FollowScroll, Layout, Viewport, Width};

fn follow() -> FollowScroll {
    FollowScroll::default()
}

fn view() -> Viewport {
    Viewport::new(120)
}

fn random_actions() -> impl Strategy<Value = Vec<Action>> {
    use Action::*;
    let action = prop_oneof![
        Just(FocusLeft),
        Just(FocusRight),
        Just(FocusUp),
        Just(FocusDown),
        Just(SplitBelow),
        Just(NewColumn),
        Just(NewRow),
        Just(MovePaneLeft),
        Just(MovePaneRight),
        prop::collection::vec(0..5, 1).prop_map(|v| ScrollViewport(v[0] - 2)),
        Just(KillPane),
    ];
    prop::collection::vec(action, 0..40)
}

/// Actions that must preserve every column's width.
fn width_preserving_actions() -> impl Strategy<Value = Vec<Action>> {
    use Action::*;
    let action = prop_oneof![
        Just(MovePaneLeft),
        Just(MovePaneRight),
        prop::collection::vec(0..5, 1).prop_map(|v| ScrollViewport(v[0] - 2)),
    ];
    prop::collection::vec(action, 0..30)
}

fn total_widths(layout: &Layout) -> Vec<Width> {
    let mut out = Vec::new();
    for row in &layout.rows {
        for col in &row.columns {
            out.push(col.width);
        }
    }
    out
}

fn seed_columns(layout: &mut Layout, n: usize) {
    for _ in 0..n {
        let _ = layout.apply(Action::NewColumn, view(), follow());
        let _ = layout.apply(Action::FocusRight, view(), follow());
    }
}

proptest! {
    #[test]
    fn focus_stays_in_bounds(actions in random_actions()) {
        let mut layout = Layout::default();
        seed_columns(&mut layout, 4);
        for a in actions {
            let _ = layout.apply(a, view(), follow());
            let row = layout.focused_row().unwrap();
            assert!(layout.focus.column < row.columns.len());
            if let Some(col) = row.columns.get(layout.focus.column) {
                assert!(layout.focus.pane < col.panes.len().max(1));
            }
        }
    }

    #[test]
    fn move_and_scroll_never_resize(actions in width_preserving_actions()) {
        let mut layout = Layout::default();
        seed_columns(&mut layout, 6);
        let before = total_widths(&layout);
        for a in actions {
            let _ = layout.apply(a, view(), follow());
        }
        // No-shrink guarantee: movement and scrolling never resize any column.
        // MovePane may reorder differing-width columns, so compare the width
        // multiset rather than the exact column order.
        let mut before_sorted: Vec<String> = before.iter().map(|w| format!("{w:?}")).collect();
        let mut after_sorted: Vec<String> =
            total_widths(&layout).iter().map(|w| format!("{w:?}")).collect();
        before_sorted.sort();
        after_sorted.sort();
        assert_eq!(before_sorted, after_sorted);
    }
}

#[test]
fn rows_never_reorder() {
    let mut layout = Layout::default();
    let a = layout.new_row("a".to_string());
    let b = layout.new_row("b".to_string());
    let ids: Vec<_> = layout.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![layout.focus.row, a, b]);
    // A NewColumn on the focused row only touches that row; order is preserved.
    let _ = layout.apply(Action::NewColumn, view(), follow());
    let ids2: Vec<_> = layout.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, ids2);
}
