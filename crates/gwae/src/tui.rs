//! TUI: the M0 render/event loop (single process, one focused row).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant};

use crate::theme::Palette;
use crossterm::cursor;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, KeyboardEnhancementFlags,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use gwae_layout::{Action, FollowScroll, Layout, PaneId, PaneStatus, Viewport, Width};
use gwae_term::{CColor, Cell, KittyApcExtractor, Size as GridSize, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};

use crate::config::Config;
use crate::select::{self, Selection};

/// What a mouse event inside a pane should do.
///
/// Mouse capture is what gives gwae click-to-focus and drag-to-copy, which
/// it takes away from the host terminal. These are the three ways an event can
/// be resolved, in the order a terminal user expects:
///  - the child asked for mouse reporting, so it owns the event (vim, an agent
///    TUI) - unless Shift is held, the long-standing xterm convention for
///    "give me the multiplexer's selection instead";
///  - otherwise a left press/drag/release drives our own drag-to-copy;
///  - anything else is handled locally, or not at all. gwae claims no wheel
///    of its own: scrollback moves with `⌥+↑/↓` (see `handle_key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseRole {
    /// Forward verbatim to the child as an SGR mouse report.
    Forward,
    /// Drive gwae's own drag-to-copy selection.
    Select,
    /// Handled locally, or ignored. gwae has no wheel behavior of its own.
    Local,
}

/// Decide what a mouse event does inside the pane under the cursor.
fn mouse_role(kind: MouseEventKind, modifiers: KeyModifiers, child_wants_mouse: bool) -> MouseRole {
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let selecting = matches!(
        kind,
        MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::Drag(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Left)
    );
    if child_wants_mouse && !(shift && selecting) {
        return MouseRole::Forward;
    }
    if selecting {
        MouseRole::Select
    } else {
        MouseRole::Local
    }
}

fn chrome_rows(_cfg: &Config) -> u16 {
    // Bottom status row has been removed; chrome is always 0.
    0
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

/// How a pane's PTY is owned.
///
/// A pane gwae spawned itself owns a `portable_pty` master. A pane *adopted*
/// across a hot reload owns only a raw file descriptor: the master survived
/// the `execve`, but `portable_pty`'s `UnixMasterPty` is a private type that
/// cannot be rebuilt from a fd, so there is nothing to reconstruct.
///
/// That turns out not to matter. gwae asks a master for exactly two things,
/// resize and a writer, and both are plain `ioctl`/`write` on the fd. This
/// enum is the whole cost of supporting adopted panes.
pub enum PaneIo {
    /// Spawned by this image of gwae.
    Owned(Box<dyn MasterPty + Send>),
    /// Inherited from the previous image across a hot reload.
    #[cfg(unix)]
    Inherited(std::os::fd::RawFd),
}

impl PaneIo {
    /// Tell the kernel the pane's new logical size, so the child re-lays out.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            PaneIo::Owned(m) => m
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string()),
            #[cfg(unix)]
            PaneIo::Inherited(fd) => {
                let ws = libc::winsize {
                    ws_row: rows,
                    ws_col: cols,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                // Safety: TIOCSWINSZ on a PTY master fd this process owns.
                let rc = unsafe { libc::ioctl(*fd, libc::TIOCSWINSZ, &ws) };
                if rc == -1 {
                    Err(std::io::Error::last_os_error().to_string())
                } else {
                    Ok(())
                }
            }
        }
    }

    /// The raw fd, when there is one. Used to hand the pane to the next image
    /// of gwae during a reload.
    #[cfg(unix)]
    pub fn raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd;
        match self {
            PaneIo::Owned(m) => m.as_raw_fd().map(|f| f.as_raw_fd()),
            PaneIo::Inherited(fd) => Some(*fd),
        }
    }
}

/// A pane's child process, which after a reload is a pid we inherited rather
/// than a `Child` we can `wait` on.
///
/// The distinction matters for teardown: `kill_pane_tree` signals a process
/// *group* and walks `ps` output, both of which need only a pid. Reaping an
/// adopted pane is therefore identical to reaping a spawned one, which is the
/// property that keeps the no-leaked-processes guarantee true across reloads.
pub enum PaneProc {
    /// Spawned by this image; we are its parent and can reap it.
    Owned(Box<dyn PtyChild + Send + Sync>),
    /// Inherited across a reload. Same pid, same process group, but this
    /// image never called `fork`, so there is no `Child` to wait on. (The pid
    /// is still ours: `execve` preserves the process, so the children were
    /// never reparented.)
    Adopted(Option<u32>),
}

impl PaneProc {
    pub fn process_id(&self) -> Option<u32> {
        match self {
            PaneProc::Owned(c) => c.process_id(),
            PaneProc::Adopted(pid) => *pid,
        }
    }

