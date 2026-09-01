//! Property tests for the layout core invariants (see docs/LAYOUT-SPEC.md).
//!
//! Invariants exercised:
//!   1. Structural/movement verbs never change a column's width (no implicit resize).
//!   2. Rows never auto-relocate or reorder.
//!   3. Focus is always within the focused row's column bounds.

use gwae_layout::{scroll_stops, Action, FollowScroll, Layout, Preset, Viewport, Width};
use proptest::prelude::*;

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
        Just(SpawnAgentRow),
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
            // An empty focused strip (niri-style, freshly created or just
            // emptied) is legal; otherwise the focus must index a column.
            assert!(row.columns.is_empty() || layout.focus.column < row.columns.len());
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
    let a = layout.new_row();
    let b = layout.new_row();
    let ids: Vec<_> = layout.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![layout.focus.row, a, b]);
    // A NewColumn on the focused row only touches that row; order is preserved.
    let _ = layout.apply(Action::NewColumn, view(), follow());
    let ids2: Vec<_> = layout.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, ids2);
}

#[test]
fn spawn_agent_inserts_column_right_of_focus_and_focuses_it() {
    let mut layout = Layout::default();
    let n_before = layout.focused_row().unwrap().columns.len();
    // Focus somewhere in the middle so "right of focus" is distinguishable
    // from "end of strip".
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let focused_before = layout.focus.column;
    let last_before = layout.focused_row().unwrap().columns.last().unwrap().panes[0];
    let _ = layout.apply(Action::SpawnAgent, view(), follow());
    let row = layout.focused_row().unwrap();
    // A single pane is inserted just after the previously focused column, and
    // it takes focus.
    assert_eq!(row.columns.len(), n_before + 1);
    assert_eq!(layout.focus.column, focused_before + 1);
    assert_eq!(row.columns[layout.focus.column].panes.len(), 1);
    assert_eq!(layout.focus.pane, 0);
    // The pre-existing rightmost column stayed at the end: nothing was
    // appended past it.
    assert_eq!(row.columns.last().unwrap().panes[0], last_before);
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
    let focused_before = layout.focus.column;
    // Spawn one more; focus jumps to it and the strip scrolls right to reveal it.
    let _ = layout.apply(Action::SpawnAgent, view(), follow());
    let row = layout.focused_row().unwrap();
    assert_eq!(row.columns.len(), n_before + 1);
    assert_eq!(layout.focus.column, focused_before + 1);
    let scroll_after = row.scroll_x;
    assert!(scroll_after > 0);
    let (s, e) = layout.focused_range(view().cols).unwrap();
    assert!(
        s as i32 - scroll_after < view().cols as i32,
        "new pane off-screen"
    );
    assert!(e as i32 - scroll_after >= 0);
}

#[test]
fn kill_pane_keeps_slot_then_falls_left() {
    // Killing a middle column collapses the strip and focus stays in the same
    // slot (the column that slid in from the right). Only the rightmost
    // column falls back to the left neighbor.
    let mut layout = Layout::default(); // 4 columns
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let _ = layout.apply(Action::FocusRight, view(), follow()); // focus col 2
    let n = layout.focused_row().unwrap().columns.len();
    let right_pid = layout.focused_row().unwrap().columns[3].panes[0];
    let _ = layout.apply(Action::KillPane, view(), follow());
    let row = layout.focused_row().unwrap();
    assert_eq!(row.columns.len(), n - 1, "column collapsed");
    assert_eq!(layout.focus.column, 2, "focus keeps its slot");
    assert_eq!(row.columns[layout.focus.column].panes[0], right_pid);

    // Now the focus is on the rightmost column: killing it must fall left.
    let left_pid = layout.focused_row().unwrap().columns[1].panes[0];
    let _ = layout.apply(Action::KillPane, view(), follow());
    assert_eq!(layout.focus.column, 1, "no column to the right, fall left");
    let row = layout.focused_row().unwrap();
    assert_eq!(row.columns[layout.focus.column].panes[0], left_pid);
}

