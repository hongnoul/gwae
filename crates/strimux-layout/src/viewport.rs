//! Viewport geometry and follow-focus scrolling (the "niri feel").
//!
//! A viewport is the window `[scroll_x, scroll_x + cols)` onto the focused
//! row. After any focus change we scroll the *minimum* distance so the focused
//! column is fully visible, honoring a `scroll_margin` of context on each side.

use crate::{Layout, RowId};

/// Follow-focus scroll options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowScroll {
    /// Context cells kept visible on each side of the focused column.
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

/// Compute the minimal `scroll_x` that fully reveals absolute range
/// `[start, end)` given the current `scroll` and viewport width.
///
/// Reimplemented from niri's documented "bring the focused window on-screen
/// with margin" semantics (niri is GPL-3.0; no code is copied).
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

/// Follow-scroll a layout's focused column and return the new scroll x.
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
    if opts.center {
        let center = (s as i32 + e as i32) / 2;
        return (center - (viewport_cols as i32) / 2).max(0);
    }
    follow_scroll(
        s as i32,
        e as i32,
        scroll,
        viewport_cols as i32,
        opts.margin as i32,
    )
}
