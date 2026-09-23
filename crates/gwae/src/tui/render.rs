//! Frame composition: pane views, the compositor, focus frames (verbatim move from `tui/mod.rs`).

use std::collections::HashMap;

use gwae_layout::{Layout, PaneId, Width};
use gwae_term::{CColor, Cell, Size as GridSize, TermGrid};

use crate::config::Config;
use crate::select::Selection;
use crate::theme::Palette;

use super::chrome::{draw_edge_ticks, draw_minimap};
use super::input::focused_pane;
use super::pty::PtyPane;
use super::Rect;

/// Peek-sliver rendering: a neighbour clipped to fewer than this many
/// visible columns is not drawn as truncated text.
pub(crate) const MIN_VISIBLE_PANE_WIDTH: u16 = 10;
pub(crate) const PEEK_SLIVER_WIDTH: u16 = 3;

pub(crate) fn chrome_rows(_cfg: &Config) -> u16 {
    // Bottom status row has been removed; chrome is always 0.
    0
}

/// One visible pane on screen: where to draw it and which grid slice to show.
#[derive(Debug)]
pub(crate) struct PaneView {
    pub(crate) pid: PaneId,
    pub(crate) col: usize,    // index of the owning column in the focused strip
    pub(crate) rect: Rect,    // screen rect (already clipped to viewport horizontally)
    pub(crate) col_x0: u16,   // grid column at the left edge of `rect` (before content scroll)
    pub(crate) h_scroll: i32, // pane content scroll in cells
    pub(crate) grid_cols: u16, // full logical content width of the grid
    pub(crate) grid_rows: u16, // vertical size of the grid
    pub(crate) peek: bool,    // squished neighbour shown as a 3-cell faded hint
}

/// Compute visible pane views for the focused row.
///
/// With `inset` (always true in the renderer) every pane's rect is shrunk by
/// 1 cell on all sides of its column box so content sits *inside* the frame
/// instead of being overlaid by it: nothing a program draws is ever covered.
pub(crate) fn focused_pane_views(
    layout: &Layout,
    cols: u16,
    rows: u16,
    content_width: u16,
    panes: &HashMap<PaneId, PtyPane>,
    inset: bool,
) -> Vec<PaneView> {
    focused_pane_views_with_chrome(layout, cols, rows, content_width, panes, inset, 0)
}

/// Logical dimensions depend on the column and terminal, never on focus or
/// viewport clipping. Rounded screen boundaries can give a column one fewer
/// visible cell at a different scroll stop. Resizing its PTY for that would
/// send SIGWINCH, whose redraw looks like fresh work to the activity heuristic.
pub(crate) fn column_grid_sizes(
    width: Width,
    pane_count: usize,
    viewport: GridSize,
    content_width: u16,
    inset: bool,
    chrome_rows: u16,
) -> impl Iterator<Item = GridSize> {
    let b = u16::from(inset);
    let cols = width
        .cells(viewport.cols)
        .saturating_sub(b)
        .max(content_width)
        .max(2);
    let inner_h = viewport
        .rows
        .saturating_sub(chrome_rows)
        .saturating_sub(2 * b)
        .max(1);
    let count = pane_count.max(1);
    // One shared divider between panes. Give remainder rows to the top panes,
    // matching the visible stack without leaving unassigned rows at the bottom.
    let avail = (inner_h as usize).saturating_sub(count - 1);
    (0..pane_count).map(move |i| GridSize {
        cols,
        rows: (avail / count + usize::from(i < avail % count)) as u16,
    })
}

pub(crate) fn focused_pane_views_with_chrome(
    layout: &Layout,
    cols: u16,
    rows: u16,
    content_width: u16,
    panes: &HashMap<PaneId, PtyPane>,
    inset: bool,
    chrome_rows: u16,
) -> Vec<PaneView> {
    let b: i32 = if inset { 1 } else { 0 }; // border thickness
    let abs_ranges = layout
        .column_x_ranges(layout.focus.row, cols)
        .unwrap_or_default();
    // Re-clamp the stored scroll against the *current* strip extent at paint
    // time. The layout clamps on every verb, but `scroll_x` can go stale
    // between verbs (e.g. the terminal was resized wider, shrinking the strip
    // relative to the viewport); trusting it verbatim would shift the strip
    // left and reveal background on the right until the next focus change.
    let total = abs_ranges.last().map(|r| r.1 as i32).unwrap_or(0);
    let max_scroll = (total - cols as i32).max(0);
    let scroll = layout
        .focused_row()
        .map(|r| r.scroll_x)
        .unwrap_or(0)
        .clamp(0, max_scroll);
    // Window-anchored ranges: boundary rounding re-anchored at the first
    // visible column, so every scroll stop paints uniform columns at
    // identical on-screen offsets (no 1-cell rounding-phase wobble).
    let ranges = layout
        .visible_column_x_ranges(layout.focus.row, cols, scroll)
        .unwrap_or_default();
    let mut out = Vec::new();
    for (ci, (s, e)) in ranges.into_iter().enumerate() {
        // Content spans the column box minus the frame ring. Neighbouring
        // column boxes *share* their boundary cell (see `FrameCanvas`), so
        // the right-hand frame of this box sits at `e` -- the next column's
        // left frame -- and content runs up to it. Only the final box, whose
        // right frame would fall off-screen at `cols`, pulls its frame (and
        // therefore its content) in by one.
        let cs = s + b;
        let ce = if b == 0 { e } else { e.min(cols as i32 - 1) };
        if ce <= cs {
            continue; // column too narrow to hold any content inside a frame
        }
        let sx = cs;
        let ex = ce;
        if ex <= 0 || sx >= cols as i32 {
            continue;
        }
        let left = sx.max(0) as u16;
        let right = (ex.min(cols as i32)) as u16;
        let raw_wv = right.saturating_sub(left); // visible width
        if raw_wv == 0 {
            continue;
        }
        let Some(col) = layout.focused_row().and_then(|r| r.columns.get(ci)) else {
            continue;
        };
        // Peek sliver: a neighbour clipped to fewer than MIN_VISIBLE columns
        // is not drawn as truncated text. Far neighbours are culled entirely;
        // the immediate neighbour of the focused column is kept as a 3-cell
        // faded hint so the user sees "something is off-screen" without
        // reading cut words. Only applies when the pane is actually clipped
        // (wv < full_w); small viewports where even a fully visible pane is
        // narrower than MIN must not be demoted to a peek.
        let focused = layout.focus.column;
        let is_neighbor = ci == focused.saturating_add(1) || (focused > 0 && ci + 1 == focused);
        // Never peek for the focused column itself.
        let is_focused_col = ci == focused;
        // Unclamped logical width for overflow detection: rightmost columns
        // extend past the viewport and ce is already clamped, so raw==full
        // would hide the squish. Compare against the true column width.
        let full_unclamped = (e - cs).max(0) as u16;
        // Only treat as squish if the column would normally be at least MIN
        // wide; tiny viewports where even a fully visible pane is <10 must not
        // be demoted to a peek.
        let (wv, peek_col) = if !is_focused_col
            && full_unclamped >= MIN_VISIBLE_PANE_WIDTH
            && raw_wv < full_unclamped
            && raw_wv < MIN_VISIBLE_PANE_WIDTH
        {
            if is_neighbor {
                // Clamp to peek width and ensure it still fits on screen.
                let peek = PEEK_SLIVER_WIDTH.min(raw_wv.max(1));
                (peek, true)
            } else {
                continue; // far clipped neighbour: cull
            }
        } else {
            (raw_wv, false)
        };
        // The PTY keeps the stable logical width even when boundary rounding
        // or the last column's right frame clips a cell from the visible view.
        let col_x0 = (left as i32 - sx).max(0) as u16; // grid col at `left`
        let gap = 1u16;
        let mut y = b as u16;
        let sizes = column_grid_sizes(
            col.width,
            col.panes.len(),
            GridSize { cols, rows },
            content_width,
            inset,
            chrome_rows,
        );
        for (pid, size) in col.panes.iter().zip(sizes) {
            let h = size.rows;
            let row_y = y;
            y = y.saturating_add(h).saturating_add(gap);
            if h == 0 {
                continue;
            }
            let h_scroll = panes.get(pid).map(|p| p.h_scroll).unwrap_or(0);
            out.push(PaneView {
                pid: *pid,
                col: ci,
                rect: Rect {
                    x: left,
                    y: row_y,
                    w: wv,
                    h,
                },
                col_x0,
                h_scroll,
                grid_cols: size.cols,
                grid_rows: size.rows,
                peek: peek_col,
            });
        }
    }
    out
}

/// The grid-column range `[start, end)` of a pane's content revealed by `w`
/// screen cells, given the viewport column offset `col_x0`, the pane content
/// scroll `h_scroll`, and the content width `grid_cols`. Returns `None` when
/// the window is fully clipped (offscreen or past the content).
pub(crate) fn pane_window(
    col_x0: u16,
    h_scroll: i32,
    w: u16,
    grid_cols: u16,
) -> Option<(u16, u16)> {
    let start = col_x0 as i32 + h_scroll;
    if start < 0 || start >= grid_cols as i32 {
        return None;
    }
    let start = start as u16;
    let end = (start + w).min(grid_cols);
    if end <= start {
        None
    } else {
        Some((start, end))
    }
}

