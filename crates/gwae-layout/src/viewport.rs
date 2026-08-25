//! Viewport geometry and quantized follow-focus scrolling.
//!
//! A viewport is the window `[scroll_x, scroll_x + cols)` onto the focused
//! row. Scrolling is *quantized*: `scroll_x` only ever rests on a column's
//! left boundary (or on `max_scroll`, the end stop that pins the last column
//! to the right edge). Every scroll state therefore starts painting a column
//! exactly at x=0, so identical grids render identically regardless of how
//! the viewport got there: no partial-column slivers, no margin drift.
//!
//! After a focus change we pick the *nearest valid stop* that fully reveals
//! the focused column, which preserves the minimal-movement "niri feel" while
//! keeping the paint stable.

use crate::{Layout, RowId};

/// Follow-focus scroll options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowScroll {
    /// Context cells kept visible on each side of the focused column.
    ///
    /// Ignored under quantized scrolling: partial-column margins are exactly
    /// the slivers quantization exists to remove. Kept for config
    /// compatibility.
    pub margin: u16,
    /// Always center the focused column (niri's centered mode).
    pub center: bool,
}

impl Default for FollowScroll {
    fn default() -> Self {
        FollowScroll {
            margin: 2,
            center: false,
        }
    }
}

/// The current viewport: how many cells of a row are visible at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub cols: u16,
}

impl Viewport {
    pub fn new(cols: u16) -> Self {
        Viewport { cols: cols.max(1) }
    }
}

/// The valid quantized scroll positions ("stops") for `row`: every column's
/// left boundary within `[0, max_scroll]`, plus `max_scroll` itself so the
/// strip can always pin its last column to the right viewport edge. Sorted
/// ascending, always non-empty (at least `[0]`).
pub fn scroll_stops(layout: &Layout, row: RowId, viewport_cols: u16) -> Vec<i32> {
    let ranges = layout
        .column_x_ranges(row, viewport_cols)
        .unwrap_or_default();
    let vw = viewport_cols.max(1) as i32;
    let total = ranges.last().map(|r| r.1 as i32).unwrap_or(0);
    let max_scroll = (total - vw).max(0);
    let mut stops: Vec<i32> = ranges
        .iter()
        .map(|(s, _)| *s as i32)
        .filter(|s| *s <= max_scroll)
        .collect();
    if stops.last() != Some(&max_scroll) {
        stops.push(max_scroll);
    }
    if stops.is_empty() {
        stops.push(0);
    }
    stops
}

/// Snap `scroll` to the nearest valid stop for `row` (ties break toward the
/// smaller stop). Used after external geometry changes such as a resize.
pub fn snap_scroll(layout: &Layout, row: RowId, viewport_cols: u16, scroll: i32) -> i32 {
    scroll_stops(layout, row, viewport_cols)
        .into_iter()
        .min_by_key(|b| ((b - scroll).abs(), *b))
        .unwrap_or(0)
}

/// Compute the minimal `scroll_x` that fully reveals absolute range
/// `[start, end)` given the current `scroll` and viewport width.
///
/// Reimplemented from niri's documented "bring the focused window on-screen
/// with margin" semantics (niri is GPL-3.0; no code is copied).
///
/// This is the *continuous* primitive; `follow_focus_scroll` quantizes its
/// result to column boundaries.
pub fn follow_scroll(start: i32, end: i32, scroll: i32, viewport_cols: i32, margin: i32) -> i32 {
    let left = start - margin;
    let right = end + margin;
    let vw = viewport_cols.max(1);
    let mut out = scroll;
    if left < scroll {
        out = left;
    }
    if right > scroll + vw {
        out = right - vw;
    }
    out.max(0)
}

/// Follow-scroll a layout's focused column and return the new scroll x,
/// quantized to a valid stop (a column boundary or `max_scroll`).
///
/// Among the stops that fully reveal the focused column, the one nearest the
/// current scroll wins (minimal movement); centered mode picks the stop
/// nearest the centering target instead. A column wider than the viewport
/// falls back to revealing its left edge.
pub fn follow_focus_scroll(
    layout: &Layout,
    row: RowId,
    column: usize,
    viewport_cols: u16,
    opts: FollowScroll,
) -> i32 {
    let ranges = match layout.column_x_ranges(row, viewport_cols) {
        Some(r) => r,
        None => return 0,
    };
    let (s, e) = match ranges.get(column) {
        Some(r) => *r,
        None => return 0,
    };
    let scroll = layout.row(row).map(|r| r.scroll_x).unwrap_or(0);
    let vw = viewport_cols.max(1) as i32;
    // Never scroll past the end of the strip: no stop reveals empty
    // background to the right of the last column (`scroll_stops` clamps every
    // stop to max_scroll).
    let total = ranges.last().map(|r| r.1).unwrap_or(0) as i32;
    let max_scroll = (total - vw).max(0);
    let stops = scroll_stops(layout, row, viewport_cols);
    // Stops that fully reveal the focused column: `stop <= s && e <= stop+vw`.
    let feasible: Vec<i32> = stops
        .iter()
        .copied()
        .filter(|b| *b <= s as i32 && e as i32 <= *b + vw)
        .collect();
    if feasible.is_empty() {
        // Column wider than the viewport: show it from its left edge.
        return (s as i32).clamp(0, max_scroll);
    }
    let target = if opts.center {
        (s as i32 + e as i32) / 2 - vw / 2
    } else {
        scroll
    };
    feasible
        .into_iter()
        .min_by_key(|b| ((b - target).abs(), *b))
        .unwrap_or(0)
}
