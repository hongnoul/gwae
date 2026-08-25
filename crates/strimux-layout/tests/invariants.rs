//! Property tests for the layout core invariants (see docs/LAYOUT-SPEC.md).
//!
//! Invariants exercised:
//!   1. Structural/movement verbs never change a column's width (no implicit resize).
//!   2. Rows never auto-relocate or reorder.
//!   3. Focus is always within the focused row's column bounds.

use proptest::prelude::*;
use strimux_layout::{Action, FollowScroll, Layout, Preset, Viewport, Width};

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
        Just(SpawnAgent),
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

#[test]
fn spawn_agent_adds_rightmost_column_and_focuses_it() {
    let mut layout = Layout::default();
    let n_before = layout.focused_row().unwrap().columns.len();
    // Focus somewhere in the middle so "rightmost" is distinguishable.
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let _ = layout.apply(Action::SpawnAgent, view(), follow());
    let row = layout.focused_row().unwrap();
    // A single pane is appended at the end of the strip, and it takes focus.
    assert_eq!(row.columns.len(), n_before + 1);
    assert_eq!(row.columns.last().unwrap().panes.len(), 1);
    assert_eq!(layout.focus.column, row.columns.len() - 1);
    assert_eq!(layout.focus.pane, 0);
    // The focused column is exactly the appended one.
    assert_eq!(
        row.columns[layout.focus.column].panes[0],
        row.columns.last().unwrap().panes[0]
    );
}

#[test]
fn new_spawn_scrolls_the_new_pane_into_view() {
    // Several fixed 1/4 columns overflow the viewport, so a rightmost spawned
    // column can start off-screen. Spawning must both focus the new pane and
    // follow-scroll so it is visible immediately.
    let mut layout = Layout::default();
    for _ in 0..8 {
        let _ = layout.apply(Action::NewColumn, view(), follow());
    }
    // Simulate an unscrolled strip: the focused rightmost column is off-screen.
    if let Some(row) = layout.row_mut(layout.focus.row) {
        row.scroll_x = 0;
    }
    let n_before = layout.focused_row().unwrap().columns.len();
    // Spawn one more; focus jumps to it and the strip scrolls right to reveal it.
    let _ = layout.apply(Action::SpawnAgent, view(), follow());
    let row = layout.focused_row().unwrap();
    assert_eq!(row.columns.len(), n_before + 1);
    assert_eq!(layout.focus.column, row.columns.len() - 1);
    let scroll_after = row.scroll_x;
    assert!(scroll_after > 0);
    let (s, e) = layout.focused_range(view().cols).unwrap();
    assert!(s as i32 - scroll_after < view().cols as i32, "new pane off-screen");
    assert!(e as i32 - scroll_after >= 0);
}

/// Build a single-row layout whose columns have exactly `widths`.
fn layout_with_widths(widths: &[Width]) -> Layout {
    let mut layout = Layout::new(1);
    if let Some(row) = layout.row_mut(layout.focus.row) {
        row.columns.clear();
    }
    let row = layout.focus.row;
    for w in widths {
        let pane = layout.alloc_pane();
        layout.add_column(row, *w, vec![pane]);
    }
    layout
}

#[test]
fn four_quarters_tile_the_viewport_exactly() {
    // The motivating bug: on widths not divisible by 4 (342 -> ceil = 86*4 =
    // 344), per-column ceil rounding pushed the rightmost pane past the edge.
    let q = Width::Preset(Preset::Quarter);
    let layout = layout_with_widths(&[q, q, q, q]);
    for cols in [341u16, 342, 343, 344, 80, 81, 82, 83, 120] {
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(
            ranges.last().unwrap().1,
            cols as u32,
            "4x quarter must end exactly at the right edge for cols={cols}"
        );
        // Adjacent columns tile with no gaps or overlaps.
        for w in ranges.windows(2) {
            assert_eq!(w[0].1, w[1].0, "gap/overlap at cols={cols}");
        }
        // Each column is within 1 cell of its ideal quarter share.
        for (s, e) in &ranges {
            let w = (e - s) as i32;
            let ideal = cols as f64 / 4.0;
            assert!(
                (w as f64 - ideal).abs() <= 1.0,
                "column width {w} deviates >1 from ideal {ideal} at cols={cols}"
            );
        }
    }
}

/// Any mix of preset columns tiles contiguously, each boundary lands within
/// one cell of its exact fractional position, and a strip whose nominal
/// shares sum to <= 1 never overflows the viewport.
fn preset_width() -> impl Strategy<Value = Width> {
    prop_oneof![
        Just(Width::Preset(Preset::Quarter)),
        Just(Width::Preset(Preset::Third)),
        Just(Width::Preset(Preset::Half)),
    ]
}

proptest! {
    #[test]
    fn column_ranges_tile_without_drift(
        widths in prop::collection::vec(preset_width(), 1..8),
        cols in 20u16..500,
    ) {
        let layout = layout_with_widths(&widths);
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        // Contiguous tiling from x=0.
        prop_assert_eq!(ranges[0].0, 0);
        for w in ranges.windows(2) {
            prop_assert_eq!(w[0].1, w[1].0);
        }
        // Every boundary is within 1 cell of its exact fractional position,
        // so rounding error never accumulates across the strip.
        let mut exact12 = 0u64; // position in twelfths of a cell
        for (i, (_, e)) in ranges.iter().enumerate() {
            exact12 += widths[i].twelfths(cols);
            let ideal = exact12 as f64 / 12.0;
            prop_assert!(
                ((*e as f64) - ideal).abs() <= 1.0,
                "boundary {} drifted from ideal {} (cols={})", e, ideal, cols
            );
        }
        // A strip that nominally fits (shares sum to <= 1) never overflows.
        let total12: u64 = widths.iter().map(|w| w.twelfths(cols)).sum();
        if total12 <= (cols as u64) * 12 {
            prop_assert!(
                ranges.last().unwrap().1 <= cols as u32,
                "strip overflows viewport: {:?} cols={}", ranges, cols
            );
        }
    }
}
