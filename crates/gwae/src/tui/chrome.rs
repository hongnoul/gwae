//! Chrome: HUD facts, center minimap dashboard, toasts, edge ticks (verbatim move from `tui/mod.rs`).

use std::collections::HashMap;
use std::time::Duration;

use gwae_layout::{Layout, PaneId, PaneStatus};
use gwae_term::{CColor, Cell};

use crate::theme::Palette;

use super::render::draw_focus_frame;
use super::Rect;

/// Paint a block of pre-wrapped text centered in `rect`.
///
/// Each line keeps its relative indentation and any line that would run past
/// the rect is clipped rather than wrapping into the neighbouring box.
pub(crate) fn draw_art(out: &mut [Cell], cols: u16, rect: Rect, lines: &[String], color: CColor) {
    let bw = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    if bw == 0 || bw > rect.w {
        return;
    }
    let x0 = rect.x + (rect.w - bw) / 2;
    for (ly, line) in lines.iter().enumerate() {
        let y = rect.y + ly as u16;
        if y >= rect.y + rect.h {
            break;
        }
        for (lx, ch) in line.chars().enumerate() {
            let x = x0 + lx as u16;
            if x >= rect.x + rect.w || x >= cols {
                break;
            }
            if ch == ' ' {
                continue;
            }
            let idx = y as usize * cols as usize + x as usize;
            if let Some(c) = out.get_mut(idx) {
                // Only the glyph and its color are ours: the cell keeps the
                // background the box was filled with, so the art blends into
                // the terminal backdrop instead of stamping a differently
                // colored rectangle over it.
                let bg = c.style.bg;
                *c = Cell {
                    ch,
                    style: gwae_term::Style {
                        fg: color,
                        bg,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
}

pub(crate) fn status_glyph_for(s: PaneStatus) -> char {
    match s {
        PaneStatus::Running => '\u{00bb}', // »
        PaneStatus::Idle => '!',
        PaneStatus::Done => '\u{2713}',   // ✓
        PaneStatus::Failed => '\u{2717}', // ✗
    }
}

/// Everything the ⌥-hold overlay knows that the layout alone cannot tell it:
/// what each pane *is* (its OSC 0/2 title) and how long it has been silent.
///
/// It is a plain data bag built at the call site from the live PTY panes so
/// the drawing code stays a pure function of the frame's facts, and so every
/// decoration can be tested without spawning a pty.
#[derive(Default)]
#[allow(dead_code)]
pub(crate) struct HudFacts {
    /// Short label per pane, already reduced from the raw window title.
    #[allow(dead_code)]
    pub(crate) titles: HashMap<PaneId, String>,
    /// How long each pane has been silent (used to age attention tiles).
    #[allow(dead_code)]
    pub(crate) quiet: HashMap<PaneId, Duration>,
    /// Whether the `caffeinate` assertion is held: stamps the keep-awake
    /// badge onto the HUD frame so the state reads without leaving the overlay.
    pub(crate) keep_awake: bool,
    /// Whether this is a dev session (`GWAE_DEV_RELOAD=1`): stamps DEV onto
    /// the bottom HUD frame row. Stable sessions render a plain frame.
    pub(crate) dev: bool,
}

/// Stamp [`crate::keepawake::KEEP_AWAKE_BADGE`] onto the top frame row of `rect`,
/// centered inside the frame. No-op when the panel is too narrow to
/// hold the badge, so small viewports degrade to the plain frame rather than
/// a clipped fragment.
pub(crate) fn stamp_keep_awake_badge(
    out: &mut [Cell],
    cols: u16,
    rect: super::Rect,
    pal: &Palette,
) {
    let badge = crate::keepawake::KEEP_AWAKE_BADGE;
    let n = badge.chars().count();
    if n == 0 || rect.w as usize <= n + 2 || rect.h == 0 {
        return;
    }
    let y = rect.y as usize;
    let start = rect.x as usize + (rect.w as usize - n) / 2;
    for (i, ch) in badge.chars().enumerate() {
        let idx = y * cols as usize + start + i;
        let Some(c) = out.get_mut(idx) else { continue };
        c.ch = ch;
        c.style.fg = pal.accent;
        c.style.bold = true;
    }
}

/// Stamp [`crate::reload::DEV_BADGE`] onto the bottom frame row of `rect`,
/// centered inside the frame. Dev-only: stable sessions never call this, so
/// their frame stays plain box-drawing. No-op when the panel is too narrow
/// to hold the badge.
pub(crate) fn stamp_dev_badge(out: &mut [Cell], cols: u16, rect: super::Rect, pal: &Palette) {
    let badge = crate::reload::DEV_BADGE;
    let n = badge.chars().count();
    if n == 0 || rect.w as usize <= n + 2 || rect.h == 0 {
        return;
    }
    let y = rect.y as usize + rect.h as usize - 1;
    let start = rect.x as usize + (rect.w as usize - n) / 2;
    for (i, ch) in badge.chars().enumerate() {
        let idx = y * cols as usize + start + i;
        let Some(c) = out.get_mut(idx) else { continue };
        c.ch = ch;
        c.style.fg = pal.accent;
        c.style.bold = true;
    }
}

/// Reduce a window title to something that fits on a minimap tile.
///
/// Shell titles are conventionally `user@host: ~/some/dir`, which is almost
/// entirely chrome at tile widths; agent harnesses set something short and
/// meaningful already. So: drop everything before the last `": "`, then keep
/// the final path segment, and strip control characters that a program could
/// have smuggled into the title.
#[allow(dead_code)]
pub(crate) fn short_title(raw: &str) -> String {
    let raw = raw.trim();
    let after_colon = raw.rsplit_once(": ").map(|(_, r)| r).unwrap_or(raw).trim();
    let base = match after_colon.rsplit_once('/') {
        // A trailing slash leaves an empty segment: keep the whole path
        // rather than showing nothing at all.
        Some((_, last)) if !last.is_empty() => last,
        _ => after_colon,
    };
    base.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Compact age for a silent pane: seconds under a minute, then minutes, then
/// hours. Two or three cells, so it fits beside a status glyph on a tile.
#[allow(dead_code)]
pub(crate) fn age_label(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", (s / 3600).min(99))
    }
}

/// WCAG relative luminance, with sRGB channels linearized before weighting.
pub(crate) fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// Choose the higher-contrast black/white ink for a known RGB background.
/// Unknown terminal colors must use the neutral tile fallback instead.
pub(crate) fn contrast_fg(bg: CColor, _pal: &Palette) -> CColor {
    match bg {
        CColor::Rgb(r, g, b) => {
            let l = relative_luminance(r, g, b);
            if (l + 0.05) / 0.05 >= 1.05 / (l + 0.05) {
                CColor::Rgb(0, 0, 0)
            } else {
                CColor::Rgb(255, 255, 255)
            }
        }
        _ => CColor::Default,
    }
}

/// ANSI indices are user-remappable, so never guess their brightness.
/// Keep addresses on the terminal's own foreground/background pair. Status
/// glyphs and focus markers still carry color, without a saturated text fill.
pub(crate) fn tile_colors(bg: CColor, pal: &Palette) -> (CColor, CColor) {
    match bg {
        CColor::Rgb(..) => (contrast_fg(bg, pal), bg),
        _ => (CColor::Default, CColor::Default),
    }
}

/// Render one minimap tile's `w` cells of text — spatial-only.
///
/// Only the geometry matters: status color already carries health, the tile's
/// position carries which column/stack it is, and the address + glyph are the
/// minimal addressing cue (`»2`). Title and age were dropped: they crowded the
/// map and duplicated what the pane's own chrome already shows. The result is
/// always exactly `w` characters, padded with blanks, so the caller can paint
/// cell-for-cell.
pub(crate) fn tile_text(w: u16, addr: &str, glyph: char) -> String {
    let w = w as usize;
    if w == 0 {
        return String::new();
    }
    if w == 1 {
        // One cell: status beats address. Which pane is *waiting* is worth
        // more than which key jumps to it, and the tile's position still gives
        // the column away.
        return glyph.to_string();
    }
    let addr: String = if addr.chars().count() < w {
        addr.to_string()
    } else {
        // A two-digit column on a two-cell tile: say "there is more here"
        // rather than lying about which column this is.
        "+".to_string()
    };
    let alen = addr.chars().count();
    if w <= alen + 1 {
        return format!("{glyph}{addr}");
    }
    // One cell after the address is a plain gap: status color already
    // carries health, so no jump marker is painted.
    let sep = ' ';
    let mut s = String::new();
    s.push(glyph);
    s.push_str(&addr);
    s.push(sep);
    let pad = w.saturating_sub(s.chars().count());
    s.extend(std::iter::repeat_n(' ', pad));
    s.chars().take(w).collect()
}

/// The inclusive column-index range of a strip that is currently on screen,
/// or `None` when the whole strip fits (in which case there is nothing to
/// point out: the viewport *is* the strip).
pub(crate) fn visible_column_range(
    layout: &Layout,
    row_idx: usize,
    cols: u16,
) -> Option<(usize, usize)> {
    let row = layout.rows.get(row_idx)?;
    let ranges = layout.column_x_ranges(row.id, cols)?;
    let total = ranges.last().map(|r| r.1).unwrap_or(0);
    if total <= cols as u32 {
        return None;
    }
    let max_scroll = total.saturating_sub(cols as u32);
    let start = (row.scroll_x.max(0) as u32).min(max_scroll);
    let end = start + cols as u32;
    let mut first = None;
    let mut last = 0usize;
    for (i, (s, e)) in ranges.iter().enumerate() {
        if *e > start && *s < end {
            first.get_or_insert(i);
            last = i;
        }
    }
    first.map(|f| (f, last))
}

/// Where every piece of the ⌥-hold dashboard lands.
///
/// Geometry is computed once, by [`plan_center_minimap`], and then consumed
/// twice: to paint the panel and to resolve a click on a tile back to a pane.
/// Sharing one plan is what keeps those two from drifting apart, which is
/// exactly how "click focuses the wrong pane" bugs are born.
pub(crate) struct HudPlan {
    /// The panel's screen rect, frame included.
    pub(crate) rect: Rect,
    /// Screen y of each shown strip's tile row.
    pub(crate) row_y: Vec<u16>,
    /// Screen y of each strip's viewport ruler, when that strip overflows.
    pub(crate) ruler_y: Vec<Option<u16>>,
    /// The visible column range of each shown strip, when it overflows.
    pub(crate) rulers: Vec<Option<(usize, usize)>>,
    /// Screen x of map cell 0 (past the frame and the strip gutter).
    pub(crate) map_ox: u16,
    /// Gutter labels, one per strip, and the gutter's width in cells.
    pub(crate) gutter: Vec<String>,
    pub(crate) gutter_w: u16,
    /// The tiles to paint (already limited to the shown strips).
    pub(crate) map: gwae_layout::minimap::Minimap,
    /// Strips cut off the bottom, if any.
    pub(crate) hidden: usize,
    /// Inner width, and the first inner row/column.
    pub(crate) inner_w: usize,
    pub(crate) inner_ox: usize,
    /// Screen y of the tally row.
    pub(crate) tally_y: Option<u16>,
}

/// The status tally shown in the dashboard footer: the pane count, then one
/// `glyph count` segment per status that has any panes. Returned with the
/// status rather than a color so the geometry pass can measure it without a
/// palette.
pub(crate) fn status_tally(layout: &Layout) -> Vec<(String, Option<PaneStatus>)> {
    let statuses = [
        PaneStatus::Running,
        PaneStatus::Idle,
        PaneStatus::Done,
        PaneStatus::Failed,
    ];
    let mut counts = [0usize; 4];
    for p in layout.panes.values() {
        counts[statuses.iter().position(|s| *s == p.status).unwrap_or(0)] += 1;
    }
    let mut out = vec![(format!("{}", layout.panes.len()), None)];
    for (i, s) in statuses.iter().enumerate() {
        if counts[i] > 0 {
            out.push((format!(" {}{}", status_glyph_for(*s), counts[i]), Some(*s)));
        }
    }
    out
}

/// Lay out the ⌥-hold dashboard, or `None` when it cannot be shown.
///
/// Pure geometry: no palette, no painting, so a test can assert where things
/// land without reading pixels back out of a frame buffer.
pub(crate) fn plan_center_minimap(
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
) -> Option<HudPlan> {
    use gwae_layout::minimap;
    if !mm.show || cols < 20 || rows < 8 {
        return None;
    }
    // A single pane has no grid to triage: nothing to show.
    let single = layout.panes.len() <= 1 && layout.rows.len() <= 1;
    if single {
        return None;
    }

    // Strip gutter: just the strip number.
    let gutter: Vec<String> = layout
        .rows
        .iter()
        .enumerate()
        .map(|(i, _)| format!("{}", i + 1))
        .collect();
    let gutter_w = if single {
        0
    } else {
        gutter
            .iter()
            .map(|g| g.chars().count())
            .max()
            .unwrap_or(0)
            .min(10) as u16
    };
    // Frame + gutter + separating space, before the map gets its budget.
    let chrome_w = 2 + gutter_w + u16::from(gutter_w > 0);
    // Tight, not padded: each tile is exactly its content (glyph + address,
    // e.g. `»12`), so the map is only as wide as the widest strip needs.
    // `minimap.max_width` still caps the *corner overlay*; the centered panel
    // treats it as a ceiling, never a floor. Never more than two-thirds of
    // the screen, so the session stays visible around it.
    let want = (WIDTH_PER_TILE * layout_widest_strip(layout) as u16).min(mm.max_width);
    let room = cols.saturating_sub(chrome_w + 4).max(1);
    let width = want.min(room).min(cols * 2 / 3).max(1);
    // Proportional, not stretched: on the centered panel the strips are read
    // against each other, and stretching a two-column strip to the width of a
    // six-column one makes the short strip look long.
    let map = minimap::build_scaled(layout, width, cols, minimap::Scale::Proportional);
    let shown_rows = mm.max_rows.min(map.height).min(rows.saturating_sub(6));
    if !single && (shown_rows == 0 || map.width == 0) {
        return None;
    }
    let hidden = if single {
        0
    } else {
        (map.height as usize).saturating_sub(shown_rows as usize)
    };

    // Each strip that overflows the screen gets a ruler row under its tiles,
    // so the two always read together.
    let rulers: Vec<Option<(usize, usize)>> = if single {
        Vec::new()
    } else {
        (0..shown_rows as usize)
            .map(|i| visible_column_range(layout, i, cols))
            .collect()
    };
    let ruler_rows = rulers.iter().filter(|r| r.is_some()).count();
    let body_rows = if single {
        0
    } else {
        shown_rows as usize + ruler_rows + usize::from(hidden > 0)
    };
    let has_summary = mm.show_counts && !single;
    let footer_rows = usize::from(has_summary);

    let map_row_w = (gutter_w + u16::from(gutter_w > 0) + map.width) as usize;
    let tally_w: usize = status_tally(layout)
        .iter()
        .map(|(t, _)| t.chars().count())
        .sum();
    let inner_w = map_row_w.max(if has_summary { tally_w } else { 0 });
    let bw = inner_w + 2;
    let bh = body_rows + footer_rows + 2;
    if bw as u16 >= cols || bh as u16 >= rows {
        return None;
    }
    let ox = ((cols as usize).saturating_sub(bw)) / 2;
    let oy = ((rows as usize).saturating_sub(bh)) / 2;
    let inner_ox = ox + 1;
    let mut y = oy + 1;
    let mut row_y = Vec::with_capacity(shown_rows as usize);
    let mut ruler_y = Vec::with_capacity(shown_rows as usize);
    for r in rulers.iter() {
        row_y.push(y as u16);
        y += 1;
        if r.is_some() {
            ruler_y.push(Some(y as u16));
            y += 1;
        } else {
            ruler_y.push(None);
        }
    }
    let mut fy = oy + 1 + body_rows;
    let tally_y = has_summary.then(|| {
        let at = fy as u16;
        fy += 1;
        at
    });
    Some(HudPlan {
        rect: Rect {
            x: ox as u16,
            y: oy as u16,
            w: bw as u16,
            h: bh as u16,
        },
        row_y,
        ruler_y,
        rulers,
        map_ox: (inner_ox + gutter_w as usize + usize::from(gutter_w > 0)) as u16,
        gutter,
        gutter_w,
        map,
        hidden,
        inner_w,
        inner_ox,
        tally_y,
    })
}

pub(crate) fn has_attention(layout: &Layout) -> bool {
    layout
        .panes
        .values()
        .any(|p| matches!(p.status, PaneStatus::Idle | PaneStatus::Failed))
}

/// Cells the centered dashboard would like per column — spatial-only tiles
/// need only glyph+address, so 5 (e.g. `»12 `) is enough. A target,
/// not a guarantee: narrow terminals get less and [`tile_text`] degrades
/// accordingly.
pub(crate) const WIDTH_PER_TILE: u16 = 5;

/// The most columns any single strip has. Sizing the map by this (rather than
/// by the focused strip) keeps the panel from resizing under the user's eyes
/// as focus moves between strips of different lengths.
pub(crate) fn layout_widest_strip(layout: &Layout) -> usize {
    layout
        .rows
        .iter()
        .map(|r| r.columns.len())
        .max()
        .unwrap_or(1)
        .max(1)
}

/// Which pane a click at `(x, y)` lands on, given a drawn dashboard. `None`
/// when the point is not on a tile.
pub(crate) fn hud_pane_at(plan: &HudPlan, x: u16, y: u16) -> Option<PaneId> {
    let ry = plan.row_y.iter().position(|r| *r == y)?;
    plan.map
        .cells
        .iter()
        .find(|c| c.y as usize == ry && x >= plan.map_ox + c.x && x < plan.map_ox + c.x + c.w)
        .map(|c| c.pane)
}

/// Centered agent dashboard, revealed while ⌥/Alt is held.
///
/// One row per strip, one tile per pane, tile width proportional to the
/// column's real width share. Spatial-only: each tile shows only the column
/// address and its status color/glyph. Titles and ages are omitted — the map
/// shows *where* panes are, not *what* they are (which lives in the pane
/// chrome itself). The strip's visible column span is still underscored.
///
/// A gutter names each strip, the footer counts panes by status and spells
/// the keys that act on what you are looking at.
///
/// Plan-and-paint in one call. The render loop keeps the two apart (it needs
/// the plan for click-to-focus); tests, which only care about what lands on
/// the screen, use this.
#[cfg(test)]
pub(crate) fn draw_center_minimap(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
    pal: &Palette,
    facts: &HudFacts,
) {
    if let Some(plan) = plan_center_minimap(cols, rows, layout, mm) {
        paint_center_minimap(out, cols, rows, layout, &plan, pal, facts);
    }
}

/// Paint a planned dashboard. Split from [`plan_center_minimap`] so geometry
/// is decided once and reused by click-to-focus.
pub(crate) fn paint_center_minimap(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    plan: &HudPlan,
    pal: &Palette,
    facts: &HudFacts,
) {
    let focus_color = pal.accent;
    let status_bg = |s: PaneStatus| pal.status(s);
    let status_fg = |s: PaneStatus| pal.status(s);
    let status_glyph = status_glyph_for;
    let tally: Vec<(String, CColor)> = status_tally(layout)
        .into_iter()
        .map(|(t, s)| {
            let c = s.map(status_fg).unwrap_or(pal.text);
            (t, c)
        })
        .collect();
    let tally_w: usize = tally.iter().map(|(t, _)| t.chars().count()).sum();
    let (ox, oy) = (plan.rect.x as usize, plan.rect.y as usize);
    let (bw, bh) = (plan.rect.w as usize, plan.rect.h as usize);
    let bg = pal.surface;
    // Fill box interior with the panel background.
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + (ox + x)) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    draw_focus_frame(out, cols, plan.rect, focus_color, true);
    let inner_ox = plan.inner_ox;
    let put = |out: &mut [Cell], x: u16, y: u16, ch: char, fg: CColor, bg: CColor, bold: bool| {
        if x >= cols || y >= rows {
            return;
        }
        let idx = y as usize * cols as usize + x as usize;
        if let Some(cell) = out.get_mut(idx) {
            *cell = Cell::default();
            cell.ch = ch;
            cell.style.fg = fg;
            cell.style.bg = bg;
            cell.style.bold = bold;
        }
    };
    let write = |out: &mut [Cell], x0: usize, y: usize, text: &str, fg: CColor, bold: bool| {
        for (i, ch) in text.chars().enumerate() {
            put(out, (x0 + i) as u16, y as u16, ch, fg, bg, bold);
        }
    };
    let map_ox = plan.map_ox as usize;
    // Strip gutter.
    for (i, gy) in plan.row_y.iter().enumerate() {
        let focused = layout
            .rows
            .get(i)
            .map(|r| r.id == layout.focus.row)
            .unwrap_or(false);
        let label = plan.gutter.get(i).cloned().unwrap_or_default();
        let label: String = label.chars().take(plan.gutter_w as usize).collect();
        if plan.gutter_w > 0 {
            write(
                out,
                inner_ox,
                *gy as usize,
                &label,
                if focused {
                    focus_color
                } else {
                    Palette::muted(pal.text)
                },
                focused,
            );
        }
    }
    for tile in &plan.map.cells {
        if tile.y as usize >= plan.row_y.len() {
            continue;
        }
        let bgc = if tile.focus_col {
            focus_color
        } else {
            status_bg(tile.status)
        };
        let neutral = !matches!(bgc, CColor::Rgb(..));
        let (fg, bgc) = tile_colors(bgc, pal);
        let gy = plan.row_y[tile.y as usize] as usize;
        let glyph = status_glyph(tile.status);
        let addr = if tile.pane_idx == 0 {
            format!("{}", tile.column + 1)
        } else {
            "·".to_string()
        };
        let text = tile_text(tile.w, &addr, glyph);
        for (dx, ch) in text.chars().enumerate() {
            let x = map_ox + tile.x as usize + dx;
            // Bold the leading `glyph + address` signature: it is what the
            // eye lands on when scanning a row of abutting tiles. The whole
            // focused tile is bold so focus survives palette-neutral fills
            // with no underline.
            let sig = dx <= addr.chars().count() || tile.focus_col;
            let ink = if neutral && ch == glyph {
                status_fg(tile.status)
            } else {
                fg
            };
            put(out, x as u16, gy as u16, ch, ink, bgc, sig);
        }
    }
    // Viewport ruler: which columns of each strip are actually on screen.
    for (i, ry) in plan.ruler_y.iter().enumerate() {
        let (Some(ry), Some((first, last))) = (ry, plan.rulers[i]) else {
            continue;
        };
        let span: Vec<&gwae_layout::minimap::MinimapCell> = plan
            .map
            .cells
            .iter()
            .filter(|c| c.y as usize == i && c.column >= first && c.column <= last)
            .collect();
        let (Some(s), Some(e)) = (
            span.iter().map(|c| c.x).min(),
            span.iter().map(|c| c.x + c.w).max(),
        ) else {
            continue;
        };
        for x in s..e {
            put(
                out,
                (map_ox + x as usize) as u16,
                *ry,
                '─',
                focus_color,
                bg,
                false,
            );
        }
    }
    // Truncation is never silent: strips past the cut are counted.
    if plan.hidden > 0 {
        let more = format!(
            "⋯ +{} strip{}",
            plan.hidden,
            if plan.hidden == 1 { "" } else { "s" }
        );
        write(
            out,
            inner_ox,
            plan.tally_y.unwrap_or(plan.rect.y + plan.rect.h - 1) as usize - 1,
            &more,
            Palette::muted(pal.text),
            false,
        );
    }
    // Footer: tallies centred on their own row.
    if let Some(fy) = plan.tally_y {
        let mut x = plan.inner_ox + (plan.inner_w.saturating_sub(tally_w)) / 2;
        for (text, fg) in &tally {
            for ch in text.chars() {
                put(out, x as u16, fy, ch, *fg, bg, true);
                x += 1;
            }
        }
    }
    if facts.keep_awake {
        stamp_keep_awake_badge(out, cols, plan.rect, pal);
    }
    if facts.dev {
        stamp_dev_badge(out, cols, plan.rect, pal);
    }
}

/// Draw a one-line toast, optionally anchored to the bottom-left of a rect
/// (a pane) instead of the bottom-left of the screen. A drag-copy note belongs
/// to the pane the text came from, so with several panes on screen it is
/// obvious which pane's selection was copied.
///
/// Used to report config reloads. It is a single row so it never covers a
/// pane's working area meaningfully, and it sits on the terminal `surface` so
/// it reads as chrome rather than as pane output.
pub(crate) fn draw_toast_at(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    text: &str,
    pal: &Palette,
    ok: bool,
    anchor: Option<Rect>,
) {
    if cols < 8 || rows == 0 {
        return;
    }
    let body: Vec<char> = format!(" {text} ").chars().collect();
    // Clip to the screen rather than wrapping: a toast is a hint, and a
    // truncated hint is better than one that reflows the layout.
    let (x0, y) = match anchor.filter(|r| r.w > 0 && r.h > 0) {
        Some(r) => (
            r.x.min(cols.saturating_sub(1)) as usize,
            (r.y + r.h - 1).min(rows - 1) as usize,
        ),
        None => (0, (rows - 1) as usize),
    };
    let w = body.len().min((cols as usize).saturating_sub(x0));
    // Errors use the failed tint so a broken config is not mistaken for a
    // successful reload at a glance.
    let fg = if ok { pal.text } else { pal.failed };
    for (x, ch) in body.iter().take(w).enumerate() {
        if let Some(c) = out.get_mut(y * cols as usize + x0 + x) {
            *c = Cell {
                ch: *ch,
                style: gwae_term::Style {
                    fg,
                    bg: pal.surface,
                    ..Default::default()
                },
                width: 1,
                ..Default::default()
            };
        }
    }
}

/// Center HUD: a concise cheat-sheet of every keybind, shown at startup and
/// toggled with `⌥+/`. Persists until the next key press.
///
/// Attention (Idle/Failed panes) is deliberately *not* surfaced here: the
/// ambient chrome already carries it (pane tints, right-edge strip ticks,
/// minimap glyphs) and `⌥+g` jumps to the pane that wants you on demand.
pub(crate) fn draw_center_hud(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    pal: &Palette,
    keep_awake: bool,
    dev: bool,
) {
    let focus_color = pal.accent;
    if cols < 30 || rows < 9 {
        return;
    }
    // Cheat-sheet as a spreadsheet: two (key, action) column pairs with a
    // header row and ruled grid lines, so keys line up in a scannable table
    // rather than reading as a paragraph of hints.
    //
    // Both columns are rendered from [`crate::binds::BINDS`], the single
    // source of truth that a test cross-checks against `handle_key`, so the
    // HUD cannot advertise a key the dispatcher does not implement.
    let rows_for = |g: crate::binds::Group| -> Vec<(String, &'static str)> {
        crate::binds::group(g)
            .map(|b| (b.label(), b.desc))
            .collect()
    };
    let nav = rows_for(crate::binds::Group::Navigate);
    let panes = rows_for(crate::binds::Group::Panes);
    // Column widths sized to their widest cell, header included.
    let width_of = |hdr: &str, it: &[(String, &str)], first: bool| -> usize {
        it.iter()
            .map(|e| {
                if first {
                    e.0.chars().count()
                } else {
                    e.1.chars().count()
                }
            })
            .chain(std::iter::once(hdr.chars().count()))
            .max()
            .unwrap_or(0)
    };
    let w = [
        width_of("key", &nav, true),
        width_of("navigate", &nav, false),
        width_of("key", &panes, true),
        width_of("panes", &panes, false),
    ];
    // Each cell is ` text `; columns joined by a vertical rule.
    let table_w: usize = w.iter().map(|c| c + 2).sum::<usize>() + 3;
    let cell = |text: &str, i: usize| -> String { format!(" {:<width$} ", text, width = w[i]) };
    let rule = |m: char| -> String {
        let mut s = String::new();
        for (i, c) in w.iter().enumerate() {
            if i > 0 {
                s.push(m);
            }
            s.extend(std::iter::repeat_n('─', c + 2));
        }
        s
    };
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "{}│{}│{}│{}",
        cell("key", 0),
        cell("navigate", 1),
        cell("key", 2),
        cell("panes", 3)
    ));
    lines.push(rule('┼'));
    for i in 0..nav.len().max(panes.len()) {
        let (k1, a1) = nav.get(i).map(|e| (e.0.as_str(), e.1)).unwrap_or(("", ""));
        let (k2, a2) = panes
            .get(i)
            .map(|e| (e.0.as_str(), e.1))
            .unwrap_or(("", ""));
        lines.push(format!(
            "{}│{}│{}│{}",
            cell(k1, 0),
            cell(a1, 1),
            cell(k2, 2),
            cell(a2, 3)
        ));
    }
    lines.push(rule('┴'));
    lines.push(format!(
        "{:^width$}",
        &format!("prefix: {}+key", crate::keys::mod_key()),
        width = table_w
    ));

    let bw = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(table_w)
        + 2;
    let bh = lines.len() + 2;
    if bw as u16 >= cols || bh as u16 >= rows {
        return;
    }
    let ox = ((cols as usize).saturating_sub(bw)) / 2;
    let oy = ((rows as usize).saturating_sub(bh)) / 2;
    let bg = pal.surface;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + (ox + x)) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    let rect = Rect {
        x: ox as u16,
        y: oy as u16,
        w: bw as u16,
        h: bh as u16,
    };
    draw_focus_frame(out, cols, rect, focus_color, true);
    if keep_awake {
        stamp_keep_awake_badge(out, cols, rect, pal);
    }
    if dev {
        stamp_dev_badge(out, cols, rect, pal);
    }
    for (idx, line) in lines.iter().enumerate() {
        let ty = oy + 1 + idx;
        let line_len = line.chars().count();
        let tx = ox + 1 + (bw - 2).saturating_sub(line_len) / 2;
        let is_header = idx == 0;
        let is_rule = line.starts_with('─');
        for (i, ch) in line.chars().enumerate() {
            if let Some(c) = out.get_mut(ty * cols as usize + (tx + i)) {
                c.ch = ch;
                c.style.fg = if is_header {
                    focus_color
                } else if is_rule || ch == '│' {
                    Palette::muted(pal.text)
                } else {
                    pal.text
                };
                c.style.bg = bg;
                c.style.bold = is_header;
                c.width = 1;
            }
        }
    }
}

/// Edge-ticks chrome: single-cell marks on the bottom/right frame edges at true x-positions.
pub(crate) fn draw_edge_ticks(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    _mm: &crate::config::Minimap,
    pal: &Palette,
) {
    let focus_color = pal.accent;
    if cols == 0 || rows == 0 {
        return;
    }
    let tick_bg = |s: PaneStatus| pal.status(s);
    let y = rows.saturating_sub(1) as usize;
    let ranges = layout
        .column_x_ranges(layout.focus.row, cols)
        .unwrap_or_default();
    for (ci, (_s, _e)) in ranges.iter().enumerate() {
        let col = match layout.focused_row().and_then(|r| r.columns.get(ci)) {
            Some(c) => c,
            None => continue,
        };
        let status = col
            .panes
            .first()
            .and_then(|pid| layout.panes.get(pid))
            .map(|pane| pane.status)
            .unwrap_or(PaneStatus::Running);
        let bg = if ci == layout.focus.column {
            focus_color
        } else {
            tick_bg(status)
        };
        // Approx tick at column's left edge clamped
        let x = ((*_s).min(cols as u32) as usize).min(cols as usize - 1);
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ' ';
            c.style.bg = bg;
            c.style.fg = CColor::Idx(231);
        }
        // Mark attention with '!' at tick neighbor if needed
        if matches!(status, PaneStatus::Idle | PaneStatus::Failed) && x + 1 < cols as usize {
            if let Some(c) = out.get_mut(y * cols as usize + x + 1) {
                if c.style.bg == CColor::Default || c.style.bg == tick_bg(status) {
                    c.ch = status_glyph_for(status);
                    c.style.bg = bg;
                    c.style.fg = CColor::Idx(231);
                }
            }
        }
    }
    // Right-edge strip ticks
    let x = cols.saturating_sub(1) as usize;
    for (ri, row) in layout.rows.iter().enumerate() {
        let is_focus = row.id == layout.focus.row;
        let needs = row.columns.iter().any(|c| {
            c.panes.iter().any(|pid| {
                layout
                    .panes
                    .get(pid)
                    .map(|pane| matches!(pane.status, PaneStatus::Idle | PaneStatus::Failed))
                    .unwrap_or(false)
            })
        });
        if ri >= rows as usize {
            break;
        }
        let bg = if is_focus {
            pal.accent
        } else if needs {
            pal.status(PaneStatus::Idle)
        } else {
            pal.overlay
        };
        if let Some(c) = out.get_mut(ri * cols as usize + x) {
            // only overwrite border-ish cells (don't clobber pane content interior — but right edge is usually chrome)
            if c.ch == ' ' || c.width == 1 {
                c.style.bg = bg;
            }
        }
    }
}