#[test]
fn close_pane_removes_by_id_and_compacts() {
    // Closing a non-focused pane by id (its process exited) compacts columns
    // and keeps the focus on the same pane it was on.
    let mut layout = Layout::default(); // 4 columns, focus col 0
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let _ = layout.apply(Action::FocusRight, view(), follow()); // focus col 2
    let focused = layout.focused_pane_id().unwrap();
    let dead = layout.focused_row().unwrap().columns[0].panes[0];
    let _ = layout.apply(Action::ClosePane(dead), view(), follow());
    let row = layout.focused_row().unwrap();
    assert_eq!(row.columns.len(), 3, "column collapsed");
    // Focus followed its pane left by one slot after compaction.
    assert_eq!(layout.focus.column, 1);
    assert_eq!(layout.focused_pane_id(), Some(focused));
    assert!(layout.locate_pane(dead).is_none(), "dead pane fully gone");
    assert!(!layout.panes.contains_key(&dead), "pane record dropped");
}

#[test]
fn close_focused_pane_in_stack_prefers_pane_above() {
    // In a stacked column, closing the focused lower pane moves focus up
    // (fill-left-first generalized: prefer the earlier neighbor).
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow()); // focus pane 1
    let above = layout.focused_row().unwrap().columns[0].panes[0];
    let focused = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::ClosePane(focused), view(), follow());
    assert_eq!(layout.focus.pane, 0);
    assert_eq!(layout.focused_pane_id(), Some(above));
}

#[test]
fn close_unknown_pane_is_noop() {
    let mut layout = Layout::default();
    let before = layout.clone();
    let _ = layout.apply(Action::ClosePane(9999), view(), follow());
    assert_eq!(layout, before);
}

#[test]
fn close_last_pane_resets_to_default() {
    let mut layout = Layout::new(1);
    let pid = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::ClosePane(pid), view(), follow());
    // The layout never ends up empty; it resets to the default grid.
    assert!(!layout.rows.is_empty());
    assert!(layout.focused_pane_id().is_some());
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

#[test]
fn focusing_rightmost_of_exact_fit_strip_never_scrolls() {
    // Regression: with four 1/4 columns tiling the viewport exactly, moving
    // focus to the rightmost pane must not scroll (the follow margin would
    // otherwise push scroll_x past the strip and reveal background on the
    // right edge).
    let q = Width::Preset(Preset::Quarter);
    for cols in [80u16, 81, 82, 83, 120, 341, 342, 343, 344] {
        let mut layout = layout_with_widths(&[q, q, q, q]);
        let vp = Viewport::new(cols);
        for _ in 0..3 {
            let scroll = layout.apply(Action::FocusRight, vp, follow()).unwrap();
            assert_eq!(scroll, 0, "over-scrolled at cols={cols}");
        }
        // Manual scroll right is also clamped: nothing to reveal.
        let scroll = layout
            .apply(Action::ScrollViewport(10), vp, follow())
            .unwrap();
        assert_eq!(
            scroll, 0,
            "manual scroll revealed background at cols={cols}"
        );
    }
}

#[test]
fn overflowing_strip_scroll_clamps_to_last_column_edge() {
    // Six 1/4 columns overflow the viewport. Focusing the last one scrolls,
    // but never past the strip's right edge.
    let q = Width::Preset(Preset::Quarter);
    let mut layout = layout_with_widths(&[q, q, q, q, q, q]);
    let vp = Viewport::new(120);
    let mut scroll = 0;
    for _ in 0..5 {
        scroll = layout.apply(Action::FocusRight, vp, follow()).unwrap();
    }
    let ranges = layout.column_x_ranges(layout.focus.row, vp.cols).unwrap();
    let total = ranges.last().unwrap().1 as i32;
    assert_eq!(scroll, total - vp.cols as i32, "scroll stops at strip edge");
    // Further manual scrolling stays clamped.
    let scroll = layout
        .apply(Action::ScrollViewport(50), vp, follow())
        .unwrap();
    assert_eq!(scroll, total - vp.cols as i32);
}

