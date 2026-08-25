//! TUI: the M0 render/event loop (single process, one focused row).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant};

use crossterm::cursor;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyEventState, KeyModifiers, KeyboardEnhancementFlags, ModifierKeyCode, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};
use strimux_layout::{Action, FollowScroll, Layout, PaneId, PaneStatus, Viewport};
use strimux_term::{CColor, Cell, KittyApcExtractor, Size as GridSize, TermGrid, Vt100Grid};

use crate::config::Config;

fn chrome_rows(cfg: &Config) -> u16 {
    cfg.minimap.chrome_rows()
}

fn has_attention(layout: &Layout) -> bool {
    layout
        .panes
        .values()
        .any(|p| matches!(p.status, PaneStatus::Idle | PaneStatus::Failed))
}

fn is_alt_modifier(ev: &KeyEvent) -> bool {
    matches!(
        ev.code,
        KeyCode::Modifier(ModifierKeyCode::LeftAlt) | KeyCode::Modifier(ModifierKeyCode::RightAlt)
    )
}

fn physical_shift(ev: &KeyEvent) -> bool {
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        return true;
    }
    // With Kitty REPORT_ALTERNATE_KEYS shifted keys arrive as their shifted
    // codepoint with SHIFT cleared (e.g. Shift+h -> 'H'). Caps Lock alone
    // also yields uppercase but sets CAPS_LOCK state, so we must not confuse
    // the two: a Caps-generated 'H' must NOT be treated as an intentional Shift.
    if let KeyCode::Char(c) = ev.code {
        if c.is_ascii_uppercase() && !ev.state.contains(KeyEventState::CAPS_LOCK) {
            return true;
        }
    }
    false
}

fn logical_char(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// How long a pane without OSC 133 shell integration must stay silent before
/// the activity heuristic calls it idle ("wants attention") instead of
/// working. Long enough that a compiler pausing between crates doesn't
/// flicker, short enough that a finished agent surfaces quickly.
const QUIET_AFTER: Duration = Duration::from_secs(4);

/// True while the user is inside the `Ctrl-b` prefix and the next key is a
/// strimux command rather than pane input. Works on every terminal, no
/// Option-as-Alt config required.
static IN_PREFIX: AtomicBool = AtomicBool::new(false);

/// A PTY-backed pane: its emulator grid plus the I/O handles.
pub struct PtyPane {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn PtyChild + Send + Sync>,
    pub grid: Vt100Grid,
    pub alive: bool,
    pub h_scroll: i32,
    /// When the pane last emitted any output (activity heuristic).
    pub last_output: Instant,
    /// True once the child has spoken OSC 133; from then on the explicit
    /// protocol owns the status and the activity heuristic stands down.
    pub saw_osc133: bool,
    /// Streaming scanner that recovers Kitty graphics APCs from this pane's
    /// raw output so they can be forwarded to the host terminal (vt100
    /// swallows them, which would otherwise leave images invisible).
    pub apc: KittyApcExtractor,
}

/// Message a per-pane reader thread sends to the main loop.
enum PaneMsg {
    Output(PaneId, Vec<u8>),
    Exited(PaneId),
}

/// Rectangle in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

/// Detect terminal capability/status queries from a child and produce a reply
/// sequence. Answers Device Attributes (DA) and Device Status Report (DSR) so
/// shells like fish don't warn that they "could not read a response to the
/// Primary Device Attribute query". Returns None when no query is present.
fn query_reply(bytes: &[u8]) -> Option<Vec<u8>> {
    // DA / DA1: ESC [ c or ESC [ 0;1;2c  ->  VT100 with advanced video option.
    if bytes.windows(3).any(|w| w == b"\x1b[c")
        || bytes
            .windows(4)
            .any(|w| w == b"\x1b[0c" || w == b"\x1b[1c" || w == b"\x1b[2c")
    {
        return Some(b"\x1b[?1;2c".to_vec());
    }
    // DSR operating status: ESC [ 5 n -> "OK".
    if bytes.windows(4).any(|w| w == b"\x1b[5n") {
        return Some(b"\x1b[0n".to_vec());
    }
    // DSR cursor position: ESC [ 6 n -> report row;col (1;1 is a safe answer).
    if bytes.windows(4).any(|w| w == b"\x1b[6n") {
        return Some(b"\x1b[1;1R".to_vec());
    }
    None
}

/// Scan a PTY output chunk for OSC 133 shell-integration markers and return
/// the status implied by the *last* one present. The protocol (emitted by
/// fish/zsh integrations and agent harnesses like jcode):
///   `133;A`   prompt shown  -> the pane is waiting for input (Idle)
///   `133;C`   command start -> the pane is working (Running)
///   `133;D;n` command done  -> Done when n == 0 (or omitted), Failed else
/// `133;B` (prompt end / input start) is ignored: focus-wise it is still the
/// prompt. Sequences may be terminated by BEL or ST and may split across
/// reads; a marker whose terminator hasn't arrived yet is picked up on a
/// later chunk (the payload we need sits right after the `133;` prefix).
fn scan_osc133(bytes: &[u8]) -> Option<PaneStatus> {
    let mut status = None;
    let mut i = 0;
    while i + 6 <= bytes.len() {
        // ESC ] 1 3 3 ;
        if bytes[i] == 0x1b && bytes[i + 1] == b']' && bytes[i + 2..i + 6] == *b"133;" {
            let rest = &bytes[i + 6..];
            match rest.first() {
                Some(b'A') => status = Some(PaneStatus::Idle),
                Some(b'C') => status = Some(PaneStatus::Running),
                Some(b'D') => {
                    // Exit code follows as `;n` up to BEL/ESC; absent means 0.
                    let code: u32 = rest
                        .get(1)
                        .filter(|c| **c == b';')
                        .map(|_| {
                            rest[2..]
                                .iter()
                                .take_while(|c| c.is_ascii_digit())
                                .fold(0u32, |a, c| a.saturating_mul(10) + (*c - b'0') as u32)
                        })
                        .unwrap_or(0);
                    status = Some(if code == 0 {
                        PaneStatus::Done
                    } else {
                        PaneStatus::Failed
                    });
                }
                _ => {}
            }
            i += 6;
        } else {
            i += 1;
        }
    }
    status
}

/// Whether the terminal strimux itself runs in understands Kitty graphics.
///
/// Env-based, mirroring how jcode and ratatui-image decide: Kitty exports
/// `KITTY_WINDOW_ID`, Kitty-protocol terminals (Ghostty, WezTerm's kitty mode)
/// advertise via TERM/TERM_PROGRAM. `STRIMUX_KITTY_GRAPHICS=1/0` overrides
/// detection either way (e.g. strimux inside ssh where env vars were dropped).
fn host_supports_kitty_graphics() -> bool {
    if let Ok(v) = std::env::var("STRIMUX_KITTY_GRAPHICS") {
        return matches!(v.trim(), "1" | "true" | "yes" | "on");
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if term.contains("kitty") || term.contains("ghostty") {
        return true;
    }
    let prog = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    prog.contains("kitty") || prog.contains("ghostty") || prog.contains("wezterm")
}

/// Strip control characters that could escape an OSC title sequence and clip
/// the result to a reasonable window-title length. Prevents a malicious child
/// title from running state-changing escapes on the host terminal.
fn sanitize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        if (c as u32) < 0x20 || c == '\x7f' {
            continue;
        }
        out.push(c);
        if out.chars().count() >= 256 {
            break;
        }
    }
    out
}

/// Tell the host terminal what title to display by writing OSC 2 (window
/// title) terminated with ST. Forwarding the focused pane's inner title makes
/// strimux effectively transparent to the host's title/status bar: the outer
/// window shows e.g. a jcode session title instead of "strimux".
fn emit_title(stdout: &mut impl Write, title: &str) -> std::io::Result<()> {
    write!(stdout, "\x1b]2;{}\x1b\\", sanitize_title(title))?;
    stdout.flush()
}

