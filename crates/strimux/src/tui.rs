//! TUI: the M0 render/event loop (single process, one focused row).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::execute;
use portable_pty::{
    native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize,
};
use strimux_layout::{Action, FollowScroll, Layout, PaneId, Viewport};
use strimux_term::{CColor, Cell, Size as GridSize, Style, TermGrid, Vt100Grid};

use crate::config::Config;

/// Bottom status/minimap chrome lines.
const CHROME_ROWS: u16 = 1;

/// A PTY-backed pane: its emulator grid plus the I/O handles.
pub struct PtyPane {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn PtyChild + Send + Sync>,
    pub grid: Vt100Grid,
    pub alive: bool,
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
    let pair = pty
        .openpty(size)
        .map_err(|e| format!("openpty: {e}"))?;
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
    let child = slave
        .spawn_command(cb)
        .map_err(|e| format!("spawn: {e}"))?;
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

    master
        .resize(size)
        .map_err(|e| format!("resize: {e}"))?;

    Ok(PtyPane {
        master,
        writer,
        child,
        grid: Vt100Grid::new(GridSize { cols: gw, rows: gh }),
        alive: true,
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
    rect: Rect,          // screen rect (already clipped to viewport horizontally)
    grid_x0: u16,        // grid column at the left edge of `rect`
    grid_cols: u16,      // full logical width of the grid
    grid_rows: u16,      // vertical size of the grid
}

/// Compute visible pane views for the focused row.
fn focused_pane_views(layout: &Layout, cols: u16, rows: u16) -> Vec<PaneView> {
    let strip_h = rows.saturating_sub(CHROME_ROWS).max(1);
    let scroll = layout.focused_row().map(|r| r.scroll_x).unwrap_or(0);
    let ranges = layout
        .column_x_ranges(layout.focus.row, cols)
        .unwrap_or_default();
    let mut out = Vec::new();
    for (ci, (s, e)) in ranges.into_iter().enumerate() {
        let sx = s as i32 - scroll;
        let ex = e as i32 - scroll;
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
        let full_w = (e - s) as u16;
        let grid_x0 = (left as i32 - sx).max(0) as u16; // grid col at `left`
        let p = col.panes.len().max(1);
        let gap = 1u16;
        let pane_h = ((strip_h as i32 - (p as i32 - 1) * gap as i32) / p as i32).max(1) as u16;
        for (pi, pid) in col.panes.iter().enumerate() {
            let y = (pi as u16) * (pane_h + gap);
            let h = pane_h.min(strip_h.saturating_sub(y));
            if h == 0 {
                continue;
            }
            out.push(PaneView {
                pid: *pid,
                rect: Rect { x: left, y, w: wv, h },
                grid_x0,
                grid_cols: full_w,
                grid_rows: h,
            });
        }
    }
    out
}

/// Build the full frame (cols x rows) including the bottom status line.
fn render_frame(
    out: &mut Vec<Cell>,
    layout: &Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cols: u16,
    rows: u16,
) {
    out.clear();
    out.resize((cols as usize) * (rows as usize), Cell::default());

    for v in focused_pane_views(layout, cols, rows) {
        let Some(pane) = panes.get_mut(&v.pid) else { continue };
        // Keep the emulator at its full logical width; only reflow vertically.
        pane.grid.resize(GridSize { cols: v.grid_cols, rows: v.grid_rows });
        for gy in 0..v.rect.h {
            for gx in 0..v.rect.w {
                let gi = v.grid_x0 as usize + gx as usize;
                if gi >= v.grid_cols as usize {
                    continue;
                }
                let idx = ((v.rect.y as usize + gy as usize) * cols as usize)
                    + (v.rect.x as usize + gx as usize);
                if idx >= out.len() {
                    continue;
                }
                out[idx] = pane.grid.cell(gi as u16, gy);
            }
        }
    }

    // Bottom status/minimap line.
    let base = (rows as usize - 1) * cols as usize;
    let style = Style {
        fg: CColor::Idx(15),
        bg: CColor::Idx(24),
        bold: false,
        underline: false,
        inverse: false,
    };
    let scroll = layout.focused_row().map(|r| r.scroll_x).unwrap_or(0);
    let mut st = format!(
        " strimux | row:{} | scroll:{} | focus: col {} pane {} | cols {} | Alt+hjkl nav, Alt+Enter new, Alt+x kill, Alt+q quit",
        layout.focused_row().map(|r| r.name.as_str()).unwrap_or("?"),
        scroll,
        layout.focus.column,
        layout.focus.pane,
        layout.focused_row().map(|r| r.columns.len()).unwrap_or(0),
    );
    st = st.chars().take(cols as usize).collect();
    for (ci, c) in st.chars().enumerate() {
        out[base + ci] = Cell { ch: c, style };
    }
    for i in st.chars().count()..(cols as usize) {
        out[base + i] = Cell { ch: ' ', style };
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
fn paint(
    buf: &mut Vec<u8>,
    out: &[Cell],
    last: &[Cell],
    cols: u16,
    rows: u16,
) -> bool {
    use crossterm::style::{Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
    use crossterm::queue;
    let cc = cols as usize;
    let mut dirty = false;
    for y in 0..rows as usize {
        let row_eq = last.get(y * cc..(y + 1) * cc) == Some(&out[y * cc..(y + 1) * cc]);
        if row_eq {
            continue;
        }
        dirty = true;
        let _ = queue!(buf, cursor::MoveTo(0, y as u16), SetAttribute(Attribute::Reset));
        // Group cells into style runs and print each run.
        let mut x = 0usize;
        while x < cc {
            let cell = out[y * cc + x];
            let style = cell.style;
            let mut run = String::new();
            run.push(cell.ch);
            let mut end = x + 1;
            while end < cc && out[y * cc + end].style == style {
                run.push(out[y * cc + end].ch);
                end += 1;
            }
            let _ = queue!(
                buf,
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
        use crossterm::terminal::Clear;
        use crossterm::terminal::ClearType;
        let _ = queue!(buf, Clear(ClearType::UntilNewLine));
    }
    dirty
}

/// A decoded keyboard instruction.
enum Cmd {
    Act(Action),
    Scroll(i32),
    Input(Vec<u8>),
    Quit,
    None,
}

/// Encode a key event that is not a strimux chord into PTY bytes.
fn key_bytes(ev: &KeyEvent) -> Vec<u8> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let mut out = Vec::new();
    match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lc = c.to_ascii_lowercase();
                if lc.is_ascii_lowercase() {
                    out.push(lc as u8 - b'a' + 1);
                } else {
                    out.extend_from_slice(&[b'^', c as u8, b'\n']);
                }
            } else if alt {
                out.extend_from_slice(&[0x1b]);
                let mut s = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut s).as_bytes());
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
        _ => {}
    }
    out
}

/// Map a key event to a command. Returns None when it is a pass-through.
fn handle_key(ev: &KeyEvent) -> Option<Cmd> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    use KeyCode::*;
    if !alt {
        // Escape must be a chord preamble only; forward everything else.
        if ev.code == Esc {
            return Some(Cmd::None);
        }
        return Some(Cmd::Input(key_bytes(ev)));
    }
    // Alt chords.
    let cmd = match ev.code {
        Char('h') if shift => Action::MovePaneLeft,
        Char('l') if shift => Action::MovePaneRight,
        Char('k') if shift => Action::MovePaneUp,
        Char('j') if shift => Action::MovePaneDown,
        Char('h') => Action::FocusLeft,
        Char('l') => Action::FocusRight,
        Char('k') => Action::FocusUp,
        Char('j') => Action::FocusDown,
        Enter if shift => Action::NewRow,
        Enter => Action::NewColumn,
        Char('a') => Action::NewColumn,
        Char('s') => Action::SplitBelow,
        Char('r') => Action::CycleWidth,
        Char('x') => Action::KillPane,
        Char('z') => Action::CycleWidth,
        Char(c) if c.is_ascii_digit() => {
            Action::JumpToColumn(c.to_digit(10).unwrap_or(1) as usize - 1)
        }
        Char('q') => return Some(Cmd::Quit),
        Char('[') => return Some(Cmd::Scroll(-200)),
        Char(']') => return Some(Cmd::Scroll(200)),
        _ => return Some(Cmd::Input(key_bytes(ev))),
    };
    Some(Cmd::Act(cmd))
}

/// Kill any pane whose id is no longer in the layout, and spawn missing ones.
fn sync_panes(
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    _cfg: &Config,
    tx: &Sender<PaneMsg>,
    _first_id: PaneId,
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
    // Spawn missing panes.
    for pid in wanted {
        if panes.contains_key(&pid) {
            continue;
        }
        let cmd = String::new();
        let pane = spawn_pane(pid, &cmd, 80, 24, tx.clone())?;
        panes.insert(pid, pane);
        tracing::debug!(pid, "spawned pane");
    }
    Ok(())
}



/// Run the interactive TUI.
pub fn run_tui(command: Option<String>, cfg: Config) -> Result<(), i32> {
    use std::io;
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| { eprintln!("raw mode: {e}"); 1 })?;
    if let Err(e) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
        eprintln!("enter alt screen: {e}");
        let _ = disable_raw_mode();
        return Err(1);
    }
    let (cols, mut rows) = term_size().map_err(|e| { eprintln!("size: {e}"); 1 })?;
    let mut cols = cols.max(1);
    rows = rows.max(2);
    let mut layout = Layout::default();
    let first_id = focused_pane(&layout).ok_or_else(|| { eprintln!("no focus"); 1 })?;
    let (tx, rx) = channel::<PaneMsg>();
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    let initial = command.clone().unwrap_or_default();
    match spawn_pane(first_id, &initial, cols.max(1), rows.saturating_sub(CHROME_ROWS).max(1), tx.clone()) {
        Ok(p) => { panes.insert(first_id, p); }
        Err(e) => {
            eprintln!("spawn: {e}");
            let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
            let _ = disable_raw_mode();
            return Err(1);
        }
    }
    let mut frame: Vec<Cell> = Vec::new();
    let mut last: Vec<Cell> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut dirty = true;