/// Fallback message for a promoted image pane on a host without Kitty
/// graphics: centered lines explaining the pane is image-only and how to
/// get pixels. Painted over the blank cells the pane loop laid down, so
/// tiny rects degrade to nothing rather than a clipped fragment.
fn paint_image_fallback(out: &mut [Cell], cols: u16, rect: Rect, pal: &Palette) {
    const LINES: &[&str] = &[
        "image pane needs Kitty graphics",
        "run under kitty / ghostty, or set GWAE_KITTY_GRAPHICS=1",
    ];
    if rect.w < 10 || rect.h < 2 {
        return;
    }
    let mut wrapped: Vec<String> = Vec::new();
    for line in LINES {
        let mut cur = String::new();
        for word in line.split_whitespace() {
            if cur.is_empty() {
                cur.push_str(word);
            } else if cur.chars().count() + 1 + word.chars().count() <= rect.w as usize {
                cur.push(' ');
                cur.push_str(word);
            } else {
                wrapped.push(std::mem::take(&mut cur));
                cur.push_str(word);
            }
        }
        if !cur.is_empty() {
            wrapped.push(cur);
        }
    }
    if wrapped.iter().any(|l| l.chars().count() > rect.w as usize) {
        return;
    }
    let y0 = rect.y + rect.h.saturating_sub(wrapped.len() as u16) / 2;
    super::chrome::draw_art(
        out,
        cols,
        Rect {
            x: rect.x,
            y: y0,
            w: rect.w,
            h: rect.h.min(wrapped.len() as u16),
        },
        &wrapped,
        pal.label,
    );
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn render_frame(
    out: &mut Vec<Cell>,
    layout: &Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cols: u16,
    rows: u16,
    content_width: u16,
    pal: &Palette,
    mm: &crate::config::Minimap,
    selection: Option<&Selection<PaneId>>,
) {
    render_frame_with_images(
        out,
        layout,
        panes,
        cols,
        rows,
        content_width,
        pal,
        mm,
        selection,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_frame_with_images(
    out: &mut Vec<Cell>,
    layout: &Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cols: u16,
    rows: u16,
    content_width: u16,
    pal: &Palette,
    mm: &crate::config::Minimap,
    selection: Option<&Selection<PaneId>>,
    mut images: Option<&mut crate::graphics_host::Host>,
) {
    // Every chrome color in this function comes from the palette; the
    // skeleton frame color is just `pal.overlay`, kept in a local so the
    // `Option`-shaped call sites below read the same as they used to.
    let background = pal.base;
    let focus_color = pal.accent;
    out.clear();
    out.resize((cols as usize) * (rows as usize), Cell::default());
    // Paint the uncovered background first so any cell not overwritten by a
    // pane (the empty right side with fewer than four panes, gaps, and the
    // overflow tail past a pane's content) shows the configured color rather
    // than the terminal's default black. Pane cells are painted over this next,
    // and the focused-pane tint layers on top of default-bg pane cells, so
    // nothing here bleeds into a pane.
    for c in out.iter_mut() {
        c.style.bg = background;
    }

    let focused = focused_pane(layout);
    let mut focused_cursor_abs: Option<(u16, u16, bool)> = None; // (screen x,y, hide)
    let pane_views = focused_pane_views(layout, cols, rows, content_width, panes, true);
    // The ring follows the *layout*, not the pty: a pane whose process has not
    // been spawned (or has already exited) still occupies its rect and can
    // still be focused, so take the rect from the view list rather than from
    // the paint loop below, which skips panes with no live emulator.
    let focus_rect = pane_views
        .iter()
        .find(|v| Some(v.pid) == focused)
        .map(|v| v.rect);
    for v in &pane_views {
        let Some(pane) = panes.get_mut(&v.pid) else {
            continue;
        };
        let is_focus = focused == Some(v.pid);
        // The emulator size matches the visible content rect exactly: rects
        // are inset 1 cell inside the column frame so nothing a program draws
        // is ever covered.
        pane.grid.resize(GridSize {
            cols: v.grid_cols,
            rows: v.grid_rows,
        });
        let (g_start, g_end) = match pane_window(v.col_x0, v.h_scroll, v.rect.w, v.grid_cols) {
            Some(x) => x,
            None => {
                continue;
            }
        };
        let tiles = if !v.peek && pane.grid.scrollback_offset() == 0 {
            images
                .as_deref_mut()
                .map(|host| {
                    // Phase 1: idle panes (no image traffic, or unchanged
                    // image state) skip the source walk and reuse cached
                    // tiles. Only changed panes pay for prepare.
                    host.prepare_cached(
                        v.pid,
                        pane.image_activity,
                        &pane.graphics,
                        (g_start, 0, g_end - g_start, v.rect.h),
                        (
                            pane.pty_size.pixel_width / pane.pty_size.cols.max(1),
                            pane.pty_size.pixel_height / pane.pty_size.rows.max(1),
                        ),
                    )
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if is_focus {
            // Map the emulator cursor into screen coords, accounting for the
            // pane's content-window ([g_start, g_end) visible in rect).
            let (cur_row, cur_col) = pane.grid.cursor_position();
            let hide = pane.grid.hide_cursor();
            // When scrolled back from live, the cursor is off-screen history:
            // don't paint a stale block. Never paint the cursor on a peek
            // sliver: it would float at the very edge and suggest that tiny
            // remnant is the focused pane.
            let live = pane.grid.scrollback_offset() == 0 && !v.peek;
            if live {
                // Only paint when row is inside the visible rect and col inside the window.
                let in_window = cur_col >= g_start && cur_col < g_end;
                if in_window && cur_row < v.rect.h {
                    let gx = cur_col - g_start;
                    let sx = v.rect.x + gx;
                    let sy = v.rect.y + cur_row;
                    focused_cursor_abs = Some((sx, sy, hide));
                }
            }
        }
        // Paint every cell of the visible rect so nothing from the previous
        // frame bleeds through ("paint overflow"). When `pane_window` reveals
        // fewer columns than the rect is wide (a pane clipped at the content or
        // viewport edge), the uncovered tail is filled with blank cells, which
        // for the focused pane keeps the highlight a clean, unbroken rectangle.
        // Phase 2: a promoted image viewer paints only its image tiles.
        // Grid text (status lines, hidden TUI chrome) stays out of the frame
        // so stale glyphs never compete with the page texture. The PTY and
        // grid stay live underneath: demotion restores text with no redraw.
        //
        // Without a host image channel there are no tiles to paint. Say so
        // instead of leaving a blank rectangle: the viewer is still running
        // and taking input underneath, but its pixels have nowhere to go.
        let image_only = pane.image_view.is_some() && !v.peek;
        let image_blind = image_only && images.is_none();
        for gy in 0..v.rect.h {
            pane.legacy_images.begin_row();
            if images.is_some() && !v.peek && !image_only {
                for gx in 0..g_start {
                    pane.legacy_images.observe(pane.grid.cell(gx, gy));
                }
            }
            for gx in 0..v.rect.w {
                let idx = ((v.rect.y as usize + gy as usize) * cols as usize)
                    + (v.rect.x as usize + gx as usize);
                if idx >= out.len() {
                    continue;
                }
                let gi = g_start + gx;
                let mut cell = if image_only {
                    Cell::default()
                } else if gi < g_end {
                    pane.grid.cell(gi, gy)
                } else {
                    Cell::default()
                };
                if !v.peek {
                    if let Some(host) = images.as_deref_mut() {
                        cell = pane.legacy_images.cell(cell, host);
                    } else if cell.ch == crate::graphics_host::PLACEHOLDER {
                        cell = Cell {
                            style: cell.style,
                            ..Cell::default()
                        };
                    }
                    for tile in &tiles {
                        if let Some(overlay) = tile.cell(gy, gi, cell) {
                            cell = overlay;
                        }
                    }
                } else if cell.ch == crate::graphics_host::PLACEHOLDER {
                    cell = Cell {
                        style: cell.style,
                        ..Cell::default()
                    };
                }
                // A wide character clipped at an edge cannot be shown as half
                // a glyph: an orphaned continuation cell at the left edge, or
                // a wide head whose second column falls past the right edge,
                // is blanked so the glyph never spills into a neighbor.
                if (gx == 0 && cell.width == 0)
                    || (cell.width == 2 && (gx + 1 >= v.rect.w || gi + 1 >= g_end))
                {
                    cell = Cell {
                        style: cell.style,
                        ..Cell::default()
                    };
                }
                // Highlight a drag selection by inverting the cell, the same
                // affordance a terminal's own selection uses. Inversion (not a
                // fixed background) keeps every glyph readable whatever colors
                // the program inside the pane is using.
                // Never highlight selection on a peek sliver: it is a hint,
                // not a target, and inverting a 3-cell remnant hides the hint.
                if !v.peek
                    && selection
                        .map(|s| s.contains(v.pid, gi, gy))
                        .unwrap_or(false)
                {
                    cell.style.inverse = !cell.style.inverse;
                }
                // Dim the whole sliver to the skeleton palette so it reads as
                // "something off-screen" rather than broken text.
                if v.peek {
                    cell.style.fg = pal.overlay;
                    cell.style.bg = background;
                    cell.style.bold = false;
                    cell.style.underline = false;
                }
                // Mark the cut edge so a 3-cell hint never reads as real text.
                let is_edge = v.peek && gx + 1 == v.rect.w;
                if is_edge {
                    cell = Cell {
                        ch: '›',
                        style: gwae_term::Style {
                            fg: pal.overlay,
                            bg: background,
                            ..Default::default()
                        },
                        width: 1,
                        ..Default::default()
                    };
                }
                out[idx] = cell;
            }
            if image_blind {
                paint_image_fallback(out, cols, v.rect, pal);
            }
        }
    }
    // Skeleton: a 1-cell frame around every column box (full strip height) so
    // the container structure always reads, plus placeholder boxes tiling any
    // empty right side at the default quarter width. The focused column's box
    // is framed in the focus accent instead of the skeleton color.
    //
    // All of it goes through a single `FrameCanvas` rather than being stamped
    // box by box. Neighbouring boxes share their boundary column, so painting
    // them independently drew a double-thick border and let whichever box was
    // painted last own every shared cell -- which is why the focused column's
    // accent kept getting overwritten by its neighbour's dim line. The canvas
    // instead accumulates edge directions plus a priority per cell, so shared
    // boundaries render as one hairline with proper `├ ┤ ┬ ┴ ┼` junctions and
    // the focus color always wins.
    let mut canvas = FrameCanvas::new(cols, rows);
    // Priorities: plain chrome < focused column < focused pane.
    const P_CHROME: u8 = 1;
    const P_FOCUS_COL: u8 = 2;
    const P_FOCUS_PANE: u8 = 3;
    // A column holding a single pane *is* that pane, so framing the whole box
    // in the accent is the focus ring. Once the column is split, the box is a
    // container of several panes and only one of them has focus: highlighting
    // the container would claim focus for its siblings too. In that case the
    // column keeps plain chrome and the accent ring is drawn tight around the
    // focused split below (`P_FOCUS_PANE`).
    let focused_col_split = layout
        .focused_row()
        .and_then(|r| r.columns.get(layout.focus.column))
        .map(|c| c.panes.len() > 1)
        .unwrap_or(false);
    // Empty boxes tile the empty right side with bare frames so the grid
    // reads while strips fill. Their interiors stay blank: no identifiers,
    // no hints. The only key help is the `⌥+/` cheat-sheet.
    {
        let sk = pal.overlay;
        let inset: u16 = 1;
        let chrome = mm.chrome_rows();
        let strip_h = rows.saturating_sub(chrome).max(1);
        let abs_ranges = layout
            .column_x_ranges(layout.focus.row, cols)
            .unwrap_or_default();
        let total = abs_ranges.last().map(|r| r.1 as i32).unwrap_or(0);
        let max_scroll = (total - cols as i32).max(0);
        let scroll = layout
            .focused_row()
            .map(|r| r.scroll_x)
            .unwrap_or(0)
            .clamp(0, max_scroll);
        // Window-anchored ranges (see focused_pane_views): frames land on the
        // same on-screen boundaries at every scroll stop. Empty boxes come
        // from the *same* accumulator, so the grid never jitters as strips
        // fill or as you move between strips with different occupancy.
        let (ranges, live) = layout
            .visible_grid_x_ranges(layout.focus.row, cols, scroll, Width::DEFAULT)
            .unwrap_or_default();
        for (ci, (s, e)) in ranges.iter().enumerate() {
            let sx = *s;
            let ex = *e;
            if ex <= 0 || sx >= cols as i32 {
                continue;
            }
            let left = sx.max(0) as u16;
            // The right frame sits *on* the shared boundary with the next
            // column (clamped to the last on-screen cell), so two adjacent
            // boxes contribute to the same rule instead of two.
            let right_init = (ex.min(cols as i32 - 1)) as u16;
            if right_init <= left {
                continue;
            }
            let placeholder = ci >= live;
            // Keep frames consistent with pane cull/peek: a clipped non-focused
            // column narrower than MIN is culled unless it is the immediate
            // neighbour of focus where it becomes a 3-cell peek.
            let right = if placeholder {
                right_init
            } else {
                let box_full = (ex - sx) as u32;
                let box_raw = (right_init as i32 - left as i32 + 1) as u32;
                let clipped = box_raw < box_full;
                let content_raw = box_raw.saturating_sub(1);
                let is_focused_col = ci == layout.focus.column;
                if clipped && !is_focused_col && content_raw < MIN_VISIBLE_PANE_WIDTH as u32 {
                    let is_neighbor = ci == layout.focus.column.saturating_add(1)
                        || (layout.focus.column > 0 && ci + 1 == layout.focus.column);
                    if is_neighbor {
                        let peek_box = PEEK_SLIVER_WIDTH as u32 + 1;
                        left.saturating_add(peek_box as u16 - 1)
                            .min(cols.saturating_sub(1))
                    } else {
                        continue;
                    }
                } else {
                    right_init
                }
            };
            if right <= left {
                continue;
            }
            let (color, prio, bold) =
                if ci == layout.focus.column && (!focused_col_split || placeholder) {
                    (focus_color, P_FOCUS_COL, true)
                } else {
                    (sk, P_CHROME, false)
                };
            let boxr = Rect {
                x: left,
                y: 0,
                w: right - left + 1,
                h: strip_h,
            };
            if placeholder {
                // Interior only: the ring is painted from the canvas below,
                // and clearing it here would also wipe the left neighbour's
                // shared edge. A centered placeholder pal keeps the box from
                // reading as broken; tiny boxes stay blank.
                for y in inset..boxr.h.saturating_sub(inset) {
                    let row = (boxr.y + y) as usize * cols as usize;
                    for x in inset..boxr.w.saturating_sub(inset) {
                        if let Some(c) = out.get_mut(row + (boxr.x + x) as usize) {
                            *c = Cell::default();
                            c.style.bg = background;
                        }
                    }
                }
                super::empty_art::paint(out, cols, boxr, ci, sk, background);
                canvas.rect(boxr, color, prio, bold);
                continue;
            }
            canvas.rect(boxr, color, prio, bold);
            // Stacked panes: the 1-cell gap between two panes of a column is
            // a shared horizontal rule that tees into the column's verticals,
            // so a stack reads as one subdivided container.
            if let Some(col) = layout.focused_row().and_then(|r| r.columns.get(ci)) {
                if col.panes.len() > 1 {
                    for v in pane_views.iter().filter(|v| v.col == ci).skip(1) {
                        canvas.hline(
                            left as i32,
                            right as i32,
                            v.rect.y as i32 - 1,
                            color,
                            prio,
                            bold,
                        );
                    }
                }
            }
        }
    }
    // The focused *column's* frame is already the accent color and content is
    // inset, so an overlay would only cover content. For a stacked column the
    // container frame stays chrome, so promote the focused pane's own ring to
    // the accent in the canvas, where it merges with the column frame instead
    // of stamping over it.
    match focus_rect {
        Some(rect) if focused_col_split => {
            {
                // Grow the rect by 1 so the ring lands on the frame/gap cells
                // around the pane, not on its content.
                let x = rect.x.saturating_sub(1);
                let y = rect.y.saturating_sub(1);
                let w = (rect.w + 2).min(cols.saturating_sub(x));
                let h = (rect.h + 2).min(rows.saturating_sub(y));
                canvas.rect(Rect { x, y, w, h }, focus_color, P_FOCUS_PANE, true);
            }
        }
        _ => {}
    }
    if !canvas.is_empty() {
        canvas.flush(out);
    }
    // Chrome dispatch: overlay / edge ticks only. The bottom reserved row has
    // been removed; status is via the centered Alt HUD/minimap (drawn in
    // run_tui) and legacy overlay/edge_ticks modes here.
    match mm.mode {
        crate::config::MinimapMode::Overlay => {
            draw_minimap(out, cols, rows, layout, mm, pal);
        }
        crate::config::MinimapMode::EdgeTicks => {
            draw_edge_ticks(out, cols, rows, layout, mm, pal);
        }
        crate::config::MinimapMode::Off => {}
    }
    // Paint the focused pane's text cursor as a kitty-style block: inverse
    // video on top of the pane's own cell so it reads exactly like the native
    // terminal cursor. Only when the emulator's cursor is visible and live
    // (not scrolled back) and not covered by chrome.
    if let Some((sx, sy, hide)) = focused_cursor_abs {
        if !hide && sy < rows {
            let idx = sy as usize * cols as usize + sx as usize;
            if let Some(c) = out.get_mut(idx) {
                // Don't inverse the frame ring glyphs (they are the hairline).
                // The cursor is always inside the pane rect; if we hit a frame
                // glyph it's a stacked-gap case — just leave it.
                let is_frame = matches!(c.ch, '╭' | '╮' | '╰' | '╯' | '─' | '│');
                if !is_frame {
                    c.style.inverse = !c.style.inverse;
                }
            }
        }
    }
}

/// Write one frame glyph into `out[idx]` in `color` on a default background.
///
/// Overwriting half of a wide (2-col) character would orphan its other half,
/// so the partner cell is blanked: a wide head loses its continuation, a
/// continuation loses its head.
fn put_frame_cell(out: &mut [Cell], idx: usize, ch: char, color: CColor, bold: bool) {
    let Some(c) = out.get(idx).copied() else {
        return;
    };
    if c.width == 2 {
        if let Some(n) = out.get_mut(idx + 1) {
            if n.width == 0 {
                *n = Cell {
                    style: n.style,
                    ..Cell::default()
                };
            }
        }
    } else if c.width == 0 && idx > 0 {
        if let Some(p) = out.get_mut(idx - 1) {
            if p.width == 2 {
                *p = Cell {
                    style: p.style,
                    ..Cell::default()
                };
            }
        }
    }
    let cell = &mut out[idx];
    cell.ch = ch;
    cell.width = 1;
    cell.style.fg = color;
    cell.style.bg = CColor::Default;
    cell.style.bold = bold;
    cell.style.underline = false;
    cell.style.inverse = false;
}

/// A frame accumulator that merges *shared* box edges into single hairlines.
///
/// Column boxes in a strip tile edge to edge, so neighbours would otherwise
/// each paint their own vertical rule in adjacent cells (a 2-cell-thick
/// double border) and whichever box was drawn last would win on any cell they
/// did share, making the focused column's accent flicker under a neighbour's
/// dim line. Instead every rect contributes *edge bits* to the cells it
/// touches, plus a color at a priority; the flush pass then picks the one
/// box-drawing glyph that matches the accumulated bits (corners, tees and
/// crosses included) and the highest-priority color. Adjacent columns share
/// the boundary cell, stacked panes join it with `├`/`┤`, and focus always
/// wins the color of a shared edge regardless of paint order.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FrameEdge {
    mask: u8, // N=1, E=2, S=4, W=8
    prio: u8,
}

struct FrameCanvas {
    cols: u16,
    rows: u16,
    edges: Vec<FrameEdge>,
    colors: Vec<CColor>,
    bolds: Vec<bool>,
}

const EDGE_N: u8 = 1;
const EDGE_E: u8 = 2;
const EDGE_S: u8 = 4;
const EDGE_W: u8 = 8;

impl FrameCanvas {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            edges: vec![FrameEdge { mask: 0, prio: 0 }; cols as usize * rows as usize],
            colors: vec![CColor::Default; cols as usize * rows as usize],
            bolds: vec![false; cols as usize * rows as usize],
        }
    }

    fn is_empty(&self) -> bool {
        self.edges.iter().all(|e| e.mask == 0)
    }

    fn add(&mut self, x: i32, y: i32, mask: u8, color: CColor, prio: u8, bold: bool) {
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return;
        }
        let idx = y as usize * self.cols as usize + x as usize;
        let e = &mut self.edges[idx];
        e.mask |= mask;
        // Highest priority wins the color and the weight; ties keep the
        // first writer so a repaint of the same box is idempotent. The
        // focus ring is bold white while plain chrome stays regular, so
        // focus reads even where color alone would not carry it.
        if prio > e.prio {
            e.prio = prio;
            self.colors[idx] = color;
            self.bolds[idx] = bold;
        }
    }

    /// A vertical rule from `y0` to `y1` (inclusive) at column `x`.
    fn vline(&mut self, x: i32, y0: i32, y1: i32, color: CColor, prio: u8, bold: bool) {
        for y in y0..=y1 {
            let mut m = EDGE_N | EDGE_S;
            if y == y0 {
                m &= !EDGE_N;
            }
            if y == y1 {
                m &= !EDGE_S;
            }
            // A 1-cell rule still has to render as something: keep it vertical.
            if m == 0 {
                m = EDGE_N | EDGE_S;
            }
            self.add(x, y, m, color, prio, bold);
        }
    }

    /// A horizontal rule from `x0` to `x1` (inclusive) at row `y`.
    fn hline(&mut self, x0: i32, x1: i32, y: i32, color: CColor, prio: u8, bold: bool) {
        for x in x0..=x1 {
            let mut m = EDGE_E | EDGE_W;
            if x == x0 {
                m &= !EDGE_W;
            }
            if x == x1 {
                m &= !EDGE_E;
            }
            if m == 0 {
                m = EDGE_E | EDGE_W;
            }
            self.add(x, y, m, color, prio, bold);
        }
    }

    /// The ring of `rect`, as four rules that join at the corners.
    fn rect(&mut self, rect: Rect, color: CColor, prio: u8, bold: bool) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let x0 = rect.x as i32;
        let y0 = rect.y as i32;
        let x1 = x0 + rect.w as i32 - 1;
        let y1 = y0 + rect.h as i32 - 1;
        if y1 > y0 {
            self.vline(x0, y0, y1, color, prio, bold);
            if x1 > x0 {
                self.vline(x1, y0, y1, color, prio, bold);
            }
        }
        if x1 > x0 {
            self.hline(x0, x1, y0, color, prio, bold);
            if y1 > y0 {
                self.hline(x0, x1, y1, color, prio, bold);
            }
        }
        if x1 == x0 && y1 == y0 {
            self.add(x0, y0, EDGE_E | EDGE_W, color, prio, bold);
        }
    }

    /// The glyph for an accumulated edge mask. Pure corners are rounded, to
    /// match the rest of the chrome; junctions use tees and a cross.
    fn glyph(mask: u8) -> Option<char> {
        Some(match mask {
            0 => return None,
            m if m == EDGE_N | EDGE_E | EDGE_S | EDGE_W => '┼',
            m if m == EDGE_N | EDGE_E | EDGE_S => '├',
            m if m == EDGE_N | EDGE_S | EDGE_W => '┤',
            m if m == EDGE_E | EDGE_S | EDGE_W => '┬',
            m if m == EDGE_N | EDGE_E | EDGE_W => '┴',
            m if m == EDGE_E | EDGE_S => '╭',
            m if m == EDGE_S | EDGE_W => '╮',
            m if m == EDGE_N | EDGE_E => '╰',
            m if m == EDGE_N | EDGE_W => '╯',
            m if m == EDGE_N | EDGE_S => '│',
            m if m == EDGE_E | EDGE_W => '─',
            m if m & (EDGE_N | EDGE_S) != 0 => '│',
            _ => '─',
        })
    }

    /// Write the merged frame into the cell buffer.
    fn flush(&self, out: &mut [Cell]) {
        for y in 0..self.rows {
            for x in 0..self.cols {
                let idx = y as usize * self.cols as usize + x as usize;
                let Some(ch) = Self::glyph(self.edges[idx].mask) else {
                    continue;
                };
                put_frame_cell(out, idx, ch, self.colors[idx], self.bolds[idx]);
            }
        }
    }
}

/// Overlay a thin frame on the edge ring of `rect`: box-drawing glyphs
/// (`╭─╮│╰╯`) with `color` as the foreground, on a default background. The
/// previous implementation preserved the underlying `background` fill (e.g.
/// `Idx(235)`), which left a dim gray slab behind the thin focus glyph.
/// Resetting the ring cells to `Default` makes the hairline float on the same
/// background as pane interiors and placeholder boxes, so only the glyph
/// remains. Focus rings pass `bold` so the ring reads even where color
/// alone would not carry it; plain chrome passes `false`.
pub(crate) fn draw_focus_frame(out: &mut [Cell], cols: u16, rect: Rect, color: CColor, bold: bool) {
    let stride = cols as usize;
    let w = rect.w as usize;
    let h = rect.h as usize;
    let x0 = rect.x as usize;
    let y0 = rect.y as usize;
    let x1 = x0 + w - 1;
    let y1 = y0 + h - 1;
    // Replace a cell with a frame glyph. Overwriting half of a wide (2-col)
    // character would orphan its other half, so the partner cell is blanked:
    // a wide head loses its continuation, a continuation loses its head.
    let put = |out: &mut [Cell], idx: usize, ch: char, color: CColor| {
        put_frame_cell(out, idx, ch, color, bold)
    };
    if h == 1 {
        for x in x0..=x1 {
            put(out, y0 * stride + x, '─', color);
        }
        return;
    }
    if w == 1 {
        for y in y0..=y1 {
            put(out, y * stride + x0, '│', color);
        }
        return;
    }
    // Top and bottom rows.
    for x in (x0 + 1)..x1 {
        put(out, y0 * stride + x, '─', color);
        put(out, y1 * stride + x, '─', color);
    }
    // Left and right columns.
    for y in (y0 + 1)..y1 {
        put(out, y * stride + x0, '│', color);
        put(out, y * stride + x1, '│', color);
    }
    // Rounded corners.
    put(out, y0 * stride + x0, '╭', color);
    put(out, y0 * stride + x1, '╮', color);
    put(out, y1 * stride + x0, '╰', color);
    put(out, y1 * stride + x1, '╯', color);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::theme::Palette;
    use gwae_layout::{Action, FollowScroll, Layout, PaneId, Viewport};
    use gwae_term::{CColor, Cell, Vt100Grid};
    use std::collections::{HashMap, HashSet};

    /// A palette with a distinctive accent and RGB status tints. Render
    /// tests assert on the accent to prove focus chrome is drawn and on
    /// full-intensity status tints for minimap tiles; indexed colors normalize to
    /// the terminal default through `tile_colors`, so the statuses stay
    /// RGB here to keep exercising the tint path.
    pub(crate) fn pal_accent(accent: CColor) -> Palette {
        Palette {
            accent,
            running: CColor::Rgb(0x89, 0xb4, 0xfa),
            idle: CColor::Rgb(0xfa, 0xb3, 0x87),
            failed: CColor::Rgb(0xf3, 0x8b, 0xa8),
            ..Palette::default()
        }
    }

    /// A palette built from the explicit colors a pre-theme render test used
    /// to pass positionally: background, focus accent, and skeleton overlay.
    /// Statuses stay RGB so minimap tint tests keep exercising the tint path
    /// (indexed colors normalize to the terminal default).
    pub(crate) fn pal_of(base: CColor, accent: CColor, overlay: CColor) -> Palette {
        Palette {
            base,
            accent,
            overlay,
            label: CColor::Rgb(0x58, 0x5b, 0x70),
            running: CColor::Rgb(0x89, 0xb4, 0xfa),
            idle: CColor::Rgb(0xfa, 0xb3, 0x87),
            failed: CColor::Rgb(0xf3, 0x8b, 0xa8),
            ..Palette::default()
        }
    }

    pub(crate) fn no_map() -> crate::config::Minimap {
        crate::config::Minimap {
            show: false,
            mode: crate::config::MinimapMode::Overlay,
            ..Default::default()
        }
    }

    /// Promoted image pane without a host image channel: the frame must say
    /// why the pane is blank instead of leaving an empty rectangle. With a
    /// channel the same pane paints tiles and no message.
    #[test]
    fn promoted_pane_without_host_images_explains_itself() {
        use crate::tui::pty::{feed_pane_output, PaneIo, PaneProc, PtyPane};
        use gwae_layout::Width;
        let mut layout = Layout::new(1);
        // Full width: the message needs room (rect.w >= 10); the default
        // quarter-width box at 80 cols is too narrow to hold it.
        // Full-width column holding a fresh pane id: Layout::new leaves
        // the strip empty after clearing, so allocate after rebuilding.
        let row = layout.focus.row;
        if let Some(r) = layout.row_mut(row) {
            r.columns.clear();
        }
        let pid = layout.alloc_pane();
        layout.add_column(row, Width::Cells(80), vec![pid]);
        let mut grid = gwae_term::Vt100Grid::new(gwae_term::Size { cols: 40, rows: 12 });
        grid.set_cell_size(8, 16);
        let mut pane = PtyPane {
            master: PaneIo::Inherited(-1),
            writer: Box::new(std::io::sink()),
            child: PaneProc::Adopted(None),
            grid,
            pty_size: crate::geometry::CellPixels {
                width: 8,
                height: 16,
            }
            .pty_size(40, 12),
            alive: true,
            h_scroll: 0,
            last_output: std::time::Instant::now(),
            saw_osc133: false,
            graphics_stream: Default::default(),
            graphics: Default::default(),
            legacy_images: Default::default(),
            image_activity: None,
            image_view: None,
            promote_streak: 0,
        };
        // Two 1x1 native commits in separate feeds (the shape the committed
        // promotion tests prove promotes; APC-only feeds do not bump the
        // grid epoch, so the streak survives across them).
        feed_pane_output(
            &mut pane,
            b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
            true,
            true,
        );
        feed_pane_output(
            &mut pane,
            b"\x1b_Ga=T,i=8,f=24,s=1,v=1,C=1;AQID\x1b\\",
            true,
            true,
        );
        assert!(pane.image_view.is_some(), "fixture must promote the pane");
        let mut panes = HashMap::from([(pid, pane)]);
        // Blind: no host image channel. Message appears, no placeholders.
        let mut out = Vec::new();
        render_frame_with_images(
            &mut out,
            &layout,
            &mut panes,
            80,
            24,
            0,
            &Palette::default(),
            &no_map(),
            None,
            None,
        );
        let text: String = out.iter().map(|c| c.ch).collect();
        assert!(
            text.contains("needs Kitty graphics"),
            "blind promoted pane must explain itself"
        );
        assert!(
            !out.iter()
                .any(|c| c.ch == crate::graphics_host::PLACEHOLDER),
            "blind pane must not leak placeholders"
        );
        // Sighted: the same promoted pane with a channel paints tiles.
        let mut host = crate::graphics_host::Host::default();
        host.begin();
        let mut out2 = Vec::new();
        render_frame_with_images(
            &mut out2,
            &layout,
            &mut panes,
            80,
            24,
            0,
            &Palette::default(),
            &no_map(),
            None,
            Some(&mut host),
        );
        host.finish();
        assert!(
            out2.iter()
                .any(|c| c.ch == crate::graphics_host::PLACEHOLDER),
            "sighted promoted pane must paint image tiles"
        );
        let text2: String = out2.iter().map(|c| c.ch).collect();
        assert!(
            !text2.contains("needs Kitty graphics"),
            "sighted pane must not show the fallback"
        );
    }

    /// Render a 2-column layout and return the placeholder box region as text,
    /// one string per screen row.
    pub(crate) fn placeholder_rows(cols: u16, rows: u16) -> Vec<String> {
        let layout = Layout::new(2); // boxes 3 and 4 are placeholders
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(
                CColor::Idx(235),
                CColor::Rgb(0xff, 0, 0),
                CColor::Rgb(0xff, 0xff, 0xff),
            ),
            &no_map(),
            None,
        );
        (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| out[y as usize * cols as usize + x as usize].ch)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn pane_window_shows_leading_content() {
        // 240-col content, 80-col rect, no scroll: reveal [0, 80).
        assert_eq!(pane_window(0, 0, 80, 240), Some((0, 80)));
    }

    #[test]
    fn pane_scroll_reveals_overflow() {
        // Scrolling 10 cells pans the window right within the content.
        assert_eq!(pane_window(0, 10, 80, 240), Some((10, 90)));
    }

    #[test]
    fn pane_scroll_clamps_to_content_end() {
        // Scrolling near the end reveals a partial (clipped) window.
        assert_eq!(pane_window(0, 200, 80, 240), Some((200, 240)));
    }

    #[test]
    fn pane_scroll_beyond_content_is_clipped() {
        // Scrolling past the content yields nothing.
        assert_eq!(pane_window(0, 250, 80, 240), None);
    }

    #[test]
    fn offscreen_column_is_clipped() {
        // A column fully left of the viewport (col_x0 negative equivalent:
        // h_scroll cannot hold it, but a col offset past content is clipped).
        assert_eq!(pane_window(240, 0, 80, 240), None);
    }

    /// A vertical split must tile the whole strip no matter how many panes
    /// are in the stack. Floor-dividing the inner height stranded
    /// `inner_h % p` rows at the bottom, which showed up as unpainted
    /// background from ~7 panes down (the first count where the remainder
    /// exceeds a cell on a typical strip).
    #[test]
    fn a_vertical_stack_tiles_the_full_strip_at_any_pane_count() {
        use gwae_layout::{Preset, Width};
        let panes_map = HashMap::new();
        let (cols, rows) = (80u16, 40u16);
        for p in 1..=12usize {
            let mut layout = Layout::new(1);
            if let Some(r) = layout.row_mut(layout.focus.row) {
                r.columns.clear();
            }
            let row = layout.focus.row;
            let ids: Vec<_> = (0..p).map(|_| layout.alloc_pane()).collect();
            layout.add_column(row, Width::Preset(Preset::Full), ids);
            for inset in [false, true] {
                let views = focused_pane_views(&layout, cols, rows, 0, &panes_map, inset);
                assert_eq!(views.len(), p, "{p} panes, inset={inset}");
                let b = inset as u16;
                let inner_top = b;
                let inner_bottom = rows - b;
                assert_eq!(views[0].rect.y, inner_top, "{p} panes: stack starts at top");
                // Panes tile with exactly one gap row between them...
                for w in views.windows(2) {
                    assert_eq!(
                        w[1].rect.y,
                        w[0].rect.y + w[0].rect.h + 1,
                        "{p} panes, inset={inset}: one gap row between panes"
                    );
                }
                // ...and the last pane reaches the bottom of the strip, so no
                // row is left unassigned.
                let last = views.last().unwrap();
                assert_eq!(
                    last.rect.y + last.rect.h,
                    inner_bottom,
                    "{p} panes, inset={inset}: stack reaches the bottom"
                );
                // Heights stay balanced: at most one row apart.
                let hs: Vec<u16> = views.iter().map(|v| v.rect.h).collect();
                let (lo, hi) = (*hs.iter().min().unwrap(), *hs.iter().max().unwrap());
                assert!(hi - lo <= 1, "{p} panes: heights {hs:?} are balanced");
                // The emulator grid matches the painted rect.
                for v in &views {
                    assert_eq!(v.grid_rows, v.rect.h);
                }
            }
        }
    }

    #[test]
    fn four_quarter_panes_fill_screen_without_overflow() {
        // Regression for the reported bug: at 342 cols (not divisible by 4),
        // per-column ceil widths summed to 344 and the 4th pane's rect ran
        // past the right edge. Exercise the real render path: layout ->
        // focused_pane_views -> on-screen Rects.
        use gwae_layout::{Preset, Width};
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        for _ in 0..4 {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
        }
        let panes = HashMap::new();
        for cols in [342u16, 341, 343, 80, 81] {
            let views = focused_pane_views(&layout, cols, 40, 0, &panes, false);
            assert_eq!(views.len(), 4, "all four panes visible at cols={cols}");
            // Panes tile the full width: start at 0, no gaps, end at the edge.
            assert_eq!(views[0].rect.x, 0);
            for w in views.windows(2) {
                assert_eq!(
                    w[0].rect.x + w[0].rect.w,
                    w[1].rect.x,
                    "gap/overlap between panes at cols={cols}"
                );
            }
            let last = views.last().unwrap();
            assert_eq!(
                last.rect.x + last.rect.w,
                cols,
                "rightmost pane must end exactly at the screen edge at cols={cols}"
            );
        }
    }

    #[test]
    fn widened_last_pane_overflows_instead_of_shrinking() {
        // Widening pane 1 to half keeps its 40-cell grid (the frame shares
        // the boundary with pane 2, so content overflows past the edge).
        // Widening pane 4 must behave the same: its grid keeps the full
        // logical width even though its right frame pulls in by one to stay
        // on screen. Before the fix the grid was clamped to the visible
        // rect, so pane 4 shrank while pane 1 overflowed.
        use gwae_layout::{Preset, Width};
        let panes = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 10;
        let mut widths = HashMap::new();
        for focus_col in [0usize, 3usize] {
            let mut layout = Layout::new(1);
            if let Some(r) = layout.row_mut(layout.focus.row) {
                r.columns.clear();
            }
            let row = layout.focus.row;
            for _ in 0..4 {
                let p = layout.alloc_pane();
                layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
            }
            layout.focus.column = focus_col;
            // Widen the focused column to half (quarter -> third -> half).
            let vp = gwae_layout::Viewport::new(cols);
            for _ in 0..2 {
                let _ = layout.apply(gwae_layout::Action::CycleWidth, vp, FollowScroll::default());
            }
            let views = focused_pane_views(&layout, cols, rows, 0, &panes, true);
            let v = views.iter().find(|v| v.col == focus_col).unwrap();
            widths.insert(focus_col, v.grid_cols);
        }
        assert_eq!(
            widths[&0], widths[&3],
            "pane 1 and pane 4 must keep the same grid width when widened"
        );
        // Half of 80 is 40; minus the 1-cell left frame inset (the right
        // frame is shared with the neighbour, so content runs up to it).
        assert_eq!(widths[&3], 40 - 1, "widened pane keeps its logical width");
    }

    #[test]
    fn peek_sliver_replaces_squished_neighbour_with_a_faded_hint() {
        use gwae_layout::{Preset, Viewport, Width};
        // Force a sliver by placing scroll between stops: immediate neighbour
        // clipped to < MIN must become a 3-cell peek, far ones culled.
        // Layout: 5 quarters at 120 => each 30, total 150, max 30.
        // Focus at col 2 (middle). Set scroll to 55 so col 1 (neighbour) shows
        // only 5 cells at the left edge => peek, col 0 far => culled.
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        for _ in 0..5 {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
        }
        layout.focus.column = 2;
        // Widen focused to Half to grow the strip (30->60).
        let vp = Viewport::new(120);
        let _ = layout.apply(gwae_layout::Action::CycleWidth, vp, FollowScroll::default());
        let _ = layout.apply(gwae_layout::Action::CycleWidth, vp, FollowScroll::default());
        // Manually place scroll between stops to create a 5-cell sliver
        // at the left edge for the neighbour.
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.scroll_x = 55;
        }
        let panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let views = focused_pane_views(&layout, 120, 30, 0, &panes, true);
        let focused_pid = layout.focused_pane_id().unwrap();
        let fv = views.iter().find(|v| v.pid == focused_pid).unwrap();
        assert!(!fv.peek, "focused pane must not be a peek");
        let mut saw_peek = false;
        for v in &views {
            if v.peek {
                saw_peek = true;
                assert_eq!(v.rect.w, PEEK_SLIVER_WIDTH, "peek is exactly 3 cols");
                assert_ne!(v.pid, focused_pid);
            } else if v.pid != focused_pid {
                // Culled far panes are simply not in views; visible non-peeks >= MIN.
                assert!(
                    v.rect.w >= MIN_VISIBLE_PANE_WIDTH || v.rect.w == 0,
                    "non-peek neighbour narrow: {}",
                    v.rect.w
                );
            }
        }
        assert!(
            saw_peek,
            "expected at least one peek sliver at off-stop scroll"
        );
        // Small viewport where even a fully visible pane is narrow must not peek.
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.scroll_x = 0;
        }
        let views_narrow = focused_pane_views(&layout, 30, 30, 0, &panes, true);
        for v in &views_narrow {
            if v.peek {
                panic!("narrow viewport incorrectly produced a peek: {:?}", v);
            }
        }
    }

    #[test]
    fn draw_focus_frame_rings_the_rect() {
        // 5x5 grid; frame the 3x3 rect at (1,1) -> rows 1..=3, cols 1..=3.
        let mut out = vec![
            Cell {
                ch: '.',
                ..Cell::default()
            };
            25
        ];
        let accent = CColor::Idx(36);
        draw_focus_frame(
            &mut out,
            5,
            Rect {
                x: 1,
                y: 1,
                w: 3,
                h: 3,
            },
            accent,
            true,
        );
        let cell = |x: usize, y: usize| out[y * 5 + x];
        // Ring edge carries thin accent-colored glyphs; bg is untouched.
        assert_eq!(cell(1, 1).ch, '╭');
        assert_eq!(cell(3, 1).ch, '╮');
        assert_eq!(cell(1, 3).ch, '╰');
        assert_eq!(cell(3, 3).ch, '╯');
        assert_eq!(cell(2, 1).ch, '─', "top edge");
        assert_eq!(cell(2, 3).ch, '─', "bottom edge");
        assert_eq!(cell(1, 2).ch, '│', "left edge");
        assert_eq!(cell(3, 2).ch, '│', "right edge");
        for (x, y) in [(1, 1), (2, 1), (1, 2), (3, 2), (2, 3), (3, 3)] {
            assert_eq!(cell(x, y).style.fg, accent, "fg accent at ({x},{y})");
            assert!(cell(x, y).style.bold, "focus ring is bold at ({x},{y})");
            assert_eq!(
                cell(x, y).style.bg,
                CColor::Default,
                "bg untouched at ({x},{y})"
            );
        }
        // Interior center: unchanged.
        assert_eq!(cell(2, 2).ch, '.');
        assert_eq!(cell(2, 2).style.fg, CColor::Default);
        // Outside the rect: unchanged.
        assert_eq!(cell(0, 0).ch, '.');
        assert_eq!(cell(0, 2).ch, '.');
    }

    #[test]
    fn draw_focus_frame_single_cell_rect() {
        let mut out = vec![Cell::default(); 1];
        draw_focus_frame(
            &mut out,
            1,
            Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            },
            CColor::Idx(1),
            true,
        );
        // A degenerate 1x1 rect degrades to a horizontal rule glyph.
        assert_eq!(out[0].ch, '─');
        assert_eq!(out[0].style.fg, CColor::Idx(1));
    }

    #[test]
    fn focus_ring_tracks_the_focused_split_not_the_whole_column() {
        // A split column is a *container*: focus belongs to one of its panes,
        // so the accent must ring only that pane. The container's own outer
        // frame stays plain chrome, otherwise the unfocused sibling looks
        // focused too.
        let mut layout = Layout::new(1);
        let row = layout.focus.row;
        let a = layout.alloc_pane();
        let b = layout.alloc_pane();
        layout.add_column(row, gwae_layout::Width::Cells(20), vec![a, b]);
        layout.focus.column = layout.focused_row().unwrap().columns.len() - 1;
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let (cols, rows) = (80u16, 14u16);
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let red = CColor::Rgb(0xff, 0, 0);
        let render = |layout: &Layout, panes: &mut HashMap<PaneId, PtyPane>| {
            let mut out = Vec::new();
            render_frame(
                &mut out,
                layout,
                panes,
                cols,
                rows,
                0,
                &pal_of(CColor::Default, red, white),
                &no_map(),
                None,
            );
            out
        };
        let ranges = layout.column_x_ranges(row, cols).unwrap();
        let (cs, ce) = ranges[layout.focus.column];
        let (cs, ce) = (cs as u16, (ce as u16).min(cols - 1));
        let views = focused_pane_views(&layout, cols, rows, 0, &panes, true);
        let stacked: Vec<&PaneView> = views
            .iter()
            .filter(|v| v.col == layout.focus.column)
            .collect();
        assert_eq!(stacked.len(), 2);
        let divider_y = stacked[1].rect.y - 1;
        let top_mid_y = divider_y / 2;
        let bot_mid_y = divider_y + (rows - 1 - divider_y) / 2;

        for (pane_idx, own_y, other_y) in
            [(0usize, top_mid_y, bot_mid_y), (1, bot_mid_y, top_mid_y)]
        {
            layout.focus.pane = pane_idx;
            let out = render(&layout, &mut panes);
            let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
            // The focused split's own side edges carry the accent...
            assert_eq!(at(cs, own_y).style.fg, red, "focused split left edge");
            assert_eq!(at(ce, own_y).style.fg, red, "focused split right edge");
            // ...while the sibling half of the same container does not.
            assert_eq!(at(cs, other_y).style.fg, white, "sibling split left edge");
            assert_eq!(at(ce, other_y).style.fg, white, "sibling split right edge");
        }

        // Collapsing the split back to a single pane restores the whole-column
        // ring: there the column and the pane are the same thing.
        let mut single = Layout::new(1);
        let row = single.focus.row;
        let p = single.alloc_pane();
        single.add_column(row, gwae_layout::Width::Cells(20), vec![p]);
        single.focus.column = single.focused_row().unwrap().columns.len() - 1;
        single.focus.pane = 0;
        let out = render(&single, &mut panes);
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        let ranges = single.column_x_ranges(row, cols).unwrap();
        let (cs, ce) = ranges[single.focus.column];
        let (cs, ce) = (cs as u16, (ce as u16).min(cols - 1));
        assert_eq!(
            at(cs, rows / 2).style.fg,
            red,
            "unsplit column rings accent"
        );
        assert_eq!(
            at(ce, rows / 2).style.fg,
            red,
            "unsplit column rings accent"
        );
    }

    #[test]
    fn shared_column_edges_are_a_single_hairline_owned_by_focus() {
        // Regression: every column box used to stamp its own ring, so two
        // adjacent columns painted two rules one cell apart (a double border)
        // and the last box painted won any cell they shared -- which made the
        // focused column's accent edge disappear under its neighbour's dim
        // line whenever focus sat left of another column. Now the strip is
        // one merged grid: shared boundaries are a single rule and the focus
        // color outranks plain chrome regardless of paint order.
        let mut layout = Layout::default();
        layout.focus.column = 1; // focus between two unfocused neighbours
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 10;
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let red = CColor::Rgb(0xff, 0, 0);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(CColor::Default, red, white),
            &no_map(),
            None,
        );
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        let (fs, fe) = (ranges[1].0 as u16, ranges[1].1 as u16);
        // Both edges of the focused column, including the two it shares with
        // its unfocused neighbours, carry the accent for the full height.
        for y in 0..rows {
            assert_eq!(at(fs, y).style.fg, red, "left shared edge at y={y}");
            assert_eq!(at(fe, y).style.fg, red, "right shared edge at y={y}");
        }
        // One rule per boundary: the cells on either side are interior.
        for x in [fs - 1, fs + 1, fe - 1, fe + 1] {
            assert_eq!(at(x, rows / 2).ch, ' ', "double border at x={x}");
        }
        // Every frame cell in the strip is a box-drawing glyph, never a mix
        // of overlapping partial rings.
        for y in 0..rows {
            for x in [ranges[0].0 as u16, fs, fe, cols - 1] {
                assert!(
                    matches!(
                        at(x, y).ch,
                        '╭' | '╮' | '╰' | '╯' | '─' | '│' | '├' | '┤' | '┬' | '┴' | '┼'
                    ),
                    "non-frame glyph {:?} at ({x},{y})",
                    at(x, y).ch
                );
            }
        }
    }

    #[test]
    fn stacked_panes_share_a_teed_divider_with_the_column_frame() {
        // A column holding two panes is one container subdivided once: the
        // divider between the panes is a horizontal rule that *tees* into the
        // column's vertical edges, rather than a free-floating focus ring
        // stamped over the frame.
        let mut layout = Layout::new(1);
        let row = layout.focus.row;
        let a = layout.alloc_pane();
        let b = layout.alloc_pane();
        layout.add_column(row, gwae_layout::Width::Cells(20), vec![a, b]);
        layout.focus.column = layout.focused_row().unwrap().columns.len() - 1;
        layout.focus.pane = 0;
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 14;
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let red = CColor::Rgb(0xff, 0, 0);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(CColor::Default, red, white),
            &no_map(),
            None,
        );
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        let views = focused_pane_views(&layout, cols, rows, 0, &panes, true);
        let stacked: Vec<&PaneView> = views
            .iter()
            .filter(|v| v.col == layout.focus.column)
            .collect();
        assert_eq!(stacked.len(), 2, "two stacked panes");
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        let (cs, ce) = ranges[layout.focus.column];
        let (cs, ce) = (cs as u16, (ce as u16).min(cols - 1));
        // The gap row above the second pane is the divider.
        let divider_y = stacked[1].rect.y - 1;
        assert_eq!(at(cs, divider_y).ch, '├', "divider tees into left edge");
        assert_eq!(at(ce, divider_y).ch, '┤', "divider tees into right edge");
        let mid = (cs + ce) / 2;
        assert_eq!(at(mid, divider_y).ch, '─', "divider is a horizontal rule");
        // The focused (upper) pane owns the accent on its own ring, and the
        // divider it shares with the lower pane.
        assert_eq!(at(mid, divider_y).style.fg, red, "focused pane divider");
        assert_eq!(at(cs, divider_y).style.fg, red, "focused tee color");
        // Pane content is never covered: the row below the divider belongs to
        // the lower pane's content area, not to any frame.
        assert!(stacked[1].rect.y > divider_y);
    }

    #[test]
    fn skeleton_frames_four_boxes_with_red_focus() {
        // (see grid_boundaries_are_identical_whatever_the_occupancy below)
        // The default 4-quarter layout with the skeleton on: every column box
        // gets a full-height white frame and the focused box's frame is the
        // focus color (red by default).
        let layout = Layout::default(); // 4 quarter columns, focus col 0
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 10;
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let red = CColor::Rgb(0xff, 0, 0);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(CColor::Default, red, white),
            &no_map(),
            None,
        );
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        assert_eq!(ranges.len(), 4);
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        // Adjacent boxes *share* their boundary column: the strip is one
        // grid, not four overlapping rectangles. Only the outermost corners
        // are true corners; every interior boundary is a tee.
        for (ci, (s, e)) in ranges.iter().enumerate() {
            let (s, e) = (*s as u16, (*e as u16).min(cols - 1));
            let first = ci == 0;
            let last = ci + 1 == ranges.len();
            assert_eq!(
                at(s, 0).ch,
                if first { '╭' } else { '┬' },
                "top-left of box {ci}"
            );
            assert_eq!(
                at(e, 0).ch,
                if last { '╮' } else { '┬' },
                "top-right of box {ci}"
            );
            assert_eq!(
                at(s, rows - 1).ch,
                if first { '╰' } else { '┴' },
                "bottom-left of box {ci}"
            );
            assert_eq!(
                at(e, rows - 1).ch,
                if last { '╯' } else { '┴' },
                "bottom-right of box {ci}"
            );
            // Vertical edges run the full strip height, one cell thick.
            assert_eq!(at(s, rows / 2).ch, '│', "left edge of box {ci}");
            assert_eq!(at(e, rows / 2).ch, '│', "right edge of box {ci}");
        }
        // The focused column owns the color of *both* of its edges, including
        // the one it shares with the unfocused neighbour to its right: focus
        // outranks plain chrome no matter which box is painted last.
        let (fs, fe) = ranges[0];
        let (fs, fe) = (fs as u16, fe as u16);
        for y in [0, rows / 2, rows - 1] {
            assert_eq!(at(fs, y).style.fg, red, "focused left edge at y={y}");
            assert_eq!(at(fe, y).style.fg, red, "focused shared right edge y={y}");
            assert!(at(fs, y).style.bold, "focused ring is bold at y={y}");
            assert!(at(fe, y).style.bold, "focused ring is bold at y={y}");
        }
        // Unshared edges of unfocused boxes stay the skeleton color.
        assert_eq!(at(ranges[2].0 as u16, rows / 2).style.fg, white);
        assert!(
            !at(ranges[2].0 as u16, rows / 2).style.bold,
            "unfocused chrome stays regular weight"
        );
        // No double borders: the cell next to a shared boundary is interior.
        assert_eq!(at(fe + 1, rows / 2).ch, ' ', "no second rule beside {fe}");
        // Box interiors are not touched by the skeleton.
        let (s0, e0) = ranges[0];
        let mid = ((s0 + e0) / 2) as u16;
        assert_eq!(at(mid, rows / 2).ch, ' ', "interior untouched");
        // The rightmost frame reaches the exact screen edge: full bleed.
        assert_eq!(at(cols - 1, 0).ch, '╮');
        assert_eq!(at(cols - 1, 0).style.fg, white);
    }

    #[test]
    fn skeleton_insets_pane_content_inside_the_frame() {
        // With the skeleton on, pane rects sit strictly inside the column
        // frame ring: content starts at (s+1, 1) and ends at (e-1, strip_h-1),
        // so the frame never covers a cell a program can draw to.
        let layout = Layout::default();
        let panes = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 10;
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        let views = focused_pane_views(&layout, cols, rows, 0, &panes, true);
        assert_eq!(views.len(), 4);
        for (v, (s, e)) in views.iter().zip(&ranges) {
            assert_eq!(v.rect.x, *s as u16 + 1, "content starts inside frame");
            // The right frame sits *on* the shared boundary `e` (the next
            // column's left frame), so content ends there rather than a cell
            // earlier -- except for the last column, whose right frame is
            // pulled in to the last on-screen cell.
            let frame_x = (*e as u16).min(cols - 1);
            assert_eq!(v.rect.x + v.rect.w, frame_x, "content ends inside frame");
            assert_eq!(v.rect.y, 1, "content below the top frame row");
            assert_eq!(v.rect.y + v.rect.h, rows - 1, "content above bottom row");
            // Emulator geometry matches the logical column width: every
            // column but the last shows its whole grid, while the last
            // column's grid extends one cell past the screen edge (its right
            // frame pulls in, but its content overflows like pane 1 does).
            let logical_w = (*e as i32 - (*s as i32 + 1)).max(0) as u16;
            assert_eq!(v.grid_cols, logical_w);
            assert_eq!(v.grid_rows, v.rect.h);
        }
        // Full-bleed mode is unchanged: rects span the whole column and strip.
        let full = focused_pane_views(&layout, cols, rows, 0, &panes, false);
        assert_eq!(full[0].rect.x, 0);
        assert_eq!(full[0].rect.y, 0);
        let last = full.last().unwrap();
        assert_eq!(last.rect.x + last.rect.w, cols);
    }

    #[test]
    fn skeleton_fills_empty_right_side_with_placeholder_boxes() {
        // With fewer than 4 columns, the skeleton still shows the container:
        // placeholder quarter-width boxes tile the empty right side.
        let layout = Layout::new(2); // 2 quarter columns, right half empty
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 10;
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(CColor::Default, CColor::Rgb(0xff, 0, 0), white),
            &no_map(),
            None,
        );
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        // Placeholder boxes tile the empty right side, sharing each boundary
        // column with their neighbour: interior boundaries are tees, only the
        // screen edge is a true corner. The live half ends on the boundary
        // column the first placeholder adopts as its own left edge.
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        let live_end = ranges.last().unwrap().1 as u16;
        assert_eq!(
            at(live_end, 0).ch,
            '┬',
            "boundary between live and placeholder at {live_end}"
        );
        assert_eq!(at(cols - 1, 0).ch, '╮', "skeleton reaches screen edge");
        assert_eq!(at(cols - 1, rows - 1).ch, '╯', "bottom-right corner");
        assert_eq!(at(live_end, rows / 2).ch, '│', "boundary is one rule");
        // No double border: the cells flanking a shared boundary are interior.
        assert_eq!(at(live_end - 1, rows / 2).ch, ' ', "no rule left of it");
        assert_eq!(at(live_end + 1, rows / 2).ch, ' ', "no rule right of it");
        // Exactly one placeholder boundary between the live half and the edge,
        // and no sliver box hugging the right edge.
        let mid_rules: Vec<u16> = ((live_end + 1)..cols - 1)
            .filter(|x| at(*x, rows / 2).ch == '│')
            .collect();
        assert_eq!(
            mid_rules.len(),
            1,
            "one placeholder divider, got {mid_rules:?}"
        );
        assert_eq!(at(cols - 2, rows / 2).ch, ' ', "no sliver before the edge");
        for x in [live_end, mid_rules[0], cols - 1] {
            assert_eq!(at(x, 0).style.fg, white, "frame color at x={x}");
        }
    }

    #[test]
    fn placeholder_boxes_carry_a_centered_pal_not_text() {
        // Empty boxes are bare frames plus one monochrome sprite: their
        // interiors carry the palette base, the art is half-block cells in
        // the skeleton color, and there is no text of any kind. The only
        // key help is the `⌥+/` cheat-sheet.
        let layout = Layout::new(2); // boxes 3 and 4 are empty
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 24;
        let dim = CColor::Idx(235);
        let skel = CColor::Rgb(0xff, 0xff, 0xff);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(dim, CColor::Rgb(0xff, 0, 0), skel),
            &no_map(),
            None,
        );
        let bg = |x: u16, y: u16| out[y as usize * cols as usize + x as usize].style.bg;
        // Empty-box interiors blend with the surrounding backdrop (boxes 3
        // and 4 live at screen columns 41..59 and 61..79; the shared rules
        // themselves use the default background).
        for (x0, x1) in [(42u16, 58u16), (62, 78)] {
            for x in x0..x1 {
                for y in 1..rows - 1 {
                    assert_eq!(
                        bg(x, y),
                        dim,
                        "empty-box interior does not blend at ({x},{y})"
                    );
                }
            }
        }
        // ... and carry no text of any kind: only frames and half-block art.
        let text: String = out.iter().map(|c| c.ch).collect();
        let non_frame: String = text
            .chars()
            .filter(|c| {
                !matches!(
                    c,
                    ' ' | '\u{2800}'
                        | '\u{2580}'
                        | '\u{2584}'
                        | '\u{2588}'
                        | '\u{2502}'
                        | '\u{2500}'
                        | '\u{256d}'
                        | '\u{256e}'
                        | '\u{2570}'
                        | '\u{256f}'
                        | '\u{251c}'
                        | '\u{2524}'
                        | '\u{252c}'
                        | '\u{2534}'
                        | '\u{253c}'
                )
            })
            .collect();
        assert!(
            non_frame.is_empty(),
            "empty boxes must paint only frames and pixel art, got {non_frame:?}"
        );
        // The art is really there: half-block cells in the skeleton color.
        let art: Vec<_> = out
            .iter()
            .filter(|c| matches!(c.ch, '\u{2580}' | '\u{2584}' | '\u{2588}'))
            .collect();
        assert!(!art.is_empty(), "empty boxes must paint a sprite");
        assert!(
            art.iter().all(|c| c.style.fg == skel && c.style.bg == dim),
            "sprite art must be skeleton ink on the box background"
        );
        // Tiny boxes degrade to blank: no room for a sprite with breathing room.
        let small = placeholder_rows(30, 6).join("\n");
        assert!(
            !small.contains('\u{2580}')
                && !small.contains('\u{2584}')
                && !small.contains('\u{2588}'),
            "tiny boxes must stay blank, got:\n{small}"
        );
    }

    #[test]
    fn empty_boxes_are_identical_across_repaints() {
        // The frame differ compares against the previous frame, so the empty
        // boxes must render deterministically (blank).
        let a = placeholder_rows(120, 24);
        let b = placeholder_rows(120, 24);
        assert_eq!(a, b, "empty boxes changed between identical renders");
    }

    #[test]
    fn empty_boxes_carry_no_text_at_any_size() {
        // Whatever the box geometry, no identifier or hint may appear: the
        // only key help is the cheat-sheet.
        for (cols, rows) in [(120u16, 24u16), (120, 6), (30, 24), (160, 40)] {
            let text = placeholder_rows(cols, rows).join("\n");
            for token in [
                "moves focus",
                "toggles",
                "spawns",
                "1-9",
                "attention",
                "1.3",
                "1.4",
            ] {
                assert!(
                    !text.contains(token),
                    "empty box must not contain {token:?} at {cols}x{rows}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn logical_pane_sizes_do_not_change_with_focus_or_scroll() {
        // Exercise both rounding phases, mixed widths, clipped neighbours,
        // content-width overrides and the unframed renderer.
        for cols in [80, 141, 142, 143, 342] {
            for inset in [false, true] {
                for content_width in [0, 120] {
                    let mut layout = Layout::new(8);
                    layout.rows[0].columns[2].width = Width::Preset(gwae_layout::Preset::Third);
                    layout.rows[0].columns[4].width = Width::Cells(47);
                    let panes = HashMap::new();
                    let mut sizes = HashMap::new();
                    let mut stops = HashSet::new();
                    for action in std::iter::repeat_n(Action::FocusRight, 7)
                        .chain(std::iter::repeat_n(Action::FocusLeft, 7))
                    {
                        layout
                            .apply(action, Viewport::new(cols), FollowScroll::default())
                            .unwrap();
                        stops.insert(layout.focused_row().unwrap().scroll_x);
                        for v in focused_pane_views_with_chrome(
                            &layout,
                            cols,
                            30,
                            content_width,
                            &panes,
                            inset,
                            2,
                        ) {
                            let size = (v.grid_cols, v.grid_rows);
                            if let Some(previous) = sizes.insert(v.pid, size) {
                                assert_eq!(
                                    size, previous,
                                    "pane {} resized on focus at {cols} cols",
                                    v.pid
                                );
                            }
                            assert!(v.grid_cols >= content_width.max(2));
                            assert!(v.grid_cols >= v.col_x0 + v.rect.w);
                        }
                    }
                    assert_eq!(sizes.len(), 8, "visit every pane");
                    assert!(stops.len() > 1, "exercise real scroll changes");
                }
            }
        }
    }

    #[test]
    fn logical_sizes_follow_real_geometry_and_tile_split_heights() {
        let sizes = |width, count, cols, rows, content| {
            column_grid_sizes(width, count, GridSize { cols, rows }, content, true, 2)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sizes(Width::DEFAULT, 1, 142, 30, 0),
            vec![GridSize { cols: 35, rows: 26 }]
        );
        assert_eq!(
            sizes(Width::DEFAULT, 1, 160, 30, 0),
            vec![GridSize { cols: 39, rows: 26 }]
        );
        assert_eq!(
            sizes(Width::Cells(50), 1, 142, 30, 0),
            vec![GridSize { cols: 49, rows: 26 }]
        );
        assert_eq!(
            sizes(Width::DEFAULT, 1, 142, 30, 120),
            vec![GridSize {
                cols: 120,
                rows: 26
            }]
        );
        assert_eq!(
            sizes(Width::DEFAULT, 1, 1, 1, 0),
            vec![GridSize { cols: 2, rows: 1 }]
        );
        for count in 1..=26 {
            let split = sizes(Width::DEFAULT, count, 142, 30, 0);
            assert_eq!(
                split.iter().map(|s| s.rows as usize).sum::<usize>() + count - 1,
                26
            );
            assert!(split
                .windows(2)
                .all(|s| s[0].rows >= s[1].rows && s[0].rows - s[1].rows <= 1));
        }
        assert!(sizes(Width::DEFAULT, 30, 142, 30, 0)
            .iter()
            .all(|s| s.rows == 0));
    }

    #[test]
    fn tiny_pane_pty_geometry_matches_the_emulator_minimum() {
        let layout = Layout::new(1);
        let views = focused_pane_views(&layout, 8, 6, 0, &HashMap::new(), true);
        assert_eq!(views.len(), 1);
        let v = &views[0];
        assert_eq!(v.rect.w, 1, "only one cell fits between the frames");
        assert_eq!(v.grid_cols, 2, "PTY keeps room for a wide glyph");
        let size = GridSize {
            cols: v.grid_cols,
            rows: v.grid_rows,
        };
        assert_eq!(Vt100Grid::new(size).size(), size);
    }
}