/// Spawn a PTY running `cmd` at the given grid size, wiring a reader thread.
fn spawn_pane(
    id: PaneId,
    cmd: &str,
    gw: u16,
    gh: u16,
    tx: Sender<PaneMsg>,
) -> Result<PtyPane, String> {
    let pty = native_pty_system();
    let size = PtySize {
        rows: gh,
        cols: gw,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty.openpty(size).map_err(|e| format!("openpty: {e}"))?;
    let master = pair.master;
    let slave = pair.slave;
    let argv = if cmd.trim().is_empty() {
        vec![std::env::var("SHELL").unwrap_or_else(|_| "sh".into())]
    } else {
        shell_split(cmd)
    };
    if argv.is_empty() {
        return Err("empty command".into());
    }
    let mut cb = CommandBuilder::new(&argv[0]);
    for a in &argv[1..] {
        cb.arg(a);
    }
    cb.env("STRIMUX_PANE", id.to_string());
    cb.env("TERM", "xterm-256color");
    let child = slave.spawn_command(cb).map_err(|e| format!("spawn: {e}"))?;
    drop(slave);

    let mut reader = master
        .try_clone_reader()
        .map_err(|e| format!("reader: {e}"))?;
    let writer = master.take_writer().map_err(|e| format!("writer: {e}"))?;

    let tid = id;
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(PaneMsg::Exited(tid));
                    break;
                }
                Ok(n) => {
                    if tx.send(PaneMsg::Output(tid, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    master.resize(size).map_err(|e| format!("resize: {e}"))?;

    Ok(PtyPane {
        master,
        writer,
        child,
        grid: Vt100Grid::new(GridSize { cols: gw, rows: gh }),
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        apc: KittyApcExtractor::new(),
    })
}

/// Naive shell splitter: split on whitespace, keeping simple quoting (\"..\").
fn shell_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in cmd.chars() {
        match c {
            '\'' | '"' => {
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One visible pane on screen: where to draw it and which grid slice to show.
struct PaneView {
    pid: PaneId,
    rect: Rect,     // screen rect (already clipped to viewport horizontally)
    col_x0: u16,    // grid column at the left edge of `rect` (before content scroll)
    h_scroll: i32,  // pane content scroll in cells
    grid_cols: u16, // full logical content width of the grid
    grid_rows: u16, // vertical size of the grid
}

/// Compute visible pane views for the focused row.
///
/// With `inset` (skeleton mode) every pane's rect is shrunk by 1 cell on all
/// sides of its column box so content sits *inside* the frame instead of
/// being overlaid by it: nothing a program draws is ever covered.
fn focused_pane_views(
    layout: &Layout,
    cols: u16,
    rows: u16,
    content_width: u16,
    panes: &HashMap<PaneId, PtyPane>,
    inset: bool,
) -> Vec<PaneView> {
    focused_pane_views_with_chrome(layout, cols, rows, content_width, panes, inset, 0)
}

fn focused_pane_views_with_chrome(
    layout: &Layout,
    cols: u16,
    rows: u16,
    content_width: u16,
    panes: &HashMap<PaneId, PtyPane>,
    inset: bool,
    chrome_rows: u16,
) -> Vec<PaneView> {
    let strip_h = rows.saturating_sub(chrome_rows).max(1);
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
        // Content spans the column box minus the frame ring.
        let cs = s + b;
        let ce = e - b;
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
        let wv = right.saturating_sub(left); // visible width
        if wv == 0 {
            continue;
        }
        let Some(col) = layout.focused_row().and_then(|r| r.columns.get(ci)) else {
            continue;
        };
        let full_w = (ce - cs) as u16;
        // The emulator matches the pane's content area exactly (the column
        // width minus any frame inset) unless an explicit content_width
        // extends the logical width for horizontal scrolling.
        let grid_cols = full_w.max(content_width);
        let col_x0 = (left as i32 - sx).max(0) as u16; // grid col at `left`
        let p = col.panes.len().max(1);
        let gap = 1u16;
        // Vertical content area: the strip minus the top/bottom frame rows.
        let inner_top = b as u16;
        let inner_h = ((strip_h as i32) - 2 * b).max(1) as u16;
        let inner_bottom = inner_top + inner_h;
        let pane_h = ((inner_h as i32 - (p as i32 - 1) * gap as i32) / p as i32).max(1) as u16;
        for (pi, pid) in col.panes.iter().enumerate() {
            let y = inner_top + (pi as u16) * (pane_h + gap);
            let h = pane_h.min(inner_bottom.saturating_sub(y));
            if h == 0 {
                continue;
            }
            let h_scroll = panes.get(pid).map(|p| p.h_scroll).unwrap_or(0);
            out.push(PaneView {
                pid: *pid,
                rect: Rect {
                    x: left,
                    y,
                    w: wv,
                    h,
                },
                col_x0,
                h_scroll,
                grid_cols,
                grid_rows: h,
            });
        }
    }
    out
}

/// The grid-column range `[start, end)` of a pane's content revealed by `w`
/// screen cells, given the viewport column offset `col_x0`, the pane content
/// scroll `h_scroll`, and the content width `grid_cols`. Returns `None` when
/// the window is fully clipped (offscreen or past the content).
fn pane_window(col_x0: u16, h_scroll: i32, w: u16, grid_cols: u16) -> Option<(u16, u16)> {
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

/// Build the full frame (cols x rows).
#[allow(clippy::too_many_arguments)]
/// The 1-based *position* of the focused strip in the stack, used for the
/// `strip.cell` addresses. Deliberately not the `RowId`: ids are monotonic
/// allocation counters, so creating and discarding strips with j/k would make
/// the visible label of the second strip climb (2, 3, 4, ...) forever.
fn strip_number(layout: &Layout) -> usize {
    layout
        .rows
        .iter()
        .position(|r| r.id == layout.focus.row)
        .unwrap_or(0)
        + 1
}

#[allow(clippy::too_many_arguments)]
fn render_frame(
    out: &mut Vec<Cell>,
    layout: &Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cols: u16,
    rows: u16,
    content_width: u16,
    background: CColor,
    focus_color: CColor,
    skeleton: Option<CColor>,
    mm: &crate::config::Minimap,
) {
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
    let mut focus_rect: Option<Rect> = None;
    let mut focused_cursor_abs: Option<(u16, u16, bool)> = None; // (screen x,y, hide)
    for v in focused_pane_views(layout, cols, rows, content_width, panes, skeleton.is_some()) {
        let Some(pane) = panes.get_mut(&v.pid) else {
            continue;
        };
        let is_focus = focused == Some(v.pid);
        // The emulator size matches the visible content rect exactly. Without
        // the skeleton, panes are full-bleed (content spans the whole column);
        // with it, rects are inset 1 cell inside the frame so nothing a
        // program draws is ever covered.
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
        if is_focus {
            focus_rect = Some(v.rect);
            // Map the emulator cursor into screen coords, accounting for the
            // pane's content-window ([g_start, g_end) visible in rect).
            let (cur_row, cur_col) = pane.grid.cursor_position();
            let hide = pane.grid.hide_cursor();
            // When scrolled back from live, the cursor is off-screen history:
            // don't paint a stale block.
            let live = pane.grid.scrollback_offset() == 0;
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
        for gy in 0..v.rect.h {
            for gx in 0..v.rect.w {
                let idx = ((v.rect.y as usize + gy as usize) * cols as usize)
                    + (v.rect.x as usize + gx as usize);
                if idx >= out.len() {
                    continue;
                }
                let gi = g_start + gx;
                let mut cell = if gi < g_end {
                    pane.grid.cell(gi, gy)
                } else {
                    Cell::default()
                };
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
                out[idx] = cell;
            }
        }
    }
    // Skeleton: a 1-cell frame around every column box (full strip height) so
    // the container structure always reads, plus placeholder boxes tiling any
    // empty right side at the default quarter width. The focused column's box
    // is framed in the focus accent instead of the skeleton color. Pane rects
    // are inset inside the frames (see focused_pane_views), so the frames
    // occupy the 1-cell ring around each content area and never cover it.
    if let Some(sk) = skeleton {
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
        // same on-screen boundaries at every scroll stop.
        let ranges = layout
            .visible_column_x_ranges(layout.focus.row, cols, scroll)
            .unwrap_or_default();
        let mut edge = 0u16;
        for (ci, (s, e)) in ranges.iter().enumerate() {
            let sx = *s;
            let ex = *e;
            if ex <= 0 || sx >= cols as i32 {
                continue;
            }
            let left = sx.max(0) as u16;
            let right = (ex.min(cols as i32)) as u16;
            if right <= left {
                continue;
            }
            let color = if ci == layout.focus.column {
                focus_color
            } else {
                sk
            };
            let boxr = Rect {
                x: left,
                y: 0,
                w: right - left,
                h: strip_h,
            };
            draw_focus_frame(out, cols, boxr, color);
            edge = edge.max(right);
        }
        // Empty right side: placeholder boxes at the default quarter width, so
        // the skeleton always shows the full four-column container even before
        // panes exist to fill it. Interiors are reset to the default (pane)
        // background so an empty box reads exactly like a live one instead of
        // showing the dimmer `background` fill, and each carries a big
        // `strip.cell` identifier centered in the box so empty cells are
        // addressable at a glance.
        // Address strips by their *position* in the stack, not their row id:
        // ids are monotonic allocation counters, so creating and discarding
        // strips (j/k past the ends) would otherwise make the label drift
        // upward (2, 3, 4, ...) for what is visibly always the second strip.
        let strip_no = strip_number(layout);
        let quarter = (cols / 4).max(1);
        let mut pcol = ranges.len();
        while edge < cols {
            let w = quarter.min(cols - edge);
            if w < 2 {
                break;
            }
            let boxr = Rect {
                x: edge,
                y: 0,
                w,
                h: strip_h,
            };
            for y in 0..boxr.h {
                let row = (boxr.y + y) as usize * cols as usize;
                for x in 0..boxr.w {
                    if let Some(c) = out.get_mut(row + (boxr.x + x) as usize) {
                        *c = Cell::default();
                    }
                }
            }
            // On an empty strip (a freshly created niri-style workspace)
            // there are no live columns, so the focus lives on a placeholder
            // box: frame it in the accent color so the strip never looks
            // focus-less.
            let color = if ranges.is_empty() && pcol == layout.focus.column {
                focus_color
            } else {
                sk
            };
            draw_focus_frame(out, cols, boxr, color);
            let label = format!("{}.{}", strip_no, pcol + 1);
            let inner = Rect {
                x: boxr.x + 1,
                y: boxr.y + 1,
                w: boxr.w.saturating_sub(2),
                h: boxr.h.saturating_sub(2),
            };
            draw_big_label(out, cols, inner, &label, CColor::Rgb(0x58, 0x5b, 0x70));
            pcol += 1;
            edge += w;
        }
    }
    // Without the skeleton, focus is a 1-cell accent frame overlaid on the
    // focused pane's edge cells (the historical full-bleed look). With the
    // skeleton, the focused *column's* frame is already the accent color and
    // content is inset, so the overlay would only cover content: for a
    // stacked column, frame the focused pane's own rect ring instead, drawn
    // in the gap/frame cells around it.
    match (skeleton, focus_rect) {
        // Full-bleed mode: the ring lands on live pane content, so tint the
        // background instead of writing glyphs over the program's own text.
        (None, Some(rect)) => tint_focus_ring(out, cols, rect, focus_color),
        (Some(_), Some(rect)) => {
            let stacked = layout
                .focused_row()
                .and_then(|r| r.columns.get(layout.focus.column))
                .map(|c| c.panes.len() > 1)
                .unwrap_or(false);
            if stacked {
                // Grow the rect by 1 so the ring lands on the frame/gap cells
                // around the pane, not on its content.
                let x = rect.x.saturating_sub(1);
                let y = rect.y.saturating_sub(1);
                let w = (rect.w + 2).min(cols.saturating_sub(x));
                let h = (rect.h + 2).min(rows.saturating_sub(y));
                draw_focus_frame(out, cols, Rect { x, y, w, h }, focus_color);
            }
        }
        _ => {}
    }
    // Chrome dispatch: reserved row (quasimode-aware) vs legacy overlay / edge ticks
    match mm.mode {
        crate::config::MinimapMode::Reserved | crate::config::MinimapMode::ReservedQuasimode => {
            // Caller (run_tui) decides whether the row should actually paint based on alt_held/has_attention.
            // render_frame itself paints the row whenever asked; alt_held==true or has_attention ensures visibility.
            // Since render_frame doesn't know alt_held, it always paints when chrome==1 — the blank-row at rest is just background.
            // To honor quasimode, run_tui will pass a Minimap with painted==false? Instead, we expose a helper:
            // keep painting here always when chrome==1; run_tui-level alt tracking will request repaint only when state flips.
            // For direct calls from tests we paint the row unconditionally when chrome==1 (covered below via draw_status_row).
            // The run_tui loop will decide whether to leave the last row as background or call draw_status_row.
            // We paint here based on mm.should_paint — but render_frame has no alt flag. So we default to painting when mode==Reserved,
            // and when ReservedQuasimode we leave it to the loop. To keep tests honest, paint for both modes when no alt info is available.
            if mm.mode == crate::config::MinimapMode::Reserved
                || mm.mode == crate::config::MinimapMode::ReservedQuasimode
            {
                // At the render_frame level we have no Alt state; paint whenever caller hasn't indicated hidden.
                // run_tui will blank after if needed — for now paint all chrome modes here.
                // (Quasimode blanking is applied as a post-step in run_tui's dirty path.)
                // We draw unconditionally here and let run_tui blank it if alt_held==false && !has_attention.
                // So we need to know has_attention — compute here as fallback when not in quasimode loop.
                // For simplicity, always draw_status_row when chrome>0; run_tui's outer logic will overwrite with background when hidden.
                let chrome = mm.chrome_rows();
                if chrome > 0 {
                    draw_status_row(out, cols, rows, layout, mm, focus_color);
                }
            }
        }
        crate::config::MinimapMode::Overlay => {
            draw_minimap(out, cols, rows, layout, mm, focus_color);
        }
        crate::config::MinimapMode::EdgeTicks => {
            draw_edge_ticks(out, cols, rows, layout, mm, focus_color);
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

/// Overlay a thin frame on the edge ring of `rect`: box-drawing glyphs
/// (`╭─╮│╰╯`) with `color` as the foreground, on a default background. The
/// previous implementation preserved the underlying `background` fill (e.g.
/// `Idx(235)`), which left a dim gray slab behind the thin red focus glyph.
/// Resetting the ring cells to `Default` makes the hairline float on the same
/// background as pane interiors and placeholder boxes, so only the red glyph
/// remains.
fn draw_focus_frame(out: &mut [Cell], cols: u16, rect: Rect, color: CColor) {
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
    fn put(out: &mut [Cell], idx: usize, ch: char, color: CColor) {
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
        cell.style.bold = false;
        cell.style.underline = false;
        cell.style.inverse = false;
    }
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

/// Tint the background of the edge ring of `rect` with `color`, preserving
/// the glyphs underneath. Used when the ring overlays live pane content
/// (full-bleed mode), where writing frame glyphs would cover program output.
fn tint_focus_ring(out: &mut [Cell], cols: u16, rect: Rect, color: CColor) {
    let stride = cols as usize;
    let w = rect.w as usize;
    let h = rect.h as usize;
    let x0 = rect.x as usize;
    let y0 = rect.y as usize;
    let x1 = x0 + w - 1;
    let y1 = y0 + h - 1;
    for x in x0..=x1 {
        if let Some(c) = out.get_mut(y0 * stride + x) {
            c.style.bg = color;
        }
        if h > 1 {
            if let Some(c) = out.get_mut(y1 * stride + x) {
                c.style.bg = color;
            }
        }
    }
    for y in (y0 + 1)..y1 {
        if let Some(c) = out.get_mut(y * stride + x0) {
            c.style.bg = color;
        }
        if w > 1 {
            if let Some(c) = out.get_mut(y * stride + x1) {
                c.style.bg = color;
            }
        }
    }
}

/// 3x5 block-font glyphs for the characters a cell identifier can contain
/// (digits and the `,`/`.` separators). Each glyph is 5 rows of 3 bits, MSB left.
fn big_glyph(ch: char) -> Option<[u8; 5]> {
    Some(match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b011, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        _ => return None,
    })
}

/// Paint `label` centered in `rect` using the 3x5 block font, one screen cell
/// per font pixel (glyphs separated by a 1-cell gap). Pixels are painted as
/// background-colored blanks in `color`. Skipped entirely when the rect is too
/// small to fit the label, so tiny boxes stay clean.
fn draw_big_label(out: &mut [Cell], cols: u16, rect: Rect, label: &str, color: CColor) {
    let glyphs: Vec<[u8; 5]> = label.chars().filter_map(big_glyph).collect();
    if glyphs.is_empty() {
        return;
    }
    let gw = (glyphs.len() * 3 + (glyphs.len() - 1)) as u16; // 3 wide + 1 gap
    let gh = 5u16;
    if rect.w < gw || rect.h < gh {
        return;
    }
    let x0 = rect.x + (rect.w - gw) / 2;
    let y0 = rect.y + (rect.h - gh) / 2;
    for (gi, glyph) in glyphs.iter().enumerate() {
        let gx = x0 + (gi as u16) * 4;
        for (ry, bits) in glyph.iter().enumerate() {
            for rx in 0..3u16 {
                if bits & (0b100 >> rx) == 0 {
                    continue;
                }
                let idx = (y0 as usize + ry) * cols as usize + (gx + rx) as usize;
                if let Some(c) = out.get_mut(idx) {
                    *c = Cell::default();
                    c.style.bg = color;
                }
            }
        }
    }
}

fn status_bg_for(s: PaneStatus) -> CColor {
    // Catppuccin Mocha accent tints, darkened for white (Idx 231) text contrast.
    match s {
        PaneStatus::Running => CColor::Rgb(0x52, 0x6c, 0x96), // blue #89b4fa @ 0.6
        PaneStatus::Idle => CColor::Rgb(0x96, 0x6b, 0x51),    // peach #fab387 @ 0.6
        PaneStatus::Done => CColor::Rgb(0x63, 0x88, 0x60),    // green #a6e3a1 @ 0.6
        PaneStatus::Failed => CColor::Rgb(0x91, 0x53, 0x64),  // red #f38ba8 @ 0.6
    }
}
fn status_fg_for(s: PaneStatus) -> CColor {
    // Bright Mocha accents for summary counts.
    match s {
        PaneStatus::Running => CColor::Rgb(0x89, 0xb4, 0xfa),
        PaneStatus::Idle => CColor::Rgb(0xfa, 0xb3, 0x87),
        PaneStatus::Done => CColor::Rgb(0xa6, 0xe3, 0xa1),
        PaneStatus::Failed => CColor::Rgb(0xf3, 0x8b, 0xa8),
    }
}
fn status_glyph_for(s: PaneStatus) -> char {
    match s {
        PaneStatus::Running => '\u{00bb}', // »
        PaneStatus::Idle => '!',
        PaneStatus::Done => '\u{2713}',   // ✓
        PaneStatus::Failed => '\u{2717}', // ✗
    }
}

/// Reserved 1-line status row: `❯1» 2✓ [3!] 4»   ↕ 2 strips !1` rendered at y = rows-1.
/// Never overwrites pane cells: panes end at strip_h = rows - chrome.
fn draw_status_row(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
    focus_color: CColor,
) {
    if rows == 0 {
        return;
    }
    let y = (rows - 1) as usize;
    let chrome = mm.chrome_rows();
    if chrome == 0 {
        return;
    }
    let bar_bg = CColor::Rgb(0x18, 0x18, 0x25);
    // Start with a solid bar background so untouched cells are chrome, not pane bg.
    for x in 0..cols as usize {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            *c = Cell {
                ch: ' ',
                style: strimux_term::Style {
                    fg: CColor::Rgb(0xa6, 0xad, 0xc8),
                    bg: bar_bg,
                    ..Default::default()
                },
                width: 1,
                ..Default::default()
            };
        }
    }
    let put = |out: &mut [Cell], x: usize, ch: char, fg: CColor, bg: CColor, bold: bool| {
        if x >= cols as usize {
            return;
        }
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            *c = Cell {
                ch,
                width: 1,
                style: strimux_term::Style {
                    fg,
                    bg,
                    bold,
                    ..Default::default()
                },
                ..*c
            };
            // ensure single-width
            if ch == '\u{276f}' {
                c.width = 1;
            } // ❯
        }
    };
    // Build tile segments for focused row
    let row = layout.focused_row();
    let mut x = 0usize;
    if let Some(r) = row {
        for (ci, col) in r.columns.iter().enumerate() {
            if x >= cols as usize {
                break;
            }
            let is_focus = ci == layout.focus.column;
            let pane_status = col
                .panes
                .first()
                .and_then(|pid| layout.panes.get(pid))
                .map(|pane| pane.status)
                .unwrap_or(PaneStatus::Running);
            let bg = if is_focus {
                focus_color
            } else {
                status_bg_for(pane_status)
            };
            let glyph = status_glyph_for(pane_status);
            // Token like "❯1»" / "[3!]" / " 2✓"
            let digit = char::from_digit(ci as u32 + 1, 10).unwrap_or('+');
            // For non-focused first tile, we want " ❯" style start; simplify: first focused gets ❯ prefix, else just digits
            let text = if is_focus && ci == 0 {
                format!("{}{}{}", '\u{276f}', digit, glyph) // ❯1»
            } else if is_focus {
                format!("[{}{}]", digit, glyph)
            } else {
                format!(" {}{} ", digit, glyph)
            };
            for ch in text.chars() {
                if x >= cols as usize {
                    break;
                }
                let fg = CColor::Idx(231);
                put(out, x, ch, fg, bg, is_focus);
                x += 1;
            }
            if x < cols as usize {
                // inter-tile gap
                put(out, x, ' ', CColor::Rgb(0xa6, 0xad, 0xc8), bar_bg, false);
                x += 1;
            }
        }
        // Right side: strip counts + tallies
        if mm.show_counts {
            let other_rows = layout.rows.len().saturating_sub(1);
            let mut segs: Vec<(String, CColor)> = Vec::new();
            if other_rows > 0 {
                segs.push((
                    format!("\u{2195} {} ", other_rows),
                    CColor::Rgb(0xa6, 0xad, 0xc8),
                )); // ↕ N
            }
            let mut counts = [0usize; 4];
            let statuses = [
                PaneStatus::Running,
                PaneStatus::Idle,
                PaneStatus::Done,
                PaneStatus::Failed,
            ];
            for pane in layout.panes.values() {
                if let Some(i) = statuses.iter().position(|s| *s == pane.status) {
                    counts[i] += 1;
                }
            }
            for (i, s) in statuses.iter().enumerate() {
                if counts[i] > 0 {
                    segs.push((
                        format!("{}{} ", status_glyph_for(*s), counts[i]),
                        status_fg_for(*s),
                    ));
                }
            }
            let total_w: usize = segs.iter().map(|(s, _)| s.chars().count()).sum();
            let mut rx = cols as usize;
            if total_w < rx {
                rx -= total_w;
            } else {
                rx = x.max(cols as usize - total_w);
            }
            // ensure we don't overwrite left tiles: start at max(x, rx)
            let start = x.max(rx);
            let mut cx = start;
            for (text, fg) in segs {
                for ch in text.chars() {
                    if cx >= cols as usize {
                        break;
                    }
                    put(out, cx, ch, fg, bar_bg, true);
                    cx += 1;
                }
            }
        }
    }
}

/// Center HUD: a brief centered box flashed when attention arrives while the
/// quasimode reserved row is hidden. Paints a 3-row box (frame + text) in the
/// middle of the viewport so the user notices without the reserved row being
/// permanently visible.
fn draw_center_hud(out: &mut [Cell], cols: u16, rows: u16, layout: &Layout, focus_color: CColor) {
    if cols < 20 || rows < 6 {
        return;
    }
    let target = smart_jump_target(layout);
    let msg = if let Some(pid) = target {
        let status = layout
            .panes
            .get(&pid)
            .map(|p| p.status)
            .unwrap_or(PaneStatus::Idle);
        let glyph = status_glyph_for(status);
        let addr = layout
            .rows
            .iter()
            .enumerate()
            .find_map(|(ri, row)| {
                row.columns.iter().enumerate().find_map(|(ci, col)| {
                    col.panes
                        .iter()
                        .position(|p| *p == pid)
                        .map(|_| format!("{}.{}", ri + 1, ci + 1))
                })
            })
            .unwrap_or_else(|| "?".to_string());
        format!(" {} {} needs you — ⌥+g ", glyph, addr)
    } else {
        " ! needs you — ⌥+g ".to_string()
    };
    let pad: usize = 2;
    let bw = msg.chars().count() + pad * 2 + 2;
    let bh: usize = 3;
    if bw as u16 >= cols {
        return;
    }
    let ox = ((cols as usize).saturating_sub(bw)) / 2;
    let oy = ((rows as usize).saturating_sub(bh)) / 2;
    let bg = CColor::Rgb(0x18, 0x18, 0x25);
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + (ox + x)) {
                *c = Cell {
                    ch: ' ',
                    style: strimux_term::Style {
                        fg: CColor::Rgb(0xa6, 0xad, 0xc8),
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
    draw_focus_frame(out, cols, rect, focus_color);
    let ty = oy + 1;
    let tx = ox + 1 + pad;
    for (i, ch) in msg.chars().enumerate() {
        if let Some(c) = out.get_mut(ty * cols as usize + (tx + i)) {
            c.ch = ch;
            c.style.fg = CColor::Idx(231);
            c.style.bg = bg;
            c.style.bold = true;
            c.width = 1;
        }
    }
}

/// Edge-ticks chrome: single-cell marks on the bottom/right frame edges at true x-positions.
fn draw_edge_ticks(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    _mm: &crate::config::Minimap,
    focus_color: CColor,
) {
    if cols == 0 || rows == 0 {
        return;
    }
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
            status_bg_for(status)
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
                if c.style.bg == CColor::Default || c.style.bg == status_bg_for(status) {
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
            focus_color
        } else if needs {
            CColor::Rgb(0x96, 0x6b, 0x51)
        } else {
            CColor::Rgb(0x6c, 0x70, 0x86)
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
/// digit (the same digit `⌥+1..9` jumps to) and, when wide enough, a status
/// glyph. The focused pane's tile is painted in the focus accent and the
/// focused strip gets a `❯` chevron in the gutter. An optional one-line
/// summary above the map counts panes by status: `4 »2 !1 ✓1`.
fn draw_minimap(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
    focus_color: CColor,
) {
    use strimux_layout::minimap;
    // With a single pane there is nothing to triage; hide the map.
    if !mm.show || (layout.panes.len() <= 1 && layout.rows.len() <= 1) {
        return;
    }
    /// Catppuccin Mocha muted backgrounds / bright foregrounds (shared with status_bg_for).
    fn status_bg(s: PaneStatus) -> CColor {
        match s {
            PaneStatus::Running => CColor::Rgb(0x52, 0x6c, 0x96),
            PaneStatus::Idle => CColor::Rgb(0x96, 0x6b, 0x51),
            PaneStatus::Done => CColor::Rgb(0x63, 0x88, 0x60),
            PaneStatus::Failed => CColor::Rgb(0x91, 0x53, 0x64),
        }
    }
    fn status_fg(s: PaneStatus) -> CColor {
        match s {
            PaneStatus::Running => CColor::Rgb(0x89, 0xb4, 0xfa),
            PaneStatus::Idle => CColor::Rgb(0xfa, 0xb3, 0x87),
            PaneStatus::Done => CColor::Rgb(0xa6, 0xe3, 0xa1),
            PaneStatus::Failed => CColor::Rgb(0xf3, 0x8b, 0xa8),
        }
    }
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
        let fg = CColor::Idx(231);
        let y = oy + tile.y;
        let glyph = status_glyph(tile.status);
        for dx in 0..tile.w {
            let x = ox + tile.x + dx;
            // First cell: the pane's column digit (what ⌥+1..9 jumps to);
            // stacked sub-panes past the first repeat the status glyph
            // instead. Last cell of a wide tile: the status glyph.
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
            put(out, x, y, ch, fg, bg, dx == 0 && tile.pane_idx == 0);
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
        let mut segs: Vec<(String, CColor)> = vec![(
            format!("{}", layout.panes.len()),
            CColor::Rgb(0xa6, 0xad, 0xc8),
        )];
        for (i, s) in statuses.iter().enumerate() {
            if counts[i] > 0 {
                segs.push((format!(" {}{}", status_glyph(*s), counts[i]), status_fg(*s)));
            }
        }
        let total_w: usize = segs.iter().map(|(t, _)| t.chars().count()).sum();
        let y = oy - 1;
        let mut x = cols.saturating_sub(total_w as u16);
        let bar_bg = CColor::Rgb(0x18, 0x18, 0x25);
        for (text, fg) in segs {
            for ch in text.chars() {
                put(out, x, y, ch, fg, bar_bg, true);
                x += 1;
            }
        }
    }
}

fn crossterm_color(c: CColor) -> crossterm::style::Color {
    match c {
        CColor::Default => crossterm::style::Color::Reset,
        CColor::Idx(i) => crossterm::style::Color::AnsiValue(i),
        CColor::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
    }
}

/// Diff and paint `out` vs `last` into `buf`. Returns true if anything changed.
///
/// Wide (two-column) characters need care to avoid shearing the row:
///  - width-0 continuation cells are skipped, because the wide glyph printed
///    just before them already covers that column; printing their placeholder
///    space would shift everything after it one column right.
///  - every run starts with an explicit `MoveTo`, and a run is cut right after
///    any non-single-width cell, so even if the host terminal disagrees with
///    the emulator about a glyph's width (a classic emoji problem) the drift
///    is bounded to that one glyph instead of shearing the rest of the row.
///
/// Attributes are reset per run, not per row: SGR attributes (bold, underline,
/// reverse) have no "set exactly these" form, only additive codes, so a run
/// that doesn't reset first would inherit whatever the previous run enabled.
/// The observed failure was a popup row with underlined entries painting an
/// underline across every cell to its right ("line overflow"), which then
/// stuck because the diff buffer believed those cells were already blank.
///
/// Runs also stop at any glyph whose *host* width (per `unicode-width`)
/// disagrees with the width the emulator recorded. East-Asian text (Hangul,
/// CJK) and ambiguous-width symbols are the common case: the host advances the
/// cursor two columns where the emulator assumed one (or vice versa), and a
/// long merged run then paints past the pane's right edge, wraps at the screen
/// margin, and stains the rows below with the run's background ("highlight
/// overflow"). Cutting the run and re-issuing an explicit `MoveTo` bounds any
/// disagreement to the offending glyph.
fn paint(buf: &mut Vec<u8>, out: &[Cell], last: &[Cell], cols: u16, rows: u16) -> bool {
    use crossterm::queue;
    use crossterm::style::{
        Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
    };
    let cc = cols as usize;
    let mut dirty = false;
    for y in 0..rows as usize {
        let row_eq = last.get(y * cc..(y + 1) * cc) == Some(&out[y * cc..(y + 1) * cc]);
        if row_eq {
            continue;
        }
        dirty = true;
        // Group cells into style runs and print each run.
        let mut x = 0usize;
        while x < cc {
            let cell = out[y * cc + x];
            if cell.width == 0 {
                // Continuation of a wide char; the glyph already covers it.
                x += 1;
                continue;
            }
            let style = cell.style;
            let mut run = String::new();
            cell.push_codepoints(&mut run);
            let mut end = x + 1;
            if cell.width == 1 && host_width_agrees(cell) {
                while end < cc && out[y * cc + end].style == style {
                    let next = out[y * cc + end];
                    if next.width != 1 || !host_width_agrees(next) {
                        break;
                    }
                    next.push_codepoints(&mut run);
                    end += 1;
                }
            }
            let _ = queue!(
                buf,
                cursor::MoveTo(x as u16, y as u16),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(crossterm_color(style.fg)),
                SetBackgroundColor(crossterm_color(style.bg)),
            );
            if style.bold {
                let _ = queue!(buf, SetAttribute(Attribute::Bold));
            }
            if style.underline {
                let _ = queue!(buf, SetAttribute(Attribute::Underlined));
            }
            if style.inverse {
                let _ = queue!(buf, SetAttribute(Attribute::Reverse));
            }
            let _ = queue!(buf, Print(run));
            x = end;
        }
    }
    dirty
}

/// Whether the host terminal is expected to advance the cursor by exactly the
/// column count the emulator recorded for this cell. Control/zero-width and
/// ambiguous- or wide-width glyphs that the emulator called single-width are
/// the disagreement cases; those cells are printed alone so any drift stays
/// bounded to one column instead of shearing (and wrapping) the whole row.
fn host_width_agrees(cell: Cell) -> bool {
    use unicode_width::UnicodeWidthChar;
    match cell.ch.width() {
        Some(w) => w as u8 == cell.width,
        None => false,
    }
}

/// A decoded keyboard instruction.
#[derive(Debug, PartialEq)]
enum Cmd {
    Act(Action),
    Scroll(i32),
    ScrollPane(i32),
    Input(Vec<u8>),
    /// Smart-jump: focus the next pane that needs the user (`⌥+g`). Resolved
    /// against the live layout in the main loop, not here.
    SmartJump,
    Quit,
    Repaint,
    None,
}

/// Encode a key event that is not a strimux chord into PTY bytes.
///
/// Alt (Option) is forwarded as Meta: an `ESC` prefix before the base
/// sequence, matching what a pane sees when run natively outside strimux
/// (e.g. Alt/Option+Backspace becomes `ESC DEL` / `\x1b\x7f` which
/// readline interprets as `backward-kill-word`). Previously only `Char`
/// keys honored the Alt bit; `Backspace`/`Delete`/arrows etc. dropped it
/// and sent plain `DEL`, so word-delete never fired inside the mux.
fn key_bytes(ev: &KeyEvent) -> Vec<u8> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let mut out = Vec::new();
    // Meta prefix: ESC before the base sequence. Ctrl combinations take
    // precedence for the base encoding but still keep the Meta ESC in front
    // (C-M-... = ESC + C-...), matching xterm's metaSendsEscape.
    let meta = alt && !matches!(ev.code, KeyCode::Esc);
    if meta {
        out.push(0x1b);
    }
    match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lc = c.to_ascii_lowercase();
                if lc.is_ascii_lowercase() {
                    out.push(lc as u8 - b'a' + 1);
                } else {
                    out.extend_from_slice(&[b'^', c as u8, b'\n']);
                }
            } else {
                let mut s = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut s).as_bytes());
            }
        }
        KeyCode::Enter => out.extend_from_slice(b"\r"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Tab => out.extend_from_slice(b"\t"),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        // Crossterm can deliver the legacy BackTab (Shift+Tab) code; forward it
        // with the same Meta prefix convention.
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        _ => {}
    }
    out
}

/// Map a key event to a command. Returns None when it is a pass-through.
fn handle_key(ev: &KeyEvent) -> Option<Cmd> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = physical_shift(ev);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    use KeyCode::*;
    // macOS Option+letter fallback: terminals that don't translate Option to
    // Meta send these Unicode glyphs instead (US layout: h->˙ j->∆ k->˚ l->¬).
    // Remap them to focus navigation so Option+hjkl works with zero config.
    // Only fires when the char arrives as plain input (never when Option-as-Alt
    // is set, which delivers ESC+h instead), so the two paths can't collide.
    if !alt && !ctrl {
        match ev.code {
            // Option+Shift+hjkl move the pane (niri-style), US layout glyphs.
            Char('\u{d3}') => return Some(Cmd::Act(Action::MovePaneLeft)), // Ó (Option+Shift+h)
            Char('\u{d4}') => return Some(Cmd::Act(Action::MovePaneDown)), // Ô (Option+Shift+j)
            Char('\u{f8ff}') => return Some(Cmd::Act(Action::MovePaneUp)), //  (Option+Shift+k)
            Char('\u{d2}') => return Some(Cmd::Act(Action::MovePaneRight)), // Ò (Option+Shift+l)
            _ => {}
        }
    }
    if !alt && !ctrl && !shift {
        match ev.code {
            Char('\u{2d9}') => return Some(Cmd::Act(Action::FocusLeft)), // ˙ (Option+h)
            Char('\u{2206}') => return Some(Cmd::Act(Action::FocusDown)), // ∆ (Option+j)
            Char('\u{2da}') => return Some(Cmd::Act(Action::FocusUp)),   // ˚ (Option+k)
            Char('\u{ac}') => return Some(Cmd::Act(Action::FocusRight)), // ¬ (Option+l)
            Char('\u{2026}') => return Some(Cmd::Act(Action::SpawnAgent)), // … (Option+;)
            Char('\u{153}') => return Some(Cmd::Act(Action::KillPane)),  // œ (Option+q)
            Char('\u{a9}') => return Some(Cmd::SmartJump),               // © (Option+g)
            _ => {}
        }
    }
    // Prefix toggle: `Ctrl-b`. Always arrives on any terminal, no config.
    if !alt && ctrl && matches!(ev.code, Char('b')) {
        IN_PREFIX.store(true, Ordering::Relaxed);
        return Some(Cmd::Repaint);
    }
    // Inside the prefix, the next key is a strimux command.
    if IN_PREFIX.load(Ordering::Relaxed) {
        IN_PREFIX.store(false, Ordering::Relaxed);
        if ev.code == Esc {
            return Some(Cmd::Repaint); // cancel prefix
        }
        if ctrl && matches!(ev.code, Char('b')) {
            return Some(Cmd::Input(vec![0x02])); // literal Ctrl-b to the pane
        }
        let lc = logical_char(ev);
        if let Some(c) = lc {
            // Shift+hjkl moves the pane (niri-style); plain hjkl focuses.
            // Use logical_char so Kitty alternate keys (Shift clears) and Caps
            // Lock are both handled: physical_shift distinguishes them, while
            // `c` is the layout-agnostic lowercased letter.
            if c == 'h' || c == 'j' || c == 'k' || c == 'l' {
                return Some(if shift {
                    match c {
                        'h' => Cmd::Act(Action::MovePaneLeft),
                        'j' => Cmd::Act(Action::MovePaneDown),
                        'k' => Cmd::Act(Action::MovePaneUp),
                        'l' => Cmd::Act(Action::MovePaneRight),
                        _ => unreachable!(),
                    }
                } else {
                    match c {
                        'h' => Cmd::Act(Action::FocusLeft),
                        'j' => Cmd::Act(Action::FocusDown),
                        'k' => Cmd::Act(Action::FocusUp),
                        'l' => Cmd::Act(Action::FocusRight),
                        _ => unreachable!(),
                    }
                });
            }
            let cmd = match c {
                'c' | 'n' => Action::NewColumn,
                'r' | 'o' => Action::NewRow,
                ';' => Action::SpawnAgent,
                's' | '-' => Action::SplitBelow,
                'x' => Action::KillPane,
                'z' | '=' => Action::CycleWidth,
                'g' => return Some(Cmd::SmartJump),
                ',' => return Some(Cmd::ScrollPane(-1)),
                '.' => return Some(Cmd::ScrollPane(1)),
                '<' => return Some(Cmd::ScrollPane(-16)),
                '>' => return Some(Cmd::ScrollPane(16)),
                '[' => return Some(Cmd::Scroll(-200)),
                ']' => return Some(Cmd::Scroll(200)),
                'q' => return Some(Cmd::Quit),
                _ => {
                    if c.is_ascii_digit() {
                        Action::JumpToColumn(c.to_digit(10).unwrap_or(1) as usize - 1)
                    } else {
                        return Some(Cmd::None);
                    }
                }
            };
            return Some(Cmd::Act(cmd));
        }
        // Non-char codes (e.g. ','/'<' are already handled above) and bare
        // punctuation that had no `c` hit the fallback.
        let cmd = match ev.code {
            Char(';') => Action::SpawnAgent,
            Char(',') => return Some(Cmd::ScrollPane(-1)),
            Char('.') => return Some(Cmd::ScrollPane(1)),
            Char('<') => return Some(Cmd::ScrollPane(-16)),
            Char('>') => return Some(Cmd::ScrollPane(16)),
            Char('[') => return Some(Cmd::Scroll(-200)),
            Char(']') => return Some(Cmd::Scroll(200)),
            _ => return Some(Cmd::None), // unknown prefix key just cancels
        };
        return Some(Cmd::Act(cmd));
    }
    if !alt {
        // Escape must be a chord preamble only; forward everything else.
        if ev.code == Esc {
            return Some(Cmd::None);
        }
        return Some(Cmd::Input(key_bytes(ev)));
    }
    // Alt chords (work when the terminal sends Option as Meta).
    if let Some(c) = logical_char(ev) {
        if c == 'h' || c == 'j' || c == 'k' || c == 'l' {
            return Some(if shift {
                match c {
                    'h' => Cmd::Act(Action::MovePaneLeft),
                    'j' => Cmd::Act(Action::MovePaneDown),
                    'k' => Cmd::Act(Action::MovePaneUp),
                    'l' => Cmd::Act(Action::MovePaneRight),
                    _ => unreachable!(),
                }
            } else {
                match c {
                    'h' => Cmd::Act(Action::FocusLeft),
                    'j' => Cmd::Act(Action::FocusDown),
                    'k' => Cmd::Act(Action::FocusUp),
                    'l' => Cmd::Act(Action::FocusRight),
                    _ => unreachable!(),
                }
            });
        }
        let act = match c {
            'a' => Some(Action::NewColumn),
            ';' => Some(Action::SpawnAgent),
            's' => Some(Action::SplitBelow),
            'r' => Some(Action::CycleWidth),
            'x' => Some(Action::KillPane),
            'z' => Some(Action::CycleWidth),
            'q' => Some(Action::KillPane),
            'g' => return Some(Cmd::SmartJump),
            _ if c.is_ascii_digit() => Some(Action::JumpToColumn(
                c.to_digit(10).unwrap_or(1) as usize - 1,
            )),
            _ => None,
        };
        if let Some(a) = act {
            return Some(Cmd::Act(a));
        }
        if matches!(c, '[' | ']') {
            return Some(if c == '[' {
                Cmd::Scroll(-200)
            } else {
                Cmd::Scroll(200)
            });
        }
    }
    // Shift+arrow and plain arrow scroll the pane content.
    if matches!(ev.code, Left | Right) {
        if shift {
            return Some(match ev.code {
                Left => Cmd::ScrollPane(-16),
                Right => Cmd::ScrollPane(16),
                _ => unreachable!(),
            });
        }
        return Some(match ev.code {
            Left => Cmd::ScrollPane(-1),
            Right => Cmd::ScrollPane(1),
            _ => unreachable!(),
        });
    }
    if ev.code == Enter {
        return Some(Cmd::Act(if shift {
            Action::NewRow
        } else {
            Action::NewColumn
        }));
    }
    // Alt+digit/punct not listed above: check the original code directly
    // since those don't need case folding.
    match ev.code {
        Char(c) if c.is_ascii_digit() => {
            return Some(Cmd::Act(Action::JumpToColumn(
                c.to_digit(10).unwrap_or(1) as usize - 1,
            )))
        }
        Char('[') => return Some(Cmd::Scroll(-200)),
        Char(']') => return Some(Cmd::Scroll(200)),
        _ => {}
    }
    Some(Cmd::Input(key_bytes(ev)))
}

/// Kill any pane whose id is no longer in the layout, and spawn missing ones.
fn sync_panes(
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cfg: &Config,
    tx: &Sender<PaneMsg>,
    _first_id: PaneId,
    agent_panes: &HashSet<PaneId>,
) -> Result<(), String> {
    let mut wanted: Vec<PaneId> = Vec::new();
    for row in &layout.rows {
        for col in &row.columns {
            for pid in &col.panes {
                wanted.push(*pid);
            }
        }
    }
    // Remove panes that disappeared.
    panes.retain(|pid, pane| {
        if wanted.contains(pid) {
            true
        } else {
            let _ = pane.child.kill();
            false
        }
    });
    // Spawn missing panes. Agent panes (created via the spawn-agent verb) run
    // the configured `default_agent` harness; everything else gets the shell.
    for pid in wanted {
        if panes.contains_key(&pid) {
            continue;
        }
        let cmd = if agent_panes.contains(&pid) {
            cfg.default_agent.clone()
        } else {
            String::new()
        };
        let pane = spawn_pane(pid, &cmd, 80, 24, tx.clone())?;
        panes.insert(pid, pane);
        tracing::debug!(pid, "spawned pane");
    }
    Ok(())
}

/// Re-read the live terminal size and adopt it whenever the OS reports
/// anything different from what we last knew. Called every loop so a resize
/// that crossterm never delivers as an `Event::Resize` (or that is coalesced
/// away) can't leave the frame short of the terminal's true right edge. This
/// is what makes the panes truly full-bleed to the right margin: the frame is
/// always sized to the currently-rendered column count, never to a cached one.
fn refresh_size(cols: &mut u16, rows: &mut u16) -> bool {
    match term_size() {
        Ok((c, r)) => {
            let c = c.max(1);
            let r = r.max(2);
            if c != *cols || r != *rows {
                if std::env::var_os("STRIMUX_DEBUG_SIZE").is_some() {
                    eprintln!("[strimux] terminal size -> {c} cols x {r} rows");
                }
                *cols = c;
                *rows = r;
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// Run the interactive TUI.
pub fn run_tui(command: Option<String>, cfg: Config) -> Result<(), i32> {
    use std::io;
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| {
        eprintln!("raw mode: {e}");
        1
    })?;
    if let Err(e) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
        eprintln!("enter alt screen: {e}");
        let _ = disable_raw_mode();
        return Err(1);
    }
    // Turn off the host's automatic margin wrap (DECAWM) for our alt screen.
    // strimux positions every run absolutely, so wrapping is never wanted: its
    // only effect is that a run which overshoots the right margin (a glyph the
    // host renders wider than the emulator assumed) spills onto the next row
    // and smears that row's background across the screen. With DECAWM off the
    // overshoot is clamped at the margin and repaired on the next frame.
    let _ = stdout.write_all(b"\x1b[?7l");
    let _ = stdout.flush();
    // Request Kitty keyboard protocol: bare Alt press/release (REPORT_ALL_KEYS)
    // so the reserved-row quasimode (hold ⌥ to reveal) can see the hold
    // itself, not just chords. REPORT_ALTERNATE_KEYS is required so
    // Shift+1 yields '!' (not '1') and shifted letters arrive as their
    // shifted codepoint with SHIFT cleared; without it shift is lost under
    // the Kitty protocol. Falls back gracefully if the terminal ignores it.
    let kitty_keyboard = matches!(
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
            )
        ),
        Ok(())
    );
    if kitty_keyboard {
        tracing::info!("kitty keyboard protocol enabled (bare Alt hover)");
    }
    // Capture the mouse so wheel events land here instead of the host
    // terminal, where they scroll the host's own scrollback / prompt history
    // right past the layout we are drawing. We route each notch to the pane
    // under the cursor.
    if cfg.mouse {
        if let Err(e) = execute!(stdout, EnableMouseCapture) {
            tracing::warn!("enable mouse: {e}");
        }
    }
    // Whether the *host* terminal understands the Kitty graphics protocol.
    // Gates APC passthrough: forwarding graphics sequences to a terminal that
    // does not parse them would print base64 garbage over the frame.
    let host_kitty_graphics = host_supports_kitty_graphics();
    if host_kitty_graphics {
        tracing::info!("host supports kitty graphics; pane image passthrough enabled");
    }
    let (cols, mut rows) = term_size().map_err(|e| {
        eprintln!("size: {e}");
        1
    })?;
    let mut cols = cols.max(1);
    rows = rows.max(2);
    if std::env::var_os("STRIMUX_DEBUG_SIZE").is_some() {
        eprintln!("[strimux] initial terminal size -> {cols} cols x {rows} rows");
    }
    let mut layout = Layout::new(cfg.startup_panes.max(1));
    let (tx, rx) = channel::<PaneMsg>();
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    let initial = command.clone().unwrap_or_default();
    let gw = cols.max(1);
    let gh = rows.saturating_sub(chrome_rows(&cfg)).max(1);
    // Spawn every pane in the initial strip. The first takes the requested
    // `run` command (if any); the rest get the user's shell. Sort by id:
    // `panes` is a HashMap, and unsorted iteration made *which pane runs the
    // command* random (ids are allocated in column order, so id order is
    // column order).
    let mut pane_ids: Vec<PaneId> = layout.panes.keys().copied().collect();
    pane_ids.sort_unstable();
    for (i, pid) in pane_ids.iter().enumerate() {
        let cmd = if i == 0 {
            initial.clone()
        } else {
            String::new()
        };
        match spawn_pane(*pid, &cmd, gw, gh, tx.clone()) {
            Ok(p) => {
                panes.insert(*pid, p);
            }
            Err(e) => {
                eprintln!("spawn: {e}");
                let _ = stdout.write_all(b"\x1b[?7h");
                if cfg.mouse {
                    let _ = execute!(stdout, DisableMouseCapture);
                }
                if cfg.mouse {
                    let _ = execute!(stdout, DisableMouseCapture);
                }
                let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
                let _ = disable_raw_mode();
                return Err(1);
            }
        }
    }
    let mut frame: Vec<Cell> = Vec::new();
    let mut last: Vec<Cell> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut dirty = true;
    let mut bare_alt_held = false;
    let mut chord_alt_until: Option<Instant> = None;
    let mut last_alt_held = false;
    let mut hud_until: Option<Instant> = None;
    let mut last_has_attention = has_attention(&layout);
    // Pane ids created by the spawn-agent verb; these are (re)spawned running
    // the configured `default_agent` harness instead of a plain shell.
    let mut agent_panes: HashSet<PaneId> = HashSet::new();
    // The title currently shown on the host terminal; we only write when it
    // changes so we don't spam the host with identical OSC sequences.
    let mut last_title: String = String::new();

    'main: loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                PaneMsg::Output(pid, bytes) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.grid.feed(&bytes);
                        p.last_output = Instant::now();
                        // Recover Kitty graphics APCs that vt100 swallows and
                        // forward them verbatim to the host. Emitters use
                        // virtual placements (U=1): the APC carries only image
                        // data + id, and the on-screen position comes from
                        // U+10EEEE placeholder cells painted through the grid,
                        // so forwarding is position- and pane-safe (hidden
                        // panes upload pixels but display nothing until their
                        // placeholders are actually painted).
                        if host_kitty_graphics {
                            let apcs = p.apc.extract(&bytes);
                            if !apcs.is_empty() {
                                let _ = stdout.write_all(&apcs);
                                let _ = stdout.flush();
                            }
                        }
                        // Explicit OSC 133 status beats the activity
                        // heuristic from the first marker onward.
                        if let Some(st) = scan_osc133(&bytes) {
                            p.saw_osc133 = true;
                            if let Some(lp) = layout.panes.get_mut(&pid) {
                                lp.status = st;
                            }
                        } else if !p.saw_osc133 {
                            // Fresh output from a protocol-less pane: working.
                            if let Some(lp) = layout.panes.get_mut(&pid) {
                                lp.status = PaneStatus::Running;
                            }
                        }
                        // Answer terminal capability/status queries (fish's DA
                        // probe etc.) so children don't warn or hang.
                        if let Some(reply) = query_reply(&bytes) {
                            let _ = p.writer.write_all(&reply);
                            let _ = p.writer.flush();
                        }
                        dirty = true;
                    }
                }
                PaneMsg::Exited(pid) => {
                    // A pane whose process exited closes naturally, exactly
                    // like Alt+q: remove it from the layout, compact columns
                    // (fill left first), and reap the PTY. When the last pane
                    // exits there is nothing left to show, so strimux quits.
                    if let Some(p) = panes.get_mut(&pid) {
                        p.alive = false;
                    }
                    let total = layout_pane_count(&layout);
                    let in_layout = layout.locate_pane(pid).is_some();
                    if in_layout && total <= 1 {
                        break 'main;
                    }
                    if in_layout {
                        let v = Viewport::new(cols);
                        let f = FollowScroll {
                            margin: cfg.scroll_margin,
                            center: cfg.center_focus,
                        };
                        let _ = layout.apply(Action::ClosePane(pid), v, f);
                        agent_panes.remove(&pid);
                        if let Err(e) =
                            sync_panes(&mut layout, &mut panes, &cfg, &tx, 0, &agent_panes)
                        {
                            tracing::error!("sync panes: {e}");
                        }
                    } else {
                        // Already removed from the layout (explicit kill);
                        // just drop the dead PTY handle.
                        panes.remove(&pid);
                    }
                    dirty = true;
                }
            }
        }

        // Keep the frame sized to the live terminal, even when a resize event
        // is dropped or coalesced. Re-measuring here guarantees the panes stay
        // full-bleed to the actual right margin.
        if refresh_size(&mut cols, &mut rows) {
            layout.clamp_scrolls(Viewport::new(cols));
            dirty = true;
        }

        // Activity heuristic for panes that never speak OSC 133 (plain
        // shells, most TUIs): output within the window means "working",
        // silence past it flips to "wants attention" so the minimap and
        // smart-jump still triage them. Panes with real shell integration
        // are owned by the explicit protocol above and skipped here.
        let now = Instant::now();
        for (pid, p) in panes.iter() {
            if p.saw_osc133 {
                continue;
            }
            let quiet = now.duration_since(p.last_output) >= QUIET_AFTER;
            let want = if quiet {
                PaneStatus::Idle
            } else {
                PaneStatus::Running
            };
            if let Some(lp) = layout.panes.get_mut(pid) {
                if lp.status != want {
                    lp.status = want;
                    dirty = true;
                }
            }
        }

        if event::poll(Duration::from_millis(cfg.input_poll_ms.clamp(1, 50))).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Press => {
                    // Bare Alt hold for quasimode: track before handle_key so chords don't double-count.
                    let bare_alt = is_alt_modifier(&ke);
                    if bare_alt {
                        if !bare_alt_held {
                            bare_alt_held = true;
                            dirty = true;
                        }
                    } else {
                        // Fallback for terminals that don't send bare Alt press/release
                        // (no Kitty keyboard protocol): any Alt chord counts as "held"
                        // for a short window so quasimode still reveals on press-and-hold
                        // via repeated chords (Option+hjkl etc.) or a single chord.
                        let alt_chord = ke.modifiers.contains(KeyModifiers::ALT)
                            || matches!(
                                ke.code,
                                KeyCode::Char('\u{2d9}')
                                    | KeyCode::Char('\u{2206}')
                                    | KeyCode::Char('\u{2da}')
                                    | KeyCode::Char('\u{ac}')
                                    | KeyCode::Char('\u{2026}')
                                    | KeyCode::Char('\u{153}')
                                    | KeyCode::Char('\u{a9}')
                                    | KeyCode::Char('\u{d3}')
                                    | KeyCode::Char('\u{d4}')
                                    | KeyCode::Char('\u{f8ff}')
                                    | KeyCode::Char('\u{d2}')
                            );
                        if alt_chord {
                            chord_alt_until = Some(Instant::now() + Duration::from_millis(600));
                            dirty = true;
                        }
                    }
                    if let Some(cmd) = handle_key(&ke) {
                        match cmd {
                            Cmd::Quit => break 'main,
                            Cmd::Scroll(d) => {
                                let v = Viewport::new(cols);
                                let _ = layout.apply(
                                    Action::ScrollViewport(d),
                                    v,
                                    FollowScroll::default(),
                                );
                                dirty = true;
                            }
                            Cmd::ScrollPane(d) => {
                                if let Some(pid) = focused_pane(&layout) {
                                    if let Some(p) = panes.get_mut(&pid) {
                                        p.h_scroll = (p.h_scroll + d).max(0);
                                        dirty = true;
                                    }
                                }
                            }
                            Cmd::Act(a) => {
                                let v = Viewport::new(cols);
                                let f = FollowScroll {
                                    margin: cfg.scroll_margin,
                                    center: cfg.center_focus,
                                };
                                // Closing the last pane leaves nothing to show,
                                // so strimux exits instead of resurrecting a
                                // fresh default layout.
                                if a == Action::KillPane && layout_pane_count(&layout) <= 1 {
                                    break 'main;
                                }
                                let _ = layout.apply(a, v, f);
                                // A spawn-agent verb ends focused on the new
                                // column just right of the previous focus; mark its pane so sync spawns
                                // the agent harness rather than a shell.
                                if a == Action::SpawnAgent {
                                    if let Some(pid) = focused_pane(&layout) {
                                        agent_panes.insert(pid);
                                    }
                                }
                                if let Err(e) =
                                    sync_panes(&mut layout, &mut panes, &cfg, &tx, 0, &agent_panes)
                                {
                                    tracing::error!("sync panes: {e}");
                                }
                                dirty = true;
                            }
                            Cmd::Input(bytes) => {
                                if let Some(pid) = focused_pane(&layout) {
                                    if let Some(p) = panes.get_mut(&pid) {
                                        // Typing means you want the prompt:
                                        // snap a scrolled-back pane to live.
                                        if p.grid.scroll_to_bottom() {
                                            dirty = true;
                                        }
                                        let _ = p.writer.write_all(&bytes);
                                        let _ = p.writer.flush();
                                    }
                                }
                            }
                            Cmd::SmartJump => {
                                // Jump to the next pane that needs attention
                                // (failed > waiting > done), if any.
                                if let Some(target) = smart_jump_target(&layout) {
                                    let v = Viewport::new(cols);
                                    let f = FollowScroll {
                                        margin: cfg.scroll_margin,
                                        center: cfg.center_focus,
                                    };
                                    let _ = layout.apply(Action::FocusPane(target), v, f);
                                    dirty = true;
                                }
                            }
                            Cmd::Repaint => dirty = true,
                            Cmd::None => {}
                        }
                    }
                }
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Release => {
                    if is_alt_modifier(&ke) && bare_alt_held {
                        bare_alt_held = false;
                        dirty = true;
                    } else if ke.modifiers.contains(KeyModifiers::ALT) || is_alt_modifier(&ke) {
                        // Keep chord hold alive until its timeout expires.
                    }
                }
                Ok(Event::Mouse(me)) => {
                    let chrome = chrome_rows(&cfg);
                    let views = focused_pane_views_with_chrome(
                        &layout,
                        cols,
                        rows,
                        cfg.content_width,
                        &panes,
                        cfg.skeleton,
                        chrome,
                    );
                    if let Some((pid, gx, gy)) = pane_at(&views, me.column, me.row) {
                        if matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
                            && focused_pane(&layout) != Some(pid)
                        {
                            let v = Viewport::new(cols);
                            let f = FollowScroll {
                                margin: cfg.scroll_margin,
                                center: cfg.center_focus,
                            };
                            let _ = layout.apply(Action::FocusPane(pid), v, f);
                            dirty = true;
                        }
                        if let Some(p) = panes.get_mut(&pid) {
                            let wheel = matches!(
                                me.kind,
                                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                            );
                            // A child that asked for mouse reporting (or that
                            // owns the alternate screen, where there is no
                            // scrollback to move) gets the event verbatim, so
                            // wheel scrolling inside vim/less/an agent TUI
                            // behaves exactly as it would natively.
                            if p.grid.wants_mouse() {
                                if let Some(bytes) = sgr_mouse_report(&me, gx, gy) {
                                    let _ = p.writer.write_all(&bytes);
                                    let _ = p.writer.flush();
                                }
                            } else if wheel {
                                if p.grid.alternate_screen() {
                                    // Alternate screen without mouse reporting
                                    // (e.g. `less`): translate the wheel into
                                    // arrow keys, the conventional fallback.
                                    let n = cfg.scroll_lines.max(1);
                                    let key: &[u8] = if me.kind == MouseEventKind::ScrollUp {
                                        b"\x1b[A"
                                    } else {
                                        b"\x1b[B"
                                    };
                                    for _ in 0..n {
                                        let _ = p.writer.write_all(key);
                                    }
                                    let _ = p.writer.flush();
                                } else {
                                    let n = cfg.scroll_lines.max(1) as i32;
                                    let d = if me.kind == MouseEventKind::ScrollUp {
                                        n
                                    } else {
                                        -n
                                    };
                                    if p.grid.scroll_by(d) {
                                        dirty = true;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(Event::Resize(c, r)) => {
                    cols = c.max(1);
                    rows = r.max(2);
                    // A wider terminal shrinks the strip relative to the
                    // viewport; drop any now-invalid scroll immediately so the
                    // strip snaps back to full bleed on the next paint.
                    layout.clamp_scrolls(Viewport::new(cols));
                    dirty = true;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("event read: {e}");
                }
            }
        }

        // Resize grids & PTYs to match current geometry.
        let chrome = chrome_rows(&cfg);
        for v in focused_pane_views_with_chrome(
            &layout,
            cols,
            rows,
            cfg.content_width,
            &panes,
            cfg.skeleton,
            chrome,
        ) {
            let pid = v.pid;
            if let Some(p) = panes.get_mut(&pid) {
                if p.grid.size()
                    != (GridSize {
                        cols: v.grid_cols,
                        rows: v.grid_rows,
                    })
                {
                    p.grid.resize(GridSize {
                        cols: v.grid_cols,
                        rows: v.grid_rows,
                    });
                    let _ = p.master.resize(PtySize {
                        rows: v.grid_rows,
                        cols: v.grid_cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                    dirty = true;
                }
            }
        }

        // Treat strimux as an invisible layer for the host title bar: mirror the
        // focused pane's inner title (set via OSC 0/2 by e.g. jcode) out to the
        // host terminal, so switching panes updates the outer window/status bar
        // to the pane you're actually looking at instead of "strimux". Fall back
        // to a plain strimux label when the focused pane has set no title.
        let effective = focused_pane(&layout)
            .and_then(|pid| panes.get(&pid))
            .map(|p| p.grid.title())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| "strimux".to_string());
        if effective != last_title {
            last_title = effective.clone();
            if let Err(e) = emit_title(&mut stdout, &effective) {
                tracing::warn!("set title: {e}");
            }
        }

        // Track attention & alt flip for quasimode repaint, plus HUD flash
        let now_for_hud = Instant::now();
        if let Some(t) = chord_alt_until {
            if now_for_hud >= t {
                chord_alt_until = None;
            }
        }
        let chord_alt_held = chord_alt_until.is_some();
        let effective_alt_held = bare_alt_held || chord_alt_held;
        let hud_visible = hud_until.map(|t| now_for_hud < t).unwrap_or(false);
        if !hud_visible && hud_until.is_some() {
            hud_until = None;
            dirty = true;
        }
        let cur_has_attention = has_attention(&layout);
        if !last_has_attention
            && cur_has_attention
            && cfg.minimap.mode == crate::config::MinimapMode::ReservedQuasimode
            && cfg.minimap.hud_on_attention_ms > 0
            && !effective_alt_held
            && chrome_rows(&cfg) > 0
        {
            // Attention just arose while the quasimode row is hidden: flash center HUD.
            hud_until =
                Some(now_for_hud + Duration::from_millis(cfg.minimap.hud_on_attention_ms as u64));
            dirty = true;
        }
        if cur_has_attention != last_has_attention || effective_alt_held != last_alt_held {
            dirty = true;
            last_has_attention = cur_has_attention;
            last_alt_held = effective_alt_held;
        }
        let show_hud = hud_until.map(|t| Instant::now() < t).unwrap_or(false);
        if dirty {
            render_frame(
                &mut frame,
                &layout,
                &mut panes,
                cols,
                rows,
                cfg.content_width,
                cfg.background.color(),
                cfg.focus_color.color(),
                cfg.skeleton.then(|| cfg.skeleton_color.color()),
                &cfg.minimap,
            );
            // Quasimode: hide reserved row when Alt not held and no attention
            let chrome = chrome_rows(&cfg);
            if chrome > 0
                && cfg.minimap.mode == crate::config::MinimapMode::ReservedQuasimode
                && !cfg
                    .minimap
                    .should_paint(effective_alt_held, cur_has_attention)
                && rows > 1
            {
                let y = (rows - 1) as usize;
                let base = y * cols as usize;
                // Blank chrome row to the background fill (no content)
                for x in 0..cols as usize {
                    if let Some(c) = frame.get_mut(base + x) {
                        *c = Cell {
                            ch: ' ',
                            style: strimux_term::Style {
                                bg: cfg.background.color(),
                                ..Default::default()
                            },
                            width: 1,
                            ..Default::default()
                        };
                    }
                }
            }
            if show_hud {
                draw_center_hud(&mut frame, cols, rows, &layout, cfg.focus_color.color());
            }
            buf.clear();
            paint(&mut buf, &frame, &last, cols, rows);
            if !buf.is_empty() {
                // Synchronized update (ESC[?2026h/l): the host terminal holds
                // the screen and applies the whole frame atomically, so a
                // repaint can never be displayed half-drawn (visible shearing
                // when a vsync lands mid-write). Terminals that don't support
                // it ignore the markers.
                let _ = stdout.write_all(b"\x1b[?2026h");
                let _ = stdout.write_all(&buf);
                let _ = stdout.write_all(b"\x1b[?2026l");
                let _ = stdout.flush();
                last = frame.clone();
            }
            dirty = false;
        }
    }

    // Teardown: kill all panes, leave raw mode & alternate screen.
    for p in panes.values_mut() {
        let _ = p.child.kill();
    }
    if kitty_keyboard {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    // Restore the host's autowrap before handing the terminal back.
    let _ = stdout.write_all(b"\x1b[?7h");
    let _ = stdout.flush();
    if cfg.mouse {
        let _ = execute!(stdout, DisableMouseCapture);
    }
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
    Ok(())
}

/// Pick the pane a smart-jump (`⌥+g`) should land on: the next pane, in
/// layout order starting just past the focused one and wrapping, whose status
/// needs the user. Priority: Failed beats Idle (attention) beats Done;
/// Running panes are never targets (they're fine on their own). Returns None
/// when every other pane is happily working.
fn smart_jump_target(layout: &Layout) -> Option<PaneId> {
    let focused = focused_pane(layout);
    // Flatten the grid in reading order: strips top-down, columns
    // left-to-right, stacks top-down.
    let order: Vec<PaneId> = layout
        .rows
        .iter()
        .flat_map(|r| r.columns.iter())
        .flat_map(|c| c.panes.iter().copied())
        .collect();
    let start = focused
        .and_then(|f| order.iter().position(|p| *p == f))
        .map(|i| i + 1)
        .unwrap_or(0);
    let rank = |s: PaneStatus| match s {
        PaneStatus::Failed => Some(0u8),
        PaneStatus::Idle => Some(1),
        PaneStatus::Done => Some(2),
        PaneStatus::Running => None,
    };
    let mut best: Option<(u8, usize, PaneId)> = None;
    for (i, pid) in order
        .iter()
        .enumerate()
        .cycle()
        .skip(start)
        .take(order.len())
    {
        if Some(*pid) == focused {
            continue;
        }
        let Some(st) = layout.panes.get(pid).map(|p| p.status) else {
            continue;
        };
        let Some(r) = rank(st) else { continue };
        // Distance from the focused pane, wrapping: nearer wins within a rank.
        let dist = (i + order.len() - start) % order.len();
        if best.map(|(br, bd, _)| (r, dist) < (br, bd)).unwrap_or(true) {
            best = Some((r, dist, *pid));
        }
    }
    best.map(|(_, _, pid)| pid)
}

/// Total number of panes currently present in the layout.
fn layout_pane_count(layout: &Layout) -> usize {
    layout
        .rows
        .iter()
        .flat_map(|r| r.columns.iter())
        .map(|c| c.panes.len())
        .sum()
}

/// The currently focused pane id.
fn focused_pane(layout: &Layout) -> Option<PaneId> {
    layout
        .focused_row()
        .and_then(|r| r.columns.get(layout.focus.column))
        .and_then(|c| c.panes.get(layout.focus.pane))
        .copied()
}

/// The pane whose on-screen rect contains `(x, y)`, plus the cell coordinates
/// *inside* that pane's grid. Only panes in the focused strip are visible, so
/// only those can be hit.
fn pane_at(views: &[PaneView], x: u16, y: u16) -> Option<(PaneId, u16, u16)> {
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

/// Encode a mouse event as an SGR (1006) report for a child that asked for
/// mouse reporting, with coordinates translated into the pane's own grid
/// (1-based, as the protocol requires).
fn sgr_mouse_report(ev: &MouseEvent, gx: u16, gy: u16) -> Option<Vec<u8>> {
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

    #[test]
    fn pane_window_shows_leading_content() {
        // 240-col content, 80-col rect, no scroll: reveal [0, 80).
        assert_eq!(pane_window(0, 0, 80, 240), Some((0, 80)));
    }

    #[test]
    fn handle_key_option_semicolon_spawns_agent() {
        // macOS sends U+2026 (…) for Option+; when it doesn't translate to Meta.
        let ev = KeyEvent::new(KeyCode::Char('\u{2026}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgent)));
        // Terminals that deliver Option as Meta send ESC+; -> Alt+;.
        let ev = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgent)));
    }

    #[test]
    fn handle_key_alt_q_kills_pane() {
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::KillPane)));
        // macOS Option+q -> œ (U+0153) on the no-Meta path.
        let ev = KeyEvent::new(KeyCode::Char('\u{153}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::KillPane)));
    }

    #[test]
    fn handle_key_option_shift_hjkl_moves_pane() {
        // Terminals delivering Option as Meta: Alt+Shift+hjkl.
        for (c, act) in [
            ('h', Action::MovePaneLeft),
            ('j', Action::MovePaneDown),
            ('k', Action::MovePaneUp),
            ('l', Action::MovePaneRight),
        ] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
            // Uppercase variant, with or without an explicit SHIFT bit.
            let up = c.to_ascii_uppercase();
            let ev = KeyEvent::new(KeyCode::Char(up), KeyModifiers::ALT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
        }
        // macOS no-Meta path: Option+Shift+hjkl arrive as Ó Ô  Ò.
        for (g, act) in [
            ('\u{d3}', Action::MovePaneLeft),
            ('\u{d4}', Action::MovePaneDown),
            ('\u{f8ff}', Action::MovePaneUp),
            ('\u{d2}', Action::MovePaneRight),
        ] {
            let ev = KeyEvent::new(KeyCode::Char(g), KeyModifiers::SHIFT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
        }
    }

    #[test]
    fn caps_lock_does_not_trigger_shift_chords() {
        // Caps+h sends an uppercase 'H' with CAPS_LOCK state (state is
        // produced by the kitty bit-64 alternate). It must NOT become
        // MovePaneLeft (which requires physical Shift).
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('H'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::FocusLeft)));
        // Same for prefix: Caps should not fake Shift+hjkl.
        IN_PREFIX.store(true, Ordering::Relaxed);
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('H'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::FocusLeft)));
    }

    #[test]
    fn kitty_shifted_key_with_shift_cleared_is_still_a_move() {
        // With REPORT_ALTERNATE_KEYS the shift is consumed: 'H' arrives with no
        // SHIFT modifier but without CAPS_LOCK, so physical_shift sees it as Shift.
        let ev = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::MovePaneLeft)));
        IN_PREFIX.store(true, Ordering::Relaxed);
        let ev = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::MovePaneUp)));
    }

    #[test]
    fn shift_and_caps_typed_text_passes_through_as_shifted() {
        // Plain Shift+a -> 'A' is pane input, not a strimux chord.
        let ev = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
        // Shift+1 -> '!' via the shifted codepoint path.
        let ev = KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"!".to_vec())));
        // Caps+a also produces 'A' (caps state) but should still type 'A' when
        // not an Alt/prefix chord, just like Shift. Focus test is plain key:
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('A'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
    }

    #[test]
    fn emit_title_writes_osc2_st() {
        let mut out = Vec::new();
        emit_title(&mut out, "jcode: my session").unwrap();
        assert_eq!(out, b"\x1b]2;jcode: my session\x1b\\");
    }

    #[test]
    fn sanitize_title_strips_control_and_clips() {
        // Ordinary text passes through untouched.
        assert_eq!(sanitize_title("abc 123"), "abc 123");
        // Control characters (ESC/BEL/CR/LF) are dropped so a child cannot
        // smuggle state-changing escapes out through the title; printable
        // characters inside the OSC payload are preserved verbatim.
        assert_eq!(sanitize_title("a\x1b]0;evil\x07b"), "a]0;evilb");
        assert_eq!(sanitize_title("\x01\x02"), "");
        // Over-long titles are clipped to a sane window-title length.
        let long = "x".repeat(1000);
        assert_eq!(sanitize_title(&long).chars().count(), 256);
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

    #[test]
    fn paint_emits_combining_marks_with_base_glyph() {
        // A cell holding a Kitty image placeholder (U+10EEEE) with row/col
        // diacritics: the diacritics are what address the image, so they must
        // reach the host bytes right after the base char.
        let mut row = vec![Cell::default(); 3];
        row[0].ch = '\u{10EEEE}';
        row[0].combining[0] = '\u{0305}';
        row[0].combining[1] = '\u{030D}';
        // width() for U+10EEEE is None (unassigned plane), so the run is cut
        // and printed alone; that must not drop the combining marks.
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            3
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 3, 1));
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("\u{10EEEE}\u{0305}\u{030D}"),
            "combining marks split from base: {s:?}"
        );
    }

    #[test]
    fn paint_skips_wide_continuation_cells() {
        // Row: wide '你' (head width 2, then a width-0 continuation), then "ab".
        // If the continuation's placeholder space were printed, 'a' would land
        // one column too far right and shear the row.
        let mut row = vec![Cell::default(); 6];
        row[0] = Cell {
            ch: '你',
            width: 2,
            ..Cell::default()
        };
        row[1] = Cell {
            ch: ' ',
            width: 0,
            ..Cell::default()
        };
        row[2].ch = 'a';
        row[3].ch = 'b';
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            6
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 6, 1));
        let s = String::from_utf8(buf).unwrap();
        // The wide glyph is printed exactly once and the continuation's
        // placeholder space is never printed between it and 'a'.
        assert_eq!(s.matches('你').count(), 1);
        assert!(!s.contains("你 a"), "continuation cell was printed: {s:?}");
        // 'a' is re-positioned to its true column (x=2) with an explicit
        // MoveTo (CUP row 1, col 3 -> ESC[1;3H) rather than relying on the
        // host's cursor advance across the wide glyph.
        assert!(
            s.contains("\u{1b}[1;3H"),
            "missing MoveTo before 'a': {s:?}"
        );
    }

    #[test]
    fn paint_cuts_runs_at_width_ambiguous_glyphs() {
        // A highlighted (styled) row of Hangul: vt100 records each syllable as
        // a single-width cell, but the host renders it two columns wide. Merged
        // into one run, the run overshoots the right margin, wraps, and smears
        // its background down the screen ("highlight overflow"). Each such
        // glyph must therefore be printed as its own MoveTo-anchored run.
        let hl = strimux_term::Style {
            bg: CColor::Idx(238),
            ..strimux_term::Style::default()
        };
        let text = "\u{ac00}\u{b098}\u{b2e4}"; // 가나다
        let mut row = vec![
            Cell {
                style: hl,
                ..Cell::default()
            };
            4
        ];
        for (i, ch) in text.chars().enumerate() {
            row[i] = Cell {
                ch,
                style: hl,
                width: 1, // emulator's (wrong for this host) idea of the width
                ..Cell::default()
            };
        }
        let last = vec![Cell::default(); 4];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 4, 1));
        let s = String::from_utf8(buf).unwrap();
        // Never merged: no two ambiguous glyphs share a run.
        assert!(
            !s.contains("\u{ac00}\u{b098}"),
            "ambiguous glyphs merged into one run: {s:?}"
        );
        // Every glyph is re-anchored with an explicit absolute cursor move, so
        // a host/emulator width disagreement cannot drift past this cell.
        for (i, ch) in text.chars().enumerate() {
            let mv = format!("\x1b[1;{}H", i + 1);
            let at = s
                .find(&mv)
                .unwrap_or_else(|| panic!("no MoveTo for col {i}: {s:?}"));
            let g = s.find(ch).unwrap();
            assert!(at < g, "glyph {ch} printed before its MoveTo: {s:?}");
        }
    }

    #[test]
    fn paint_keeps_merging_plain_ascii_runs() {
        // The cut must be surgical: ordinary text still batches into one run.
        let mut row = vec![Cell::default(); 6];
        for (i, ch) in "hello".chars().enumerate() {
            row[i].ch = ch;
        }
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            6
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 6, 1));
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("hello"), "ascii run was split: {s:?}");
    }

    #[test]
    fn paint_resets_attributes_between_runs() {
        // Regression for the popup "line overflow": an underlined run followed
        // by a plain run on the same row. SGR attrs are additive, so without a
        // reset at the start of the second run the underline bleeds across the
        // rest of the row on the host terminal.
        let mut row = vec![Cell::default(); 4];
        row[0].ch = 'u';
        row[0].style.underline = true;
        row[1].ch = 'p';
        let last = vec![
            Cell {
                ch: 'x',
                ..Cell::default()
            };
            4
        ];
        let mut buf = Vec::new();
        assert!(paint(&mut buf, &row, &last, 4, 1));
        let s = String::from_utf8(buf).unwrap();
        // Underline (SGR 4) is enabled for the first run, and a full reset
        // (SGR 0) is emitted after it and before the plain run's text.
        let under = s.find("\u{1b}[4m").expect("underline never set");
        let reset_after = s[under..]
            .find("\u{1b}[0m")
            .expect("no attribute reset after underlined run");
        let plain = s.find('p').expect("plain run missing");
        assert!(
            under + reset_after < plain,
            "underline leaks into the plain run: {s:?}"
        );
    }

    #[test]
    fn mouse_hit_test_maps_screen_cell_to_pane_grid() {
        use strimux_layout::{Preset, Width};
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
    fn four_quarter_panes_fill_screen_without_overflow() {
        // Regression for the reported bug: at 342 cols (not divisible by 4),
        // per-column ceil widths summed to 344 and the 4th pane's rect ran
        // past the right edge. Exercise the real render path: layout ->
        // focused_pane_views -> on-screen Rects.
        use strimux_layout::{Preset, Width};
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
    fn draw_minimap_highlights_focus_bottom_right() {
        use strimux_layout::Width;
        let mut layout = Layout::default();
        // Add a second strip so the map has something to orient against.
        let r2 = layout.new_row("two".to_string());
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
            accent,
        );
        // Two strips -> map height 2, width 32 (default max). Bottom-right:
        // ox = 40-32 = 8, oy = 8-2 = 6.
        let cell = |x: usize, y: usize| out[y * 40 + x];
        // Focused pane tile (row 0, col 0) is painted with the accent.
        let focus = cell(8, 6);
        assert_eq!(focus.style.bg, accent, "focused tile uses the focus color");
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
        // Fresh panes are Running: a muted Mocha blue tint carrying a `»` glyph at
        // the tile's right edge.
        assert_eq!(
            other.style.bg,
            CColor::Rgb(0x52, 0x6c, 0x96),
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
            accent,
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
            accent,
        );
        assert!(
            out3.iter().any(|c| c.style.bg != CColor::Default),
            "multi-pane single strip draws the map"
        );
    }

    #[test]
    fn draw_minimap_status_colors_and_failed_glyph() {
        use strimux_layout::Width;
        let mut layout = Layout::default(); // 4 quarter panes on strip 1
        let r2 = layout.new_row("two".to_string());
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
            accent,
        );
        let cell = |x: usize, y: usize| out[y * cols + x];
        // Map: ox=8, oy=6. Strip 1 has 4 tiles of 8 cells each.
        let (ox, y) = (8usize, 6usize);
        assert_eq!(cell(ox, y).style.bg, accent, "tile 1 focused");
        assert_eq!(
            cell(ox + 8, y).style.bg,
            CColor::Rgb(0x63, 0x88, 0x60),
            "tile 2 done"
        );
        assert_eq!(
            cell(ox + 16, y).style.bg,
            CColor::Rgb(0x91, 0x53, 0x64),
            "tile 3 failed"
        );
        assert_eq!(
            cell(ox + 24, y).style.bg,
            CColor::Rgb(0x96, 0x6b, 0x51),
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
    fn scan_osc133_maps_protocol_to_status() {
        // Prompt marker -> waiting for input.
        assert_eq!(scan_osc133(b"\x1b]133;A\x07"), Some(PaneStatus::Idle));
        // Command start -> running.
        assert_eq!(scan_osc133(b"\x1b]133;C\x07"), Some(PaneStatus::Running));
        // Command done, exit 0 (and the bare form) -> done.
        assert_eq!(scan_osc133(b"\x1b]133;D;0\x07"), Some(PaneStatus::Done));
        assert_eq!(scan_osc133(b"\x1b]133;D\x1b\\"), Some(PaneStatus::Done));
        // Non-zero exit -> failed.
        assert_eq!(scan_osc133(b"\x1b]133;D;127\x07"), Some(PaneStatus::Failed));
        // The *last* marker in a chunk wins (C then D;1 -> failed).
        assert_eq!(
            scan_osc133(b"\x1b]133;C\x07output\x1b]133;D;1\x07"),
            Some(PaneStatus::Failed)
        );
        // Ordinary output and other OSCs carry no status.
        assert_eq!(scan_osc133(b"plain output"), None);
        assert_eq!(scan_osc133(b"\x1b]2;title\x07"), None);
        // B (input start) is not a status change.
        assert_eq!(scan_osc133(b"\x1b]133;B\x07"), None);
    }

    #[test]
    fn smart_jump_prefers_failed_then_attention() {
        let mut layout = Layout::default(); // 4 panes, focus on pane 0
        let ids: Vec<PaneId> = layout.rows[0]
            .columns
            .iter()
            .flat_map(|c| c.panes.clone())
            .collect();
        // All running: nothing needs the user.
        assert_eq!(smart_jump_target(&layout), None);
        // Pane 3 done, pane 2 idle, pane 1 failed: failed wins outright.
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Done;
        assert_eq!(smart_jump_target(&layout), Some(ids[3]));
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Idle;
        assert_eq!(
            smart_jump_target(&layout),
            Some(ids[2]),
            "attention beats done"
        );
        layout.panes.get_mut(&ids[1]).unwrap().status = PaneStatus::Failed;
        assert_eq!(smart_jump_target(&layout), Some(ids[1]), "failed beats all");
        // The focused pane is never a target even when it failed.
        layout.panes.get_mut(&ids[0]).unwrap().status = PaneStatus::Failed;
        assert_eq!(smart_jump_target(&layout), Some(ids[1]));
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
        );
        // A degenerate 1x1 rect degrades to a horizontal rule glyph.
        assert_eq!(out[0].ch, '─');
        assert_eq!(out[0].style.fg, CColor::Idx(1));
    }

    /// A disabled minimap config for geometry tests that assert the bottom
    /// screen rows the map would otherwise overlay.
    fn no_map() -> crate::config::Minimap {
        crate::config::Minimap {
            show: false,
            mode: crate::config::MinimapMode::Overlay,
            ..Default::default()
        }
    }

    #[test]
    fn skeleton_frames_four_boxes_with_red_focus() {
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
            CColor::Default,
            red,
            Some(white),
            &no_map(),
        );
        let ranges = layout.column_x_ranges(layout.focus.row, cols).unwrap();
        assert_eq!(ranges.len(), 4);
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        for (ci, (s, e)) in ranges.iter().enumerate() {
            let want = if ci == 0 { red } else { white };
            let (s, e) = (*s as u16, *e as u16 - 1);
            // Corners of the box frame: thin rounded glyphs in the frame color.
            assert_eq!(at(s, 0).ch, '╭', "top-left of box {ci}");
            assert_eq!(at(e, 0).ch, '╮', "top-right of box {ci}");
            assert_eq!(at(s, rows - 1).ch, '╰', "bottom-left of box {ci}");
            assert_eq!(at(e, rows - 1).ch, '╯', "bottom-right of box {ci}");
            assert_eq!(at(s, 0).style.fg, want, "frame color of box {ci}");
            // Vertical edges run the full strip height.
            assert_eq!(at(s, rows / 2).ch, '│', "left edge of box {ci}");
            assert_eq!(at(e, rows / 2).ch, '│', "right edge of box {ci}");
            assert_eq!(at(s, rows / 2).style.fg, want, "left edge color {ci}");
            assert_eq!(at(e, rows / 2).style.fg, want, "right edge color {ci}");
        }
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
            assert_eq!(
                v.rect.x + v.rect.w,
                *e as u16 - 1,
                "content ends inside frame"
            );
            assert_eq!(v.rect.y, 1, "content below the top frame row");
            assert_eq!(v.rect.y + v.rect.h, rows - 1, "content above bottom row");
            // Emulator geometry matches the inset rect exactly.
            assert_eq!(v.grid_cols, v.rect.w);
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
            CColor::Default,
            CColor::Rgb(0xff, 0, 0),
            Some(white),
            &no_map(),
        );
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        // Placeholder boxes cover [40,60) and [60,80): white thin frames all
        // the way to the last screen column.
        assert_eq!(at(40, 0).ch, '╭', "placeholder box 3 left edge");
        assert_eq!(at(59, 0).ch, '╮', "placeholder box 3 right edge");
        assert_eq!(at(60, 0).ch, '╭', "placeholder box 4 left edge");
        assert_eq!(at(cols - 1, 0).ch, '╮', "skeleton reaches screen edge");
        assert_eq!(at(cols - 1, rows - 1).ch, '╯', "bottom-right corner");
        for (x, y) in [(40, 0), (59, 0), (60, 0), (cols - 1, 0)] {
            assert_eq!(at(x, y).style.fg, white, "frame color at ({x},{y})");
        }
    }

    #[test]
    fn placeholder_boxes_are_not_dimmed_and_show_cell_identifiers() {
        // Empty placeholder boxes read like live panes: their interiors are
        // reset to the default background (not the dim `background` fill) and
        // a big block-font `strip.cell` identifier is centered in each.
        let layout = Layout::new(2); // boxes 3 and 4 are placeholders
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let cols: u16 = 80;
        let rows: u16 = 12;
        let dim = CColor::Idx(235);
        let label = CColor::Rgb(0x58, 0x5b, 0x70);
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            dim,
            CColor::Rgb(0xff, 0, 0),
            Some(CColor::Rgb(0xff, 0xff, 0xff)),
            &no_map(),
        );
        let bg = |x: u16, y: u16| out[y as usize * cols as usize + x as usize].style.bg;
        // Placeholder interiors are default-bg, never the dim background.
        for x in 41..59 {
            for y in 1..rows - 1 {
                assert_ne!(bg(x, y), dim, "dim background leaked at ({x},{y})");
            }
        }
        // The identifier is drawn in the label color somewhere inside each
        // placeholder box (boxes 3 and 4 -> labels "1.3" and "1.4").
        for (x0, x1) in [(41u16, 59u16), (61, 79)] {
            let painted = (x0..x1)
                .flat_map(|x| (1..rows - 1).map(move |y| (x, y)))
                .filter(|&(x, y)| bg(x, y) == label)
                .count();
            assert!(
                painted >= 11,
                "expected a block-font identifier in box [{x0},{x1}), found {painted} cells"
            );
        }
        // Live (non-placeholder) box interiors carry no identifier.
        let painted_live = (1..19u16)
            .flat_map(|x| (1..rows - 1).map(move |y| (x, y)))
            .filter(|&(x, y)| bg(x, y) == label)
            .count();
        assert_eq!(painted_live, 0, "live boxes must not show identifiers");
    }

    #[test]
    fn big_label_skipped_when_rect_too_small() {
        // A rect too small for the 3x5 font stays untouched instead of
        // rendering a clipped, unreadable fragment.
        let cols: u16 = 10;
        let mut out = vec![Cell::default(); (cols as usize) * 4];
        let rect = Rect {
            x: 0,
            y: 0,
            w: 6,
            h: 4,
        };
        draw_big_label(&mut out, cols, rect, "1.1", CColor::Rgb(0x58, 0x5b, 0x70));
        assert!(
            out.iter().all(|c| c.style.bg == CColor::Default),
            "no partial label may be painted"
        );
    }

    #[test]
    fn center_hud_paints_centered_box_with_attention_hint() {
        // HUD flash: a centered 3-row box with the smart-jump target hint.
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
        draw_center_hud(&mut out, cols, rows, &layout, frame_color);
        // The box is 3 rows tall centered at (cols,bw) = 80, rows=24 -> oy = 10, mid text row = 11.
        let has_frame = out
            .iter()
            .any(|c| c.ch == '╭' || c.ch == '╮' || c.ch == '╰' || c.ch == '╯');
        assert!(has_frame, "HUD box frame painted");
        let oy = ((rows as usize) - 3) / 2;
        let mid = oy + 1;
        let row: String = (0..cols)
            .map(|x| out[mid * cols as usize + x as usize].ch)
            .collect();
        assert!(
            row.contains("needs you"),
            "HUD hint text centered, got {row:?}"
        );
        assert!(row.contains("⌥+g"), "HUD jump hint present, got {row:?}");
        // Also verify any row in the box contains the hint, as a fallback if geometry changes.
        let any_hint = (0..rows).any(|y| {
            let s: String = (0..cols)
                .map(|x| out[y as usize * cols as usize + x as usize].ch)
                .collect();
            s.contains("needs you")
        });
        assert!(any_hint, "HUD hint present somewhere in box");
        // Tiny viewport: nothing painted.
        let mut tiny = vec![Cell::default(); 10 * 4];
        draw_center_hud(&mut tiny, 10, 4, &layout, frame_color);
        assert!(
            tiny.iter().all(|c| c.ch == ' '),
            "tiny viewport draws no HUD"
        );
    }

    #[test]
    fn quasimode_chrome_and_hud_defaults() {
        // Defaults: ReservedQuasimode with 1 chrome row, HUD on.
        let mm = crate::config::Minimap::default();
        assert_eq!(mm.mode, crate::config::MinimapMode::ReservedQuasimode);
        assert_eq!(mm.chrome_rows(), 1);
        assert_eq!(mm.hud_on_attention_ms, 2500);
        // should_paint: quasimode reveals only when alt held or attention.
        assert!(!mm.should_paint(false, false), "quasimode hidden at rest");
        assert!(mm.should_paint(true, false), "alt held reveals");
        assert!(mm.should_paint(false, true), "attention reveals");
        // Off never paints; Overlay/Reserved ignore hidden.
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
            mode: crate::config::MinimapMode::Reserved,
            ..mm
        }
        .should_paint(false, false));
        // hud_on_attention_ms = 0 disables flash explicitly.
        let no_hud = crate::config::Minimap {
            hud_on_attention_ms: 0,
            ..mm
        };
        assert_eq!(no_hud.hud_on_attention_ms, 0);
    }

    #[test]
    fn draw_minimap_overlay_still_paints_without_chrome() {
        // Overlay path does not depend on chrome rows.
        let mut layout = Layout::default();
        let r2 = layout.new_row("two".to_string());
        let pid = layout.alloc_pane();
        layout.add_column(r2, strimux_layout::Width::Cells(20), vec![pid]);
        let mm = crate::config::Minimap {
            mode: crate::config::MinimapMode::Overlay,
            ..Default::default()
        };
        let mut out = vec![Cell::default(); 40 * 8];
        draw_minimap(&mut out, 40, 8, &layout, &mm, CColor::Idx(36));
        let any = out
            .iter()
            .any(|c| c.style.bg != CColor::Default && c.ch != ' ');
        assert!(any, "overlay draws tiles even with hidden quasimode row");
    }
}