    /// Best-effort direct kill, complementing the group signal and tree walk
    /// in [`kill_pane_tree`].
    pub fn kill(&mut self) {
        match self {
            PaneProc::Owned(c) => {
                let _ = c.kill();
            }
            #[cfg(unix)]
            PaneProc::Adopted(Some(pid)) => {
                // Safety: `kill(2)` on a pid we are the parent of; ESRCH when
                // it has already exited, which is ignored.
                unsafe {
                    libc::kill(*pid as libc::pid_t, libc::SIGKILL);
                }
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
}

/// A PTY-backed pane: its emulator grid plus the I/O handles.
pub struct PtyPane {
    pub master: PaneIo,
    pub writer: Box<dyn Write + Send>,
    pub child: PaneProc,
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

/// Whether the terminal gwae itself runs in understands Kitty graphics.
///
/// Env-based, mirroring how jcode and ratatui-image decide: Kitty exports
/// `KITTY_WINDOW_ID`, Kitty-protocol terminals (Ghostty, WezTerm's kitty mode)
/// advertise via TERM/TERM_PROGRAM. `GWAE_KITTY_GRAPHICS=1/0` overrides
/// detection either way (e.g. gwae inside ssh where env vars were dropped).
fn host_supports_kitty_graphics() -> bool {
    if let Ok(v) = std::env::var("GWAE_KITTY_GRAPHICS") {
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
/// gwae effectively transparent to the host's title/status bar: the outer
/// window shows e.g. a jcode session title instead of "gwae".
fn emit_title(stdout: &mut impl Write, title: &str) -> std::io::Result<()> {
    write!(stdout, "\x1b]2;{}\x1b\\", sanitize_title(title))?;
    stdout.flush()
}

/// The command an agent pane runs: this very binary's `agent` subcommand.
///
/// `current_exe` rather than a bare `gwae`, so a binary that is not on
/// `PATH` (a `cargo run` build, or an install into a directory the shell does
/// not know about) still spawns *itself* rather than some other gwae, or
/// nothing at all.
fn agent_gateway_cmd() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "gwae".to_string());
    // Quoted so an install path containing spaces survives `shell_split`.
    format!("\"{exe}\" agent")
}

/// Persist the picked spawn directory as `agent_dir` in the config file.
///
/// Reuses the agent gateway's comment-preserving rewrite, so saving a
/// directory from the picker cannot reformat a hand-written config or drop
/// its comments the way a parse/serialize round trip would.
fn write_agent_dir(path: &std::path::Path, dir: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let out =
        crate::agent::set_scalar_text(&text, "agent_dir", &crate::agent::toml_string_pub(dir));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// Every descendant of `root`, deepest first, as reported by `ps`.
///
/// Returned deepest-first so a caller can signal children before their
/// parents: killing a parent first can leave a grandchild reparented to init
/// and unreachable by the time we get to it.
#[cfg(unix)]
pub(crate) fn descendants(root: u32) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if let (Some(Ok(pid)), Some(Ok(ppid))) = (
            it.next().map(str::parse::<u32>),
            it.next().map(str::parse::<u32>),
        ) {
            children.entry(ppid).or_default().push(pid);
        }
    }
    // Breadth-first from the root, then reverse: parents are discovered
    // before their children, so reversing yields deepest-first.
    let mut order = Vec::new();
    let mut queue = vec![root];
    while let Some(p) = queue.pop() {
        for &c in children.get(&p).into_iter().flatten() {
            // `ps` is a snapshot of a tree that cannot contain cycles, but
            // guard anyway: a PID that somehow repeats must not spin here.
            if c != root && !order.contains(&c) {
                order.push(c);
                queue.push(c);
            }
        }
    }
    order.reverse();
    order
}

/// Terminate a pane and everything it left running.
///
/// `Child::kill` signals only the process gwae spawned. That is enough for
/// the common cases (the shell's own foreground and background jobs die with
/// it, because they share its process group and get the hangup), but it is
/// *not* enough for a job that deliberately escaped: `nohup cmd &`, a daemon
/// that called `setsid`, or anything else sitting in its own process group.
/// Those survived a force-quit and kept running invisibly after the window
/// they belonged to was gone, which is exactly the "I quit gwae, why is this
/// still running" case. Quitting is documented as terminating everything in
/// the panes, so walk the real process tree and signal each descendant too.
///
/// SIGKILL, not SIGTERM: this path is only ever reached from an explicit,
/// already-confirmed teardown (force quit, or closing a pane), where the
/// user has said to stop things now and a process that ignores SIGTERM
/// would otherwise leak exactly as before.
fn kill_pane_tree(child: &mut PaneProc) {
    #[cfg(unix)]
    {
        // Collect descendants *before* killing the root: once the root is
        // gone its children are reparented to init and the link that
        // identifies them as ours is lost.
        let root = child.process_id();
        let kids = root.map(descendants).unwrap_or_default();
        if let Some(p) = root {
            // The pane's own jobs live in its process group (it is a session
            // leader on a PTY). Signal the group first: that reaches jobs the
            // `ps` snapshot could miss because they were reparented between
            // the walk and the kill.
            unsafe {
                libc::kill(-(p as libc::pid_t), libc::SIGKILL);
            }
            // This pane is being torn down deliberately, so the exit-time
            // reaper must not try again on a pid the OS may have recycled.
            crate::reap::unregister(p);
        }
        child.kill();
        for pid in kids {
            // Safety: `kill(2)` with a pid we just read from `ps`. A pid that
            // has already exited returns ESRCH, which is ignored.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// Spawn a PTY running `cmd` at the given grid size, wiring a reader thread.
fn spawn_pane(
    id: PaneId,
    cmd: &str,
    gw: u16,
    gh: u16,
    tx: Sender<PaneMsg>,
    cwd: Option<&std::path::Path>,
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
    // The spawn directory (config `agent_dir`, `--dir`, or the `⌥+d`
    // picker). `None` inherits gwae's own cwd, which is the pre-feature
    // behavior. Set before spawn only: a pane's cwd is the child's business
    // afterwards.
    if let Some(dir) = cwd {
        cb.cwd(dir);
    }
    cb.env("GWAE_PANE", id.to_string());
    cb.env("TERM", "xterm-256color");
    let child = slave.spawn_command(cb).map_err(|e| format!("spawn: {e}"))?;
    drop(slave);
    // Register before anything else can fail: from here on, however gwae
    // dies (signal, panic, early return), this pane and its jobs are killed.
    if let Some(p) = child.process_id() {
        crate::reap::register(p);
    }

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
        master: PaneIo::Owned(master),
        writer,
        child: PaneProc::Owned(child),
        grid: Vt100Grid::new(GridSize { cols: gw, rows: gh }),
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        apc: KittyApcExtractor::new(),
    })
}

/// Naive shell splitter: split on whitespace, keeping simple quoting (\"..\").
pub fn shell_split(cmd: &str) -> Vec<String> {
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

/// Rebuild a pane around a PTY master fd inherited from the previous image of
/// gwae across a hot reload.
///
/// The child is untouched: it was never signalled, never reparented, and does
/// not know a reload happened. All that is rebuilt here is gwae's own side —
/// a reader thread, a writer, and an empty grid of the right shape.
///
/// The grid starts blank because the previous image's grid contents were not
/// carried across (they are large, and versioning them across two builds that
/// are by definition different code is a bad trade). The pane repaints as
/// soon as its child writes anything; [`nudge_repaint`] asks it to do so
/// immediately.
#[cfg(unix)]
fn adopt_pane(
    id: PaneId,
    fd: std::os::fd::RawFd,
    pid: Option<u32>,
    cols: u16,
    rows: u16,
    tx: Sender<PaneMsg>,
) -> Result<PtyPane, String> {
    use std::os::fd::FromRawFd;

    // Re-register with the reaper first. Signal handlers do *not* survive
    // execve, so until `reap::install()` runs and these pids are back in the
    // registry, a SIGTERM would leave every pane's background jobs running.
    // This ordering is the whole reason adoption is not just "make a struct".
    if let Some(p) = pid {
        crate::reap::register(p);
    }

    // Two independent handles on the same PTY: one for the blocking reader
    // thread, one for writes from the main loop. `dup` rather than sharing,
    // so closing one does not hang up the other.
    // Safety: `fd` was inherited across execve and named in the handover; it
    // is a live PTY master this process owns.
    let read_fd = unsafe { libc::dup(fd) };
    if read_fd == -1 {
        return Err(format!("dup pane fd: {}", std::io::Error::last_os_error()));
    }
    // Safety: `read_fd` is a fresh descriptor owned solely by this File.
    let reader_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let write_fd = unsafe { libc::dup(fd) };
    if write_fd == -1 {
        return Err(format!("dup pane fd: {}", std::io::Error::last_os_error()));
    }
    // Safety: as above, a fresh descriptor with a single owner.
    let writer_file = unsafe { std::fs::File::from_raw_fd(write_fd) };

    let tid = id;
    let mut reader = reader_file;
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

    Ok(PtyPane {
        master: PaneIo::Inherited(fd),
        writer: Box::new(writer_file),
        child: PaneProc::Adopted(pid),
        grid: Vt100Grid::new(GridSize { cols, rows }),
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        apc: KittyApcExtractor::new(),
    })
}

/// Ask every adopted pane's child to repaint, so a reloaded screen is not
/// blank until the user types.
///
/// `Ctrl-L` is the closest thing to a universal "redraw" a terminal program
/// understands: shells redraw their prompt, and full-screen apps (vim, agent
/// TUIs) repaint their whole surface. Sending it costs nothing when the child
/// ignores it.
fn nudge_repaint(panes: &mut HashMap<PaneId, PtyPane>) {
    for p in panes.values_mut() {
        let _ = p.writer.write_all(b"\x0c");
        let _ = p.writer.flush();
    }
}

/// Collect the state the next image of gwae needs, then replace this process
/// with it. Returns only on failure, in which case this image carries on.
#[cfg(unix)]
fn perform_reload(
    layout: &Layout,
    panes: &HashMap<PaneId, PtyPane>,
    agent_panes: &HashSet<PaneId>,
    spawn_dir: Option<&std::path::Path>,
) -> Result<std::convert::Infallible, String> {
    let exe = crate::reload::own_path()?;
    let mut handover_panes = Vec::new();
    for (pid, pane) in panes {
        let Some(fd) = pane.master.raw_fd() else {
            return Err(format!("pane {pid} has no fd to hand over"));
        };
        let (cols, rows) = {
            let sz = pane.grid.size();
            (sz.cols, sz.rows)
        };
        handover_panes.push(crate::reload::PaneHandover {
            id: *pid,
            fd,
            pid: pane.child.process_id(),
            cols,
            rows,
            is_agent: agent_panes.contains(pid),
        });
    }
    // Stable order so the new image rebuilds panes deterministically.
    handover_panes.sort_by_key(|p| p.id);
    let handover = crate::reload::Handover {
        layout: layout.clone(),
        panes: handover_panes,
        spawn_dir: spawn_dir.map(|p| p.to_path_buf()),
        from: exe.clone(),
    };
    crate::reload::exec_into(&exe, &handover)
}

/// Hand the terminal back to the host: leave the alt screen, drop raw mode,
/// and undo every mode gwae turned on.
///
/// Shared by the normal exit path and by hot reload. Terminal modes are
/// kernel tty state, so they survive an `execve`: a reload that skipped this
/// would leave the new image in raw mode on an alt screen it never entered,
/// which looks exactly like a hung terminal.
fn restore_terminal(stdout: &mut std::io::Stdout, kitty_keyboard: bool) {
    if kitty_keyboard {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = stdout.write_all(b"\x1b[?7h");
    let _ = stdout.flush();
    let _ = execute!(stdout, DisableBracketedPaste);
    let _ = execute!(stdout, DisableMouseCapture);
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

/// Re-acquire the terminal after [`restore_terminal`], for the one case where
/// a reload was attempted and the `execve` failed: this image is still alive
/// and still owns every pane, so it has to take the screen back rather than
/// exit and take the panes with it.
fn re_enter_terminal(stdout: &mut std::io::Stdout) -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    execute!(stdout, EnterAlternateScreen, cursor::Hide).map_err(|e| format!("alt screen: {e}"))?;
    let _ = stdout.write_all(b"\x1b[?7l");
    let _ = stdout.flush();
    let _ = execute!(stdout, EnableMouseCapture);
    Ok(())
}

/// One visible pane on screen: where to draw it and which grid slice to show.
struct PaneView {
    pid: PaneId,
    col: usize,     // index of the owning column in the focused strip
    rect: Rect,     // screen rect (already clipped to viewport horizontally)
    col_x0: u16,    // grid column at the left edge of `rect` (before content scroll)
    h_scroll: i32,  // pane content scroll in cells
    grid_cols: u16, // full logical content width of the grid
    grid_rows: u16, // vertical size of the grid
}

/// Compute visible pane views for the focused row.
///
/// With `inset` (always true in the renderer) every pane's rect is shrunk by
/// 1 cell on all sides of its column box so content sits *inside* the frame
/// instead of being overlaid by it: nothing a program draws is ever covered.
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
        // Split the inner height across the stack *exactly*: floor division
        // alone strands `avail % p` rows at the bottom of the column, which
        // paint as unassigned background (visible from 7 panes down on a
        // typical strip). Hand the remainder out one row at a time to the
        // top panes so the stack always tiles the full strip.
        let avail = (inner_h as i32 - (p as i32 - 1) * gap as i32).max(0);
        let base = avail / p as i32;
        let rem = avail % p as i32;
        let mut y = inner_top;
        for (pi, pid) in col.panes.iter().enumerate() {
            let want = (base + ((pi as i32) < rem) as i32).max(0) as u16;
            let row_y = y;
            y = y.saturating_add(want).saturating_add(gap);
            let h = want.min(inner_bottom.saturating_sub(row_y));
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
    pal: &Palette,
    mm: &crate::config::Minimap,
    cow: &crate::config::Cowsay,
    cell_labels: bool,
    selection: Option<&Selection<PaneId>>,
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
        if is_focus {
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
                // Highlight a drag selection by inverting the cell, the same
                // affordance a terminal's own selection uses. Inversion (not a
                // fixed background) keeps every glyph readable whatever colors
                // the program inside the pane is using.
                if selection
                    .map(|s| s.contains(v.pid, gi, gy))
                    .unwrap_or(false)
                {
                    cell.style.inverse = !cell.style.inverse;
                }
                out[idx] = cell;
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
    // Placeholder boxes tile the empty right side: an empty grid must show
    // where the next pane will go, and (with `cowsay`) advertise the key that
    // puts one there.
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
        // same on-screen boundaries at every scroll stop. Placeholder boxes
        // for the empty right side come from the *same* accumulator, so cell
        // `1.2` is the identical span of screen columns whether it holds a
        // PTY or not and the grid never jitters as strips fill or as you
        // move between strips with different occupancy.
        let strip_no = strip_number(layout);
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
            let right = (ex.min(cols as i32 - 1)) as u16;
            if right <= left {
                continue;
            }
            let placeholder = ci >= live;
            let (color, prio) = if ci == layout.focus.column && (!focused_col_split || placeholder)
            {
                (focus_color, P_FOCUS_COL)
            } else {
                (sk, P_CHROME)
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
                // shared edge. Reset to the default (pane) background so an
                // empty box reads exactly like a live one.
                for y in inset..boxr.h.saturating_sub(inset) {
                    let row = (boxr.y + y) as usize * cols as usize;
                    for x in inset..boxr.w.saturating_sub(inset) {
                        if let Some(c) = out.get_mut(row + (boxr.x + x) as usize) {
                            *c = Cell::default();
                            // Keep the themed backdrop: a placeholder box is
                            // empty chrome, not a pane, so its interior must
                            // blend with `theme.base` rather than punching a
                            // hole of the terminal's own background through it.
                            c.style.bg = background;
                        }
                    }
                }
                canvas.rect(boxr, color, prio);
                let inner = Rect {
                    x: boxr.x + inset,
                    y: boxr.y + inset,
                    w: boxr.w.saturating_sub(inset * 2),
                    h: boxr.h.saturating_sub(inset * 2),
                };
                draw_placeholder_contents(
                    out,
                    cols,
                    inner,
                    &format!("{}.{}", strip_no, ci + 1),
                    pal.label,
                    cow,
                    // Ordinal among *empty* boxes, not the absolute column,
                    // so the pinned cheat-sheet hint sits where the eye lands
                    // whatever the layout.
                    ci - live,
                    cell_labels,
                );
                continue;
            }
            canvas.rect(boxr, color, prio);
            // Stacked panes: the 1-cell gap between two panes of a column is
            // a shared horizontal rule that tees into the column's verticals,
            // so a stack reads as one subdivided container.
            if let Some(col) = layout.focused_row().and_then(|r| r.columns.get(ci)) {
                if col.panes.len() > 1 {
                    for v in pane_views.iter().filter(|v| v.col == ci).skip(1) {
                        canvas.hline(left as i32, right as i32, v.rect.y as i32 - 1, color, prio);
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
                canvas.rect(Rect { x, y, w, h }, focus_color, P_FOCUS_PANE);
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
fn put_frame_cell(out: &mut [Cell], idx: usize, ch: char, color: CColor) {
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
        }
    }

    fn is_empty(&self) -> bool {
        self.edges.iter().all(|e| e.mask == 0)
    }

    fn add(&mut self, x: i32, y: i32, mask: u8, color: CColor, prio: u8) {
        if x < 0 || y < 0 || x >= self.cols as i32 || y >= self.rows as i32 {
            return;
        }
        let idx = y as usize * self.cols as usize + x as usize;
        let e = &mut self.edges[idx];
        e.mask |= mask;
        // Highest priority wins the color; ties keep the first writer so a
        // repaint of the same box is idempotent.
        if prio > e.prio {
            e.prio = prio;
            self.colors[idx] = color;
        }
    }

    /// A vertical rule from `y0` to `y1` (inclusive) at column `x`.
    fn vline(&mut self, x: i32, y0: i32, y1: i32, color: CColor, prio: u8) {
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
            self.add(x, y, m, color, prio);
        }
    }

    /// A horizontal rule from `x0` to `x1` (inclusive) at row `y`.
    fn hline(&mut self, x0: i32, x1: i32, y: i32, color: CColor, prio: u8) {
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
            self.add(x, y, m, color, prio);
        }
    }

    /// The ring of `rect`, as four rules that join at the corners.
    fn rect(&mut self, rect: Rect, color: CColor, prio: u8) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let x0 = rect.x as i32;
        let y0 = rect.y as i32;
        let x1 = x0 + rect.w as i32 - 1;
        let y1 = y0 + rect.h as i32 - 1;
        if y1 > y0 {
            self.vline(x0, y0, y1, color, prio);
            if x1 > x0 {
                self.vline(x1, y0, y1, color, prio);
            }
        }
        if x1 > x0 {
            self.hline(x0, x1, y0, color, prio);
            if y1 > y0 {
                self.hline(x0, x1, y1, color, prio);
            }
        }
        if x1 == x0 && y1 == y0 {
            self.add(x0, y0, EDGE_E | EDGE_W, color, prio);
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
                put_frame_cell(out, idx, ch, self.colors[idx]);
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
    let put = put_frame_cell;
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

/// Fill the interior of an empty placeholder box: the big block-font cell
/// identifier, and (room permitting) a cowsay hint under it.
///
/// The identifier is the box's *addressing* affordance and always wins: the
/// cow is only drawn when it fits underneath without crowding the label, so
/// narrow or short boxes silently degrade to the label alone rather than to a
/// clipped mess. Vertically the pair is centered as a unit, so the box doesn't
/// look top-heavy.
#[allow(clippy::too_many_arguments)]
fn draw_placeholder_contents(
    out: &mut [Cell],
    cols: u16,
    rect: Rect,
    label: &str,
    color: CColor,
    cow: &crate::config::Cowsay,
    // Position among the strip's empty boxes; `0` is pinned to the hint.
    cow_ordinal: usize,
    cell_labels: bool,
) {
    let art = if cow.enabled {
        crate::cowsay::message_for(&cow.messages, cow_ordinal, cow_ordinal == 0)
            .map(|m| crate::cowsay::cow_frame(m, rect.w.min(40)))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // 5 rows of block font, then a blank spacer row, then the art.
    const LABEL_H_FULL: u16 = 5;
    let label_h: u16 = if cell_labels { LABEL_H_FULL } else { 0 };
    let art_h = art.len() as u16;
    let fits = !art.is_empty() && rect.h >= label_h + 1 + art_h;
    if !fits {
        if cell_labels {
            draw_big_label(out, cols, rect, label, color);
        }
        return;
    }
    let total = label_h + 1 + art_h;
    let top = rect.y + (rect.h - total) / 2;
    if cell_labels {
        draw_big_label(
            out,
            cols,
            Rect {
                x: rect.x,
                y: top,
                w: rect.w,
                h: label_h,
            },
            label,
            color,
        );
    }
    draw_art(
        out,
        cols,
        Rect {
            x: rect.x,
            y: top + label_h + 1,
            w: rect.w,
            h: art_h,
        },
        &art,
        color,
    );
}

/// Paint a block of pre-wrapped ASCII art centered in `rect`.
///
/// Unlike [`draw_big_label`], which paints background-colored blanks, this
/// writes real glyphs in `color`. The block is centered as a unit (each line
/// keeps its relative indentation, so the cow doesn't shear), and any line
/// that would run past the rect is clipped rather than wrapping into the
/// neighbouring box.
fn draw_art(out: &mut [Cell], cols: u16, rect: Rect, lines: &[String], color: CColor) {
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
                // the themed backdrop instead of stamping a differently
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

fn status_glyph_for(s: PaneStatus) -> char {
    match s {
        PaneStatus::Running => '\u{00bb}', // »
        PaneStatus::Idle => '!',
        PaneStatus::Done => '\u{2713}',   // ✓
        PaneStatus::Failed => '\u{2717}', // ✗
    }
}

/// Everything the ⌥-hold overlay knows that the layout alone cannot tell it:
/// what each pane *is* (its OSC 0/2 title), how long it has been silent, where
/// `⌥+g` would take you, and which column an in-flight `⌥+<number>` is
/// addressing.
///
/// It is a plain data bag built at the call site from the live PTY panes so
/// the drawing code stays a pure function of the frame's facts, and so every
/// decoration can be tested without spawning a pty.
#[derive(Default)]
struct HudFacts {
    /// Short label per pane, already reduced from the raw window title.
    titles: HashMap<PaneId, String>,
    /// How long each pane has been silent (used to age attention tiles).
    quiet: HashMap<PaneId, Duration>,
    /// The pane `⌥+g` would jump to right now, if any.
    jump_target: Option<PaneId>,
    /// The 1-based column an un-committed `⌥+<number>` is pointing at.
    pending_jump: Option<usize>,
}

/// Reduce a window title to something that fits on a minimap tile.
///
/// Shell titles are conventionally `user@host: ~/some/dir`, which is almost
/// entirely chrome at tile widths; agent harnesses set something short and
/// meaningful already. So: drop everything before the last `": "`, then keep
/// the final path segment, and strip control characters that a program could
/// have smuggled into the title.
fn short_title(raw: &str) -> String {
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
fn age_label(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h", (s / 3600).min(99))
    }
}

/// Readable ink for text painted *on* `bg`.
///
/// Tiles used to hardcode `Idx(231)` (near-white), which is invisible on the
/// light themes' status tints. Indexed and default colors carry no components
/// to measure, so they keep the old near-white assumption; RGB tints pick dark
/// ink on a light tile and light ink on a dark one, from the palette itself.
fn contrast_fg(bg: CColor, pal: &Palette) -> CColor {
    let luma = |c: CColor| match c {
        CColor::Rgb(r, g, b) => {
            Some((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0)
        }
        _ => None,
    };
    match luma(bg) {
        Some(l) if l > 0.55 => match luma(pal.base) {
            // The theme's own darkest surface, when it is actually dark.
            Some(bl) if bl < 0.4 => pal.base,
            _ => CColor::Rgb(0x11, 0x11, 0x14),
        },
        Some(_) => CColor::Rgb(0xf5, 0xf5, 0xf5),
        None => CColor::Idx(231),
    }
}

/// Render one minimap tile's `w` cells of text.
///
/// Screen order is fixed - status glyph, address, jump marker, title, age -
/// and the pieces are fitted in priority order as the tile narrows.
///
/// Everything that identifies the tile is packed at its *left* edge, and the
/// spare cells fall at the right. Tiles abut with no separator, so a
/// right-aligned glyph would collide with the next tile's address (`»2 clau…»3`
/// reads as one run); leading with `»2` gives every tile the same unmistakable
/// signature, which is also where the eye goes when scanning a column of them.
///
/// The glyph and address always survive, since they are what makes a tile
/// triageable and addressable. The *title* is dropped before the age: a name
/// cut to one or two characters says nothing, while the age (which only
/// appears on a pane that wants attention at all) is exactly the news you held
/// ⌥ to get. The result is always exactly `w` characters, so the caller can
/// paint it cell-for-cell.
fn tile_text(w: u16, addr: &str, target: bool, title: &str, age: &str, glyph: char) -> String {
    let w = w as usize;
    if w == 0 {
        return String::new();
    }
    if w == 1 {
        // One cell: status beats address. Which pane is *waiting* is worth
        // more than which key jumps to it, and the tile's position in the
        // row still gives the column away.
        return glyph.to_string();
    }
    // The address is the tile's identity; a stacked sub-pane passes "·".
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
    // One cell after the address separates it from the name, and doubles as
    // the smart-jump marker when this is the pane `⌥+g` would take you to.
    let sep = if target { '\u{25b8}' } else { ' ' }; // ▸
    let mut rest = w - alen - 1 /* glyph */ - 1 /* sep */;
    let age: String = if !age.is_empty() && age.chars().count() <= rest {
        rest -= age.chars().count();
        // One cell stays blank between the name and the age, or `deploy` and
        // `50m` run together into `deploy50m` and neither is readable.
        rest = rest.saturating_sub(1);
        age.to_string()
    } else {
        String::new()
    };
    // A name cut below three characters is noise, so those cells stay blank.
    const MIN_TITLE: usize = 3;
    let mut mid = String::new();
    if rest >= MIN_TITLE && !title.is_empty() {
        let n = title.chars().count();
        mid = if n <= rest {
            title.to_string()
        } else {
            // Mark the cut so a truncated name never reads as the whole name.
            title
                .chars()
                .take(rest - 1)
                .chain(std::iter::once('\u{2026}')) // …
                .collect()
        };
    }
    let mut s = String::new();
    s.push(glyph);
    s.push_str(&addr);
    s.push(sep);
    s.push_str(&mid);
    let pad = w.saturating_sub(s.chars().count() + age.chars().count());
    s.extend(std::iter::repeat_n(' ', pad));
    s.push_str(&age);
    // Belt and braces: the caller paints `w` cells, so never return more.
    s.chars().take(w).collect()
}

/// The inclusive column-index range of a strip that is currently on screen,
/// or `None` when the whole strip fits (in which case there is nothing to
/// point out: the viewport *is* the strip).
fn visible_column_range(layout: &Layout, row_idx: usize, cols: u16) -> Option<(usize, usize)> {
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
/// three times: to paint the panel, to scrim everything around it, and to
/// resolve a click on a tile back to a pane. Sharing one plan is what keeps
/// those three from drifting apart, which is exactly how "click focuses the
/// wrong pane" bugs are born.
struct HudPlan {
    /// The panel's screen rect, frame included.
    rect: Rect,
    /// Screen y of each shown strip's tile row.
    row_y: Vec<u16>,
    /// Screen y of each strip's viewport ruler, when that strip overflows.
    ruler_y: Vec<Option<u16>>,
    /// The visible column range of each shown strip, when it overflows.
    rulers: Vec<Option<(usize, usize)>>,
    /// Screen x of map cell 0 (past the frame and the strip gutter).
    map_ox: u16,
    /// Gutter labels, one per strip, and the gutter's width in cells.
    gutter: Vec<String>,
    gutter_w: u16,
    /// The tiles to paint (already limited to the shown strips).
    map: gwae_layout::minimap::Minimap,
    /// Strips cut off the bottom, if any.
    hidden: usize,
    /// Inner width, and the first inner row/column.
    inner_w: usize,
    inner_ox: usize,
    /// Screen y of the tally row and of the key-hint row.
    tally_y: Option<u16>,
    hint_y: u16,
}

/// The status tally shown in the dashboard footer: the pane count, then one
/// `glyph count` segment per status that has any panes. Returned with the
/// status rather than a color so the geometry pass can measure it without a
/// palette.
fn status_tally(layout: &Layout) -> Vec<(String, Option<PaneStatus>)> {
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
fn plan_center_minimap(
    cols: u16,
    rows: u16,
    layout: &Layout,
    mm: &crate::config::Minimap,
) -> Option<HudPlan> {
    use gwae_layout::minimap;
    if !mm.show || cols < 20 || rows < 8 {
        return None;
    }
    // A single pane has no grid to triage, but the hold must still answer
    // *something*: silence taught first-run users that ⌥ does nothing at all.
    // Fall back to the key hints alone.
    let single = layout.panes.len() <= 1 && layout.rows.len() <= 1;

    // Strip gutter: the strip's position, plus its name when it has a real
    // one. `Row.name` has existed since M0 and was never surfaced anywhere.
    let gutter: Vec<String> = layout
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let name = r.name.trim();
            let generic = name.is_empty()
                || name.eq_ignore_ascii_case("row")
                || name.eq_ignore_ascii_case(&format!("strip {}", i + 1));
            if generic {
                format!("{}", i + 1)
            } else {
                format!("{} {}", i + 1, name)
            }
        })
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
    // `minimap.max_width` caps the *corner overlay*, where 32 cells is a
    // deliberately small footprint over live panes. The centered panel is a
    // different thing at a different moment: it owns the screen for as long
    // as ⌥ is held and spends its cells on pane names, so it asks for enough
    // to seat one per tile and treats the configured number as a floor it may
    // exceed. Raising `max_width` still widens it; lowering it will not
    // squeeze names out of a panel that has room for them. Never more than
    // two-thirds of the screen, so the session stays visible around it.
    let want = (WIDTH_PER_TILE * layout_widest_strip(layout) as u16).max(mm.max_width);
    let room = cols.saturating_sub(chrome_w + 4).max(1);
    let width = want
        .min(room)
        .min((cols * 2 / 3).max(mm.max_width.min(room)));
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
    let footer_rows = usize::from(has_summary) + 1; // tallies + key hints

    let map_row_w = (gutter_w + u16::from(gutter_w > 0) + map.width) as usize;
    let tally_w: usize = status_tally(layout)
        .iter()
        .map(|(t, _)| t.chars().count())
        .sum();
    let inner_w = map_row_w
        .max(hud_hint().chars().count())
        .max(if has_summary { tally_w } else { 0 });
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
        hint_y: fy as u16,
    })
}

/// The key-hint line under the dashboard. Spelled from [`crate::keys`] so it
/// reads `⌥` on macOS and `Alt` everywhere else.
fn hud_hint() -> String {
    format!(
        "{n}1-9 col · {n}g attention · {n}hjkl move · {n}/ keys",
        n = crate::keys::mod_key()
    )
}

/// Cells the centered dashboard would like per column, so a tile can seat an
/// address, the `⌥+g` marker, a name worth reading, an age and a status
/// glyph. A target, not a guarantee: narrow terminals get less and
/// [`tile_text`] degrades accordingly.
const WIDTH_PER_TILE: u16 = 12;

/// The most columns any single strip has. Sizing the map by this (rather than
/// by the focused strip) keeps the panel from resizing under the user's eyes
/// as focus moves between strips of different lengths.
fn layout_widest_strip(layout: &Layout) -> usize {
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
fn hud_pane_at(plan: &HudPlan, x: u16, y: u16) -> Option<PaneId> {
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
/// column's real width share. Beyond the position-indicator basics, each tile
/// answers the questions you actually hold ⌥ to ask:
///
///  * **which one is it** - the pane's own window title (OSC 0/2), shortened,
///    so you read `jcode` and `cargo` rather than `2` and `3`;
///  * **how do I get there** - the column address `⌥+<n>` jumps to, with the
///    in-flight number highlighted as you type it and everything else dimmed;
///  * **where should I look** - the pane `⌥+g` would take you to is marked
///    `▸`, and a pane that wants attention carries how long it has waited;
///  * **what is on screen right now** - the strip's visible column span is
///    underscored, which is the one thing an infinite strip cannot show you
///    by itself.
///
/// A gutter names each strip, the footer counts panes by status and spells
/// the keys that act on what you are looking at.
///
/// Plan-and-paint in one call. The render loop keeps the two apart (it needs
/// the plan for the scrim and for click-to-focus); tests, which only care
/// about what lands on the screen, use this.
#[cfg(test)]
fn draw_center_minimap(
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
/// is decided once and reused by the scrim and by click-to-focus.
fn paint_center_minimap(
    out: &mut [Cell],
    cols: u16,
    rows: u16,
    layout: &Layout,
    plan: &HudPlan,
    pal: &Palette,
    facts: &HudFacts,
) {
    let focus_color = pal.accent;
    let status_bg = |s: PaneStatus| pal.status_muted(s);
    let status_fg = |s: PaneStatus| pal.status(s);
    let status_glyph = status_glyph_for;
    let hint = hud_hint();
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
    draw_focus_frame(out, cols, plan.rect, focus_color);
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
        // While a `⌥+<number>` is being typed, the tiles it does not address
        // step back so the target reads instantly.
        let addressed = facts
            .pending_jump
            .map(|n| n == tile.column + 1)
            .unwrap_or(true);
        let bgc = if tile.focus_col {
            focus_color
        } else if !addressed {
            pal.overlay
        } else {
            status_bg(tile.status)
        };
        let bgc = if facts.pending_jump.is_some() && addressed && !tile.focus_col {
            // The pending target is lit at full status intensity: the point
            // of the preview is that it stands out from the dimmed rest.
            status_fg(tile.status)
        } else {
            bgc
        };
        let fg = contrast_fg(bgc, pal);
        let gy = plan.row_y[tile.y as usize] as usize;
        let glyph = status_glyph(tile.status);
        let target = facts.jump_target == Some(tile.pane);
        // Columns past 9 are addressable with `⌥+1 0`, so print both digits
        // when the tile can hold them rather than falling back to `+`.
        let addr = if tile.pane_idx == 0 {
            format!("{}", tile.column + 1)
        } else {
            "·".to_string()
        };
        let age = match tile.status {
            PaneStatus::Idle | PaneStatus::Failed => facts
                .quiet
                .get(&tile.pane)
                .filter(|d| d.as_secs() >= 5)
                .map(|d| age_label(*d))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let title = facts.titles.get(&tile.pane).cloned().unwrap_or_default();
        let text = tile_text(tile.w, &addr, target, &title, &age, glyph);
        for (dx, ch) in text.chars().enumerate() {
            let x = map_ox + tile.x as usize + dx;
            // Bold the leading `glyph + address` signature: it is what the
            // eye lands on when scanning a row of abutting tiles, and it is
            // what the `⌥+<n>` keys act on.
            let sig = dx <= addr.chars().count();
            put(out, x as u16, gy as u16, ch, fg, bgc, sig);
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
            plan.tally_y.unwrap_or(plan.hint_y) as usize - 1,
            &more,
            Palette::muted(pal.text),
            false,
        );
    }
    // Footer: tallies right-aligned on their own row, then the key hints
    // centred on the last inner row.
    if let Some(fy) = plan.tally_y {
        let mut x = (ox + bw).saturating_sub(tally_w + 1);
        for (text, fg) in &tally {
            for ch in text.chars() {
                put(out, x as u16, fy, ch, *fg, bg, true);
                x += 1;
            }
        }
    }
    let hint_len = hint.chars().count();
    if hint_len <= plan.inner_w {
        write(
            out,
            inner_ox + (plan.inner_w - hint_len) / 2,
            plan.hint_y as usize,
            &hint,
            Palette::muted(pal.text),
            false,
        );
    }
}

/// Dim every cell outside `keep` so a centered panel reads as *above* the
/// session rather than pasted into it. Pane content stays legible (this is a
/// scrim, not a blackout) and the panel's own cells are untouched.
fn dim_behind(out: &mut [Cell], cols: u16, rows: u16, keep: Rect) {
    let scale = |c: CColor| match c {
        CColor::Rgb(r, g, b) => CColor::Rgb(
            ((r as u16 * 9) / 16) as u8,
            ((g as u16 * 9) / 16) as u8,
            ((b as u16 * 9) / 16) as u8,
        ),
        // Indexed and default colors have no components to scale; dropping
        // them to a fixed grey would fight the user's own terminal scheme.
        other => other,
    };
    for y in 0..rows {
        for x in 0..cols {
            let inside = x >= keep.x
                && y >= keep.y
                && x < keep.x.saturating_add(keep.w)
                && y < keep.y.saturating_add(keep.h);
            if inside {
                continue;
            }
            if let Some(c) = out.get_mut(y as usize * cols as usize + x as usize) {
                c.style.fg = scale(c.style.fg);
                c.style.bg = scale(c.style.bg);
                c.style.bold = false;
            }
        }
    }
}

/// Center HUD: a concise cheat-sheet of every keybind, shown at startup and
/// toggled with `⌥+/`. Persists until the next key press.
///
/// Attention (Idle/Failed panes) is deliberately *not* surfaced here: the
/// ambient chrome already carries it (pane tints, right-edge strip ticks,
/// minimap glyphs) and `⌥+g` jumps to the pane that wants you on demand.
/// Draw the theme picker: a small centered panel naming the previewed theme.
///
/// The picker deliberately shows almost nothing, because the *whole screen*
/// is already the preview: stepping through presets re-themes the live
/// chrome behind this panel. All it has to answer is "which one am I looking
/// at, and how do I keep it".
/// Live state of the `⌥+d` spawn-directory picker.
///
/// Mirrors the theme picker's grammar (open, step, ⏎ keep, esc cancel) and
/// adds a typed filter, because the candidate list is dozens of repos rather
/// than eight themes. `s` writes the highlighted directory back to the config
/// file, which is the difference between "this session" and "from now on".
struct DirPicker {
    all: Vec<crate::spawndir::Candidate>,
    query: String,
    sel: usize,
}

impl DirPicker {
    fn shown(&self) -> Vec<crate::spawndir::Candidate> {
        crate::spawndir::filter(&self.all, &self.query)
    }
    fn current(&self) -> Option<crate::spawndir::Candidate> {
        self.shown().get(self.sel).cloned()
    }
    /// Move the highlight, clamped to the filtered list. Wrapping matches the
    /// theme picker, so a long repo list is reachable from either end.
    fn step(&mut self, d: i32) {
        let n = self.shown().len();
        if n == 0 {
            self.sel = 0;
            return;
        }
        let i = self.sel as i32 + d;
        self.sel = i.rem_euclid(n as i32) as usize;
    }
}

/// Draw the spawn-directory picker: the filter line, the matching directories
/// with the selection highlighted, and the key legend.
///
/// Unlike the theme picker there is nothing to preview live (a directory does
/// not repaint the screen), so this panel has to actually show the list.
fn draw_dir_picker(out: &mut [Cell], cols: u16, rows: u16, pick: &DirPicker, pal: &Palette) {
    let shown = pick.shown();
    let rows_shown = shown.len().clamp(1, 10);
    let title = format!(" spawn dir: {}_ ", pick.query);
    // The save key is the *chord*, not a bare `s`: every printable key types
    // into the filter, so advertising `s` would tell the user to type a
    // letter that filters instead of saving.
    let help = format!(
        " ↑/↓ pick   ⏎ session   {} save to config   esc cancel ",
        crate::keys::chord("s")
    );
    let help = help.as_str();
    let widest = shown
        .iter()
        .take(rows_shown)
        .map(|c| c.label.chars().count() + c.origin.chars().count() + 4)
        .max()
        .unwrap_or(0);
    let bw = widest.max(title.chars().count()).max(help.chars().count()) + 2;
    let bh = rows_shown + 4;
    if (cols as usize) < bw + 2 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.accent;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    // A free function rather than a closure: the selection highlight below
    // also needs `&mut out`, and a capturing closure would hold the borrow
    // for the whole body.
    #[allow(clippy::too_many_arguments)]
    fn text(
        out: &mut [Cell],
        cols: u16,
        limit: usize,
        row: usize,
        col: usize,
        s: &str,
        fg: CColor,
        bg: CColor,
        bold: bool,
    ) {
        for (i, ch) in s.chars().enumerate() {
            if col + i >= limit {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + col + i) {
                c.ch = ch;
                c.style.fg = fg;
                c.style.bg = bg;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    }
    let lim = ox + bw - 1;
    text(
        out,
        cols,
        lim,
        oy + 1,
        ox + 1,
        &title,
        pal.accent,
        pal.surface,
        true,
    );
    if shown.is_empty() {
        text(
            out,
            cols,
            lim,
            oy + 2,
            ox + 2,
            "no match",
            pal.overlay,
            pal.surface,
            false,
        );
    }
    // Scroll the window so the selection is always on screen, even when the
    // filter leaves more matches than the panel can hold.
    let first = pick.sel.saturating_sub(rows_shown.saturating_sub(1));
    for (i, c) in shown.iter().skip(first).take(rows_shown).enumerate() {
        let y = oy + 2 + i;
        let selected = first + i == pick.sel;
        let (fg, bg) = if selected {
            (pal.base, pal.accent)
        } else {
            (pal.text, pal.surface)
        };
        if selected {
            for x in 1..bw - 1 {
                if let Some(cell) = out.get_mut(y * cols as usize + ox + x) {
                    cell.ch = ' ';
                    cell.style.bg = bg;
                    cell.style.fg = fg;
                    cell.width = 1;
                }
            }
        }
        text(out, cols, lim, y, ox + 2, &c.label, fg, bg, selected);
        let ow = c.origin.chars().count();
        let at = ox + bw - 2 - ow.min(bw.saturating_sub(4));
        let ofg = if selected { fg } else { pal.overlay };
        text(out, cols, lim, y, at, c.origin, ofg, bg, false);
    }
    text(
        out,
        cols,
        lim,
        oy + bh - 2,
        ox + 1,
        help,
        pal.overlay,
        pal.surface,
        false,
    );
}

fn draw_theme_picker(out: &mut [Cell], cols: u16, rows: u16, sel: usize, pal: &Palette) {
    let names = Palette::NAMES;
    let Some(name) = names.get(sel) else {
        return;
    };
    let title = format!(" theme {}/{}: {} ", sel + 1, names.len(), name);
    let help = " ←/→ preview   ⏎ keep   esc cancel ";
    let bw = title.chars().count().max(help.chars().count()) + 2;
    let bh = 4usize;
    if (cols as usize) < bw + 2 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    // Panel background.
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    // Accent border, so the picker itself demonstrates the previewed accent.
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.accent;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    let mut text = |row: usize, s: &str, fg: CColor, bold: bool| {
        let chars: Vec<char> = s.chars().collect();
        let tx = ox + 1 + (bw - 2).saturating_sub(chars.len()) / 2;
        for (i, ch) in chars.iter().enumerate() {
            if tx + i >= ox + bw - 1 {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + tx + i) {
                c.ch = *ch;
                c.style.fg = fg;
                c.style.bg = pal.surface;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    };
    text(oy + 1, &title, pal.accent, true);
    text(oy + 2, help, pal.text, false);
}

/// Centered disclaimer for the force-quit chord (`⌥+Shift+q`).
///
/// Quitting kills every pane and everything running in them, which is the one
/// irreversible thing gwae can do, so the chord opens this overlay instead
/// of exiting outright: it names the cost (how many panes die) and requires a
/// second, deliberate keystroke.
fn draw_quit_confirm(out: &mut [Cell], cols: u16, rows: u16, panes: usize, pal: &Palette) {
    let title = format!(
        " force quit gwae? {} pane{} will be killed ",
        panes,
        if panes == 1 { "" } else { "s" }
    );
    let warn = " running commands are terminated immediately ";
    let help = format!(
        " {} again or ⏎ quits   esc cancels ",
        crate::keys::shift_chord("q")
    );
    let bw = [title.as_str(), warn, help.as_str()]
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let bh = 5usize;
    if (cols as usize) < bw + 2 || (rows as usize) < bh + 2 {
        return;
    }
    let ox = ((cols as usize) - bw) / 2;
    let oy = ((rows as usize) - bh) / 2;
    for y in 0..bh {
        for x in 0..bw {
            if let Some(c) = out.get_mut((oy + y) * cols as usize + ox + x) {
                *c = Cell {
                    ch: ' ',
                    style: gwae_term::Style {
                        fg: pal.text,
                        bg: pal.surface,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
    // The border uses the failed tint: this is the destructive overlay, and it
    // must not be mistaken at a glance for the theme picker.
    let mut edge = |x: usize, y: usize, ch: char| {
        if let Some(c) = out.get_mut(y * cols as usize + x) {
            c.ch = ch;
            c.style.fg = pal.failed;
            c.style.bg = pal.surface;
            c.width = 1;
        }
    };
    for x in 0..bw {
        edge(ox + x, oy, '─');
        edge(ox + x, oy + bh - 1, '─');
    }
    for y in 0..bh {
        edge(ox, oy + y, '│');
        edge(ox + bw - 1, oy + y, '│');
    }
    edge(ox, oy, '╭');
    edge(ox + bw - 1, oy, '╮');
    edge(ox, oy + bh - 1, '╰');
    edge(ox + bw - 1, oy + bh - 1, '╯');

    let mut text = |row: usize, s: &str, fg: CColor, bold: bool| {
        let chars: Vec<char> = s.chars().collect();
        let tx = ox + 1 + (bw - 2).saturating_sub(chars.len()) / 2;
        for (i, ch) in chars.iter().enumerate() {
            if tx + i >= ox + bw - 1 {
                break;
            }
            if let Some(c) = out.get_mut(row * cols as usize + tx + i) {
                c.ch = *ch;
                c.style.fg = fg;
                c.style.bg = pal.surface;
                c.style.bold = bold;
                c.width = 1;
            }
        }
    };
    text(oy + 1, &title, pal.failed, true);
    text(oy + 2, warn, pal.text, false);
    text(oy + 3, &help, pal.text, false);
}

/// Draw a one-line toast along the bottom of the screen.
///
/// Used to report config reloads. It is a single row so it never covers a
/// pane's working area meaningfully, and it sits on the theme's `surface` so
/// it reads as chrome rather than as pane output.
fn draw_toast(out: &mut [Cell], cols: u16, rows: u16, text: &str, pal: &Palette, ok: bool) {
    draw_toast_at(out, cols, rows, text, pal, ok, None)
}

/// Like [`draw_toast`] but optionally anchored to the bottom-left of a rect
/// (a pane) instead of the bottom-left of the screen. A drag-copy note belongs
/// to the pane the text came from, so with several panes on screen it is
/// obvious which pane's selection was copied.
fn draw_toast_at(
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

fn draw_center_hud(out: &mut [Cell], cols: u16, rows: u16, pal: &Palette) {
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
    draw_focus_frame(out, cols, rect, focus_color);
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
fn draw_edge_ticks(
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
    let tick_bg = |s: PaneStatus| pal.status_muted(s);
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
            pal.status_muted(PaneStatus::Idle)
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
    pal: &Palette,
) {
    let focus_color = pal.accent;
    use gwae_layout::minimap;
    // With a single pane there is nothing to triage; hide the map.
    if !mm.show || (layout.panes.len() <= 1 && layout.rows.len() <= 1) {
        return;
    }
    // Muted tile backgrounds and bright foregrounds, both from the palette.
    let status_bg = |s: PaneStatus| pal.status_muted(s);
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
    /// Move the focused pane's *vertical* scrollback by this many rows
    /// (positive = back into history). The keyboard route into scrollback,
    /// and since gwae no longer claims the wheel, the only one.
    ScrollBack(i32),
    Input(Vec<u8>),
    /// Smart-jump: focus the next pane that needs the user (`⌥+g`). Resolved
    /// against the live layout in the main loop, not here.
    SmartJump,
    /// Open the theme picker (`⌥+t`), or step through it while it is open.
    /// `0` opens, `-1`/`+1` move the selection.
    ThemePick(i32),
    /// Open the spawn-directory picker (`⌥+d`): choose the directory new
    /// panes start in, for this session or written back to the config.
    DirPick,
    /// Toggle the centered cheat-sheet HUD (`⌥+/`), the same overlay shown
    /// once at startup. Any other key still dismisses it.
    ToggleHud,
    /// One digit of a column jump (`⌥+1`, or `⌥+1 2` while Option stays down).
    ///
    /// Deliberately *not* resolved to a column here: a single keypress is
    /// ambiguous, because `⌥+1` may be the whole address or the first half of
    /// `⌥+12`. Only the main loop knows when the chord ends (Option released,
    /// or the idle timeout), so it owns the accumulator; this just reports
    /// "digit N was typed as part of a jump".
    JumpDigit(u32),
    /// Paste the system clipboard into the focused pane (`⌥+v`).
    ///
    /// The explicit route, for terminals that never bracket a paste for us and
    /// for the muscle memory that every gwae verb is an `⌥` chord. Resolved in
    /// the main loop, which owns the clipboard read and the large-paste
    /// confirmation.
    Paste,
    /// Copy from the focused pane to the system clipboard (`⌥+c`).
    ///
    /// Scope is chosen by context in the main loop: a live drag-selection if
    /// there is one, else the visible pane.
    Copy,
    Quit,
    None,
}

/// Accumulates the digits of a column jump typed while the modifier is held.
///
/// `⌥+1..9` used to jump on the keystroke itself, which made columns 10 and
/// up unreachable by address: there is no `⌥+10` key. Holding Option is
/// already a mode (it reveals the HUD/minimap), so the natural fix is to let
/// that mode collect a *number* rather than a single digit and commit it when
/// the mode ends.
///
/// Commit happens on whichever comes first:
/// * Option released (the precise signal, available under the Kitty keyboard
///   protocol, which gwae requests at startup);
/// * [`Self::TIMEOUT`] of no further digits (the fallback for terminals that
///   never report a bare release, where the accumulator would otherwise hang
///   forever and swallow the jump);
/// * any other chord, which ends the number the same way a non-digit ends a
///   count in vi.
///
/// Kept free of terminal types so the whole state machine is unit testable.
#[derive(Debug, Default)]
struct JumpAccum {
    /// The 1-based column number typed so far, if any.
    value: Option<usize>,
    /// When an un-committed number goes stale. Refreshed by every digit.
    deadline: Option<Instant>,
}

impl JumpAccum {
    /// How long a pending number survives without a release event. Long
    /// enough to type a second digit deliberately, short enough that a
    /// terminal without release reporting still feels immediate.
    const TIMEOUT: Duration = Duration::from_millis(500);

    /// Absurd addresses are refused rather than accumulated forever: a jump is
    /// clamped to the columns that exist anyway, and this keeps `value` from
    /// overflowing when a key repeat spams digits.
    const MAX: usize = 999;

    /// Record one digit. `0` extends an existing number (`⌥+1 0` -> 10) but
    /// starts nothing on its own, since there is no column 0.
    fn push(&mut self, d: u32, now: Instant) {
        let d = d as usize;
        let next = match self.value {
            Some(v) => v * 10 + d,
            None if d == 0 => return,
            None => d,
        };
        if next > Self::MAX {
            return;
        }
        self.value = Some(next);
        self.deadline = Some(now + Self::TIMEOUT);
    }

    /// Take the accumulated number as a 0-based column index, clearing state.
    fn take(&mut self) -> Option<usize> {
        self.deadline = None;
        self.value.take().map(|v| v.saturating_sub(1))
    }

    /// Commit if the idle timeout has passed. Returns the column index to
    /// focus, if any.
    fn take_if_expired(&mut self, now: Instant) -> Option<usize> {
        match self.deadline {
            Some(t) if now >= t => self.take(),
            _ => None,
        }
    }

    /// Whether a number is being typed right now (drives the HUD hint).
    fn pending(&self) -> Option<usize> {
        self.value
    }
}

/// Encode a key event that is not a gwae chord into PTY bytes.
///
/// Alt (Option) is forwarded as Meta: an `ESC` prefix before the base
/// sequence, matching what a pane sees when run natively outside gwae
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
    // Bare modifier press/release (Alt, Shift, Ctrl, Super, etc.) must never
    // become pane input. With the Kitty keyboard protocol a lone Option press
    // arrives as `Modifier(LeftAlt)` with the Alt bit set; the previous
    // fallthrough turned that into `key_bytes() == ESC` and cleared the
    // focused pane's line editor (e.g. jcode). Treat every pure modifier key
    // as a no-op — Alt-hold tracking is handled in run_tui, not here.
    if matches!(ev.code, KeyCode::Modifier(_)) {
        return Some(Cmd::None);
    }
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
            // ¿ (Option+Shift+/), i.e. Option+? — same toggle as Option+/.
            Char('\u{bf}') => return Some(Cmd::ToggleHud),
            // Ú (Option+Shift+;) spawns an agent on a new strip.
            Char('\u{da}') => return Some(Cmd::Act(Action::SpawnAgentRow)),
            _ => {}
        }
    }
    if !alt && !ctrl && !shift {
        match ev.code {
            Char('\u{2026}') => return Some(Cmd::Act(Action::SpawnAgent)), // … (Option+;)
            Char('\u{2d9}') => return Some(Cmd::Act(Action::FocusLeft)),   // ˙ (Option+h)
            Char('\u{2206}') => return Some(Cmd::Act(Action::FocusDown)),  // ∆ (Option+j)
            Char('\u{2da}') => return Some(Cmd::Act(Action::FocusUp)),     // ˚ (Option+k)
            Char('\u{ac}') => return Some(Cmd::Act(Action::FocusRight)),   // ¬ (Option+l)

            Char('\u{153}') => return Some(Cmd::Act(Action::KillPane)), // œ (Option+q)
            Char('\u{a9}') => return Some(Cmd::SmartJump),              // © (Option+g)
            Char('\u{192}') => return Some(Cmd::Act(Action::ToggleFullWidth)), // ƒ (Option+f)
            Char('\u{2020}') => return Some(Cmd::ThemePick(0)),         // † (Option+t)
            Char('\u{2202}') => return Some(Cmd::DirPick),              // ∂ (Option+d)
            Char('\u{f7}') => return Some(Cmd::ToggleHud),              // ÷ (Option+/)
            Char('\u{e7}') => return Some(Cmd::Copy),                   // ç (Option+c)
            Char('\u{221a}') => return Some(Cmd::Paste),                // √ (Option+v)
            _ => {}
        }
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
        // ⌥+Shift+q and ⌥+Shift+? are the only shifted chords outside hjkl.
        // Everything else below is an *unshifted* chord: `logical_char` folds
        // case, so without this guard ⌥+Shift+s would be indistinguishable from
        // ⌥+s and gwae would split the column instead of forwarding the key.
        // That silently ate chords the focused pane owns (jcode binds
        // ⌥+Shift+s to copy), so shifted variants fall through to the pane.
        if shift && !matches!(c, 'q' | '/' | '?' | ';' | ':') {
            // Forward the shifted codepoint. Some terminals report Shift as a
            // modifier bit alongside the *unshifted* char; `key_bytes` encodes
            // `ev.code` verbatim and has no shift handling for `Char`, so the
            // pane would receive ESC+'s' and see a plain ⌥+s. Re-apply the
            // shift here so the pane sees ESC+'S' either way.
            let mut ev = *ev;
            if let KeyCode::Char(raw) = ev.code {
                ev.code = KeyCode::Char(raw.to_ascii_uppercase());
            }
            return Some(Cmd::Input(key_bytes(&ev)));
        }
        let act = match c {
            // Some terminals deliver ⌥+Shift+; as a bare ':' with no shift
            // bit; the shifted codepoint itself is the signal.
            ';' | ':' => Some(if shift || matches!(ev.code, Char(':')) {
                Action::SpawnAgentRow
            } else {
                Action::SpawnAgent
            }),
            's' => Some(Action::SplitBelow),
            'r' => Some(Action::CycleWidth),
            'f' => Some(Action::ToggleFullWidth),
            'z' => Some(Action::CycleWidth),
            'q' => {
                if shift {
                    return Some(Cmd::Quit);
                } else {
                    Some(Action::KillPane)
                }
            }
            'g' => return Some(Cmd::SmartJump),
            'c' => return Some(Cmd::Copy),
            // `⌥+y` is the vi-flavoured alias for copy, kept from the
            // roadmap's yank entry so that muscle memory is not wasted.
            'y' => return Some(Cmd::Copy),
            'v' => return Some(Cmd::Paste),
            't' => return Some(Cmd::ThemePick(0)),
            'd' => return Some(Cmd::DirPick),
            '/' | '?' => return Some(Cmd::ToggleHud),
            _ if c.is_ascii_digit() => {
                return Some(Cmd::JumpDigit(c.to_digit(10).unwrap_or(1)));
            }
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
    // Up/Down move the focused pane's scrollback: gwae does not claim the
    // wheel, so this is how you read back through a pane's history. Shift (and
    // PageUp/PageDown) move by a screenful-ish jump rather than a line.
    if matches!(ev.code, Up | Down | PageUp | PageDown) {
        let step = if shift || matches!(ev.code, PageUp | PageDown) {
            20
        } else {
            3
        };
        return Some(match ev.code {
            Up | PageUp => Cmd::ScrollBack(step),
            _ => Cmd::ScrollBack(-step),
        });
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
        Char(c) if c.is_ascii_digit() => return Some(Cmd::JumpDigit(c.to_digit(10).unwrap_or(1))),
        Char('[') => return Some(Cmd::Scroll(-200)),
        Char(']') => return Some(Cmd::Scroll(200)),
        _ => {}
    }
    Some(Cmd::Input(key_bytes(ev)))
}

/// Deliver pasted text to a pane, bracketed the way that child expects.
///
/// The one paste path: both the host's `Event::Paste` (a ⌘/Ctrl+V typed into
/// gwae) and the explicit `⌥+v` clipboard read end up here, so the two
/// entrances cannot diverge in their newline handling or their bracketing.
///
/// Writes in chunks with a flush between: a paste can be a whole file, a PTY
/// buffer is 4-64 KiB, and a single blocking write of the lot would stall the
/// event loop — freezing every *other* pane's paint until the child drains.
///
/// Returns the number of bytes handed to the pane (0 when the payload was
/// empty or the pane is gone), which the caller reports in a toast.
fn write_paste(pane: &mut PtyPane, text: &str) -> usize {
    let bytes = select::paste_bytes(text, pane.grid.wants_bracketed_paste());
    if bytes.is_empty() {
        return 0;
    }
    // Pasting means you want the prompt: snap a scrolled-back pane to live,
    // exactly as typing does, or the user pastes into a view of the past.
    pane.grid.scroll_to_bottom();
    for chunk in bytes.chunks(select::PASTE_CHUNK) {
        if pane.writer.write_all(chunk).is_err() {
            return 0;
        }
        let _ = pane.writer.flush();
    }
    bytes.len()
}

/// Above this many lines, or this many bytes, `⌥+v` asks before pasting.
///
/// Small enough that a command, a URL, or a stack trace goes straight through
/// (the overwhelming majority of pastes), large enough that "I had a whole file
/// on the clipboard and forgot" gets caught. A wrong paste into an agent's
/// prompt is not undoable from gwae's side: the child owns the bytes the
/// instant they are written.
const PASTE_CONFIRM_LINES: usize = 8;
const PASTE_CONFIRM_BYTES: usize = 2048;

/// The anchor rect for a toast about the focused pane, or `None` when there is
/// no focused pane (the toast then falls back to the screen's bottom-left).
fn focused_pane_rect(
    layout: &Layout,
    panes: &HashMap<PaneId, PtyPane>,
    cfg: &Config,
    cols: u16,
    rows: u16,
) -> Option<Rect> {
    let pid = focused_pane(layout)?;
    focused_pane_views_with_chrome(
        layout,
        cols,
        rows,
        cfg.content_width,
        panes,
        true,
        chrome_rows(cfg),
    )
    .iter()
    .find(|v| v.pid == pid)
    .map(|v| v.rect)
}

/// Paste `text` into the focused pane, returning the toast to show and where
/// to anchor it. Backs `⌥+v`; `Event::Paste` uses `write_paste` directly
/// because it already holds the pane.
fn paste_into_focused(
    layout: &Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    cfg: &Config,
    cols: u16,
    rows: u16,
    text: &str,
) -> (Option<String>, Option<Rect>) {
    let anchor = focused_pane_rect(layout, panes, cfg, cols, rows);
    let Some(pid) = focused_pane(layout) else {
        return (Some("no pane focused".to_string()), None);
    };
    let Some(p) = panes.get_mut(&pid) else {
        return (Some("no pane focused".to_string()), None);
    };
    let bracketed = p.grid.wants_bracketed_paste();
    if write_paste(p, text) == 0 {
        return (Some("nothing to paste".to_string()), anchor);
    }
    // A single-line paste needs no narration through this route either, but
    // `⌥+v` is an explicit request: confirming that *something* happened is
    // worth one line, since unlike ⌘+V there is no OS-level feedback.
    (Some(paste_note(text, bracketed)), anchor)
}

/// Copy from the focused pane to the clipboard, choosing the scope by context:
/// a finished drag-selection if there is one, else everything visible in the
/// pane. Returns the toast and its anchor.
///
/// Trailing blank lines are dropped from the whole-pane scope: a pane's grid is
/// padded to its full height, so copying it verbatim yields a wall of empty
/// lines after the last real output, which is never what anyone pastes.
fn copy_from_focused(
    layout: &Layout,
    panes: &HashMap<PaneId, PtyPane>,
    cfg: &Config,
    cols: u16,
    rows: u16,
    selection: Option<Selection<PaneId>>,
) -> (String, Option<Rect>) {
    let anchor = focused_pane_rect(layout, panes, cfg, cols, rows);
    let Some(pid) = focused_pane(layout) else {
        return ("no pane focused".to_string(), None);
    };
    let Some(p) = panes.get(&pid) else {
        return ("no pane focused".to_string(), None);
    };
    // Scope 1: a selection in *this* pane. A selection left in some other pane
    // is not what "copy" means while focus is here.
    let text = match selection.filter(|s| s.pane == pid && !s.is_empty()) {
        Some(s) => select::selected_text(&p.grid, &s),
        // Scope 2: the visible pane, top to bottom.
        None => {
            let size = p.grid.size();
            let whole = Selection {
                pane: pid,
                anchor: select::Point::new(0, 0),
                cursor: select::Point::new(
                    size.cols.saturating_sub(1),
                    size.rows.saturating_sub(1),
                ),
                dragging: false,
            };
            let text = select::selected_text(&p.grid, &whole);
            text.trim_end_matches('\n').to_string()
        }
    };
    if text.is_empty() {
        return ("nothing to copy".to_string(), anchor);
    }
    if select::copy_to_clipboard(&text) {
        (copy_note(&text), anchor)
    } else {
        ("clipboard unavailable".to_string(), anchor)
    }
}

/// Kill any pane whose id is no longer in the layout, and spawn missing ones.
fn sync_panes(
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    tx: &Sender<PaneMsg>,
    _first_id: PaneId,
    agent_panes: &HashSet<PaneId>,
    cwd: Option<&std::path::Path>,
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
            kill_pane_tree(&mut pane.child);
            false
        }
    });
    // Spawn missing panes. Agent panes (created via the spawn-agent verb) run
    // the agent gateway, which becomes the harness; everything else gets the
    // shell.
    for pid in wanted {
        if panes.contains_key(&pid) {
            continue;
        }
        // Agent panes run the gateway, not the harness directly: it resolves
        // `default_agent`, prompts when there is nothing to resolve, and execs
        // the result, so the pane's process ends up *being* the harness.
        // Resolving inside the pane (rather than here) is what lets the "not
        // installed" case be a real interactive screen instead of a toast.
        let cmd = if agent_panes.contains(&pid) {
            agent_gateway_cmd()
        } else {
            String::new()
        };
        let pane = spawn_pane(pid, &cmd, 80, 24, tx.clone(), cwd)?;
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
                if std::env::var_os("GWAE_DEBUG_SIZE").is_some() {
                    eprintln!("[gwae] terminal size -> {c} cols x {r} rows");
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

/// How often the config file is checked for edits.
///
/// Fast enough that saving a theme feels immediate, slow enough that the
/// `stat` never shows up next to the render loop's own work.
const CONFIG_POLL: Duration = Duration::from_millis(400);

/// How long the binary's mtime must hold still before a hot reload fires.
///
/// A link is not atomic: the new image is truncated and written over some
/// milliseconds, so a poll can catch a fresh mtime on a file that is not yet
/// a valid executable. Requiring quiet costs one extra poll and turns "exec a
/// half-written binary" (which kills the session and every pane with it) into
/// "reload a moment later".
const BINARY_SETTLE: Duration = Duration::from_millis(300);

/// How often the safety-net size re-measure runs.
///
/// `refresh_size` is a *backstop* for resizes crossterm never delivers as an
/// `Event::Resize`; the event path already handles every resize the host does
/// report, and it stays instant. Running the backstop on every loop iteration
/// meant a `TIOCGWINSZ` — which on macOS opens and closes `/dev/tty` — at the
/// input poll rate (500/s at the default `input_poll_ms = 2`). That syscall
/// storm was the bulk of gwae's idle CPU: a completely idle mux sat at ~3.5%
/// of a core forever, which on a laptop is a warm chassis and a spinning fan
/// for no work at all. A quarter second is far below human resize perception
/// for the rare dropped-event case, and costs 4 stats/s instead of 500.
const SIZE_POLL: Duration = Duration::from_millis(250);

/// How long the whole screen must be quiet before the input poll relaxes.
const IDLE_AFTER: Duration = Duration::from_millis(750);

/// The relaxed poll interval used once idle (~33 wakeups/s).
const IDLE_POLL_MS: u64 = 30;

/// How long to block in `event::poll` this iteration.
///
/// `event::poll` returns *immediately* when a key arrives, so a longer
/// timeout never costs keystroke latency; all it delays is the loop's own
/// periodic work. So: run at the configured (tight) rate while anything is
/// happening, and back off once every pane and the keyboard have been silent
/// for `IDLE_AFTER`. At the default 2 ms the idle mux woke 500x a second
/// forever to find nothing, which on a laptop is a warm chassis for a screen
/// that is not changing. The relaxed ceiling still repaints within one frame
/// at 30 fps, and the very first byte of output or keystroke snaps the rate
/// back to tight.
fn input_poll_interval(configured_ms: u64, since_activity: Duration) -> Duration {
    let base = configured_ms.clamp(1, 50);
    let ms = if since_activity < IDLE_AFTER {
        base
    } else {
        base.max(IDLE_POLL_MS)
    };
    Duration::from_millis(ms)
}

/// How long the reload note stays on screen.
const NOTE_LINGER: Duration = Duration::from_millis(2500);

/// The first line of a multi-line error, for one-line status display.
fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

/// Run the interactive TUI.
pub fn run_tui(command: Option<String>, cfg: Config, cli_dir: Option<String>) -> Result<(), i32> {
    use std::io;
    // Config and palette are re-resolved whenever the config file changes on
    // disk (see the reload check in the render loop), so both are mutable.
    let mut cfg = cfg;
    // The theme: preset lookup + `[theme]` overrides + the legacy top-level
    // color keys. Every chrome color painted below reads from this palette.
    let mut pal = cfg.palette();
    let mut stdout = io::stdout();
    // Arm signal handlers + panic hook before the first pane exists, and hold
    // a drop guard so every early return below still reaps. Quitting gwae is
    // documented as killing everything in the panes; this makes that true for
    // the abnormal exits too, not just ⌥+Shift+q.
    crate::reap::install();
    let _reap_guard = crate::reap::Guard;
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
    // gwae positions every run absolutely, so wrapping is never wanted: its
    // only effect is that a run which overshoots the right margin (a glyph the
    // host renders wider than the emulator assumed) spills onto the next row
    // and smears that row's background across the screen. With DECAWM off the
    // overshoot is clamped at the margin and repaired on the next frame.
    let _ = stdout.write_all(b"\x1b[?7l");
    let _ = stdout.flush();
    // Request Kitty keyboard protocol: bare Alt press/release (REPORT_ALL_KEYS)
    // so the centered Alt HUD/minimap (hold ⌥ to reveal) can see the hold
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
    // Capture the mouse so clicks and drags land here: focus follows a click,
    // a drag selects text, and a child that asked for mouse reporting gets the
    // event forwarded verbatim. gwae itself does nothing with the wheel.
    if let Err(e) = execute!(stdout, EnableMouseCapture) {
        tracing::warn!("enable mouse: {e}");
    }
    // Ask the host to bracket pastes. Without this a ⌘/Ctrl+V arrives as raw
    // key events, every newline decodes to `KeyCode::Enter`, and a five-line
    // paste submits five times — running half-typed commands in a shell and
    // half-written prompts in an agent. With it, crossterm hands us the whole
    // payload as one `Event::Paste` that `write_paste` re-brackets for the
    // child. A terminal that ignores the request leaves the old behaviour,
    // which is why `⌥+v` exists as the explicit route.
    if let Err(e) = execute!(stdout, EnableBracketedPaste) {
        tracing::warn!("enable bracketed paste: {e}");
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
    if std::env::var_os("GWAE_DEBUG_SIZE").is_some() {
        eprintln!("[gwae] initial terminal size -> {cols} cols x {rows} rows");
    }
    // Where every pane in this session starts. `--dir` beats `agent_dir`
    // beats gwae's inherited cwd; `⌥+d` rebinds it live for panes spawned
    // from then on (existing panes keep whatever they were born with, since
    // a process's cwd is not ours to change).
    let mut spawn_dir: Option<std::path::PathBuf> =
        crate::spawndir::resolve(cli_dir.as_deref(), &cfg.agent_dir);
    // A configured directory that does not exist is a typo worth surfacing:
    // the panes silently opening in `~` is exactly the confusion this
    // feature exists to remove.
    let bad_dir: Option<String> = {
        let raw = cli_dir
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(cfg.agent_dir.as_str());
        let fell_back = spawn_dir == crate::spawndir::inherited();
        match (fell_back, raw.trim().is_empty()) {
            (true, false) => crate::spawndir::check(raw).err(),
            _ => None,
        }
    };
    // A hot reload hands the previous image's session over in a temp file
    // (see `crate::reload`). Taken before the default layout is built so the
    // adopted tree replaces it rather than racing it.
    let handover = crate::reload::Handover::take();
    let mut layout = match &handover {
        Some(h) => h.layout.clone(),
        None => Layout::new(cfg.startup_panes.max(1)),
    };
    // A ⌥+d choice made before the reload must not be forgotten by the new
    // image, so the handover's value wins over re-resolving the config.
    if let Some(h) = &handover {
        if h.spawn_dir.is_some() {
            spawn_dir = h.spawn_dir.clone();
        }
    }
    // Ask (at most once a day, on a background thread) whether a newer gwae
    // exists. Started here so the request overlaps pane spawn instead of
    // adding to startup; the answer lands in a slot the main loop reads, and
    // a session that ends first simply never sees it. Nothing is ever
    // installed by this: the notice names the command and stops.
    let update_slot = crate::update::spawn_check(cfg.update.check, cfg.update.source_detected());
    let (tx, rx) = channel::<PaneMsg>();
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    let initial = command.clone().unwrap_or_default();
    let gw = cols.max(1);
    let gh = rows.saturating_sub(chrome_rows(&cfg)).max(1);
    // Spawn every pane in the initial strip; the rest get the user's shell.
    // Sort by id: `panes` is a HashMap, and unsorted iteration made *which
    // pane runs the command* random (ids are allocated in column order, so id
    // order is column order).
    //
    // Pane 1.1 is the agent gateway unless `run <cmd>` named something else.
    // gwae exists to drive agents, so opening on a bare shell asked every
    // user to type the harness name themselves on every launch; the gateway
    // either goes straight to the configured agent (indistinguishable from
    // launching it directly) or shows the selector. An explicit `run` command
    // still wins, since that is the user being specific.
    let mut pane_ids: Vec<PaneId> = layout.panes.keys().copied().collect();
    pane_ids.sort_unstable();
    let mut first_is_agent = false;
    // Reload path: the panes already exist as live PTYs inherited across the
    // execve, so they are adopted rather than spawned. Their children never
    // learn that gwae's code was replaced underneath them.
    #[cfg(unix)]
    let reloaded_agents: HashSet<PaneId> = match &handover {
        Some(h) => {
            for hp in &h.panes {
                match adopt_pane(hp.id, hp.fd, hp.pid, hp.cols, hp.rows, tx.clone()) {
                    Ok(p) => {
                        panes.insert(hp.id, p);
                    }
                    // One unusable fd must not cost the whole session: the
                    // pane is dropped from the layout and the rest carry on.
                    Err(e) => {
                        tracing::error!("adopt pane {}: {e}", hp.id);
                    }
                }
            }
            h.panes
                .iter()
                .filter(|p| p.is_agent)
                .map(|p| p.id)
                .collect()
        }
        None => HashSet::new(),
    };
    #[cfg(not(unix))]
    let reloaded_agents: HashSet<PaneId> = HashSet::new();
    let reloading = handover.is_some();
    for (i, pid) in pane_ids.iter().enumerate() {
        // Already adopted above; spawning would start a second child on a
        // pane that already has a live one.
        if reloading {
            break;
        }
        let cmd = if i == 0 {
            if initial.trim().is_empty() {
                first_is_agent = true;
                agent_gateway_cmd()
            } else {
                initial.clone()
            }
        } else {
            String::new()
        };
        match spawn_pane(*pid, &cmd, gw, gh, tx.clone(), spawn_dir.as_deref()) {
            Ok(p) => {
                panes.insert(*pid, p);
            }
            Err(e) => {
                eprintln!("spawn: {e}");
                let _ = stdout.write_all(b"\x1b[?7h");
                let _ = execute!(stdout, DisableBracketedPaste);
                let _ = execute!(stdout, DisableMouseCapture);
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
    // Digits of an in-flight `⌥+<number>` column jump. See `JumpAccum`.
    let mut jump = JumpAccum::default();
    let mut last_alt_held = false;
    // Startup-only cheat-sheet HUD: shown once at init, dismissed on first key.
    let mut hud_active: bool = true;
    // The ⌥-hold dashboard's geometry for the frame currently on screen, so a
    // click can be resolved against the tiles the user is actually looking at.
    // `None` whenever the panel is not up.
    let mut hud_plan: Option<HudPlan> = None;
    let mut last_has_attention = has_attention(&layout);
    // Pane ids created by the spawn-agent verb; these are (re)spawned running
    // the agent gateway instead of a plain shell. A respawn re-resolves, so
    // installing a harness mid-session is picked up without a restart.
    let mut agent_panes: HashSet<PaneId> = HashSet::new();
    // Pane 1.1 counts as an agent pane when it opened on the gateway, so a
    // respawn re-resolves rather than dropping the user into a bare shell.
    if first_is_agent {
        if let Some(pid) = pane_ids.first() {
            agent_panes.insert(*pid);
        }
    }
    // Panes that were agent panes before a reload stay marked as such.
    //
    // Narrower than it sounds, and worth stating precisely: a pane whose
    // process exits is *closed*, not respawned (see the `PaneMsg::Exited`
    // arm, which also drops the pane from this set), so this does not
    // resurrect a dead harness. What it preserves is the marking itself, so
    // the set stays consistent with the layout it describes across a reload
    // rather than silently emptying.
    agent_panes.extend(reloaded_agents.iter().copied());
    // An adopted pane's grid starts empty (contents are not carried across a
    // reload), so ask each child to redraw. Without this the screen stays
    // blank until the user types, which reads as a crash.
    if reloading {
        nudge_repaint(&mut panes);
    }
    // The title currently shown on the host terminal; we only write when it
    // changes so we don't spam the host with identical OSC sequences.
    let mut last_title: String = String::new();
    // Live config reload state: the file we watch, its last seen mtime, when
    // we last checked, and the transient note shown after a reload.
    let cfg_path = Config::default_path();
    let mut cfg_mtime = Config::mtime(&cfg_path);
    let mut reload_check = Instant::now();
    // Hot reload (dev): watch our own binary and swap into the new build
    // in place, keeping every pane. See `crate::reload`.
    let hot_reload = crate::reload::enabled();
    let exe_path = crate::reload::own_path().ok();
    let mut exe_mtime = exe_path.as_deref().and_then(crate::reload::binary_mtime);
    // A rebuild is not atomic: the linker truncates and writes, so the file
    // can be seen mid-write with a fresh mtime and a broken image. Waiting
    // for the mtime to stop moving costs one poll interval and avoids
    // exec'ing a half-written binary.
    let mut exe_changed_at: Option<Instant> = None;
    let mut size_check = Instant::now();
    // Last time *anything* happened (a keystroke or a byte from any pane).
    // Drives the adaptive input poll below: tight while in use, relaxed once
    // the whole screen has gone quiet.
    let mut last_activity = Instant::now();
    // A bad `agent_dir` announces itself on the first frame; the panes have
    // already opened in the inherited cwd by then, so this explains what the
    // user is looking at rather than blocking anything.
    let mut reload_note: Option<String> =
        bad_dir.map(|e| format!("agent_dir: {e}; panes opened in gwae's cwd"));
    let mut reload_note_until: Option<Instant> = reload_note
        .as_ref()
        .map(|_| Instant::now() + NOTE_LINGER * 2);
    // Where the note is drawn: `None` = bottom-left of the screen,
    // `Some(rect)` = bottom-left of that pane (drag-copy notes).
    let mut reload_note_anchor: Option<Rect> = None;
    // Whether the update notice still has to be shown. Latched false after
    // one showing so a user who dismissed it is not told again this session.
    let mut update_note_pending = true;
    // A large `⌥+v` awaiting confirmation: the text, and the deadline by which
    // a second `⌥+v` commits it. Pasting a whole file into an agent's prompt
    // is expensive and irreversible from gwae's side (the child has it the
    // instant it is written), so the big ones ask first. Same grammar as the
    // `⌥+Shift+q` confirmation: repeat the chord to mean it.
    let mut paste_confirm: Option<(String, Instant)> = None;
    // Theme picker (⌥+t): Some(index into Palette::NAMES) while open. The
    // selection previews live, so the whole screen is the preview and the
    // picker itself only needs to show the name.
    let mut theme_pick: Option<usize> = None;
    // Spawn-directory picker (⌥+d): the candidate list, the typed filter, and
    // the highlighted row. Built when the picker opens rather than at startup
    // so a repo cloned mid-session shows up without a restart.
    let mut dir_pick: Option<DirPicker> = None;
    // Force-quit confirmation (⌥+Shift+q): true while the centered disclaimer
    // is up. Quitting kills every pane's process, so the chord arms this
    // overlay and a second deliberate keystroke commits.
    let mut quit_confirm = false;
    // Drag-to-copy: the live (or just-completed) pane selection, if any.
    let mut selection: Option<Selection<PaneId>> = None;

    'main: loop {
        while let Ok(msg) = rx.try_recv() {
            // Any pane traffic (output or an exit) is activity: keep the
            // input poll tight so a burst of output is drained and drawn at
            // full rate rather than at the idle backoff.
            last_activity = Instant::now();
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
                    // exits there is nothing left to show, so gwae quits.
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
                        if let Err(e) = sync_panes(
                            &mut layout,
                            &mut panes,
                            &tx,
                            0,
                            &agent_panes,
                            spawn_dir.as_deref(),
                        ) {
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
        if size_check.elapsed() >= SIZE_POLL {
            size_check = Instant::now();
            if refresh_size(&mut cols, &mut rows) {
                layout.clamp_scrolls(Viewport::new(cols));
                dirty = true;
            }
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

        // Live config reload. Editing the config file repaints the running
        // session, so tweaking a theme is a save away rather than a restart:
        // restarting would kill every pane, which is exactly what someone
        // running long-lived agents cannot afford.
        //
        // Polled by mtime rather than a filesystem watcher: one `stat` at the
        // rate below is negligible next to the render loop, and it avoids a
        // dependency plus the cross-platform watcher differences on the three
        // OSes gwae supports.
        if reload_check.elapsed() >= CONFIG_POLL {
            reload_check = Instant::now();
            let now = Config::mtime(&cfg_path);
            if now != cfg_mtime {
                cfg_mtime = now;
                match Config::load_checked(&cfg_path) {
                    Ok(new) => {
                        let (new_pal, bad) = new.palette_checked();
                        // Keep the panes and their harnesses exactly as they
                        // are; only adopt what is re-read every frame.
                        cfg.adopt_appearance(new);
                        pal = new_pal;
                        reload_note_anchor = None;
                        reload_note = Some(match bad {
                            Some(name) => format!("unknown theme {name:?}"),
                            None => format!("config reloaded: {}", cfg.theme_name()),
                        });
                        dirty = true;
                    }
                    Err(e) => {
                        // Keep the running config: a half-written file (the
                        // editor saved mid-keystroke) must not blow away a
                        // working theme.
                        reload_note_anchor = None;
                        reload_note = Some(format!("config error: {}", first_line(&e)));
                        dirty = true;
                    }
                }
                reload_note_until = Some(Instant::now() + NOTE_LINGER);
            }
        }
        // Hot reload: the binary on disk changed, so replace this process
        // with the new build and carry every pane across (see
        // `crate::reload`). Dev-gated: the failure mode of a subtly wrong
        // reload is orphaned agent processes, not a bad frame.
        #[cfg(unix)]
        if hot_reload {
            if let Some(exe) = exe_path.as_deref() {
                let now = crate::reload::binary_mtime(exe);
                if now != exe_mtime {
                    // Changed: note when, and wait for it to settle.
                    exe_mtime = now;
                    exe_changed_at = Some(Instant::now());
                } else if exe_changed_at
                    .map(|t| t.elapsed() >= BINARY_SETTLE)
                    .unwrap_or(false)
                {
                    exe_changed_at = None;
                    // Leave the terminal exactly as a normal exit would: the
                    // tty is kernel state and survives the exec, so a new
                    // image would otherwise inherit raw mode and the alt
                    // screen and paint into a screen it never entered.
                    tracing::info!("hot reload: binary changed, execing new image");
                    restore_terminal(&mut stdout, kitty_keyboard);
                    match perform_reload(&layout, &panes, &agent_panes, spawn_dir.as_deref()) {
                        // `Ok` is uninhabited: the process is gone.
                        Ok(never) => match never {},
                        Err(e) => {
                            // The exec failed, so this image is still running
                            // and still owns every pane. Put the screen back
                            // and carry on rather than dying with them.
                            tracing::error!("hot reload failed: {e}");
                            if let Err(e) = re_enter_terminal(&mut stdout) {
                                tracing::error!("restore after failed reload: {e}");
                                break 'main;
                            }
                            last.clear();
                            reload_note_anchor = None;
                            reload_note = Some(format!("hot reload failed: {}", first_line(&e)));
                            reload_note_until = Some(Instant::now() + NOTE_LINGER);
                            dirty = true;
                        }
                    }
                }
            }
        }

        // The background update check, if it found something. Taken (not
        // read) so the notice is shown exactly once per session, and only
        // when the screen is not already saying something else: an upgrade
        // hint is the least urgent thing gwae ever has to say, so it yields
        // to a config error or a copy confirmation rather than stomping it.
        if update_note_pending && reload_note.is_none() {
            if let Some(text) = update_slot.lock().ok().and_then(|mut g| g.take()) {
                update_note_pending = false;
                reload_note_anchor = None;
                reload_note = Some(text);
                // Longer than a reload note: this one asks the reader to
                // remember a command, not just to notice that a color
                // changed.
                reload_note_until = Some(Instant::now() + NOTE_LINGER * 3);
                dirty = true;
            }
        }
        // Expire the reload note so it does not sit on screen forever.
        if let Some(t) = reload_note_until {
            if Instant::now() >= t {
                reload_note = None;
                reload_note_until = None;
                reload_note_anchor = None;
                dirty = true;
            }
        }

        // Tight while in use, relaxed once the screen has gone quiet: see
        // `input_poll_interval`.
        let poll_for = input_poll_interval(cfg.input_poll_ms, last_activity.elapsed());
        if event::poll(poll_for).unwrap_or(false) {
            last_activity = Instant::now();
            match event::read() {
                Ok(Event::Key(ke))
                    if matches!(ke.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    // The HUD persists until the next key press; ⌥+/ toggles
                    // it explicitly, so remember whether it was up before we
                    // dismiss it here.
                    let hud_was_active = hud_active;
                    // Typing dismisses a finished selection, the way it does in
                    // any editor: the highlight is a transient artifact of the
                    // drag, and leaving it inverted over live pane output would
                    // read as corruption. A drag still in flight is left alone.
                    if selection.take_if(|s| !s.dragging).is_some() {
                        dirty = true;
                    }
                    if hud_active {
                        hud_active = false;
                        dirty = true;
                    }
                    // Bare Alt hold: track before handle_key so chords don't double-count.
                    let bare_alt = is_alt_modifier(&ke);
                    if bare_alt {
                        if !bare_alt_held {
                            bare_alt_held = true;
                            dirty = true;
                        }
                    } else {
                        // Fallback for terminals that don't send bare Alt press/release
                        // (no Kitty keyboard protocol): any Alt chord counts as "held"
                        // for a short window so the centered HUD/minimap still reveals on press-and-hold
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
                            // Short window so tapping Option+h for navigation
                            // doesn't linger 600ms after release. Hold stays
                            // visible via repeats (each chord refreshes) but
                            // vanishes ~180ms after the last chord.
                            chord_alt_until = Some(Instant::now() + Duration::from_millis(180));
                            dirty = true;
                        }
                    }
                    // While the force-quit disclaimer is up it owns the
                    // keyboard: nothing may reach a pane, because the very
                    // next keystroke may destroy every pane. Only the same
                    // chord again (or Enter) commits; anything else cancels,
                    // so a stray key can never quit by accident.
                    if quit_confirm {
                        let confirmed = matches!(handle_key(&ke), Some(Cmd::Quit))
                            || matches!(ke.code, KeyCode::Enter);
                        if confirmed {
                            break 'main;
                        }
                        quit_confirm = false;
                        dirty = true;
                        continue;
                    }
                    // While the directory picker is open it owns the
                    // keyboard: every printable key types into the filter, so
                    // nothing may reach a pane. Arrows move, ⏎ takes it for
                    // the session, `⌥+s` writes it to the config file, esc
                    // cancels. Bare `s` cannot save, because `s` is a filter
                    // character like any other.
                    if let Some(pick) = dir_pick.as_mut() {
                        let alt = ke.modifiers.contains(KeyModifiers::ALT);
                        let mut chosen: Option<(std::path::PathBuf, bool)> = None;
                        let mut close = false;
                        match ke.code {
                            KeyCode::Up => pick.step(-1),
                            KeyCode::Down => pick.step(1),
                            KeyCode::Esc => close = true,
                            KeyCode::Backspace => {
                                pick.query.pop();
                                pick.sel = 0;
                            }
                            KeyCode::Enter => {
                                if let Some(c) = pick.current() {
                                    chosen = Some((c.path, false));
                                }
                                close = true;
                            }
                            KeyCode::Char('s') if alt => {
                                if let Some(c) = pick.current() {
                                    chosen = Some((c.path, true));
                                }
                                close = true;
                            }
                            // ß is what macOS sends for ⌥+s when Option is not
                            // mapped to Meta, the same fallback the rest of
                            // the chords carry.
                            KeyCode::Char('\u{df}') => {
                                if let Some(c) = pick.current() {
                                    chosen = Some((c.path, true));
                                }
                                close = true;
                            }
                            KeyCode::Char(c)
                                if !alt && !ke.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                pick.query.push(c);
                                pick.sel = 0;
                            }
                            _ => {}
                        }
                        if close {
                            dir_pick = None;
                        }
                        if let Some((path, save)) = chosen {
                            spawn_dir = Some(path.clone());
                            let shown = crate::spawndir::tilde(&path);
                            reload_note_anchor = None;
                            reload_note = Some(if save {
                                match write_agent_dir(&cfg_path, &shown) {
                                    Ok(()) => format!("spawn dir: {shown} (saved to config)"),
                                    Err(e) => format!(
                                        "spawn dir: {shown} (this session; save error: {e})"
                                    ),
                                }
                            } else {
                                format!("spawn dir: {shown} — new panes start here")
                            });
                            reload_note_until = Some(Instant::now() + NOTE_LINGER);
                            // A config write bumps the mtime; adopt it now so
                            // the reload watcher does not report our own save
                            // back to us as an external edit.
                            cfg_mtime = Config::mtime(&cfg_path);
                        }
                        dirty = true;
                        continue;
                    }
                    // While the theme picker is open it owns the keyboard:
                    // arrows/hjkl step through presets, Enter keeps the
                    // choice, Escape restores what was there before. Without
                    // this the keys would reach the focused pane instead.
                    if let Some(sel) = theme_pick {
                        let n = Palette::NAMES.len();
                        let mut close: Option<bool> = None; // Some(keep?)
                        let mut next = sel;
                        match ke.code {
                            KeyCode::Left | KeyCode::Up => next = (sel + n - 1) % n,
                            KeyCode::Right | KeyCode::Down => next = (sel + 1) % n,
                            KeyCode::Char('h') | KeyCode::Char('k') => next = (sel + n - 1) % n,
                            KeyCode::Char('l') | KeyCode::Char('j') => next = (sel + 1) % n,
                            KeyCode::Char('\u{2d9}') | KeyCode::Char('\u{2da}') => {
                                next = (sel + n - 1) % n
                            }
                            KeyCode::Char('\u{ac}') | KeyCode::Char('\u{2206}') => {
                                next = (sel + 1) % n
                            }
                            KeyCode::Enter => close = Some(true),
                            KeyCode::Esc => close = Some(false),
                            KeyCode::Char('\u{2020}') => close = Some(true),
                            KeyCode::Char('t') if ke.modifiers.contains(KeyModifiers::ALT) => {
                                close = Some(true)
                            }
                            _ => {}
                        }
                        match close {
                            Some(true) => {
                                theme_pick = None;
                                reload_note_anchor = None;
                                reload_note = Some(format!(
                                    "theme: {} — add `theme = \"{}\"` to keep it",
                                    Palette::NAMES[sel],
                                    Palette::NAMES[sel]
                                ));
                                reload_note_until = Some(Instant::now() + NOTE_LINGER);
                            }
                            Some(false) => {
                                // Restore the configured theme: the preview
                                // never touched the config file.
                                theme_pick = None;
                                pal = cfg.palette();
                            }
                            None => {
                                if next != sel {
                                    theme_pick = Some(next);
                                    pal = Palette::preset(Palette::NAMES[next]).unwrap_or_default();
                                }
                            }
                        }
                        dirty = true;
                        continue;
                    }
                    if let Some(cmd) = handle_key(&ke) {
                        // Any command other than another digit ends the number
                        // being typed, the way a non-count key ends a vi count.
                        // The pending jump commits first, so `⌥+1 2` then
                        // `⌥+s` lands the split on column 12, not on wherever
                        // focus happened to be.
                        if !matches!(cmd, Cmd::JumpDigit(_)) {
                            if let Some(n) = jump.take() {
                                let v = Viewport::new(cols);
                                let f = FollowScroll {
                                    margin: cfg.scroll_margin,
                                    center: cfg.center_focus,
                                };
                                let _ = layout.apply(Action::JumpToColumn(n), v, f);
                                dirty = true;
                            }
                        }
                        match cmd {
                            Cmd::JumpDigit(d) => {
                                jump.push(d, Instant::now());
                                dirty = true;
                            }
                            // Arm the disclaimer rather than exiting: the
                            // second press (handled above) is the one that
                            // actually kills every pane.
                            Cmd::Quit => {
                                quit_confirm = true;
                                dirty = true;
                            }
                            Cmd::ToggleHud => {
                                hud_active = !hud_was_active;
                                dirty = true;
                            }
                            Cmd::DirPick => {
                                // Rebuilt on every open: repos are cloned and
                                // deleted while gwae runs, and the scan is a
                                // handful of readdirs.
                                let all = crate::spawndir::candidates(
                                    spawn_dir.as_deref(),
                                    &cfg.agent_dir,
                                    &cfg.agent_dirs,
                                    &cfg.agent_dir_roots,
                                );
                                dir_pick = Some(DirPicker {
                                    all,
                                    query: String::new(),
                                    sel: 0,
                                });
                                dirty = true;
                            }
                            Cmd::ThemePick(_) => {
                                // Open on the currently configured theme when
                                // it is a known preset, so stepping starts
                                // from what the user is actually looking at.
                                let cur = Palette::NAMES
                                    .iter()
                                    .position(|n| Palette::preset(n) == Some(pal))
                                    .unwrap_or(0);
                                theme_pick = Some(cur);
                                pal = Palette::preset(Palette::NAMES[cur]).unwrap_or_default();
                                dirty = true;
                            }
                            Cmd::Scroll(d) => {
                                let v = Viewport::new(cols);
                                let _ = layout.apply(
                                    Action::ScrollViewport(d),
                                    v,
                                    FollowScroll::default(),
                                );
                                dirty = true;
                            }
                            Cmd::ScrollBack(d) => {
                                if let Some(pid) = focused_pane(&layout) {
                                    if let Some(p) = panes.get_mut(&pid) {
                                        // A full-screen app (vim, less) owns
                                        // its own scrolling and has no
                                        // scrollback of ours to move, so send
                                        // it the arrow keys it expects instead.
                                        if p.grid.alternate_screen() {
                                            let key: &[u8] =
                                                if d > 0 { b"\x1b[A" } else { b"\x1b[B" };
                                            for _ in 0..d.abs().min(20) {
                                                let _ = p.writer.write_all(key);
                                            }
                                            let _ = p.writer.flush();
                                        } else if p.grid.scroll_by(d) {
                                            dirty = true;
                                        }
                                    }
                                }
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
                                // so gwae exits instead of resurrecting a
                                // fresh default layout.
                                if a == Action::KillPane && layout_pane_count(&layout) <= 1 {
                                    break 'main;
                                }
                                let _ = layout.apply(a, v, f);
                                // A spawn-agent verb ends focused on the new
                                // column just right of the previous focus; mark its pane so sync spawns
                                // the agent harness rather than a shell.
                                if matches!(a, Action::SpawnAgent | Action::SpawnAgentRow) {
                                    if let Some(pid) = focused_pane(&layout) {
                                        agent_panes.insert(pid);
                                    }
                                }
                                if let Err(e) = sync_panes(
                                    &mut layout,
                                    &mut panes,
                                    &tx,
                                    0,
                                    &agent_panes,
                                    spawn_dir.as_deref(),
                                ) {
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
                            Cmd::Paste => {
                                // Explicit clipboard paste. gwae reads the
                                // clipboard itself here rather than relying on
                                // the host to bracket a ⌘/Ctrl+V, so this is
                                // the route that works when the terminal has
                                // no bracketed-paste support at all.
                                //
                                // A pending confirmation means this ⌥+v is the
                                // second one: paste what was held, no questions.
                                let confirmed = paste_confirm
                                    .take()
                                    .filter(|(_, until)| Instant::now() < *until)
                                    .map(|(t, _)| t);
                                let (note, anchor) = match confirmed {
                                    Some(text) => paste_into_focused(
                                        &layout, &mut panes, &cfg, cols, rows, &text,
                                    ),
                                    None => match select::read_clipboard() {
                                        None => (Some("clipboard unreadable".to_string()), None),
                                        Some(text) => {
                                            let lines = text.lines().count();
                                            if lines > PASTE_CONFIRM_LINES
                                                || text.len() > PASTE_CONFIRM_BYTES
                                            {
                                                let n = format!(
                                                    "paste {lines} lines? {} again",
                                                    crate::keys::chord("v")
                                                );
                                                paste_confirm =
                                                    Some((text, Instant::now() + NOTE_LINGER));
                                                (Some(n), None)
                                            } else {
                                                paste_into_focused(
                                                    &layout, &mut panes, &cfg, cols, rows, &text,
                                                )
                                            }
                                        }
                                    },
                                };
                                if note.is_some() {
                                    reload_note = note;
                                    reload_note_anchor = anchor;
                                    reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                }
                                dirty = true;
                            }
                            Cmd::Copy => {
                                // Keyboard copy. Scope is contextual: a
                                // finished drag-selection if there is one
                                // (copying it again is what the user means by
                                // pressing copy *after* selecting), else the
                                // visible pane.
                                let (note, anchor) =
                                    copy_from_focused(&layout, &panes, &cfg, cols, rows, selection);
                                reload_note = Some(note);
                                reload_note_anchor = anchor;
                                reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                dirty = true;
                            }
                            Cmd::None => {}
                        }
                    }
                }
                Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Release => {
                    if is_alt_modifier(&ke) && bare_alt_held {
                        // Releasing the modifier ends the chord, so a pending
                        // `⌥+<number>` commits here: this is the whole point
                        // of accumulating, and it is what makes columns past 9
                        // addressable at all.
                        if let Some(n) = jump.take() {
                            let v = Viewport::new(cols);
                            let f = FollowScroll {
                                margin: cfg.scroll_margin,
                                center: cfg.center_focus,
                            };
                            let _ = layout.apply(Action::JumpToColumn(n), v, f);
                        }
                        bare_alt_held = false;
                        // Bare release means the physical key is up — drop the
                        // fallback window too so a preceding Alt+hjkl chord
                        // doesn't linger after the hold is gone.
                        chord_alt_until = None;
                        dirty = true;
                    } else if ke.modifiers.contains(KeyModifiers::ALT) || is_alt_modifier(&ke) {
                        // Keep chord hold alive until its timeout expires.
                    }
                }
                Ok(Event::Mouse(me)) => {
                    // The ⌥-hold dashboard is a control surface, not a
                    // picture: while it is up, a click on a tile focuses that
                    // pane and a click anywhere else on the panel is
                    // swallowed, so the box never leaks a selection drag into
                    // the pane it is covering.
                    if let Some(plan) = &hud_plan {
                        let r = plan.rect;
                        let on_panel = me.column >= r.x
                            && me.row >= r.y
                            && me.column < r.x.saturating_add(r.w)
                            && me.row < r.y.saturating_add(r.h);
                        if on_panel {
                            if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) {
                                if let Some(pid) = hud_pane_at(plan, me.column, me.row) {
                                    if focused_pane(&layout) != Some(pid) {
                                        let v = Viewport::new(cols);
                                        let f = FollowScroll {
                                            margin: cfg.scroll_margin,
                                            center: cfg.center_focus,
                                        };
                                        let _ = layout.apply(Action::FocusPane(pid), v, f);
                                        dirty = true;
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    let chrome = chrome_rows(&cfg);
                    let views = focused_pane_views_with_chrome(
                        &layout,
                        cols,
                        rows,
                        cfg.content_width,
                        &panes,
                        true,
                        chrome,
                    );
                    // A drag that wanders outside the pane (or off-screen)
                    // must still extend and finish the selection, exactly as
                    // it does in a browser or a native terminal. So resolve
                    // the target against the *owning* pane of a live drag
                    // first, clamping the point to that pane's rect, and only
                    // fall back to "whatever pane is under the cursor" when no
                    // drag is in flight.
                    let mut handled = false;
                    let hit = selection
                        .filter(|s| s.dragging)
                        .and_then(|s| clamped_pane_point(&views, s.pane, me.column, me.row))
                        .or_else(|| pane_at(&views, me.column, me.row));
                    if let Some((pid, gx, gy)) = hit {
                        let child_wants_mouse = panes
                            .get(&pid)
                            .map(|p| p.grid.wants_mouse())
                            .unwrap_or(false);
                        if mouse_role(me.kind, me.modifiers, child_wants_mouse) == MouseRole::Select
                        {
                            let point = select::Point::new(gx, gy);
                            match me.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    // Press arms a selection but shows nothing
                                    // yet: a plain click must clear the old
                                    // highlight, not paint a one-cell one.
                                    if selection.is_some() {
                                        dirty = true;
                                    }
                                    selection = Some(Selection {
                                        pane: pid,
                                        anchor: point,
                                        cursor: point,
                                        dragging: true,
                                    });
                                    // Clicking a pane still focuses it.
                                    if focused_pane(&layout) != Some(pid) {
                                        let v = Viewport::new(cols);
                                        let f = FollowScroll {
                                            margin: cfg.scroll_margin,
                                            center: cfg.center_focus,
                                        };
                                        let _ = layout.apply(Action::FocusPane(pid), v, f);
                                        dirty = true;
                                    }
                                }
                                MouseEventKind::Drag(MouseButton::Left) => {
                                    if let Some(s) = selection.as_mut() {
                                        if s.dragging && s.cursor != point {
                                            s.cursor = point;
                                            dirty = true;
                                        }
                                    }
                                }
                                MouseEventKind::Up(MouseButton::Left) => {
                                    if let Some(s) = selection.as_mut() {
                                        s.cursor = point;
                                        s.dragging = false;
                                    }
                                    // A press+release without movement is a
                                    // plain click, not a selection: drop it so
                                    // no stray highlight lingers and nothing
                                    // overwrites the user's clipboard.
                                    let done = selection.filter(|s| !s.is_empty());
                                    match done {
                                        Some(s) => {
                                            let text = panes
                                                .get(&s.pane)
                                                .map(|p| select::selected_text(&p.grid, &s))
                                                .unwrap_or_default();
                                            let copied = select::copy_to_clipboard(&text);
                                            reload_note = Some(if copied {
                                                copy_note(&text)
                                            } else {
                                                "clipboard unavailable".to_string()
                                            });
                                            reload_note_anchor = views
                                                .iter()
                                                .find(|v| v.pid == s.pane)
                                                .map(|v| v.rect);
                                            reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                        }
                                        None => selection = None,
                                    }
                                    dirty = true;
                                }
                                _ => {}
                            }
                            handled = true;
                        }
                    }
                    if let Some((pid, gx, gy)) = (!handled)
                        .then(|| pane_at(&views, me.column, me.row))
                        .flatten()
                    {
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
                            // A child that asked for mouse reporting owns the
                            // event, translated into its own grid coordinates,
                            // so vim/less/an agent TUI behave exactly as they
                            // would natively. gwae claims no wheel of its
                            // own: scrollback is `⌥+↑/↓` (see `handle_key`).
                            if p.grid.wants_mouse() {
                                if let Some(bytes) = sgr_mouse_report(&me, gx, gy) {
                                    let _ = p.writer.write_all(&bytes);
                                    let _ = p.writer.flush();
                                }
                            }
                        }
                    }
                }
                Ok(Event::Paste(text)) => {
                    // The host bracketed a paste for us (⌘/Ctrl+V). Hand the
                    // whole payload to the focused pane in one delivery
                    // instead of letting it arrive as N keystrokes, which is
                    // what used to submit a multi-line paste line by line.
                    // A finished selection is dismissed the way typing does.
                    if selection.take_if(|s| !s.dragging).is_some() {
                        dirty = true;
                    }
                    if let Some(pid) = focused_pane(&layout) {
                        // Resolve the toast's anchor rect *before* taking the
                        // pane mutably: the view list borrows `panes`.
                        let anchor = focused_pane_views_with_chrome(
                            &layout,
                            cols,
                            rows,
                            cfg.content_width,
                            &panes,
                            true,
                            chrome_rows(&cfg),
                        )
                        .iter()
                        .find(|v| v.pid == pid)
                        .map(|v| v.rect);
                        if let Some(p) = panes.get_mut(&pid) {
                            let lines = text.lines().count();
                            let bracketed = p.grid.wants_bracketed_paste();
                            if write_paste(p, &text) > 0 && lines > 1 {
                                // Say so only when it is the case a user can
                                // get wrong: a multi-line paste is the one
                                // that used to run commands on its own.
                                reload_note = Some(paste_note(&text, bracketed));
                                reload_note_anchor = anchor;
                                reload_note_until = Some(Instant::now() + NOTE_LINGER);
                            }
                            dirty = true;
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
            true,
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
                    let _ = p.master.resize(v.grid_cols, v.grid_rows);
                    dirty = true;
                }
            }
        }

        // Treat gwae as an invisible layer for the host title bar: mirror the
        // focused pane's inner title (set via OSC 0/2 by e.g. jcode) out to the
        // host terminal, so switching panes updates the outer window/status bar
        // to the pane you're actually looking at instead of "gwae". Fall back
        // to a plain gwae label when the focused pane has set no title.
        let effective = focused_pane(&layout)
            .and_then(|pid| panes.get(&pid))
            .map(|p| p.grid.title())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| "gwae".to_string());
        if effective != last_title {
            last_title = effective.clone();
            if let Err(e) = emit_title(&mut stdout, &effective) {
                tracing::warn!("set title: {e}");
            }
        }

        // Track attention & alt flip for repaint, plus HUD (persist-until-key).
        let now_for_hud = Instant::now();
        if let Some(t) = chord_alt_until {
            if now_for_hud >= t {
                chord_alt_until = None;
            }
        }
        // Fallback commit for terminals that never report a bare Option
        // release: without this a typed number would sit in the accumulator
        // forever and the jump would simply never happen.
        if let Some(n) = jump.take_if_expired(now_for_hud) {
            let v = Viewport::new(cols);
            let f = FollowScroll {
                margin: cfg.scroll_margin,
                center: cfg.center_focus,
            };
            let _ = layout.apply(Action::JumpToColumn(n), v, f);
            dirty = true;
        }
        let chord_alt_held = chord_alt_until.is_some();
        let effective_alt_held = bare_alt_held || chord_alt_held;
        let cur_has_attention = has_attention(&layout);
        if cur_has_attention != last_has_attention || effective_alt_held != last_alt_held {
            dirty = true;
            last_has_attention = cur_has_attention;
            last_alt_held = effective_alt_held;
        }
        let show_hud = hud_active;
        let show_center_minimap = effective_alt_held && !hud_active && cfg.minimap.show;
        // Everything the overlay knows beyond the layout: what each pane is,
        // how long it has been silent, and where the two jump keys point.
        // Built only when the panel is actually up, so a normal frame pays
        // nothing for it.
        let hud_facts = if show_center_minimap && !show_hud {
            let now = Instant::now();
            HudFacts {
                titles: panes
                    .iter()
                    .filter_map(|(pid, p)| {
                        let t = short_title(p.grid.title());
                        (!t.is_empty()).then_some((*pid, t))
                    })
                    .collect(),
                quiet: panes
                    .iter()
                    .map(|(pid, p)| (*pid, now.saturating_duration_since(p.last_output)))
                    .collect(),
                jump_target: smart_jump_target(&layout),
                pending_jump: jump.pending(),
            }
        } else {
            HudFacts::default()
        };
        // Geometry is planned once: the scrim, the panel, and click-to-focus
        // all read the same plan, so a click can never land on a tile the
        // paint put somewhere else.
        hud_plan = (show_center_minimap && !show_hud)
            .then(|| plan_center_minimap(cols, rows, &layout, &cfg.minimap))
            .flatten();
        if dirty {
            render_frame(
                &mut frame,
                &layout,
                &mut panes,
                cols,
                rows,
                cfg.content_width,
                &pal,
                &cfg.minimap,
                &cfg.cowsay,
                cfg.cell_labels,
                selection.as_ref(),
            );
            if show_hud {
                draw_center_hud(&mut frame, cols, rows, &pal);
            }
            if show_center_minimap && !show_hud {
                if let Some(plan) = &hud_plan {
                    // Scrim first, panel second: dimming the session behind
                    // the box is what makes the reveal unmissable over busy
                    // output.
                    dim_behind(&mut frame, cols, rows, plan.rect);
                    paint_center_minimap(&mut frame, cols, rows, &layout, plan, &pal, &hud_facts);
                }
            }
            if let Some(sel) = theme_pick {
                draw_theme_picker(&mut frame, cols, rows, sel, &pal);
            }
            if let Some(pick) = &dir_pick {
                draw_dir_picker(&mut frame, cols, rows, pick, &pal);
            }
            // Echo the number as it is typed. Without this, a multi-digit
            // jump is invisible until it commits and `⌥+1 2` is
            // indistinguishable from a dropped keystroke.
            if let Some(n) = jump.pending() {
                draw_toast(
                    &mut frame,
                    cols,
                    rows,
                    &format!("{} → column {}", crate::keys::mod_key(), n),
                    &pal,
                    true,
                );
            }
            if let Some(note) = &reload_note {
                let ok = !note.contains("error") && !note.starts_with("unknown theme");
                draw_toast_at(&mut frame, cols, rows, note, &pal, ok, reload_note_anchor);
            }
            // Topmost: the destructive confirmation must never be obscured by
            // chrome that happens to be showing when the chord is pressed.
            if quit_confirm {
                draw_quit_confirm(&mut frame, cols, rows, layout_pane_count(&layout), &pal);
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
        kill_pane_tree(&mut p.child);
    }
    // Anything registered but no longer in `panes` (a pane dropped from the
    // map without going through `kill_pane_tree`) is caught here, so the
    // process leaves nothing behind.
    crate::reap::reap_all();
    restore_terminal(&mut stdout, kitty_keyboard);
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

/// Resolve `(x, y)` inside `pid`'s view, clamping a point that has wandered
/// outside the pane's rect to its nearest edge cell.
///
/// This is what makes a drag that leaves the pane behave like a native
/// selection: dragging off the right edge selects to end of line, dragging
/// below the last row selects to the bottom, instead of the selection simply
/// freezing at the last in-bounds position.
fn clamped_pane_point(
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

/// The toast shown after a successful drag-copy: how much was taken, in the
/// unit the user was actually thinking in (lines when multi-line, characters
/// otherwise).
fn copy_note(text: &str) -> String {
    let lines = text.lines().count();
    if lines > 1 {
        format!("copied {lines} lines")
    } else {
        let n = text.chars().count();
        format!("copied {n} char{}", if n == 1 { "" } else { "s" })
    }
}

/// The toast shown after a multi-line paste. A one-line paste is silent: it
/// behaves exactly like typing and needs no narration.
///
/// A multi-line paste is the case that used to run each line as its own
/// command, so gwae says what it delivered. When the child never asked for
/// bracketed paste (`bracketed` false) those newlines genuinely are Returns —
/// nothing can prevent that, it is what the program asked for — so the toast
/// says so rather than letting the user infer safety from silence.
fn paste_note(text: &str, bracketed: bool) -> String {
    let lines = text.lines().count();
    if bracketed {
        format!("pasted {lines} lines")
    } else {
        format!("pasted {lines} lines · no bracket, newlines run")
    }
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

    /// A palette with a distinctive accent, everything else Mocha. Render
    /// tests assert on the accent to prove focus chrome is drawn, so the
    /// remaining colors just need to be stable.
    fn pal_accent(accent: CColor) -> Palette {
        Palette {
            accent,
            ..Palette::default()
        }
    }

    /// A palette built from the explicit colors a pre-theme render test used
    /// to pass positionally: background, focus accent, and skeleton overlay.
    fn pal_of(base: CColor, accent: CColor, overlay: CColor) -> Palette {
        Palette {
            base,
            accent,
            overlay,
            ..Palette::default()
        }
    }

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
    fn handle_key_option_shift_semicolon_spawns_an_agent_row() {
        // macOS glyph fallback: Option+Shift+; is Ú.
        let ev = KeyEvent::new(KeyCode::Char('\u{da}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
        // Option-as-Meta: ESC+':' arrives as Alt+':' (shifted codepoint, and
        // some terminals also set the Shift bit).
        let ev = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
        let ev = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
    }

    #[test]
    fn option_a_and_option_x_belong_to_the_pane_again() {
        // Both bindings were removed: gwae must not swallow them, so the
        // focused pane sees the chord (jcode and vim bind ⌥+a / ⌥+x).
        for c in ['a', 'x'] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(key_bytes(&ev))),
                "⌥+{c} should be forwarded, not claimed"
            );
        }
    }

    #[test]
    fn alt_up_down_is_the_keyboard_route_into_scrollback() {
        // gwae no longer claims the wheel, so this is the *only* way to
        // read back through a pane's history. If it regressed, scrollback
        // would exist with nothing able to reach it.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(3)),
            "no way back into scrollback"
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(-3))
        );
        // Shift and PageUp/PageDown take a bigger bite.
        assert_eq!(
            handle_key(&KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::ALT | KeyModifiers::SHIFT
            )),
            Some(Cmd::ScrollBack(20))
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(20))
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::PageDown, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(-20))
        );
        // Without Alt these are ordinary keys the program inside the pane
        // owns: stealing a bare Up arrow would break every shell's history.
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
        // And the horizontal pan it sits next to still works.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            Some(Cmd::ScrollPane(-1))
        );
    }

    /// Typing must never be slowed down by the idle backoff: while anything
    /// is happening the loop polls at exactly the configured rate.
    #[test]
    fn a_busy_session_polls_at_the_configured_rate() {
        for ms in [1u64, 2, 8, 50] {
            assert_eq!(
                input_poll_interval(ms, Duration::from_millis(0)),
                Duration::from_millis(ms),
                "fresh activity at {ms}ms"
            );
            // Still tight just before the idle threshold.
            assert_eq!(
                input_poll_interval(ms, IDLE_AFTER - Duration::from_millis(1)),
                Duration::from_millis(ms)
            );
        }
    }

    /// The bug this guards: at the default 2 ms the loop woke 500x a second
    /// forever, burning ~3% of a core on a mux nobody was touching. Once the
    /// screen is quiet the wakeups must drop by more than an order of
    /// magnitude.
    #[test]
    fn an_idle_session_backs_off_to_far_fewer_wakeups() {
        let idle = input_poll_interval(2, IDLE_AFTER);
        assert_eq!(idle, Duration::from_millis(IDLE_POLL_MS));
        let busy = input_poll_interval(2, Duration::from_millis(0));
        assert!(
            idle.as_micros() >= busy.as_micros() * 10,
            "idle backoff {idle:?} must be >=10x the busy interval {busy:?}"
        );
        // Still fast enough to repaint within one frame at 30 fps.
        assert!(idle <= Duration::from_millis(33), "{idle:?} too sluggish");
    }

    /// A user who deliberately configured a *slower* poll than the idle
    /// backoff keeps their setting: the backoff is a floor, never a ceiling,
    /// so it can only ever reduce wakeups.
    #[test]
    fn the_backoff_never_polls_more_often_than_configured() {
        let cfg = 50;
        assert_eq!(
            input_poll_interval(cfg, Duration::from_secs(60)),
            Duration::from_millis(cfg)
        );
        // Out-of-range config values are still clamped the way they were.
        assert_eq!(
            input_poll_interval(0, Duration::from_millis(0)),
            Duration::from_millis(1)
        );
        assert_eq!(
            input_poll_interval(9999, Duration::from_millis(0)),
            Duration::from_millis(50)
        );
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
        // Same with Alt+Shift: Caps should not fake Shift+hjkl either.
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('H'),
            KeyModifiers::ALT,
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
        let ev = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::MovePaneUp)));
    }

    #[test]
    fn alt_shift_letter_is_forwarded_not_swallowed_as_the_unshifted_chord() {
        // Regression: ⌥+Shift+s used to case-fold to 's' and split the column,
        // stealing a chord the focused pane owns (jcode copies with ⌥+Shift+s).
        // Both encodings a terminal may send must reach the pane as ESC+'S'.
        for ev in [
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::ALT | KeyModifiers::SHIFT),
            // Kitty REPORT_ALTERNATE_KEYS consumes the shift bit.
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::ALT),
            // Terminals that keep the unshifted codepoint plus a SHIFT bit.
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        ] {
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(b"\x1bS".to_vec())),
                "{ev:?} should be forwarded to the pane"
            );
        }
        // The unshifted chord still splits.
        let ev = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SplitBelow)));
        // Caps Lock is not Shift: ⌥+CapsLock+s must still split.
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('S'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SplitBelow)));
        // The two intentional shifted chords keep working.
        let ev = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Quit));
    }

    #[test]
    fn shift_and_caps_typed_text_passes_through_as_shifted() {
        // Plain Shift+a -> 'A' is pane input, not a gwae chord.
        let ev = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
        // Shift+1 -> '!' via the shifted codepoint path.
        let ev = KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"!".to_vec())));
        // Caps+a also produces 'A' (caps state) but should still type 'A' when
        // not an Alt chord, just like Shift. Focus test is plain key:
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('A'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
    }

    #[test]
    fn bare_modifier_never_sends_input_to_pane() {
        // With the Kitty keyboard protocol a lone Option press arrives as
        // `Modifier(LeftAlt)` with the Alt bit set. The old fallthrough
        // treated it as Meta+<nothing> -> a bare ESC, which clears jcode's
        // line editor (e.g. pressing Option to poll the HUD erased the
        // input line). Every pure modifier press/release must be a no-op.
        for code in [
            KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            KeyCode::Modifier(ModifierKeyCode::RightAlt),
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyCode::Modifier(ModifierKeyCode::RightShift),
            KeyCode::Modifier(ModifierKeyCode::LeftControl),
            KeyCode::Modifier(ModifierKeyCode::RightControl),
            KeyCode::Modifier(ModifierKeyCode::LeftSuper),
            KeyCode::Modifier(ModifierKeyCode::RightSuper),
        ] {
            let mods = if matches!(
                code,
                KeyCode::Modifier(ModifierKeyCode::LeftAlt)
                    | KeyCode::Modifier(ModifierKeyCode::RightAlt)
            ) {
                KeyModifiers::ALT
            } else {
                KeyModifiers::NONE
            };
            let ev = KeyEvent::new(code, mods);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::None),
                "bare modifier {code:?} must not become pane input"
            );
        }
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
        let hl = gwae_term::Style {
            bg: CColor::Idx(238),
            ..gwae_term::Style::default()
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
        use gwae_layout::{Preset, Width};
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
    fn drag_outside_a_pane_clamps_to_its_edges() {
        use gwae_layout::{Preset, Width};
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
        let left = views[0].pid;
        let r = views[0].rect;
        // Inside the pane the clamp is a no-op: same answer as `pane_at`.
        assert_eq!(clamped_pane_point(&views, left, 5, 3), Some((left, 5, 3)));
        // Dragging right, past the pane into its neighbour, still extends the
        // left pane's selection to its last column instead of freezing.
        let (pid, gx, gy) = clamped_pane_point(&views, left, 200, 3).unwrap();
        assert_eq!(pid, left);
        assert_eq!(gx, r.w - 1);
        assert_eq!(gy, 3);
        // Dragging below the last row clamps to the bottom row.
        let (_, _, gy) = clamped_pane_point(&views, left, 5, 200).unwrap();
        assert_eq!(gy, r.h - 1);
        // Dragging above/left of the pane clamps to its first cell.
        assert_eq!(clamped_pane_point(&views, left, 0, 0), Some((left, 0, 0)));
        // A pane that is not on screen cannot be resolved at all.
        let gone: PaneId = 9999;
        assert_eq!(clamped_pane_point(&views, gone, 5, 3), None);
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
        // The wheel is never a selection: gwae resolves it locally, which
        // now means it does nothing unless the child asked for reporting.
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, plain, false),
            MouseRole::Local
        );
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, plain, true),
            MouseRole::Forward
        );
        // Shift+wheel is still the child's business when it reports mouse.
        assert_eq!(
            mouse_role(MouseEventKind::ScrollUp, KeyModifiers::SHIFT, true),
            MouseRole::Forward
        );
        // Right-drag is not our selection either.
        assert_eq!(
            mouse_role(MouseEventKind::Drag(MouseButton::Right), plain, false),
            MouseRole::Local
        );
    }

    #[test]
    fn selection_highlight_inverts_exactly_the_dragged_cells() {
        let layout = Layout::new(1);
        let pid = *layout
            .focused_row()
            .and_then(|r| r.columns.first())
            .and_then(|c| c.panes.first())
            .unwrap();
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let (tx, _rx) = channel::<PaneMsg>();
        let mut pane = spawn_pane(pid, "sleep 30", 80, 24, tx, None).expect("spawn pane");
        pane.grid.feed(b"hello world\r\nsecond line");
        panes.insert(pid, pane);
        let (cols, rows) = (80u16, 24u16);
        let sel = Selection {
            pane: pid,
            anchor: select::Point::new(0, 0),
            cursor: select::Point::new(4, 0),
            dragging: true,
        };
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &Palette::default(),
            &no_map(),
            &no_cow(),
            false,
            Some(&sel),
        );
        // Content is inset 1 cell inside the column frame, so grid (0,0)
        // lands at screen (1,1).
        let at = |x: u16, y: u16| out[(y + 1) as usize * cols as usize + (x + 1) as usize];
        // "hello" is inverted, both ends inclusive; the space after is not.
        for x in 0..=4u16 {
            assert!(at(x, 0).style.inverse, "cell {x} should be highlighted");
        }
        assert!(
            !at(5, 0).style.inverse,
            "past the drag end, not highlighted"
        );
        assert!(!at(0, 1).style.inverse, "other rows untouched");
        // The text itself is unchanged: highlighting only restyles.
        assert_eq!(at(0, 0).ch, 'h');
        assert_eq!(at(4, 0).ch, 'o');
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
        draw_toast(&mut frame, cols, rows, "hi", &Palette::default(), true);
        assert_eq!(frame[9 * cols as usize + 1].ch, 'h');
    }

    #[test]
    fn copy_and_paste_are_symmetric_chords_on_both_input_paths() {
        // ⌥+c / ⌥+v are the pair everyone already knows from their OS. Both
        // routes must reach them: Option-as-Meta (ESC+letter) and the macOS
        // Unicode glyph a terminal sends when Option is *not* mapped to Meta.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT)),
            Some(Cmd::Copy)
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)),
            Some(Cmd::Paste)
        );
        // ç and √ are what macOS emits for Option+c / Option+v.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('\u{e7}'), KeyModifiers::NONE)),
            Some(Cmd::Copy)
        );
        assert_eq!(
            handle_key(&KeyEvent::new(
                KeyCode::Char('\u{221a}'),
                KeyModifiers::NONE
            )),
            Some(Cmd::Paste)
        );
        // ⌥+y is the vi-flavoured alias for copy, inherited from the roadmap's
        // yank entry so that muscle memory is not wasted.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT)),
            Some(Cmd::Copy)
        );
        // Plain c/v are ordinary text the focused pane owns: a mux that ate
        // them would break typing "cv" in every pane.
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
    }

    #[test]
    fn paste_note_warns_when_the_child_cannot_bracket() {
        // Silence would imply safety. When the child never enabled DECSET
        // 2004 those newlines really do submit, and the toast has to say so.
        assert_eq!(paste_note("a\nb\nc", true), "pasted 3 lines");
        assert!(paste_note("a\nb\nc", false).contains("newlines run"));
    }

    #[test]
    fn copy_note_counts_lines_or_characters() {
        assert_eq!(copy_note("hello"), "copied 5 chars");
        assert_eq!(copy_note("x"), "copied 1 char");
        assert_eq!(copy_note("a\nb\nc"), "copied 3 lines");
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
        use gwae_layout::Width;
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
            &pal_accent(accent),
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
            &pal_accent(accent),
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
    fn hud_and_center_minimap_panels_follow_the_theme() {
        // The HUD and centered minimap are the only chrome that uses
        // `surface` and `text`, and they are reachable only while holding
        // Option, so no render test above covers them. Assert both panels
        // paint the theme's colors and leak none of Mocha's.
        let mut layout = Layout::default();
        let r2 = layout.new_row("two".to_string());
        let p = layout.alloc_pane();
        layout.add_column(r2, gwae_layout::Width::Cells(20), vec![p]);
        let nord = Palette::NORD;
        let mocha = Palette::CATPPUCCIN_MOCHA;
        let (cols, rows) = (80u16, 24u16);

        for (what, draw) in [("hud", 0), ("center minimap", 1)] {
            let mut out = vec![Cell::default(); cols as usize * rows as usize];
            if draw == 0 {
                draw_center_hud(&mut out, cols, rows, &nord);
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
                    &nord,
                    &HudFacts::default(),
                );
            }
            assert!(
                out.iter().any(|c| c.style.bg == nord.surface),
                "{what} panel should be filled with the theme's surface"
            );
            assert!(
                out.iter().any(|c| c.style.fg == nord.text),
                "{what} text should use the theme's text color"
            );
            assert!(
                !out.iter().any(|c| c.style.bg == mocha.surface),
                "{what} leaked the Mocha surface"
            );
            assert!(
                !out.iter().any(|c| c.style.fg == mocha.text),
                "{what} leaked the Mocha text color"
            );
        }
    }

    #[test]
    fn minimap_status_tints_follow_the_theme() {
        // The sibling test above pins the *default* status tints. This one
        // proves they are not merely defaults hiding behind the palette: with
        // a non-default theme, every tile must be that theme's status color,
        // muted, and none of Mocha's may appear.
        use gwae_layout::Width;
        let mut layout = Layout::default(); // 4 quarter panes on strip 1
        let r2 = layout.new_row("two".to_string());
        let p = layout.alloc_pane();
        layout.add_column(r2, Width::Cells(20), vec![p]);
        let ids: Vec<PaneId> = {
            let row = layout.rows[0].clone();
            row.columns.iter().flat_map(|c| c.panes.clone()).collect()
        };
        layout.panes.get_mut(&ids[1]).unwrap().status = PaneStatus::Done;
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Failed;
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Idle;

        let nord = Palette::NORD;
        let cols = 40usize;
        let mut out = vec![Cell::default(); cols * 8];
        draw_minimap(
            &mut out,
            cols as u16,
            8,
            &layout,
            &crate::config::Minimap::default(),
            &nord,
        );
        let cell = |x: usize, y: usize| out[y * cols + x];
        let (ox, y) = (8usize, 6usize);
        assert_eq!(
            cell(ox, y).style.bg,
            nord.accent,
            "focused tile uses accent"
        );
        assert_eq!(
            cell(ox + 8, y).style.bg,
            Palette::muted(nord.done),
            "done tile uses the theme's done tint"
        );
        assert_eq!(
            cell(ox + 16, y).style.bg,
            Palette::muted(nord.failed),
            "failed tile uses the theme's failed tint"
        );
        assert_eq!(
            cell(ox + 24, y).style.bg,
            Palette::muted(nord.idle),
            "idle tile uses the theme's idle tint"
        );
        // No Catppuccin Mocha status tint may survive anywhere on the map.
        let mocha = Palette::CATPPUCCIN_MOCHA;
        for s in [
            PaneStatus::Running,
            PaneStatus::Idle,
            PaneStatus::Done,
            PaneStatus::Failed,
        ] {
            let stale = Palette::muted(mocha.status(s));
            assert!(
                !out.iter().any(|c| c.style.bg == stale),
                "a Mocha {s:?} tint leaked into a Nord minimap"
            );
        }
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
    /// A disabled cow, for tests that assert on placeholder box contents and
    /// predate the cowsay feature. Keeping them cow-free means those
    /// assertions still describe exactly what they did before.
    fn no_cow() -> crate::config::Cowsay {
        crate::config::Cowsay {
            enabled: false,
            messages: Vec::new(),
        }
    }

    fn no_map() -> crate::config::Minimap {
        crate::config::Minimap {
            show: false,
            mode: crate::config::MinimapMode::Overlay,
            ..Default::default()
        }
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
            &no_cow(),
            true,
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
        }
        // Unshared edges of unfocused boxes stay the skeleton color.
        assert_eq!(at(ranges[2].0 as u16, rows / 2).style.fg, white);
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
            &no_cow(),
            true,
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
            &no_cow(),
            true,
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
                &no_cow(),
                true,
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
            &pal_of(CColor::Default, CColor::Rgb(0xff, 0, 0), white),
            &no_map(),
            &no_cow(),
            true,
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
    fn placeholder_boxes_are_not_dimmed_and_show_cell_identifiers() {
        // Empty placeholder boxes are chrome, not panes: their interiors carry
        // the themed backdrop (`theme.base`, the same fill as the rest of the
        // gwae background) rather than punching the terminal's own default
        // background through, and a big block-font `strip.cell` identifier is
        // centered in each.
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
            &pal_of(dim, CColor::Rgb(0xff, 0, 0), CColor::Rgb(0xff, 0xff, 0xff)),
            &no_map(),
            &no_cow(),
            true,
            None,
        );
        let bg = |x: u16, y: u16| out[y as usize * cols as usize + x as usize].style.bg;
        // Placeholder interiors blend with the surrounding backdrop: every
        // interior cell is either the themed base or the identifier's own
        // pixels, never the terminal's default background (which would read
        // as a differently colored rectangle punched into the grid).
        for x in 41..59 {
            for y in 1..rows - 1 {
                let b = bg(x, y);
                assert!(
                    b == dim || b == label,
                    "placeholder interior does not blend at ({x},{y}): {b:?}"
                );
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

    /// Render a 2-column layout and return the placeholder box region as text,
    /// one string per screen row, so cow assertions can just look for the art.
    fn placeholder_rows(cols: u16, rows: u16, cow: &crate::config::Cowsay) -> Vec<String> {
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
            cow,
            true,
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

    /// A full strip has no placeholder to pin to. The user's next empty box
    /// is `n+1.1`, the first cell of the strip below, so the cheat-sheet hint
    /// must appear there rather than being lost until a pane is closed.
    #[test]
    fn a_full_strip_moves_the_pinned_hint_to_the_next_strip() {
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec![
                "PINNED hint".to_string(),
                "filler one".to_string(),
                "filler two".to_string(),
            ],
        };
        // Four panes fill the strip: no empty box remains beside them.
        let mut layout = Layout::new(4);
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let render = |layout: &Layout, panes: &mut HashMap<PaneId, PtyPane>| -> String {
            let mut out = Vec::new();
            render_frame(
                &mut out,
                layout,
                panes,
                160,
                40,
                0,
                &pal_of(
                    CColor::Idx(235),
                    CColor::Rgb(0xff, 0, 0),
                    CColor::Rgb(0xff, 0xff, 0xff),
                ),
                &no_map(),
                &cow,
                true,
                None,
            );
            (0..40)
                .map(|y| {
                    (0..160)
                        .map(|x| out[y as usize * 160 + x as usize].ch)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        // The full strip itself has nowhere to put the cow.
        let full = render(&layout, &mut panes);
        assert!(
            !full.contains("PINNED"),
            "a full strip has no placeholder to pin to:\n{full}"
        );
        // Moving down past the last strip creates an empty one (niri
        // workspace semantics); its first cell is the newly-visible empty box
        // and must carry the pinned hint.
        let _ = layout.apply(
            Action::FocusDown,
            Viewport::new(160),
            FollowScroll::default(),
        );
        let next = render(&layout, &mut panes);
        assert!(
            next.contains("PINNED"),
            "the pinned hint should move to the next strip's first cell:\n{next}"
        );
    }

    #[test]
    fn placeholder_boxes_show_a_cow_when_there_is_room() {
        // A tall, wide-enough box gets the hint cow under its identifier.
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec!["press c for a new pane".to_string()],
        };
        let text = placeholder_rows(120, 24, &cow).join("\n");
        assert!(
            text.contains("^__^"),
            "expected the cow's head in a roomy box:\n{text}"
        );
        assert!(
            text.contains("(oo)"),
            "expected the cow's face in a roomy box:\n{text}"
        );
        assert!(
            text.contains("press c for a new pane"),
            "expected the message in the bubble:\n{text}"
        );
    }

    #[test]
    fn cow_never_displaces_the_cell_identifier() {
        // The identifier is the addressing affordance and must survive: in a
        // box with the cow, the block-font label is still painted.
        let cols: u16 = 120;
        let rows: u16 = 24;
        let layout = Layout::new(2);
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let label = CColor::Rgb(0x58, 0x5b, 0x70);
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec!["hi".to_string()],
        };
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(CColor::Idx(235), CColor::Rgb(0xff, 0, 0), label),
            &no_map(),
            &cow,
            true,
            None,
        );
        let bg = |x: u16, y: u16| out[y as usize * cols as usize + x as usize].style.bg;
        let painted = (61..89u16)
            .flat_map(|x| (1..rows - 1).map(move |y| (x, y)))
            .filter(|&(x, y)| bg(x, y) == label)
            .count();
        assert!(
            painted >= 11,
            "identifier missing from a box with a cow, found {painted} cells"
        );
    }

    #[test]
    fn short_boxes_drop_the_cow_and_keep_the_identifier() {
        // Not enough vertical room for label + spacer + art: degrade to the
        // label alone rather than painting a clipped cow.
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec!["press c for a new pane".to_string()],
        };
        let text = placeholder_rows(120, 10, &cow).join("\n");
        assert!(
            !text.contains("^__^"),
            "a short box must not draw a clipped cow:\n{text}"
        );
    }

    #[test]
    fn narrow_boxes_drop_the_cow() {
        // Quarter of 80 cols = 20-wide boxes, under the cow's fixed 23: the
        // art would be clipped, so it is skipped entirely.
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec!["press c for a new pane".to_string()],
        };
        let text = placeholder_rows(80, 24, &cow).join("\n");
        assert!(
            !text.contains("^__^"),
            "a narrow box must not draw a clipped cow:\n{text}"
        );
    }

    #[test]
    fn cow_art_keeps_the_boxs_background() {
        // Regression: the art was written with a fresh `Style`, so every cow
        // glyph carried `CColor::Default` as its background and the block read
        // as a gray rectangle floating over the themed backdrop. The art must
        // inherit whatever background the box was filled with.
        let cols: u16 = 120;
        let rows: u16 = 24;
        let layout = Layout::new(2);
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        let base = CColor::Idx(235);
        let cow = crate::config::Cowsay {
            enabled: true,
            messages: vec!["moo".to_string()],
        };
        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &pal_of(base, CColor::Rgb(0xff, 0, 0), CColor::Rgb(0xff, 0xff, 0xff)),
            &no_map(),
            &cow,
            true,
            None,
        );
        let art: Vec<Cell> = out
            .iter()
            .copied()
            .filter(|c| matches!(c.ch, '^' | '(' | ')' | 'o' | '_' | 'w' | '|'))
            .collect();
        assert!(!art.is_empty(), "no cow was painted");
        assert!(
            art.iter().all(|c| c.style.bg == base),
            "cow glyphs do not sit on the themed background"
        );
    }

    #[test]
    fn disabled_cow_paints_nothing() {
        let text = placeholder_rows(120, 24, &no_cow()).join("\n");
        assert!(!text.contains("^__^"), "cow drawn while disabled:\n{text}");
    }

    #[test]
    fn cow_is_identical_across_repaints() {
        // The frame differ compares against the previous frame, so an unstable
        // (e.g. randomly chosen) message would force a repaint every frame.
        let cow = crate::config::Cowsay::default();
        let a = placeholder_rows(120, 24, &cow);
        let b = placeholder_rows(120, 24, &cow);
        assert_eq!(a, b, "placeholder cow changed between identical renders");
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

    /// The registry in [`crate::binds`] is only a single source of truth if
    /// the dispatcher agrees with it. Feed every machine-checkable entry
    /// through the real `handle_key` (both the Meta path and, where one
    /// exists, the macOS glyph fallback) and require the advertised effect.
    #[test]
    fn advertised_bindings_match_the_dispatcher() {
        use crate::binds::{Effect, Trigger, BINDS};
        let expect = |e: Effect| -> Option<Cmd> {
            Some(match e {
                Effect::Act(a) => Cmd::Act(a),
                Effect::SmartJump => Cmd::SmartJump,
                Effect::ThemePick => Cmd::ThemePick(0),
                Effect::DirPick => Cmd::DirPick,
                Effect::ToggleHud => Cmd::ToggleHud,
                Effect::Copy => Cmd::Copy,
                Effect::Paste => Cmd::Paste,
                Effect::Quit => Cmd::Quit,
                Effect::Scroll(n) => Cmd::Scroll(n),
                Effect::Unverifiable => return None,
            })
        };
        for b in BINDS {
            if expect(b.effect).is_none() {
                continue;
            }
            let want = expect(b.effect);
            match b.trigger {
                Trigger::Chord(c) => {
                    let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
                    assert_eq!(
                        handle_key(&ev),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                }
                Trigger::ShiftChord(c) => {
                    let ev =
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT);
                    assert_eq!(
                        handle_key(&ev),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                }
                Trigger::EnterChord { shift } => {
                    let mut mods = KeyModifiers::ALT;
                    if shift {
                        mods |= KeyModifiers::SHIFT;
                    }
                    assert_eq!(
                        handle_key(&KeyEvent::new(KeyCode::Enter, mods)),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                    // A *bare* Return (no modifier) belongs to the focused
                    // pane. The cheat-sheet used to label this row `↵`, which
                    // told users to press a key that only types a newline.
                    assert!(
                        matches!(
                            handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                            Some(Cmd::Input(_))
                        ),
                        "bare Return must reach the pane, not the layout"
                    );
                }
                Trigger::ModProse(_) | Trigger::Prose(_) => continue,
            }
            // The macOS glyph fallback must reach the same command, so the
            // cheat-sheet is honest on terminals without "Option as Meta".
            if let Some(g) = b.glyph {
                let mods = if matches!(b.trigger, Trigger::ShiftChord(_)) {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                assert_eq!(
                    handle_key(&KeyEvent::new(KeyCode::Char(g), mods)),
                    expect(b.effect),
                    "glyph {g:?} fallback for {} must match",
                    b.label()
                );
            }
        }
    }

    #[test]
    fn force_quit_chord_arms_a_centered_disclaimer() {
        // The chord itself must still decode as Quit (the run loop turns that
        // into "arm the overlay"), and the overlay must actually paint a
        // centered, framed box that names the cost in the user's own key
        // vocabulary. A quit that exits without this box is the bug.
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Quit));

        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut out = vec![Cell::default(); cols as usize * rows as usize];
        let pal = Palette::default();
        draw_quit_confirm(&mut out, cols, rows, 4, &pal);
        let lines: Vec<String> = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| out[y as usize * cols as usize + x as usize].ch)
                    .collect()
            })
            .collect();
        assert!(
            lines.iter().any(|s| s.contains("force quit gwae?")),
            "disclaimer names the action, got {lines:?}"
        );
        assert!(
            lines.iter().any(|s| s.contains("4 panes will be killed")),
            "disclaimer names the cost, got {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|s| s.contains(&crate::keys::shift_chord("q"))),
            "disclaimer says how to confirm, got {lines:?}"
        );
        assert!(
            lines.iter().any(|s| s.contains("esc cancels")),
            "disclaimer says how to back out, got {lines:?}"
        );
        // Framed and centered: corners exist, and the painted rows sit around
        // the middle of the screen rather than at an edge.
        let painted: Vec<usize> = (0..rows as usize)
            .filter(|y| {
                (0..cols as usize).any(|x| out[y * cols as usize + x].style.bg != CColor::Default)
            })
            .collect();
        assert!(
            out.iter().any(|c| c.ch == '╭') && out.iter().any(|c| c.ch == '╯'),
            "disclaimer is a framed box"
        );
        let mid = rows as usize / 2;
        assert!(
            painted.first().is_some_and(|f| *f < mid) && painted.last().is_some_and(|l| *l > mid),
            "disclaimer straddles the screen center, painted {painted:?}"
        );
        // Singular/plural, because "1 panes" reads like a bug in a warning.
        let mut one = vec![Cell::default(); cols as usize * rows as usize];
        draw_quit_confirm(&mut one, cols, rows, 1, &pal);
        let text: String = one.iter().map(|c| c.ch).collect();
        assert!(
            text.contains("1 pane will be killed"),
            "singular pane count"
        );
        // Too small to render honestly: paint nothing rather than a clipped
        // warning the user cannot read.
        let mut tiny = vec![Cell::default(); 10 * 4];
        draw_quit_confirm(&mut tiny, 10, 4, 3, &pal);
        assert!(
            tiny.iter().all(|c| c.style.bg == CColor::Default),
            "no partial disclaimer may be painted"
        );
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
        draw_center_hud(&mut out, cols, rows, &pal_accent(frame_color));
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
            all.iter().any(|s| s.contains("click")),
            "HUD cheat-sheet covers mouse, got {all:?}"
        );
        // With no attention at all the cheat-sheet is unchanged.
        for pid in &ids {
            layout.panes.get_mut(pid).unwrap().status = PaneStatus::Running;
        }
        let mut out2 = vec![Cell::default(); cols as usize * rows as usize];
        draw_center_hud(&mut out2, cols, rows, &pal_accent(frame_color));
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
        draw_center_hud(&mut tiny, 10, 4, &pal_accent(frame_color));
        assert!(
            tiny.iter().all(|c| c.ch == ' '),
            "tiny viewport draws no HUD"
        );
    }