#[test]
fn clamp_scrolls_snaps_back_after_viewport_widens() {
    // A scroll valid at a narrow viewport can overshoot after the terminal
    // widens (the strip grows slower than the viewport when columns are
    // viewport-relative fractions of a *smaller* whole in absolute terms).
    // clamp_scrolls must pull it back so no background is revealed.
    let q = Width::Preset(Preset::Quarter);
    let mut layout = layout_with_widths(&[q, q, q, q, q, q]);
    let narrow = Viewport::new(80);
    for _ in 0..5 {
        let _ = layout.apply(Action::FocusRight, narrow, follow());
    }
    assert!(layout.row(layout.focus.row).unwrap().scroll_x > 0);
    // Simulate a resize to a much wider terminal where all 6 columns fit...
    // (6 quarters = 1.5x viewport, so they never all fit; use max_scroll.)
    let wide = Viewport::new(300);
    layout.clamp_scrolls(wide);
    let scroll = layout.row(layout.focus.row).unwrap().scroll_x;
    assert!(
        scroll <= layout.max_scroll(wide),
        "stale scroll not clamped"
    );
    let ranges = layout.column_x_ranges(layout.focus.row, wide.cols).unwrap();
    let total = ranges.last().unwrap().1 as i32;
    assert!(
        scroll + (wide.cols as i32) <= total || scroll == 0,
        "viewport extends past strip: background revealed"
    );
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

#[test]
fn focus_down_past_last_strip_creates_one_and_up_reclaims_it() {
    // niri workspace semantics: j past the end opens a fresh empty strip,
    // a second j there is a no-op (the current strip is empty), and leaving
    // the empty strip drops it again.
    let mut layout = Layout::new(1);
    assert_eq!(layout.rows.len(), 1);
    let first = layout.focus.row;
    let _ = layout.apply(Action::FocusDown, view(), follow());
    assert_eq!(layout.rows.len(), 2, "empty strip created below");
    let new_row = layout.focus.row;
    assert_ne!(new_row, first);
    assert!(layout.row_is_empty(new_row));
    // No stacking of empty strips.
    let _ = layout.apply(Action::FocusDown, view(), follow());
    assert_eq!(layout.rows.len(), 2);
    assert_eq!(layout.focus.row, new_row);
    // Going back up discards the empty strip.
    let _ = layout.apply(Action::FocusUp, view(), follow());
    assert_eq!(layout.focus.row, first);
    assert_eq!(layout.rows.len(), 1, "empty strip reclaimed on leave");
}

#[test]
fn spawning_into_a_new_strip_keeps_it() {
    let mut layout = Layout::new(1);
    let first = layout.focus.row;
    let _ = layout.apply(Action::FocusDown, view(), follow());
    let _ = layout.apply(Action::NewColumn, view(), follow());
    assert_eq!(layout.focused_row().unwrap().columns.len(), 1);
    let _ = layout.apply(Action::FocusUp, view(), follow());
    assert_eq!(layout.focus.row, first);
    assert_eq!(layout.rows.len(), 2, "populated strip survives");
}

#[test]
fn killing_the_last_pane_of_a_strip_shifts_focus_up() {
    let mut layout = Layout::new(1);
    let first = layout.focus.row;
    let _ = layout.apply(Action::FocusDown, view(), follow());
    let _ = layout.apply(Action::NewColumn, view(), follow());
    let strip = layout.focus.row;
    let _ = layout.apply(Action::KillPane, view(), follow());
    // The emptied strip is reclaimed and the focus moves to the strip above.
    assert_eq!(layout.rows.len(), 1);
    assert_eq!(layout.focus.row, first);
    assert!(!layout.rows.iter().any(|r| r.id == strip));
    assert!(!layout.row_is_empty(layout.focus.row));
}

#[test]
fn killing_the_last_pane_of_the_first_strip_shifts_focus_down() {
    let mut layout = Layout::new(1);
    let first = layout.focus.row;
    let _ = layout.apply(Action::FocusDown, view(), follow());
    let _ = layout.apply(Action::NewColumn, view(), follow());
    let second = layout.focus.row;
    let _ = layout.apply(Action::FocusUp, view(), follow());
    assert_eq!(layout.focus.row, first);
    let _ = layout.apply(Action::KillPane, view(), follow());
    assert_eq!(layout.focus.row, second);
    assert_eq!(layout.rows.len(), 1);
}

#[test]
fn move_pane_down_carries_it_to_a_new_strip() {
    // Alt+Shift+j from the bottom of a stack sends the pane to the strip
    // below, creating it when needed (niri "move window to workspace").
    let mut layout = Layout::new(2);
    let pid = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::MovePaneDown, view(), follow());
    assert_eq!(layout.rows.len(), 2);
    assert_eq!(
        layout.focused_pane_id(),
        Some(pid),
        "pane travels with focus"
    );
    assert_eq!(layout.focused_row().unwrap().columns.len(), 1);
    // The source strip kept its other pane.
    assert_eq!(layout.rows[0].columns.len(), 1);
    // And back up again: the emptied strip is discarded.
    let _ = layout.apply(Action::MovePaneUp, view(), follow());
    assert_eq!(layout.rows.len(), 1);
    assert_eq!(layout.focused_pane_id(), Some(pid));
    assert_eq!(layout.rows[0].columns.len(), 2);
}