#[test]
fn content_scroll_reveals_overflow_e2e() {
    let mut layout = Layout::default();
    let pid = focused_pane(&layout).expect("default layout has a focused pane");
    // Widen the single default column to the full viewport so we see 80 cells.
    if let Some(row) = layout.row_mut(layout.focus.row) {
        row.columns[0].width = strimux_layout::Width::Cells(80);
    }
    let (tx, rx) = channel::<PaneMsg>();
    let cmd = "sh -c \"for i in $(seq 1 240); do printf '%s' $((i % 10)); done; echo\"";
    let pane = spawn_pane(pid, cmd, 240, 10, tx.clone()).expect("spawn pane");
    let mut pane = pane;
    // Feed PTY output until the 240-cell digit line has landed.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    'feed: while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PaneMsg::Output(_, bytes)) => {
                pane.grid.feed(&bytes);
                if pane.grid.cell(239, 0).ch != ' ' {
                    break 'feed;
                }
            }
            Ok(PaneMsg::Exited(_)) | Err(_) => break 'feed,
        }
    }
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    panes.insert(pid, pane);
    let mut out = Vec::new();

    // At scroll 0 the viewport shows content columns 0..79 (digits 1,2,...,0).
    panes.get_mut(&pid).unwrap().h_scroll = 0;
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        80,
        10,
        240,
        CColor::Default,
        CColor::Default,
        None,
        &crate::config::Minimap::default(),
    );
    assert_eq!(out[0].ch, '1'); // content col 0 -> screen x=0 (full-bleed)
    assert_eq!(out[9].ch, '0'); // content col 9  -> screen x=9
    assert_eq!(out[77].ch, '8'); // content col 77 -> screen x=77

    // Scrolling 60 pans 60 cells; content col 60 leads at screen x=0.
    panes.get_mut(&pid).unwrap().h_scroll = 60;
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        80,
        10,
        240,
        CColor::Default,
        CColor::Default,
        None,
        &crate::config::Minimap::default(),
    );
    assert_eq!(out[0].ch, '1'); // content col 60 -> screen x=0
    assert_eq!(out[1].ch, '2'); // content col 61 -> screen x=1
    assert_eq!(out[77].ch, '8'); // content col 137 -> screen x=77

    // Past the 240-col content the window reveals blanks.
    panes.get_mut(&pid).unwrap().h_scroll = 200;
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        80,
        10,
        240,
        CColor::Default,
        CColor::Default,
        None,
        &crate::config::Minimap::default(),
    );
    assert_eq!(out[0].ch, '1'); // content col 200 -> screen x=0
    assert_eq!(out[39].ch, '0'); // content col 239 -> screen x=39
    assert_eq!(out[45].ch, ' '); // past content end -> blank

    let _ = panes.get_mut(&pid).unwrap().child.kill();
}