    'main: loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                PaneMsg::Output(pid, bytes) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.grid.feed(&bytes);
                        dirty = true;
                    }
                }
                PaneMsg::Exited(pid) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.alive = false;
                    }
                    dirty = true;
                }
            }
        }

        if event::poll(Duration::from_millis(10)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Press => {
                    if let Some(cmd) = handle_key(&ke) {
                        match cmd {
                            Cmd::Quit => break 'main,
                            Cmd::Scroll(d) => {
                                let v = Viewport::new(cols);
                                let _ = layout.apply(Action::ScrollViewport(d), v, FollowScroll::default());
                                dirty = true;
                            }
                            Cmd::Act(a) => {
                                let v = Viewport::new(cols);
                                let f = FollowScroll { margin: cfg.scroll_margin, center: cfg.center_focus };
                                let _ = layout.apply(a, v, f);
                                if let Err(e) = sync_panes(&mut layout, &mut panes, &cfg, &tx, first_id) {
                                    tracing::error!("sync panes: {e}");
                                }
                                dirty = true;
                            }
                            Cmd::Input(bytes) => {
                                if let Some(pid) = focused_pane(&layout) {
                                    if let Some(p) = panes.get_mut(&pid) {
                                        let _ = p.writer.write_all(&bytes);
                                        let _ = p.writer.flush();
                                    }
                                }
                            }
                            Cmd::None => {}
                        }
                    }
                }
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Release => {}
                Ok(Event::Resize(c, r)) => {
                    cols = c.max(1);
                    rows = r.max(2);
                    dirty = true;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("event read: {e}");
                }
            }
        }

        // Resize grids & PTYs to match current geometry.
        for v in focused_pane_views(&layout, cols, rows) {
            let pid = v.pid;
            if let Some(p) = panes.get_mut(&pid) {
                if p.grid.size() != (GridSize { cols: v.grid_cols, rows: v.grid_rows }) {
                    p.grid.resize(GridSize { cols: v.grid_cols, rows: v.grid_rows });
                    dirty = true;
                }
                let _ = p.master.resize(PtySize {
                    rows: v.grid_rows,
                    cols: v.grid_cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }

        if dirty {
            render_frame(&mut frame, &layout, &mut panes, cols, rows);
            buf.clear();
            paint(&mut buf, &frame, &last, cols, rows);
            if !buf.is_empty() {
                let _ = stdout.write_all(&buf);
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
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
    Ok(())
}

/// The currently focused pane id.
fn focused_pane(layout: &Layout) -> Option<PaneId> {
    layout
        .focused_row()
        .and_then(|r| r.columns.get(layout.focus.column))
        .and_then(|c| c.panes.get(layout.focus.pane))
        .copied()
}