#[test]
fn moving_a_lone_pane_off_its_strip_is_a_noop() {
    let mut layout = Layout::new(1);
    let before = layout.clone();
    let _ = layout.apply(Action::MovePaneDown, view(), follow());
    assert_eq!(layout, before, "a lone pane has nowhere new to go");
}

/// Every row's `scroll_x` rests on a valid quantized stop: a column's left
/// boundary, or `max_scroll` (the end stop pinning the last column to the
/// right viewport edge). This is what guarantees identical grids paint
/// identically across scroll states: a column always starts exactly at x=0.
fn assert_scrolls_on_stops(layout: &Layout, vp: Viewport, ctx: &str) {
    for row in &layout.rows {
        let stops = scroll_stops(layout, row.id, vp.cols);
        assert!(
            stops.contains(&row.scroll_x),
            "{ctx}: scroll_x={} not on a stop {:?} (row {})",
            row.scroll_x,
            stops,
            row.id
        );
    }
}

proptest! {
    #[test]
    fn scroll_is_always_quantized(actions in random_actions()) {
        let mut layout = Layout::default();
        seed_columns(&mut layout, 6);
        assert_scrolls_on_stops(&layout, view(), "seed");
        for a in actions {
            let _ = layout.apply(a, view(), follow());
            assert_scrolls_on_stops(&layout, view(), &format!("after {a:?}"));
        }
    }

    #[test]
    fn clamp_scrolls_lands_on_stops_after_resize(
        actions in random_actions(),
        cols in 20u16..500,
    ) {
        let mut layout = Layout::default();
        seed_columns(&mut layout, 6);
        for a in actions {
            let _ = layout.apply(a, view(), follow());
        }
        // Simulate a terminal resize: geometry changes under the stored
        // scrolls, clamp_scrolls must re-snap every row.
        let vp = Viewport::new(cols);
        layout.clamp_scrolls(vp);
        assert_scrolls_on_stops(&layout, vp, &format!("after resize to {cols}"));
    }
}

#[test]
fn manual_scroll_pages_between_column_boundaries() {
    // Six 1/4 columns overflow a 120-col viewport (each column is 30 cells).
    // Manual scrolls land exactly on column boundaries, ending at max_scroll,
    // and reverse the same way.
    let q = Width::Preset(Preset::Quarter);
    let mut layout = layout_with_widths(&[q, q, q, q, q, q]);
    let vp = Viewport::new(120);
    let ranges = layout.column_x_ranges(layout.focus.row, vp.cols).unwrap();
    let total = ranges.last().unwrap().1 as i32;
    let max_scroll = total - vp.cols as i32;
    let starts: Vec<i32> = ranges.iter().map(|(s, _)| *s as i32).collect();
    let mut seen = vec![0];
    loop {
        let before = layout.row(layout.focus.row).unwrap().scroll_x;
        let after = layout
            .apply(Action::ScrollViewport(1), vp, follow())
            .unwrap();
        if after == before {
            break;
        }
        assert!(
            starts.contains(&after) || after == max_scroll,
            "scroll {after} is not a column boundary or the end stop"
        );
        seen.push(after);
    }
    assert_eq!(
        *seen.last().unwrap(),
        max_scroll,
        "paging reaches the end stop"
    );
    assert!(seen.len() > 2, "multiple stops traversed");
    // And back: the same stops in reverse, ending at 0.
    for want in seen.iter().rev().skip(1) {
        let after = layout
            .apply(Action::ScrollViewport(-1), vp, follow())
            .unwrap();
        assert_eq!(after, *want, "reverse paging retraces the stops");
    }
    assert_eq!(layout.row(layout.focus.row).unwrap().scroll_x, 0);
}