/// End-to-end acceptance for the quarter-pane overflow fix: four real PTY
/// children, one per quarter column, rendered by `render_frame` at 342 cols
/// (the reported failure width, not divisible by 4). Every screen cell up to
/// and including the rightmost column must show the pane that owns it, with
/// pane content never spilling past a column boundary or the screen edge.
#[test]
fn four_quarter_panes_render_to_screen_edge_e2e() {
    use strimux_layout::{Preset, Width};
    let cols: u16 = 342;
    let rows: u16 = 8;
    let mut layout = Layout::new(1);
    if let Some(r) = layout.row_mut(layout.focus.row) {
        r.columns.clear();
    }
    let row = layout.focus.row;
    let fills = ['A', 'B', 'C', 'D'];
    let mut pids = Vec::new();
    for _ in fills {
        let p = layout.alloc_pane();
        layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
        pids.push(p);
    }
    let ranges = layout
        .column_x_ranges(row, cols)
        .expect("ranges for the four-quarter row");
    assert_eq!(ranges.last().unwrap().1, cols as u32);

    // Spawn one real child per pane, each filling a line with its letter.
    let (tx, rx) = channel::<PaneMsg>();
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    for (i, pid) in pids.iter().enumerate() {
        let w = (ranges[i].1 - ranges[i].0) as u16;
        let cmd = format!(
            "sh -c \"for i in $(seq 1 {w}); do printf '%s' {}; done; echo\"",
            fills[i]
        );
        let pane = spawn_pane(*pid, &cmd, w, rows, tx.clone()).expect("spawn pane");
        panes.insert(*pid, pane);
    }
    // Feed PTY output until every pane's first row is fully painted.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let done = pids.iter().enumerate().all(|(i, pid)| {
            let w = (ranges[i].1 - ranges[i].0) as u16;
            panes
                .get(pid)
                .map(|p| p.grid.cell(w.saturating_sub(1), 0).ch == fills[i])
                .unwrap_or(false)
        });
        if done {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PaneMsg::Output(pid, bytes)) => {
                if let Some(p) = panes.get_mut(&pid) {
                    p.grid.feed(&bytes);
                }
            }
            Ok(PaneMsg::Exited(_)) => {}
            Err(_) => {}
        }
    }

    let mut out = Vec::new();
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        cols,
        rows,
        0,
        CColor::Default,
        CColor::Default,
        None,
        &crate::config::Minimap::default(),
    );
    // The top row shows each pane's letter across its exact range: the
    // rightmost screen cell belongs to pane 'D' and no boundary bleeds.
    for (i, (s, e)) in ranges.iter().enumerate() {
        for x in *s..*e {
            assert_eq!(
                out[x as usize].ch, fills[i],
                "screen x={x} must show pane {} content",
                fills[i]
            );
        }
    }
    assert_eq!(out[cols as usize - 1].ch, 'D', "rightmost cell is pane D");
    for p in panes.values_mut() {
        let _ = p.child.kill();
    }
}