/// Overlay the minimap in the bottom-right corner: an agent dashboard, not
/// just a position indicator. Rows of the map are strips, each tile is a pane
/// (columns subdivided by their stacks) with width proportional to the
/// column's real width share. Every tile is painted in its *status* color
/// (working / wants-attention / done / failed), carries the pane's column
/// digit and, when wide enough, a status
/// glyph. The focused pane's tile is painted in the focus accent and the
/// focused strip gets a `❯` chevron in the gutter. An optional one-line
/// summary above the map counts panes by status: `4 »2 !1 ✓1`.
pub(crate) fn draw_minimap(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
    pal: &Palette,
) {
    let focus_color = pal.accent;
    use gwae_layout::minimap;
    // With a single pane there is nothing to triage; hide the map.
    if !mm.show || (layout.panes.len() <= 1 && layout.rows.len() <= 1) {
        return;
    }
    // Full-intensity tile backgrounds and bright foregrounds, both from the palette.
    let status_bg = |s: PaneStatus| pal.status(s);
    let status_fg = |s: PaneStatus| pal.status(s);
    /// Single-width status glyph (every one is width 1 per unicode-width, so
    /// the painter never has to cut a run around it).
    fn status_glyph(s: PaneStatus) -> char {
        match s {
            PaneStatus::Running => '»',
            PaneStatus::Idle => '!',
            PaneStatus::Done => '✓',
            PaneStatus::Failed => '✗',
        }
    }
    let width = mm.max_width.min(cols.saturating_sub(2).max(1));
    let map = minimap::build(layout, width, cols);
    let height = mm.max_rows.min(map.height).min(rows);
    let ox = cols - map.width;
    let oy = rows - height;
    let put = |out: &mut [Cell], x: u16, y: u16, ch: char, fg: CColor, bg: CColor, bold: bool| {
        if x >= cols || y >= rows {
            return;
        }
        let idx = y as usize * cols as usize + x as usize;
        if let Some(cell) = out.get_mut(idx) {
            *cell = Cell::default();
            cell.ch = ch;
            cell.style.fg = fg;
            cell.style.bg = bg;
            cell.style.bold = bold;
        }
    };
    for tile in &map.cells {
        if tile.y >= height {
            continue;
        }
        let bg = if tile.focus_col {
            focus_color
        } else {
            status_bg(tile.status)
        };
        let neutral = !matches!(bg, CColor::Rgb(..));
        let (fg, bg) = tile_colors(bg, pal);
        let y = oy + tile.y;
        let glyph = status_glyph(tile.status);
        for dx in 0..tile.w {
            let x = ox + tile.x + dx;
            // First cell: the pane's column digit; stacked sub-panes past the
            // first repeat the status glyph instead. Last cell of a wide tile: the status glyph.
            let ch = if dx == 0 {
                if tile.pane_idx == 0 {
                    char::from_digit(tile.column as u32 + 1, 10).unwrap_or('+')
                } else {
                    glyph
                }
            } else if dx == tile.w - 1 && tile.w >= 2 {
                glyph
            } else {
                ' '
            };
            let ink = if neutral && ch == glyph {
                status_fg(tile.status)
            } else {
                fg
            };
            put(
                out,
                x,
                y,
                ch,
                ink,
                bg,
                tile.focus_col || (dx == 0 && tile.pane_idx == 0),
            );
        }
        // Focused strip: a chevron in the gutter just left of the map row.
        if tile.focus_row && tile.x == 0 && ox > 0 {
            put(out, ox - 1, y, '❯', focus_color, CColor::Default, true);
        }
    }
    // Summary bar above the map: total pane count plus per-status tallies
    // (zero counts are skipped). Right-aligned flush with the map.
    if mm.show_counts && oy > 0 {
        let statuses = [
            PaneStatus::Running,
            PaneStatus::Idle,
            PaneStatus::Done,
            PaneStatus::Failed,
        ];
        let mut counts = [0usize; 4];
        for p in layout.panes.values() {
            counts[statuses.iter().position(|s| *s == p.status).unwrap_or(0)] += 1;
        }
        // Segments: (text, fg). The total is dim; each tally is colored.
        let mut segs: Vec<(String, CColor)> = vec![(format!("{}", layout.panes.len()), pal.text)];
        for (i, s) in statuses.iter().enumerate() {
            if counts[i] > 0 {
                segs.push((format!(" {}{}", status_glyph(*s), counts[i]), status_fg(*s)));
            }
        }
        let total_w: usize = segs.iter().map(|(t, _)| t.chars().count()).sum();
        let y = oy - 1;
        let mut x = cols.saturating_sub(total_w as u16);
        let bar_bg = pal.surface;
        for (text, fg) in segs {
            for ch in text.chars() {
                put(out, x, y, ch, fg, bar_bg, true);
                x += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::pty::PtyPane;
    use super::super::render::render_frame;
    use super::super::render::tests::{no_map, pal_accent, pal_of};
    use super::*;
    use crate::theme::Palette;
    use gwae_layout::{Action, FollowScroll, Layout, PaneId, PaneStatus, Viewport};
    use gwae_term::{CColor, Cell};
    use std::collections::HashMap;

    /// The host terminal's own colors for tests that pin terminal-native
    /// behavior: default fg/bg plus ANSI indices. (The prod `TERMINAL` const
    /// was retired with the theme refactor; tests that need a native palette
    /// build it here.)
    fn pal_terminal() -> Palette {
        Palette {
            base: CColor::Default,
            surface: CColor::Default,
            overlay: CColor::Idx(8),
            accent: CColor::Idx(6),
            text: CColor::Default,
            label: CColor::Idx(8),
            running: CColor::Idx(12),
            idle: CColor::Idx(11),
            done: CColor::Idx(10),
            failed: CColor::Idx(9),
        }
    }

    #[test]
    fn toast_anchors_to_pane_bottom_left() {
        let (cols, rows) = (40u16, 10u16);
        let mut frame = vec![Cell::default(); cols as usize * rows as usize];
        let rect = Rect {
            x: 20,
            y: 2,
            w: 20,
            h: 5,
        };
        draw_toast_at(
            &mut frame,
            cols,
            rows,
            "copied 3 lines",
            &Palette::default(),
            true,
            Some(rect),
        );
        let row: String = (0..cols)
            .map(|x| frame[6 * cols as usize + x as usize].ch)
            .collect();
        assert!(row.trim_start().starts_with("copied 3 lines"), "{row:?}");
        assert_eq!(row.find('c'), Some(21), "starts at the pane's left edge");
        // Screen-anchored toasts still land on the last row at column 0.
        let mut frame = vec![Cell::default(); cols as usize * rows as usize];
        draw_toast_at(
            &mut frame,
            cols,
            rows,
            "hi",
            &Palette::default(),
            true,
            None,
        );
        assert_eq!(frame[9 * cols as usize + 1].ch, 'h');
    }

    #[test]
    fn draw_minimap_highlights_focus_bottom_right() {
        use gwae_layout::Width;
        let mut layout = Layout::default();
        // Add a second strip so the map has something to orient against.
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, Width::Cells(20), vec![p]);
        let mut out = vec![Cell::default(); 40 * 8];
        let accent = CColor::Idx(36);
        draw_minimap(
            &mut out,
            40,
            8,
            &layout,
            &crate::config::Minimap::default(),
            &pal_accent(accent),
        );
        // Two strips -> map height 2, width 32 (default max). Bottom-right:
        // ox = 40-32 = 8, oy = 8-2 = 6.
        let cell = |x: usize, y: usize| out[y * 40 + x];
        // An unknown indexed accent uses neutral fill; focus reads via bold.
        let focus = cell(8, 6);
        assert_eq!(
            focus.style.bg,
            CColor::Default,
            "unknown accent uses neutral fill"
        );
        assert!(!focus.style.underline, "no focus underline");
        assert!(focus.style.bold, "focus reads via bold without a fill");
        // The tile carries its column digit (column 0 -> '1').
        assert_eq!(focus.ch, '1', "tile shows the ⌥+digit column address");
        // The non-focused strip's tile is a status tint, not the accent.
        let other = cell(8, 7);
        assert_ne!(other.style.bg, accent, "idle strip must not use the accent");
        assert_ne!(
            other.style.bg,
            CColor::Default,
            "other strip is painted chrome"
        );
        // Fresh panes are Running: a full-intensity Mocha blue tint carrying
        // a `»` glyph at the tile's right edge.
        assert_eq!(
            other.style.bg,
            CColor::Rgb(0x89, 0xb4, 0xfa),
            "running tint"
        );
        let other_end = cell(8 + 31, 7);
        assert_eq!(other_end.ch, '»', "status glyph at the tile end");
        // The focused strip carries a chevron in the gutter left of the map.
        assert_eq!(cell(7, 6).ch, '❯', "focused-strip chevron");
        assert_eq!(cell(7, 6).style.fg, accent);
        // The summary bar sits directly above the map: total 5 panes, all
        // running -> "5 »5" right-aligned at the screen edge.
        let bar: String = (0..40).map(|x| cell(x, 5).ch).collect();
        assert!(
            bar.trim_start().ends_with("5 »5"),
            "summary bar shows totals, got {bar:?}"
        );
        // Nothing above the summary bar is painted by the minimap.
        let above = cell(8, 4);
        assert_eq!(above.style.bg, CColor::Default);
        assert_eq!(above.ch, ' ');
        // A single-pane layout hides the minimap entirely.
        let single = Layout::new(1);
        let mut out2 = vec![Cell::default(); 40 * 8];
        draw_minimap(
            &mut out2,
            40,
            8,
            &single,
            &crate::config::Minimap::default(),
            &pal_accent(accent),
        );
        assert!(
            out2.iter().all(|c| c.style.bg == CColor::Default),
            "no map for one pane"
        );
        // A single *strip* of several panes now shows the map: multiple
        // agents need triage even without a second strip.
        let strip = Layout::default(); // 4 panes, one strip
        let mut out3 = vec![Cell::default(); 40 * 8];
        draw_minimap(
            &mut out3,
            40,
            8,
            &strip,
            &crate::config::Minimap::default(),
            &pal_accent(accent),
        );
        assert!(
            out3.iter().any(|c| c.style.bg != CColor::Default),
            "multi-pane single strip draws the map"
        );
    }

    #[test]
    fn draw_minimap_status_colors_and_failed_glyph() {
        use gwae_layout::Width;
        let mut layout = Layout::default(); // 4 quarter panes on strip 1
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, Width::Cells(20), vec![p]);
        // Statuses: pane1 focused (accent), pane2 done, pane3 failed,
        // pane4 idle; the strip-2 pane keeps Running.
        let ids: Vec<PaneId> = {
            let row = layout.rows[0].clone();
            row.columns.iter().flat_map(|c| c.panes.clone()).collect()
        };
        layout.panes.get_mut(&ids[1]).unwrap().status = PaneStatus::Done;
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Failed;
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Idle;
        let cols = 40usize;
        let mut out = vec![Cell::default(); cols * 8];
        let accent = CColor::Idx(36);
        draw_minimap(
            &mut out,
            cols as u16,
            8,
            &layout,
            &crate::config::Minimap::default(),
            &pal_accent(accent),
        );
        let cell = |x: usize, y: usize| out[y * cols + x];
        // Map: ox=8, oy=6. Strip 1 has 4 tiles of 8 cells each.
        let (ox, y) = (8usize, 6usize);
        assert_eq!(cell(ox, y).style.bg, CColor::Default, "tile 1 neutral");
        assert!(cell(ox, y).style.bold, "tile 1 focused reads via bold");
        assert!(!cell(ox, y).style.underline, "no focus underline");
        assert_eq!(
            cell(ox + 8, y).style.bg,
            CColor::Rgb(0xa6, 0xe3, 0xa1),
            "tile 2 done"
        );
        assert_eq!(
            cell(ox + 16, y).style.bg,
            CColor::Rgb(0xf3, 0x8b, 0xa8),
            "tile 3 failed"
        );
        assert_eq!(
            cell(ox + 24, y).style.bg,
            CColor::Rgb(0xfa, 0xb3, 0x87),
            "tile 4 idle"
        );
        // Tiles carry their ⌥+digit address and end-of-tile status glyph.
        assert_eq!(cell(ox + 8, y).ch, '2');
        assert_eq!(cell(ox + 15, y).ch, '✓', "done glyph");
        assert_eq!(cell(ox + 23, y).ch, '✗', "failed glyph");
        assert_eq!(cell(ox + 31, y).ch, '!', "attention glyph");
        // Summary counts every status: 5 panes, 1 running, 1 attention,
        // 1 done, 1 failed (focused pane is still Running).
        let bar: String = (0..cols).map(|x| cell(x, 5).ch).collect();
        assert!(
            bar.trim_start().ends_with("5 »2 !1 ✓1 ✗1"),
            "summary tallies by status, got {bar:?}"
        );
    }

    #[test]
    fn hud_and_center_minimap_panels_use_terminal_colors() {
        // The HUD and centered minimap are the only chrome that uses
        // `surface` and `text`, and they are reachable only while holding
        // Option, so no render test above covers them. Assert both panels
        // paint the terminal-native palette.
        let mut layout = Layout::default();
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, gwae_layout::Width::Cells(20), vec![p]);
        let term = pal_terminal();
        let (cols, rows) = (80u16, 24u16);

        for (what, draw) in [("hud", 0), ("center minimap", 1)] {
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            if draw == 0 {
                draw_center_hud(&mut out, cols, rows, &term, false, false);
            } else {
                let mm = crate::config::Minimap {
                    mode: crate::config::MinimapMode::Off,
                    ..Default::default()
                };
                draw_center_minimap(
                    &mut out,
                    cols,
                    rows,
                    &layout,
                    &mm,
                    &term,
                    &HudFacts::default(),
                );
            }
            assert!(
                out.iter().any(|c| c.style.bg == term.surface),
                "{what} panel should be filled with the terminal surface"
            );
            assert!(
                out.iter().any(|c| c.style.fg == term.text),
                "{what} text should use the terminal text color"
            );
        }
    }

    #[test]
    fn minimap_status_tints_use_the_palette() {
        // Indexed status colors normalize through `tile_colors`: tile
        // backgrounds stay the terminal default while the status glyphs
        // carry the color. (RGB statuses paint full-intensity tint tiles;
        // covered by `draw_minimap_status_colors_and_failed_glyph`.)
        use gwae_layout::Width;
        let mut layout = Layout::default(); // 4 quarter panes on strip 1
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, Width::Cells(20), vec![p]);
        let ids: Vec<PaneId> = {
            let row = layout.rows[0].clone();
            row.columns.iter().flat_map(|c| c.panes.clone()).collect()
        };
        layout.panes.get_mut(&ids[1]).unwrap().status = PaneStatus::Done;
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Failed;
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Idle;

        let term = pal_terminal();
        let cols = 40usize;
        let mut out = vec![Cell::default(); cols * 8];
        draw_minimap(
            &mut out,
            cols as u16,
            8,
            &layout,
            &crate::config::Minimap::default(),
            &term,
        );
        let cell = |x: usize, y: usize| out[y * cols + x];
        let (ox, y) = (8usize, 6usize);
        // Focus stays visible without a fill: bold, not background.
        assert_eq!(
            cell(ox, y).style.bg,
            CColor::Default,
            "focused tile keeps the terminal background"
        );
        assert!(!cell(ox, y).style.underline, "no focus underline");
        assert!(cell(ox, y).style.bold, "focus reads via bold");
        for (dx, status) in [
            (8, PaneStatus::Done),
            (16, PaneStatus::Failed),
            (24, PaneStatus::Idle),
        ] {
            assert_eq!(
                cell(ox + dx, y).style.bg,
                CColor::Default,
                "{status:?} tile keeps the terminal background"
            );
        }
        // ... and the status glyphs carry the palette colors.
        assert_eq!(cell(ox + 15, y).style.fg, term.done, "done glyph");
        assert_eq!(cell(ox + 23, y).style.fg, term.failed, "failed glyph");
        assert_eq!(cell(ox + 31, y).style.fg, term.idle, "idle glyph");
    }

    #[test]
    fn draw_center_minimap_paints_centered_dashboard() {
        let mut layout = Layout::default();
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, gwae_layout::Width::Cells(20), vec![p]);
        // Mark one pane failed so we can assert status tint appears.
        let pid = *layout.panes.keys().next().unwrap();
        layout.panes.get_mut(&pid).unwrap().status = PaneStatus::Failed;
        let mm = crate::config::Minimap {
            mode: crate::config::MinimapMode::Off,
            ..Default::default()
        };
        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_minimap(
            &mut out,
            cols,
            rows,
            &layout,
            &mm,
            &pal_accent(CColor::Idx(36)),
            &HudFacts::default(),
        );
        let has_frame = out
            .iter()
            .any(|c| c.ch == '╭' || c.ch == '╮' || c.ch == '╰' || c.ch == '╯');
        assert!(has_frame, "center minimap frame painted");
        // At least one digit + status glyph from the minimap tiles should be visible.
        let all: String = out.iter().map(|c| c.ch).collect();
        assert!(
            all.contains('1') || all.contains('2'),
            "digit tile present, got {all:?}"
        );
        assert!(
            all.contains('✗') || all.contains('»'),
            "status glyph present, got {all:?}"
        );
        // A single pane has no grid to triage: the panel stays hidden and
        // the hold is a no-op rather than a wall of hints.
        let single = Layout::new(1);
        let (sc, sr) = (80u16, 24u16);
        let mut out2 = vec![Cell::default(); sc as usize * sr as usize];
        draw_center_minimap(
            &mut out2,
            sc,
            sr,
            &single,
            &mm,
            &pal_accent(CColor::Idx(36)),
            &HudFacts::default(),
        );
        assert!(
            out2.iter().all(|c| c.ch == ' '),
            "one pane paints no dashboard"
        );
    }

    #[test]
    fn center_minimap_footer_is_tally_only() {
        // No key-hint row: the footer is the status tally, or nothing when
        // counts are off. The panel carries no key help; `\u{2325}+/` owns that.
        let layout = Layout::default();
        for show_counts in [true, false] {
            let mm = crate::config::Minimap {
                show_counts,
                ..Default::default()
            };
            let plan = plan_center_minimap(100, 24, &layout, &mm).unwrap();
            assert_eq!(plan.tally_y.is_some(), show_counts);
            let mut out = vec![Cell::default(); 100 * 24];
            paint_center_minimap(
                &mut out,
                100,
                24,
                &layout,
                &plan,
                &Palette::default(),
                &HudFacts::default(),
            );
            let text: String = out.iter().map(|c| c.ch).collect();
            assert!(text.contains('\u{256d}'), "map remains visible");
            for token in ["1-9", "attention", "hjkl", "keys", "⌥+/"] {
                assert!(
                    !text.contains(token),
                    "dashboard must not carry hints, found {token:?}"
                );
            }
        }
        // A single pane has no grid to triage: no plan at all.
        let single = Layout::new(1);
        let mm = crate::config::Minimap::default();
        assert!(plan_center_minimap(100, 24, &single, &mm).is_none());
    }

    #[test]
    fn tile_text_degrades_in_a_fixed_order_as_the_tile_narrows() {
        // Spatial-only: glyph, address, gap, then padding.
        let wide = tile_text(16, "2", '!');
        assert_eq!(wide.chars().count(), 16, "always exactly the tile width");
        assert!(wide.starts_with("!2 "), "got {wide:?}");
        assert!(
            wide.trim().ends_with('2') || wide.contains("!2"),
            "glyph and address kept, got {wide:?}"
        );
        // The gap cell is always blank: no jump marker is painted.
        let plain = tile_text(16, "2", '!');
        assert_eq!(plain, format!("!2 {}", " ".repeat(13)), "got {plain:?}");
        // Narrower still padded blanks, glyph/address/sep always readable.
        let mid = tile_text(6, "2", '!');
        assert_eq!(mid.chars().count(), 6);
        assert_eq!(mid, "!2    ", "glyph, address, gap, padding");
        // Tight: still glyph + address when 3 cells.
        let tight = tile_text(4, "2", '!');
        assert_eq!(tight.chars().count(), 4);
        assert!(
            tight.starts_with("!2"),
            "glyph outlives everything: {tight:?}"
        );
        // Two cells: glyph + address. One cell: the status alone, since a
        // waiting pane matters more than which key jumps to it.
        assert_eq!(tile_text(2, "2", '!'), "!2");
        assert_eq!(tile_text(1, "2", '!'), "!");
        // Two-digit columns fit when there is room and degrade to `+` when
        // there is not: a `1 0` chord addresses column 10, so the map must
        // not silently claim it is column 1.
        assert!(tile_text(4, "10", '\u{bb}').starts_with("\u{bb}10"));
        assert_eq!(tile_text(2, "10", '\u{bb}'), "\u{bb}+");
        assert_eq!(tile_text(3, "10", '\u{bb}'), "\u{bb}10");
        // Whatever the width, the tile is exactly that many cells.
        for w in 1..=24u16 {
            assert_eq!(
                tile_text(w, "12", '\u{bb}').chars().count(),
                w as usize,
                "width {w}"
            );
        }
        // The sep cell is always a blank gap.
        assert_eq!(tile_text(3, "1", '»'), "»1 ");
    }

    #[test]
    fn short_title_keeps_the_part_that_identifies_the_pane() {
        // The shell's `user@host: ~/dir` convention is nearly all chrome.
        assert_eq!(short_title("justin@mac: ~/git/gwae"), "gwae");
        assert_eq!(short_title("jcode"), "jcode");
        assert_eq!(short_title("  cargo test  "), "cargo test");
        // A trailing slash must not leave an empty label.
        assert_eq!(short_title("~/git/gwae/"), "~/git/gwae/");
        // A title is attacker-controlled text; control characters never reach
        // the frame buffer.
        assert_eq!(short_title("ev\u{7}il\u{1b}"), "evil");
        assert_eq!(short_title(""), "");
    }

    #[test]
    fn age_label_is_two_or_three_cells_at_every_scale() {
        assert_eq!(age_label(Duration::from_secs(7)), "7s");
        assert_eq!(age_label(Duration::from_secs(59)), "59s");
        assert_eq!(age_label(Duration::from_secs(60)), "1m");
        assert_eq!(age_label(Duration::from_secs(4 * 60 + 30)), "4m");
        assert_eq!(age_label(Duration::from_secs(3600)), "1h");
        // Absurd ages clamp rather than widening the tile.
        assert_eq!(age_label(Duration::from_secs(3600 * 500)), "99h");
        for d in [1u64, 59, 60, 3599, 3600, 3600 * 500] {
            assert!(age_label(Duration::from_secs(d)).chars().count() <= 3);
        }
    }

    #[test]
    fn contrast_fg_defers_to_terminal_defaults_for_unknown_colors() {
        let term = pal_terminal();
        // Unknown colors defer to terminal defaults rather than guessing white ink.
        assert_eq!(contrast_fg(CColor::Idx(4), &term), CColor::Default);
        assert_eq!(contrast_fg(CColor::Default, &term), CColor::Default);
    }

    #[test]
    fn rgb_tile_text_meets_contrast_target() {
        // Sample the RGB cube, including the midtones the old heuristic missed.
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let bg = CColor::Rgb(r, g, b);
                    let (fg, actual_bg) = tile_colors(bg, &Palette::default());
                    assert_eq!(actual_bg, bg);
                    let CColor::Rgb(fr, fg, fb) = fg else {
                        panic!("RGB ink")
                    };
                    let a = relative_luminance(r, g, b);
                    let b = relative_luminance(fr, fg, fb);
                    assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn unknown_tile_colors_never_guess_white_on_yellow() {
        for idx in 0..=255 {
            assert_eq!(
                tile_colors(CColor::Idx(idx), &pal_terminal()),
                (CColor::Default, CColor::Default)
            );
        }
    }

    #[test]
    fn terminal_dashboard_keeps_addresses_neutral_and_focus_visible() {
        let (mut layout, ids) = dashboard_layout(4);
        for (id, status) in ids.iter().zip([
            PaneStatus::Idle,
            PaneStatus::Running,
            PaneStatus::Done,
            PaneStatus::Failed,
        ]) {
            layout.panes.get_mut(id).unwrap().status = status;
        }
        let plan =
            plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default()).unwrap();
        let mut out = vec![Cell::default(); 100 * 24];
        paint_center_minimap(
            &mut out,
            100,
            24,
            &layout,
            &plan,
            &pal_terminal(),
            &HudFacts::default(),
        );
        for tile in &plan.map.cells {
            let y = plan.row_y[tile.y as usize] as usize;
            let cells = &out[y * 100 + (plan.map_ox + tile.x) as usize..][..tile.w as usize];
            assert!(cells.iter().all(|c| c.style.bg == CColor::Default));
            assert!(
                cells.iter().all(|c| !c.style.underline),
                "no focus underline"
            );
            for c in cells.iter().filter(|c| c.ch.is_ascii_digit()) {
                assert_eq!(c.style.fg, CColor::Default);
            }
            // Without a fill or an underline, focus reads via bold: every
            // cell of the focused tile is bold, while the rest of a tile is
            // plain (only its address cell is bold).
            if tile.focus_col {
                assert!(
                    cells.iter().all(|c| c.style.bold),
                    "focused tile reads via bold"
                );
            } else if tile.w >= 2 {
                assert!(
                    cells.iter().any(|c| !c.style.bold),
                    "unfocused tile is not all bold"
                );
            }
        }
        let mut out = vec![Cell::default(); 100 * 24];
        draw_minimap(
            &mut out,
            100,
            24,
            &layout,
            &crate::config::Minimap::default(),
            &pal_terminal(),
        );
        let digits: Vec<_> = out[23 * 100..]
            .iter()
            .filter(|c| c.ch.is_ascii_digit())
            .collect();
        assert!(!digits.is_empty());
        assert!(digits
            .iter()
            .all(|c| c.style.bg == CColor::Default && c.style.fg == CColor::Default));
        assert!(
            digits.iter().all(|c| !c.style.underline),
            "no focus underline"
        );
        assert!(
            out.iter().any(|c| c.ch == '❯'),
            "the focused strip keeps its chevron"
        );
    }

    #[test]
    fn dashboard_tiles_carry_no_jump_marker() {
        let (mut layout, ids) = dashboard_layout(4);
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Idle;
        let facts = HudFacts::default();
        let out = paint_dashboard(&layout, &facts, 100, 24);
        let text = screen_rows(&out, 100).join("\n");
        // Spatial-only: titles and ages are not rendered; only glyph+addr.
        assert!(
            !text.contains("jcode") && !text.contains("cargo"),
            "spatial tiles carry no title text, got:\n{text}"
        );
        assert!(
            !text.contains('\u{25b8}'),
            "no smart-jump marker is painted, got:\n{text}"
        );
        assert!(
            !text.contains("4m"),
            "spatial tiles carry no age, got:\n{text}"
        );
    }

    #[test]
    fn dashboard_underlines_the_columns_that_are_on_screen() {
        // Eight quarter-width columns: only four fit, so the strip scrolls
        // and the map has something to point at.
        let (mut layout, _) = dashboard_layout(8);
        let plan = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        assert_eq!(plan.rulers.len(), 1, "one strip");
        let (first, last) = plan.rulers[0].expect("an overflowing strip gets a ruler");
        assert_eq!((first, last), (0, 3), "columns 1-4 are on screen at rest");
        // Scrolling the strip moves the ruler with it.
        let v = Viewport::new(100);
        let f = FollowScroll::default();
        for _ in 0..5 {
            let _ = layout.apply(Action::FocusRight, v, f);
        }
        let plan2 = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        let (f2, l2) = plan2.rulers[0].expect("still overflowing");
        assert!(
            f2 > first && l2 > last,
            "the visible span follows the viewport: {f2}..{l2} vs {first}..{last}"
        );
        // A strip that fits entirely has nothing to point out.
        let (small, _) = dashboard_layout(2);
        let plan3 = plan_center_minimap(100, 24, &small, &crate::config::Minimap::default())
            .expect("dashboard fits");
        assert_eq!(plan3.rulers[0], None, "no ruler when the strip fits");
    }

    #[test]
    fn clicking_a_tile_resolves_to_the_pane_it_draws() {
        let (layout, ids) = dashboard_layout(4);
        let plan = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        let y = plan.row_y[0];
        // Every cell of a tile belongs to that tile's pane, so a click
        // anywhere on it focuses the right pane.
        for tile in &plan.map.cells {
            for dx in 0..tile.w {
                assert_eq!(
                    hud_pane_at(&plan, plan.map_ox + tile.x + dx, y),
                    Some(tile.pane),
                    "column {} cell {dx}",
                    tile.column
                );
            }
        }
        assert_eq!(hud_pane_at(&plan, plan.map_ox, y), Some(ids[0]));
        // The frame and the tally row are not tiles.
        assert_eq!(hud_pane_at(&plan, plan.rect.x, y), None, "frame");
        if let Some(tally_y) = plan.tally_y {
            assert_eq!(hud_pane_at(&plan, plan.map_ox, tally_y), None, "footer");
        }
    }

    #[test]
    fn truncated_strips_are_counted_not_silently_dropped() {
        let mut layout = Layout::default();
        for _ in 0..9 {
            let r = layout.new_row();
            let p = layout.alloc_pane();
            layout.add_column(r, gwae_layout::Width::Cells(20), vec![p]);
        }
        let mm = crate::config::Minimap {
            max_rows: 3,
            ..Default::default()
        };
        let plan = plan_center_minimap(100, 24, &layout, &mm).expect("dashboard fits");
        assert_eq!(plan.row_y.len(), 3, "capped at max_rows");
        assert_eq!(plan.hidden, 7, "the rest are counted, not forgotten");
        let mut out = vec![Cell::default(); 100 * 24];
        paint_center_minimap(
            &mut out,
            100,
            24,
            &layout,
            &plan,
            &pal_accent(CColor::Idx(36)),
            &HudFacts::default(),
        );
        let text = screen_rows(&out, 100).join("\n");
        assert!(text.contains("+7 strips"), "says how many are cut:\n{text}");
    }

    #[test]
    fn strips_are_labelled_by_number() {
        let mut layout = Layout::default();
        let r2 = layout.new_row();
        let p = layout.alloc_pane();
        layout.add_column(r2, gwae_layout::Width::Cells(20), vec![p]);
        let r3 = layout.new_row();
        let p2 = layout.alloc_pane();
        layout.add_column(r3, gwae_layout::Width::Cells(20), vec![p2]);
        let plan = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        assert_eq!(plan.gutter[1], "2");
        assert_eq!(plan.gutter[2], "3");
    }

    #[test]
    fn the_dashboard_never_paints_outside_its_own_rect() {
        // Hostile sizes: the panel either fits entirely or is not drawn.
        let (layout, _) = dashboard_layout(6);
        for (cols, rows) in [(20u16, 8u16), (24, 9), (40, 10), (80, 24), (200, 60)] {
            let mm = crate::config::Minimap::default();
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            draw_center_minimap(
                &mut out,
                cols,
                rows,
                &layout,
                &mm,
                &pal_accent(CColor::Idx(36)),
                &HudFacts::default(),
            );
            let plan = plan_center_minimap(cols, rows, &layout, &mm);
            let r = plan.as_ref().map(|p| p.rect).unwrap_or(Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            });
            for y in 0..rows {
                for x in 0..cols {
                    let c = out[y as usize * cols as usize + x as usize];
                    if c.ch == ' ' && c.style.bg == CColor::Default {
                        continue;
                    }
                    assert!(
                        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h,
                        "painted ({x},{y}) outside {r:?} at {cols}x{rows}"
                    );
                }
            }
        }
    }

    #[test]
    fn grid_boundaries_are_identical_whatever_the_occupancy() {
        // Regression: placeholder boxes for empty cells were tiled with their
        // own `cols / 4` integer arithmetic while live columns came from the
        // twelfths accumulator in `column_x_ranges`. At viewport widths not
        // divisible by 4 the two disagree (at 205 cols, `cols / 4` is 51 but
        // the real quarter boundaries are 51/103/154/205), so cell `1.2` sat
        // at a different screen column when empty than when populated, and
        // the grid jittered as panes appeared or as you moved between strips
        // with different fill. Every occupancy must paint the same rules.
        let rows: u16 = 10;
        let white = CColor::Rgb(0xff, 0xff, 0xff);
        let red = CColor::Rgb(0xff, 0, 0);
        // Widths deliberately chosen to be indivisible by 2, 3 and 4, where
        // the two rounding schemes diverge.
        for cols in [80u16, 81, 82, 83, 100, 101, 137, 205, 206, 207] {
            let mut reference: Option<Vec<u16>> = None;
            for filled in 1..=4usize {
                let layout = Layout::new(filled);
                let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
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
                let b = frame_boundaries(&out, cols, rows);
                assert!(
                    !b.is_empty(),
                    "no vertical rules at cols={cols} filled={filled}"
                );
                match &reference {
                    None => reference = Some(b),
                    Some(r) => assert_eq!(
                        &b, r,
                        "grid moved at cols={cols} with {filled} live columns: \
                         {b:?} != {r:?}"
                    ),
                }
            }
            // And the grid really is the quarter grid: four boxes, so five
            // rules counting both outer edges.
            let r = reference.unwrap();
            assert_eq!(
                r.len(),
                5,
                "expected a 4-box grid at cols={cols}, got {r:?}"
            );
            assert_eq!(r[0], 0, "grid starts flush at cols={cols}");
            assert_eq!(
                *r.last().unwrap(),
                cols - 1,
                "grid ends flush at cols={cols}"
            );
        }
    }

    #[test]
    fn quasimode_chrome_and_hud_defaults() {
        // Defaults: no bottom row (Off), centered Alt HUD/minimap only.
        let mm = crate::config::Minimap::default();
        assert_eq!(mm.mode, crate::config::MinimapMode::Off);
        assert_eq!(mm.chrome_rows(), 0);
        // should_paint: Off never paints; Overlay/EdgeTicks paint when enabled.
        assert!(!mm.should_paint(false, false), "Off hidden");
        assert!(!crate::config::Minimap {
            mode: crate::config::MinimapMode::Off,
            ..mm
        }
        .should_paint(true, true));
        assert!(crate::config::Minimap {
            mode: crate::config::MinimapMode::Overlay,
            ..mm
        }
        .should_paint(false, false));
        assert!(crate::config::Minimap {
            mode: crate::config::MinimapMode::EdgeTicks,
            ..mm
        }
        .should_paint(false, false));
        // Legacy values parse as Off (bottom row removed).
        let legacy: crate::config::Config =
            toml::from_str("[minimap]\nmode=\"reserved_quasimode\"").unwrap();
        assert_eq!(legacy.minimap.mode, crate::config::MinimapMode::Off);
        let legacy2: crate::config::Config =
            toml::from_str("[minimap]\nmode=\"reserved\"").unwrap();
        assert_eq!(legacy2.minimap.mode, crate::config::MinimapMode::Off);
    }

    #[test]
    fn draw_minimap_overlay_still_paints_without_chrome() {
        // Overlay path does not depend on chrome rows.
        let mut layout = Layout::default();
        let r2 = layout.new_row();
        let pid = layout.alloc_pane();
        layout.add_column(r2, gwae_layout::Width::Cells(20), vec![pid]);
        let mm = crate::config::Minimap {
            mode: crate::config::MinimapMode::Overlay,
            ..Default::default()
        };
        let mut out = vec![Cell::default(); 40 * 8];
        draw_minimap(&mut out, 40, 8, &layout, &mm, &pal_accent(CColor::Idx(36)));
        let any = out
            .iter()
            .any(|c| c.style.bg != CColor::Default && c.ch != ' ');
        assert!(any, "overlay draws tiles");
    }

    #[test]
    fn center_hud_paints_centered_box_with_cheat_sheet() {
        // HUD flash: centered box with a concise keybind cheat-sheet. Pane
        // attention is deliberately absent; ambient chrome + `⌥+g` cover it.
        let mut layout = Layout::default();
        let ids: Vec<PaneId> = layout.rows[0]
            .columns
            .iter()
            .flat_map(|c| c.panes.clone())
            .collect();
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Failed;
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Idle;
        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        let frame_color = CColor::Rgb(0x74, 0xc7, 0xec);
        draw_center_hud(&mut out, cols, rows, &pal_accent(frame_color), false, false);
        let has_frame = out
            .iter()
            .any(|c| c.ch == '╭' || c.ch == '╮' || c.ch == '╰' || c.ch == '╯');
        assert!(has_frame, "HUD box frame painted");
        // Whole buffer must contain the keybind lines and no attention nag.
        let all: Vec<String> = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| out[y as usize * cols as usize + x as usize].ch)
                    .collect()
            })
            .collect();
        assert!(
            !all.iter().any(|s| s.contains("needs you")),
            "HUD must not nag about attention, got {all:?}"
        );
        assert!(
            all.iter().any(|s| s.contains("focus left")),
            "HUD cheat-sheet present, got {all:?}"
        );
        assert!(
            all.iter().any(|s| s.contains("scroll up")),
            "HUD cheat-sheet covers Ctrl+Shift+K scroll, got {all:?}"
        );
        assert!(
            all.iter().any(|s| s.contains("click")),
            "HUD cheat-sheet covers mouse, got {all:?}"
        );
        // With no attention at all the cheat-sheet is unchanged.
        for pid in &ids {
            layout.panes.get_mut(pid).unwrap().status = PaneStatus::Running;
        }
        let mut out2 = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_hud(
            &mut out2,
            cols,
            rows,
            &pal_accent(frame_color),
            false,
            false,
        );
        let all2: Vec<String> = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| out2[y as usize * cols as usize + x as usize].ch)
                    .collect()
            })
            .collect();
        assert!(
            all2.iter().any(|s| s.contains("focus left")),
            "startup HUD shows cheat-sheet, got {all2:?}"
        );
        // Spreadsheet shape: header row, ruled separator, aligned columns.
        assert!(
            all2.iter()
                .any(|s| s.contains("key") && s.contains("navigate")),
            "HUD has table headers, got {all2:?}"
        );
        assert!(
            all2.iter().any(|s| s.contains('┼')),
            "HUD has a ruled header separator, got {all2:?}"
        );
        let cols_at: Vec<Vec<usize>> = all2
            .iter()
            .filter(|s| {
                s.contains('│') && s.contains("focus left")
                    || s.contains('│') && s.contains("smart jump")
            })
            .map(|s| {
                s.chars()
                    .enumerate()
                    .filter(|(_, c)| *c == '│')
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        assert!(cols_at.len() >= 2, "found body rows, got {all2:?}");
        assert!(
            cols_at.windows(2).all(|w| w[0] == w[1]),
            "column rules align across rows: {cols_at:?}"
        );
        // Tiny viewport: nothing painted.
        let mut tiny = vec![Cell::default(); 10 * 4];
        draw_center_hud(&mut tiny, 10, 4, &pal_accent(frame_color), false, false);
        assert!(
            tiny.iter().all(|c| c.ch == ' '),
            "tiny viewport draws no HUD"
        );
    }

    /// A wide grid whose columns overflow the viewport, so the dashboard has
    /// something to say about titles, ages, jumps and the visible span.
    /// Returns the layout and its pane ids in column order.
    fn dashboard_layout(columns: usize) -> (Layout, Vec<PaneId>) {
        use gwae_layout::{Preset, Width};
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        let mut ids = Vec::new();
        for _ in 0..columns {
            let p = layout.alloc_pane();
            ids.push(p);
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
        }
        (layout, ids)
    }

    /// Every character the dashboard painted, as one string per screen row.
    fn screen_rows(out: &[Cell], cols: u16) -> Vec<String> {
        out.chunks(cols as usize)
            .map(|r| r.iter().map(|c| c.ch).collect())
            .collect()
    }

    fn paint_dashboard(layout: &Layout, facts: &HudFacts, cols: u16, rows: u16) -> Vec<Cell> {
        let mm = crate::config::Minimap::default();
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_minimap(
            &mut out,
            cols,
            rows,
            layout,
            &mm,
            &pal_accent(CColor::Idx(36)),
            facts,
        );
        out
    }

    #[test]
    fn keep_awake_badge_stamps_the_dashboard_frame_only_while_awake() {
        // The keep-awake state reads on the Option HUD chrome: the badge is
        // stamped onto the panel frame while the guard is active, and the
        // frame is plain box-drawing when it is not.
        let (layout, _) = dashboard_layout(4);
        for keep_awake in [false, true] {
            let facts = HudFacts {
                keep_awake,
                ..HudFacts::default()
            };
            let out = paint_dashboard(&layout, &facts, 100, 24);
            let text = screen_rows(&out, 100).join("\n");
            assert_eq!(
                text.contains("keep-awake"),
                keep_awake,
                "badge reads on the HUD frame iff awake:\n{text}"
            );
        }
    }

    #[test]
    fn keep_awake_badge_stamps_the_center_help_frame_only_while_awake() {
        // Same promise for the startup / `⌥+/` help panel.
        let (cols, rows) = (80u16, 24u16);
        for keep_awake in [false, true] {
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            draw_center_hud(&mut out, cols, rows, &pal_terminal(), keep_awake, false);
            let text: String = out.iter().map(|c| c.ch).collect();
            assert_eq!(
                text.contains("keep-awake"),
                keep_awake,
                "badge reads on the help frame iff awake: {text:?}"
            );
        }
    }

    #[test]
    fn keep_awake_badge_sits_top_center_of_the_help_frame() {
        // The badge interrupts the top frame row at its horizontal center,
        // not tucked into a corner.
        let (cols, rows) = (80u16, 24u16);
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_hud(&mut out, cols, rows, &pal_terminal(), true, false);
        let lines = screen_rows(&out, cols);
        let badge = crate::keepawake::KEEP_AWAKE_BADGE;
        let badge_w = badge.chars().count();
        let row = lines
            .iter()
            .find(|l| l.contains("keep-awake"))
            .expect("badge must stamp one row");
        // Char-based indexing: the rounded corners are multi-byte, so byte
        // offsets would misplace the expectation.
        let chars: Vec<char> = row.chars().collect();
        let frame_start = chars.iter().position(|&c| c == '╭').unwrap();
        let frame_end = chars.iter().position(|&c| c == '╮').unwrap();
        let frame_w = frame_end - frame_start + 1;
        let needle: Vec<char> = "keep-awake".chars().collect();
        let start_char = chars
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .expect("badge text must read in row chars");
        let expected = frame_start + (frame_w - badge_w) / 2 + 1;
        assert_eq!(
            start_char, expected,
            "badge must be centered on the top frame row:\n{row}"
        );
    }

    #[test]
    fn dev_badge_stamps_the_dashboard_frame_only_in_dev() {
        // The dev tab reads DEV on the Option HUD chrome; the stable tab next
        // to it renders a plain frame. Same stamp contract as keep-awake,
        // gated on the dev session flag instead of the guard.
        let (layout, _) = dashboard_layout(4);
        for dev in [false, true] {
            let facts = HudFacts {
                dev,
                ..HudFacts::default()
            };
            let out = paint_dashboard(&layout, &facts, 100, 24);
            let text = screen_rows(&out, 100).join("\n");
            assert_eq!(
                text.contains("DEV"),
                dev,
                "DEV reads on the HUD frame iff dev:\n{text}"
            );
        }
    }

    #[test]
    fn dev_badge_stamps_the_center_help_frame_only_in_dev() {
        // Same promise for the startup / `⌥+/` help panel.
        let (cols, rows) = (80u16, 24u16);
        for dev in [false, true] {
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            draw_center_hud(&mut out, cols, rows, &pal_terminal(), false, dev);
            let text: String = out.iter().map(|c| c.ch).collect();
            assert_eq!(
                text.contains("DEV"),
                dev,
                "DEV reads on the help frame iff dev: {text:?}"
            );
        }
    }

    #[test]
    fn dev_badge_sits_bottom_center_of_the_help_frame() {
        // DEV interrupts the bottom frame row at its horizontal center,
        // mirroring keep-awake on the top row.
        let (cols, rows) = (80u16, 24u16);
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_hud(&mut out, cols, rows, &pal_terminal(), false, true);
        let lines = screen_rows(&out, cols);
        let badge = crate::reload::DEV_BADGE;
        let badge_w = badge.chars().count();
        let row = lines
            .iter()
            .rev()
            .find(|l| l.contains("DEV"))
            .expect("badge must stamp one row");
        let chars: Vec<char> = row.chars().collect();
        let frame_start = chars.iter().position(|&c| c == '╰').unwrap();
        let frame_end = chars.iter().rposition(|&c| c == '╯').unwrap();
        let frame_w = frame_end - frame_start + 1;
        let needle: Vec<char> = "DEV".chars().collect();
        let start_char = chars
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .expect("badge text must read in row chars");
        let expected = frame_start + (frame_w - badge_w) / 2 + 1;
        assert_eq!(
            start_char, expected,
            "badge must be centered on the bottom frame row:\n{row}"
        );
    }

    /// The vertical rules of the skeleton grid, by screen column.
    fn frame_boundaries(out: &[Cell], cols: u16, rows: u16) -> Vec<u16> {
        let y = (rows / 2) as usize;
        (0..cols)
            .filter(|x| {
                let c = out[y * cols as usize + *x as usize];
                c.ch == '│'
            })
            .collect()
    }
}