#[test]
fn focus_scroll_states_paint_columns_at_identical_offsets() {
    // The motivating bug: walking focus across an overflowing strip must
    // produce scroll states where every visible column starts at an offset
    // that is exactly some column's boundary distance, i.e. the visible grid
    // is always a suffix of columns starting flush at x=0 (or the end stop).
    // With uniform quarters, all scroll states then paint the same 4-column
    // grid shape.
    let q = Width::Preset(Preset::Quarter);
    let mut layout = layout_with_widths(&[q, q, q, q, q, q, q, q]);
    let vp = Viewport::new(120);
    let starts: Vec<i32> = layout
        .column_x_ranges(layout.focus.row, vp.cols)
        .unwrap()
        .iter()
        .map(|(s, _)| *s as i32)
        .collect();
    let max_scroll = layout.max_scroll(vp);
    for _ in 0..7 {
        let scroll = layout.apply(Action::FocusRight, vp, follow()).unwrap();
        assert!(
            starts.contains(&scroll) || scroll == max_scroll,
            "focus walk produced off-boundary scroll {scroll}"
        );
        // The focused column is fully visible with no margin slivers.
        let (s, e) = layout.focused_range(vp.cols).unwrap();
        assert!(s as i32 >= scroll, "focused column clipped on the left");
        assert!(
            e as i32 <= scroll + vp.cols as i32,
            "focused column clipped on the right"
        );
    }
    // Walk back: same guarantee.
    for _ in 0..7 {
        let scroll = layout.apply(Action::FocusLeft, vp, follow()).unwrap();
        assert!(
            starts.contains(&scroll) || scroll == max_scroll,
            "reverse focus walk produced off-boundary scroll {scroll}"
        );
    }
}

#[test]
fn visible_ranges_are_identical_at_every_stop_for_uniform_columns() {
    // The renderer paints with visible_column_x_ranges. At a viewport width
    // not divisible by 4 (342: the reported wobble width), absolute rounding
    // shifts inner boundaries by one cell between stops (86/171/257 vs
    // 85/171/256). Window-anchored rounding must paint the identical grid at
    // every stop.
    let q = Width::Preset(Preset::Quarter);
    let layout = layout_with_widths(&[q, q, q, q, q, q, q, q]);
    for cols in [342u16, 341, 343, 82, 83, 121] {
        let vp = Viewport::new(cols);
        let row = layout.focus.row;
        let stops = scroll_stops(&layout, row, vp.cols);
        let mut grids: Vec<Vec<(i32, i32)>> = Vec::new();
        for stop in &stops {
            let vis = layout.visible_column_x_ranges(row, vp.cols, *stop).unwrap();
            // Only the on-screen part matters for painting.
            let on: Vec<(i32, i32)> = vis
                .into_iter()
                .filter(|(s, e)| *e > 0 && *s < cols as i32)
                .collect();
            // Full tiling: flush at x=0, contiguous, flush at the right edge.
            assert_eq!(on.first().unwrap().0, 0, "cols={cols} stop={stop}");
            for w in on.windows(2) {
                assert_eq!(w[0].1, w[1].0, "gap at cols={cols} stop={stop}");
            }
            assert_eq!(
                on.last().unwrap().1,
                cols as i32,
                "right edge uncovered at cols={cols} stop={stop}"
            );
            grids.push(on);
        }
        // Uniform columns: every stop paints the same 4-column grid shape.
        let shape = |g: &Vec<(i32, i32)>| -> Vec<i32> { g.iter().map(|(s, _)| *s).collect() };
        let first = shape(&grids[0]);
        for (i, g) in grids.iter().enumerate() {
            assert_eq!(
                shape(g),
                first,
                "grid shape changed at cols={cols} stop index {i}"
            );
        }
    }
}