#[cfg(test)]
mod strip_label_tests {
    use super::*;
    use strimux_layout::{Action, FollowScroll, Viewport};

    #[test]
    fn strip_number_tracks_position_not_row_id() {
        let v = Viewport::new(80);
        let f = FollowScroll::default();
        let mut layout = Layout::new(1);
        assert_eq!(strip_number(&layout), 1);
        // Down into a fresh strip: it is the second strip, so "2".
        let _ = layout.apply(Action::FocusDown, v, f);
        assert_eq!(strip_number(&layout), 2);
        // Bouncing up and down repeatedly allocates new row ids each time, but
        // the label must stay 2 rather than drifting to 3, 4, 5 ...
        for _ in 0..4 {
            let _ = layout.apply(Action::FocusUp, v, f);
            assert_eq!(strip_number(&layout), 1);
            let _ = layout.apply(Action::FocusDown, v, f);
            assert_eq!(strip_number(&layout), 2);
        }
        // A populated second strip plus a new third one keeps counting by
        // position.
        let _ = layout.apply(Action::NewColumn, v, f);
        let _ = layout.apply(Action::FocusDown, v, f);
        assert_eq!(strip_number(&layout), 3);
    }
}

/// Acceptance for scroll-state paint stability (the user-visible bug: "grids
/// are painted slightly differently across different scroll states despite
/// identical panes"). Eight identical quarter columns of real PTY panes at
/// 342 cols (not divisible by 4, the width where absolute-rounded boundaries
/// wobble by one cell between stops). Walking focus across the whole strip in
/// the skeleton renderer, the x-positions of the vertical frame edges painted
/// on a mid-strip row must be identical in every frame.
#[test]
fn identical_grids_paint_identically_across_scroll_states_e2e() {
    use strimux_layout::{Action, FollowScroll, Preset, Viewport, Width};
    let cols: u16 = 342;
    let rows: u16 = 8;
    let n = 8usize;
    let mut layout = Layout::new(1);
    if let Some(r) = layout.row_mut(layout.focus.row) {
        r.columns.clear();
    }
    let row = layout.focus.row;
    let mut pids = Vec::new();
    for _ in 0..n {
        let p = layout.alloc_pane();
        layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
        pids.push(p);
    }
    // Real PTY children (sleeping shells: content is irrelevant, the frame
    // geometry is what must not wobble).
    let (tx, _rx) = channel::<PaneMsg>();
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    for pid in &pids {
        let pane = spawn_pane(*pid, "sleep 30", 80, rows, tx.clone()).expect("spawn pane");
        panes.insert(*pid, pane);
    }

    let vp = Viewport::new(cols);
    let f = FollowScroll::default();
    let mut out = Vec::new();
    // Frames are thin box-drawing glyphs; the vertical edges (`│`) crossing a
    // mid-strip row mark every column boundary on screen.
    let mut boundary_sets: Vec<(i32, Vec<u16>)> = Vec::new();
    let mut paint = |layout: &Layout, panes: &mut HashMap<PaneId, PtyPane>| -> Vec<u16> {
        render_frame(
            &mut out,
            layout,
            panes,
            cols,
            rows,
            0,
            CColor::Rgb(0x1e, 0x1e, 0x2e),
            CColor::Rgb(0x74, 0xc7, 0xec),
            Some(CColor::Rgb(0x6c, 0x70, 0x86)),
            &crate::config::Minimap::default(),
        );
        let y = 2usize;
        (0..cols)
            .filter(|x| out[y * cols as usize + *x as usize].ch == '│')
            .collect()
    };
    // Walk focus right across the whole strip, painting at every stop, then
    // back left (reverse stops can differ from forward ones).
    let mut boundary_scroll = 0;
    boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
    for _ in 0..n - 1 {
        boundary_scroll = layout.apply(Action::FocusRight, vp, f).unwrap();
        boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
    }
    for _ in 0..n - 1 {
        boundary_scroll = layout.apply(Action::FocusLeft, vp, f).unwrap();
        boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
    }
    // Every painted frame shows the same vertical-edge skeleton: the grid
    // never shifts by a cell between scroll states.
    let first = &boundary_sets[0].1;
    assert!(
        !first.is_empty(),
        "skeleton frame painted no vertical edges"
    );
    for (scroll, set) in &boundary_sets {
        assert_eq!(
            set, first,
            "grid boundaries moved at scroll={scroll}: {set:?} != {first:?}"
        );
    }
    // Distinct scroll stops were actually exercised (not one static frame).
    let stops: std::collections::HashSet<i32> = boundary_sets.iter().map(|(s, _)| *s).collect();
    assert!(
        stops.len() >= 4,
        "expected several scroll stops, got {stops:?}"
    );
    for p in panes.values_mut() {
        let _ = p.child.kill();
    }
}