    #[test]
    fn alt_digit_is_a_jump_digit_not_an_immediate_jump() {
        // The regression this whole feature exists for: a digit alone can't
        // decide the column, because it may be the first of two.
        let ev = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::JumpDigit(4)));
        // Every digit decodes, including the ones that used to be unreachable
        // as a *second* digit.
        for d in 0..=9u32 {
            let c = char::from_digit(d, 10).unwrap();
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
            assert_eq!(handle_key(&ev), Some(Cmd::JumpDigit(d)), "digit {d}");
        }
    }

    #[test]
    fn jump_accum_builds_multi_digit_columns() {
        let t = Instant::now();
        let mut j = JumpAccum::default();
        j.push(1, t);
        j.push(2, t);
        // 1-based typing, 0-based layout index: column 12 is index 11.
        assert_eq!(j.take(), Some(11));
        // Taking clears, so a second commit can't re-jump.
        assert_eq!(j.take(), None);
    }

    #[test]
    fn jump_accum_ignores_a_leading_zero() {
        // There is no column 0, so a bare `0` must not start a number (and
        // must not commit a jump to index -1 on release).
        let t = Instant::now();
        let mut j = JumpAccum::default();
        j.push(0, t);
        assert_eq!(j.pending(), None);
        // But zero still extends a real number: 1 then 0 is column 10.
        j.push(1, t);
        j.push(0, t);
        assert_eq!(j.take(), Some(9));
    }

    #[test]
    fn jump_accum_refuses_absurd_numbers() {
        // Key repeat must not overflow the accumulator into a nonsense index.
        let t = Instant::now();
        let mut j = JumpAccum::default();
        for _ in 0..12 {
            j.push(9, t);
        }
        assert_eq!(j.pending(), Some(999), "clamped at MAX, not overflowed");
    }

    #[test]
    fn jump_accum_expires_without_a_release_event() {
        // Terminals without the Kitty protocol never report Option release,
        // so the idle timeout is the only thing that commits the jump.
        let t = Instant::now();
        let mut j = JumpAccum::default();
        j.push(3, t);
        assert_eq!(j.take_if_expired(t), None, "still being typed");
        assert_eq!(
            j.take_if_expired(t + JumpAccum::TIMEOUT),
            Some(2),
            "commits once idle"
        );
        assert_eq!(j.pending(), None);
    }

    #[test]
    fn jump_accum_timeout_is_refreshed_by_each_digit() {
        // Typing the second digit slowly must not split `12` into `1` then
        // `2`; every digit restarts the idle window.
        let t = Instant::now();
        let mut j = JumpAccum::default();
        j.push(1, t);
        let late = t + JumpAccum::TIMEOUT - Duration::from_millis(1);
        j.push(2, late);
        assert_eq!(j.take_if_expired(t + JumpAccum::TIMEOUT), None);
        assert_eq!(j.take_if_expired(late + JumpAccum::TIMEOUT), Some(11));
    }

    #[test]
    fn alt_slash_toggles_the_cheat_sheet_hud() {
        // Option-as-Meta path and the macOS glyph path both map to ToggleHud.
        let ev = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::ALT);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('\u{f7}'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        // Option+Shift+/ (Option+?) is the same toggle, both paths.
        let ev = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('\u{bf}'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert!(matches!(handle_key(&ev), Some(Cmd::Input(_))));
        // Plain `/` stays pane input.
        let ev = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::Input(_))));
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
        let r2 = layout.new_row("two".to_string());
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
    fn draw_center_minimap_paints_centered_dashboard() {
        let mut layout = Layout::default();
        let r2 = layout.new_row("two".to_string());
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
        // A single pane has no grid to triage, but holding ⌥ must still do
        // something visible or the gesture teaches the user it is broken:
        // the panel degrades to the key hints alone.
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
        let solo: String = out2.iter().map(|c| c.ch).collect();
        assert!(
            solo.contains(&format!("{}g", crate::keys::mod_key())),
            "one pane still gets the key hints, got {solo:?}"
        );
        assert!(
            !solo.contains('»'),
            "...but no tiles, since there is nothing to compare: {solo:?}"
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
    fn tile_text_degrades_in_a_fixed_order_as_the_tile_narrows() {
        // Widest: glyph, address, jump marker, title and age all fit.
        let wide = tile_text(16, "2", true, "jcode", "4m", '!');
        assert_eq!(wide.chars().count(), 16, "always exactly the tile width");
        assert!(wide.starts_with("!2\u{25b8}jcode"), "got {wide:?}");
        assert!(wide.ends_with("4m"), "age closes the tile, got {wide:?}");
        // No jump marker when this is not the smart-jump target.
        let plain = tile_text(16, "2", false, "jcode", "4m", '!');
        assert!(plain.starts_with("!2 jcode"), "got {plain:?}");
        // Narrower: the name is cut, but marked, and the age survives - a
        // waiting pane's age is the news, a two-letter name is not.
        let cut = tile_text(11, "2", false, "jcode-main", "4m", '!');
        assert_eq!(cut.chars().count(), 11);
        assert!(cut.ends_with("4m"), "age kept, got {cut:?}");
        assert!(cut.starts_with("!2"), "glyph and address kept, got {cut:?}");
        assert!(cut.contains('\u{2026}'), "the cut is marked, got {cut:?}");
        // Narrower still: the title goes entirely rather than shrinking to
        // noise, and the glyph/address/age triple still reads.
        let mid = tile_text(6, "2", false, "jcode", "4m", '!');
        assert_eq!(mid.chars().count(), 6);
        assert_eq!(mid, "!2  4m", "glyph, address, gap, age");
        assert!(!mid.contains('j'), "no one-letter names, got {mid:?}");
        // Narrow enough that even the age cannot fit: the glyph is last out.
        let tight = tile_text(4, "2", false, "jcode", "4m", '!');
        assert_eq!(tight.chars().count(), 4);
        assert!(
            tight.starts_with("!2"),
            "glyph outlives everything: {tight:?}"
        );
        assert!(!tight.contains("4m"), "age dropped when it cannot fit");
        // Two cells: glyph + address. One cell: the status alone, since a
        // waiting pane matters more than which key jumps to it.
        assert_eq!(tile_text(2, "2", true, "jcode", "4m", '!'), "!2");
        assert_eq!(tile_text(1, "2", true, "jcode", "4m", '!'), "!");
        // Two-digit columns fit when there is room and degrade to `+` when
        // there is not: a `1 0` chord addresses column 10, so the map must
        // not silently claim it is column 1.
        assert!(tile_text(4, "10", false, "", "", '\u{bb}').starts_with("\u{bb}10"));
        assert_eq!(tile_text(2, "10", false, "", "", '\u{bb}'), "\u{bb}+");
        // Whatever the width, the tile is exactly that many cells.
        for w in 1..=24u16 {
            for (t, a) in [("", ""), ("jcode", "4m"), ("a-very-long-name", "59s")] {
                assert_eq!(
                    tile_text(w, "12", true, t, a, '\u{bb}').chars().count(),
                    w as usize,
                    "width {w} with {t:?}/{a:?}"
                );
            }
        }
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
    fn contrast_fg_stays_legible_on_light_and_dark_tints() {
        let light = Palette::CATPPUCCIN_LATTE;
        let dark = Palette::CATPPUCCIN_MOCHA;
        let luma = |c: CColor| match c {
            CColor::Rgb(r, g, b) => {
                (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
            }
            _ => panic!("expected rgb"),
        };
        // Ink on a light tile is dark, and on a dark tile is light: the old
        // hardcoded near-white was invisible on Latte's status colors.
        assert!(luma(contrast_fg(CColor::Rgb(0xef, 0xf1, 0xf5), &light)) < 0.4);
        assert!(luma(contrast_fg(CColor::Rgb(0x1e, 0x1e, 0x2e), &dark)) > 0.6);
        // An indexed or default tint has no components to measure, so the
        // near-white assumption is kept rather than guessed at.
        assert_eq!(contrast_fg(CColor::Idx(4), &dark), CColor::Idx(231));
        assert_eq!(contrast_fg(CColor::Default, &dark), CColor::Idx(231));
    }

    #[test]
    fn dashboard_names_panes_and_marks_the_attention_target() {
        let (mut layout, ids) = dashboard_layout(4);
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Idle;
        let facts = HudFacts {
            titles: [(ids[0], "jcode".to_string()), (ids[2], "cargo".to_string())]
                .into_iter()
                .collect(),
            quiet: [(ids[2], Duration::from_secs(4 * 60))]
                .into_iter()
                .collect(),
            jump_target: smart_jump_target(&layout),
            pending_jump: None,
        };
        assert_eq!(facts.jump_target, Some(ids[2]), "the idle pane wants you");
        let out = paint_dashboard(&layout, &facts, 100, 24);
        let text = screen_rows(&out, 100).join("\n");
        assert!(
            text.contains("jcode"),
            "the pane's own title identifies it, got:\n{text}"
        );
        assert!(
            text.contains("cargo"),
            "every titled pane is named:\n{text}"
        );
        assert!(
            text.contains('\u{25b8}'),
            "the smart-jump target is marked, got:\n{text}"
        );
        assert!(
            text.contains("4m"),
            "an idle pane says how long it has waited:\n{text}"
        );
        // A running pane is not aged: only attention has a clock on it.
        let quiet_running = HudFacts {
            quiet: [(ids[0], Duration::from_secs(9 * 60))]
                .into_iter()
                .collect(),
            ..HudFacts::default()
        };
        let out2 = paint_dashboard(&layout, &quiet_running, 100, 24);
        assert!(
            !screen_rows(&out2, 100).join("\n").contains("9m"),
            "a working pane's silence is not news"
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
    fn a_pending_jump_lights_the_column_it_addresses() {
        let (layout, _) = dashboard_layout(4);
        let pal = pal_accent(CColor::Idx(36));
        let facts = HudFacts {
            pending_jump: Some(3),
            ..HudFacts::default()
        };
        let plan = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        let mut out = vec![Cell::default(); 100 * 24];
        paint_center_minimap(&mut out, 100, 24, &layout, &plan, &pal, &facts);
        let bg_of = |out: &[Cell], col: usize| {
            let tile = plan
                .map
                .cells
                .iter()
                .find(|c| c.column == col)
                .expect("tile exists");
            out[plan.row_y[0] as usize * 100 + (plan.map_ox + tile.x) as usize]
                .style
                .bg
        };
        // Column 3 is lit at full status intensity; the columns the number
        // does not address step back to the overlay tint.
        assert_eq!(bg_of(&out, 2), pal.status(PaneStatus::Running));
        assert_eq!(bg_of(&out, 3), pal.overlay);
        // Without a pending jump nothing is dimmed: tiles are their own
        // muted status tint again.
        let mut plain = vec![Cell::default(); 100 * 24];
        paint_center_minimap(
            &mut plain,
            100,
            24,
            &layout,
            &plan,
            &pal,
            &HudFacts::default(),
        );
        assert_eq!(
            bg_of(&plain, 3),
            pal.status_muted(PaneStatus::Running),
            "no pending number, no dimming"
        );
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
        // The frame and the footer are not tiles.
        assert_eq!(hud_pane_at(&plan, plan.rect.x, y), None, "frame");
        assert_eq!(hud_pane_at(&plan, plan.map_ox, plan.hint_y), None, "footer");
    }

    #[test]
    fn truncated_strips_are_counted_not_silently_dropped() {
        let mut layout = Layout::default();
        for i in 0..9 {
            let r = layout.new_row(format!("strip {}", i + 2));
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
    fn named_strips_are_labelled_and_generated_ones_are_not() {
        let mut layout = Layout::default();
        let named = layout.new_row("deploy".to_string());
        let p = layout.alloc_pane();
        layout.add_column(named, gwae_layout::Width::Cells(20), vec![p]);
        // `strip 3` is what the NewRow verb generates; repeating it in the
        // gutter beside the number would just say "3 strip 3".
        let generated = layout.new_row("strip 3".to_string());
        let p2 = layout.alloc_pane();
        layout.add_column(generated, gwae_layout::Width::Cells(20), vec![p2]);
        let plan = plan_center_minimap(100, 24, &layout, &crate::config::Minimap::default())
            .expect("dashboard fits");
        assert_eq!(plan.gutter[1], "2 deploy", "a real name is worth showing");
        assert_eq!(plan.gutter[2], "3", "a generated one is just its number");
    }

    #[test]
    fn the_scrim_dims_around_the_panel_and_never_through_it() {
        let (cols, rows) = (40u16, 12u16);
        let bright = CColor::Rgb(200, 200, 200);
        let mut out = vec![
            Cell {
                ch: 'x',
                style: gwae_term::Style {
                    fg: bright,
                    bg: bright,
                    bold: true,
                    ..Default::default()
                },
                width: 1,
                ..Default::default()
            };
            cols as usize * rows as usize
        ];
        let keep = Rect {
            x: 10,
            y: 4,
            w: 8,
            h: 3,
        };
        dim_behind(&mut out, cols, rows, keep);
        let at = |x: u16, y: u16| out[y as usize * cols as usize + x as usize];
        // Inside the panel rect: untouched, so the panel paints over a clean
        // backdrop rather than a pre-dimmed one.
        assert_eq!(at(10, 4).style.bg, bright);
        assert_eq!(at(17, 6).style.bg, bright);
        assert!(at(12, 5).style.bold);
        // Outside: dimmed, but still legible content rather than a blackout.
        for (x, y) in [(0u16, 0u16), (9, 4), (18, 6), (39, 11)] {
            let c = at(x, y);
            assert_ne!(c.style.bg, bright, "cell ({x},{y}) should be dimmed");
            assert_eq!(c.ch, 'x', "the scrim restyles, it does not erase");
            assert!(!c.style.bold, "bold would fight the scrim");
        }
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
                    &no_cow(),
                    true,
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
}

#[test]
fn content_scroll_reveals_overflow_e2e() {
    let mut layout = Layout::default();
    let pid = focused_pane(&layout).expect("default layout has a focused pane");
    // Widen the single default column to the full viewport so we see 80 cells.
    if let Some(row) = layout.row_mut(layout.focus.row) {
        row.columns[0].width = gwae_layout::Width::Cells(80);
    }
    let (tx, rx) = channel::<PaneMsg>();
    let cmd = "sh -c \"for i in $(seq 1 240); do printf '%s' $((i % 10)); done; echo\"";
    let pane = spawn_pane(pid, cmd, 240, 10, tx.clone(), None).expect("spawn pane");
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
        &Palette::default(),
        &crate::config::Minimap::default(),
        &crate::config::Cowsay {
            enabled: false,
            messages: Vec::new(),
        },
        true,
        None,
    );
    // Content is inset 1 cell inside the column frame: grid (x,0) is at
    // screen (x + 1, 1).
    let at = |out: &Vec<Cell>, x: usize| out[80 + 1 + x].ch;
    assert_eq!(at(&out, 0), '1'); // content col 0 -> first content cell
    assert_eq!(at(&out, 9), '0'); // content col 9
    assert_eq!(at(&out, 77), '8'); // content col 77

    // Scrolling 60 pans 60 cells; content col 60 leads at screen x=0.
    panes.get_mut(&pid).unwrap().h_scroll = 60;
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        80,
        10,
        240,
        &Palette::default(),
        &crate::config::Minimap::default(),
        &crate::config::Cowsay {
            enabled: false,
            messages: Vec::new(),
        },
        true,
        None,
    );
    assert_eq!(at(&out, 0), '1'); // content col 60
    assert_eq!(at(&out, 1), '2'); // content col 61
    assert_eq!(at(&out, 77), '8'); // content col 137

    // Past the 240-col content the window reveals blanks.
    panes.get_mut(&pid).unwrap().h_scroll = 200;
    render_frame(
        &mut out,
        &layout,
        &mut panes,
        80,
        10,
        240,
        &Palette::default(),
        &crate::config::Minimap::default(),
        &crate::config::Cowsay {
            enabled: false,
            messages: Vec::new(),
        },
        true,
        None,
    );
    assert_eq!(at(&out, 0), '1'); // content col 200
    assert_eq!(at(&out, 39), '0'); // content col 239
    assert_eq!(at(&out, 45), ' '); // past content end -> blank

    panes.get_mut(&pid).unwrap().child.kill();
}

/// End-to-end acceptance for the quarter-pane overflow fix: four real PTY
/// children, one per quarter column, rendered by `render_frame` at 342 cols
/// (the reported failure width, not divisible by 4). Every screen cell up to
/// and including the rightmost column must show the pane that owns it, with
/// pane content never spilling past a column boundary or the screen edge.
#[test]
fn four_quarter_panes_render_to_screen_edge_e2e() {
    use gwae_layout::{Preset, Width};
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
        let pane = spawn_pane(*pid, &cmd, w, rows, tx.clone(), None).expect("spawn pane");
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
        &Palette::default(),
        &crate::config::Minimap::default(),
        &crate::config::Cowsay {
            enabled: false,
            messages: Vec::new(),
        },
        true,
        None,
    );
    // The first content row shows each pane's letter across its column's
    // interior: the frame owns the boundary cells, content never bleeds past
    // them, and the rightmost column reaches the screen edge.
    let row1 = cols as usize; // screen row 1: the first content row
    for (i, (s, e)) in ranges.iter().enumerate() {
        for x in (*s + 1)..(*e - 1) {
            assert_eq!(
                out[row1 + x as usize].ch,
                fills[i],
                "screen x={x} must show pane {} content",
                fills[i]
            );
        }
    }
    assert_eq!(
        out[cols as usize - 1].ch,
        '╮',
        "the rightmost column's frame reaches the screen edge"
    );
    assert_eq!(
        out[row1 + cols as usize - 2].ch,
        'D',
        "pane D's content runs up to its frame"
    );
    for p in panes.values_mut() {
        kill_pane_tree(&mut p.child);
    }
}

#[cfg(test)]
mod strip_label_tests {
    use super::*;
    use gwae_layout::{Action, FollowScroll, Viewport};

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
    use gwae_layout::{Action, FollowScroll, Preset, Viewport, Width};
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
        let pane = spawn_pane(*pid, "sleep 30", 80, rows, tx.clone(), None).expect("spawn pane");
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
            &Palette::default(),
            &crate::config::Minimap::default(),
            &crate::config::Cowsay {
                enabled: false,
                messages: Vec::new(),
            },
            true,
            None,
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
        kill_pane_tree(&mut p.child);
    }
}