#[test]
fn toggle_full_width_round_trips_to_quarter() {
    let mut layout = layout_with_widths(&[Width::Preset(Preset::Third), Width::DEFAULT]);
    let width = |l: &Layout| l.focused_row().unwrap().columns[l.focus.column].width;

    // Any non-full width goes full first.
    layout
        .apply(Action::ToggleFullWidth, view(), follow())
        .unwrap();
    assert_eq!(width(&layout), Width::Preset(Preset::Full));

    // Toggling back lands on a quarter.
    layout
        .apply(Action::ToggleFullWidth, view(), follow())
        .unwrap();
    assert_eq!(width(&layout), Width::Preset(Preset::Quarter));

    layout
        .apply(Action::ToggleFullWidth, view(), follow())
        .unwrap();
    assert_eq!(width(&layout), Width::Preset(Preset::Full));
}

#[test]
fn column_focus_persists_across_horizontal_movement() {
    // Vertical focus is per column: stepping off a stack and back returns to
    // the pane you were on, not the top of the column.
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    assert_eq!(layout.focus.pane, 2);
    let deep = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::FocusRight, view(), follow());
    assert_eq!(
        layout.focus.pane, 0,
        "shallow neighbor starts at its own top"
    );
    let _ = layout.apply(Action::FocusLeft, view(), follow());
    assert_eq!(layout.focus.pane, 2);
    assert_eq!(layout.focused_pane_id(), Some(deep));
}

#[test]
fn column_focus_persists_across_strips() {
    // Crossing to another strip and back restores the per-column pane too,
    // including when the strip-level memory picks the column.
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let deep = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::FocusDown, view(), follow()); // new empty strip
    let _ = layout.apply(Action::NewColumn, view(), follow());
    let _ = layout.apply(Action::FocusUp, view(), follow());
    assert_eq!(layout.focus.pane, 1);
    assert_eq!(layout.focused_pane_id(), Some(deep));
}

#[test]
fn column_focus_survives_a_jump_and_neighbor_edits() {
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let deep = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::JumpToColumn(2), view(), follow());
    // Editing another column must not disturb this one's memory.
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let _ = layout.apply(Action::JumpToColumn(0), view(), follow());
    assert_eq!(layout.focused_pane_id(), Some(deep));
}

#[test]
fn column_focus_tracks_panes_removed_above_it() {
    // Closing a pane above the remembered one shifts every later index down;
    // memory must follow the same pane rather than drift up a slot.
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let deep = layout.focused_pane_id().unwrap();
    let top = layout.focused_row().unwrap().columns[0].panes[0];
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let _ = layout.apply(Action::ClosePane(top), view(), follow());
    let _ = layout.apply(Action::FocusLeft, view(), follow());
    assert_eq!(layout.focused_pane_id(), Some(deep));
}

#[test]
fn column_focus_clamps_when_the_stack_shrinks() {
    let mut layout = Layout::default();
    let _ = layout.apply(Action::SplitBelow, view(), follow());
    let bottom = layout.focused_pane_id().unwrap();
    let _ = layout.apply(Action::FocusRight, view(), follow());
    let _ = layout.apply(Action::ClosePane(bottom), view(), follow());
    let _ = layout.apply(Action::FocusLeft, view(), follow());
    assert_eq!(layout.focus.pane, 0);
    assert!(layout.focused_pane_id().is_some());
}

#[test]
fn cycle_width_scrolls_rightmost_column_into_view() {
    // Regression: widening the rightmost visible column used to leave the
    // scroll stale, so the new width only showed up after a focus change.
    let mut layout = layout_with_widths(&[
        Width::Preset(Preset::Half),
        Width::Preset(Preset::Half),
        Width::Preset(Preset::Half),
    ]);
    layout.focus.column = 2;
    layout.apply(Action::FocusRight, view(), follow()).unwrap();
    layout.apply(Action::FocusLeft, view(), follow()).unwrap();

    for _ in 0..4 {
        layout.apply(Action::CycleWidth, view(), follow()).unwrap();
        let cols = view().cols;
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        let (s, e) = ranges[layout.focus.column];
        let scroll = layout.focused_row().unwrap().scroll_x;
        assert!(
            (s as i32) >= scroll && (e as i32) <= scroll + cols as i32,
            "focused column {s}..{e} not fully visible at scroll {scroll}"
        );
    }
}
