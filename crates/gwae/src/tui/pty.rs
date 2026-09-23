//! PTY pane ownership: types, spawning, adoption, teardown, sync (verbatim move from `tui/mod.rs`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::time::Instant;

use gwae_layout::{Layout, PaneId};
use gwae_term::{Size as GridSize, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};

use super::render::chrome_rows;
use super::render::column_grid_sizes;
use crate::config::Config;
use crate::geometry::CellPixels;

use super::shell::agent_gateway_cmd;
use super::shell::default_shell;
use super::shell::shell_split;

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
    pub fn resize(&self, size: PtySize) -> Result<(), String> {
        match self {
            PaneIo::Owned(m) => m.resize(size).map_err(|e| e.to_string()),
            #[cfg(unix)]
            PaneIo::Inherited(fd) => {
                let ws = libc::winsize {
                    ws_row: size.rows,
                    ws_col: size.cols,
                    ws_xpixel: size.pixel_width,
                    ws_ypixel: size.pixel_height,
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
    #[cfg(unix)]
    Adopted(Option<u32>),
}

impl PaneProc {
    #[allow(dead_code)]
    pub fn process_id(&self) -> Option<u32> {
        match self {
            PaneProc::Owned(c) => c.process_id(),
            #[cfg(unix)]
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
    /// Last geometry successfully sent to the kernel, including pixel-only
    /// font changes that do not alter character rows or columns.
    pub pty_size: PtySize,
    pub alive: bool,
    pub h_scroll: i32,
    /// When the pane last emitted any output (activity heuristic).
    pub last_output: Instant,
    /// True once the child has spoken OSC 133; from then on the explicit
    /// protocol owns the status and the activity heuristic stands down.
    pub saw_osc133: bool,
    pub graphics_stream: crate::graphics_stream::Stream,
    pub graphics: crate::graphics::Graphics,
    pub legacy_images: crate::graphics_host::Legacy,
    /// Phase 1 image isolation token: `None` until the pane carries image
    /// traffic, bumped on every image commit/placement/delete, reset on
    /// graphics clear. `Host::prepare_cached` skips panes whose token is
    /// unchanged since the last prepared frame.
    pub image_activity: Option<u64>,
    /// Phase 2 promotion: `Some` once sustained native image commits prove
    /// this pane is an image viewer (e.g. tdf). A promoted pane paints only
    /// its image tiles; grid text hides until demotion. The PTY stays live
    /// for input; demotion restores full text painting with no state loss.
    pub image_view: Option<ImageView>,
    /// Phase 2 streak accumulator: consecutive native commits with no grid
    /// text-screen change between them. Crate-visible so render tests can
    /// build promoted-pane fixtures; production code uses the accessors.
    pub(crate) promote_streak: u32,
}

/// Phase 2 viewer promotion state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageView {
    /// Image-activity token at promotion time. Later token changes (new
    /// page, zoom) keep the promotion; only explicit demote clears it.
    pub promoted_at: u64,
    /// Consecutive native commits observed when promotion fired (diagnostic).
    pub commits: u32,
}

impl PtyPane {
    pub(crate) fn promote_streak(&self) -> u32 {
        self.promote_streak
    }
    pub(crate) fn set_promote_streak(&mut self, streak: u32) {
        self.promote_streak = streak;
    }

    /// Apply a horizontal pan (`⌥+←/→`): plain panes move gwae's own
    /// `h_scroll` window, but a full-screen child (nvim, less) owns
    /// horizontal movement itself, so it gets the arrow keys it expects
    /// instead. Returns true when the frame needs a repaint (plain pan).
    pub(crate) fn scroll_pane(&mut self, d: i32) -> bool {
        if self.grid.alternate_screen() {
            let key: &[u8] = if d > 0 { b"\x1b[C" } else { b"\x1b[D" };
            for _ in 0..d.unsigned_abs().min(20) {
                let _ = self.writer.write_all(key);
            }
            let _ = self.writer.flush();
            false
        } else {
            self.h_scroll = (self.h_scroll + d).max(0);
            true
        }
    }
}

/// Native commits with no interleaving text-screen change needed to promote.
/// A real viewer promotes on its first page; a pasted thumbnail never does.
pub(crate) const IMAGE_PROMOTE_COMMITS: u32 = 2;

/// Graphics placements use the cursor at their position in the byte stream,
/// never the cursor after a whole PTY read. Replies go only to this child.
pub(crate) fn feed_pane_output(
    pane: &mut PtyPane,
    bytes: &[u8],
    graphics_enabled: bool,
    promote: bool,
) {
    use crate::graphics_stream::Event;
    let mut streak = pane.promote_streak();
    for event in pane.graphics_stream.feed(bytes) {
        let before = pane.grid.screen_epoch();
        match &event {
            Event::Text(bytes) | Event::Apc(bytes) => pane.grid.feed(bytes),
            Event::Oversized => {
                pane.graphics.abort_transfer();
                pane.legacy_images.abort_transfer();
                pane.grid.feed(b"\x18")
            }
        };
        if pane.grid.screen_epoch() != before {
            pane.graphics.clear_all();
            pane.legacy_images = Default::default();
            pane.image_activity = None;
            pane.image_view = None;
            streak = 0;
        }
        let mut replies = pane.grid.take_pty_replies();
        if let Event::Apc(apc) = event {
            if graphics_enabled {
                let graphics_gen = pane.graphics.generation();
                let legacy_rev = pane.legacy_images.revision();
                let outcome = pane.graphics.command(
                    &apc,
                    pane.grid.cursor_position(),
                    (
                        pane.pty_size.pixel_width / pane.pty_size.cols.max(1),
                        pane.pty_size.pixel_height / pane.pty_size.rows.max(1),
                    ),
                );
                if outcome.unsupported {
                    tracing::debug!("unsupported pane graphics feature");
                }
                pane.legacy_images.delete_command(&apc);
                if let Some(id) = outcome.committed_image {
                    pane.legacy_images.forget_image(id);
                }
                if outcome.legacy {
                    pane.legacy_images.accept(&apc);
                    if let Some(id) = pane.legacy_images.take_committed_image() {
                        pane.graphics.forget_image(id);
                    }
                }
                if let Some((row, col)) = outcome.cursor {
                    pane.grid.set_graphics_cursor(row, col);
                }
                if pane.graphics.generation() != graphics_gen
                    || pane.legacy_images.revision() != legacy_rev
                {
                    pane.image_activity = Some(pane.image_activity.unwrap_or(0).wrapping_add(1));
                }
                if promote && outcome.committed_image.is_some() {
                    streak += 1;
                    if streak >= IMAGE_PROMOTE_COMMITS && pane.image_view.is_none() {
                        pane.image_view = Some(ImageView {
                            promoted_at: pane.image_activity.unwrap_or(0),
                            commits: streak,
                        });
                    }
                }
                replies.extend_from_slice(&outcome.replies);
            }
        }
        if !replies.is_empty() {
            let _ = pane.writer.write_all(&replies);
            let _ = pane.writer.flush();
        }
    }
    pane.set_promote_streak(streak);
}

/// Message a background thread sends to the main loop: pane traffic from the
/// per-pane PTY readers, or a host terminal event from the input forwarder.
/// One channel means one blocking wait wakes on *any* event source, so a
/// keystroke's echo never sits in a queue waiting for a poll tick to expire.
pub(crate) enum PaneMsg {
    Output(PaneId, Vec<u8>),
    Exited(PaneId),
    Input(crossterm::event::Event),
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
pub(crate) fn kill_pane_tree(child: &mut PaneProc) {
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
        child.kill();
    }
}

/// Spawn a PTY running `cmd` at the given grid size, wiring a reader thread.
pub(crate) fn spawn_pane(
    id: PaneId,
    cmd: &str,
    gw: u16,
    gh: u16,
    tx: Sender<PaneMsg>,
    cwd: Option<&std::path::Path>,
    cell_pixels: CellPixels,
) -> Result<PtyPane, String> {
    let pty = native_pty_system();
    let size = cell_pixels.pty_size(gw, gh);
    let pair = pty.openpty(size).map_err(|e| format!("openpty: {e}"))?;
    let master = pair.master;
    let slave = pair.slave;
    let argv = if cmd.trim().is_empty() {
        vec![default_shell()]
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
        pty_size: size,
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        graphics_stream: Default::default(),
        graphics: Default::default(),
        legacy_images: Default::default(),
        image_view: None,
        promote_streak: 0,
        image_activity: None,
    })
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
pub(crate) fn adopt_pane(
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
        // Refresh even if character geometry survived reload unchanged.
        pty_size: PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        },
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        graphics_stream: Default::default(),
        graphics: Default::default(),
        legacy_images: Default::default(),
        image_view: None,
        promote_streak: 0,
        image_activity: None,
    })
}

/// Ask every adopted pane's child to repaint, so a reloaded screen is not
/// blank until the user types.
///
/// An adopted pane's grid starts empty (contents are not carried across the
/// exec), so the child has to redraw *everything*. Getting that to happen
/// without typing into the pane is subtler than it looks.
///
/// What does not work, and why:
///
/// 1. **Writing `Ctrl-L`** (the original shape). Only a shell reads `\x0c` as
///    "redraw"; to a full-screen program it is an ordinary key. jcode binds it
///    to terminal-style clear, so every reload pushed the agent transcript
///    off-screen and left the pane black. Any byte written here is input, and
///    input means guessing at the child's keymap.
/// 2. **Re-asserting the same size**, with or without a hand-delivered
///    `SIGWINCH`. The kernel only raises the signal when the winsize actually
///    changes, and ratatui's `Terminal::flush` diffs each frame against its
///    *own* previous buffer, which still holds the pre-reload frame.
///    `autoresize` only force-clears when the area changed, so the diff comes
///    out empty: the child dutifully "repaints" and writes nothing. Measured
///    against a real ratatui app: 25 bytes, none of them content.
/// 3. **Growing and restoring back to back.** Both `ioctl`s land before the
///    child is scheduled, so it only ever observes the final size and the
///    change collapses to case 2. Also measured at 25 bytes.
///
/// What works is a size change the child can actually *observe*: grow by one
/// row, let it run, then restore. It then repaints its full surface (measured:
/// 92 bytes, content included) and ends at exactly the size it started with.
///
/// This returns the panes that still need restoring, so the caller can do it
/// after a short delay instead of blocking the event loop in a sleep. See
/// [`RepaintRestore`].
///
/// Growing rather than shrinking is deliberate: a pane on the normal screen
/// can never scroll content off the top on the way out, and the restore is
/// what returns it to the true size. Nothing is ever written to the child.
#[must_use = "the pane sizes must be restored or every pane is left one row too tall"]
pub(crate) fn nudge_repaint(panes: &mut HashMap<PaneId, PtyPane>) -> RepaintRestore {
    let mut pending = Vec::new();
    for (id, p) in panes.iter_mut() {
        // A height change, not a width one: changing width makes
        // text-wrapping children rewrap twice, which is visible. Alt-screen
        // programs (every agent TUI) have no scrollback to disturb at all.
        let grown = PtySize {
            rows: p.pty_size.rows.saturating_add(1),
            ..p.pty_size
        };
        if p.master.resize(grown).is_ok() {
            pending.push((*id, p.pty_size));
        }
    }
    RepaintRestore {
        pending,
        at: Instant::now() + REPAINT_SETTLE,
    }
}

/// How long a repaint nudge leaves the pane one row taller before restoring.
///
/// The child has to be scheduled in between or it never sees two distinct
/// sizes and never fully repaints (see [`nudge_repaint`] case 3). 50ms was
/// measured sufficient against a real ratatui app on an idle machine, but a
/// child that polls its size (rather than handling `SIGWINCH`) can sleep
/// through a window that short when the machine is loaded, so this is set well
/// above the measurement. It costs nothing: it is a deadline for one extra
/// `ioctl`, not a sleep, and the loop keeps painting throughout. Still an
/// order of magnitude below the several seconds a reload already takes.
pub(crate) const REPAINT_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// The second half of a repaint nudge: sizes to put back, and when.
///
/// Carried rather than slept on so the event loop keeps painting and stays
/// responsive to input while the panes are momentarily one row taller.
#[derive(Debug)]
pub(crate) struct RepaintRestore {
    pending: Vec<(PaneId, PtySize)>,
    at: Instant,
}

impl Default for RepaintRestore {
    /// An empty restore, owed to nobody. `Instant` has no `Default`, and the
    /// timestamp is meaningless while `pending` is empty (every method checks
    /// that first), so "now" is the honest placeholder.
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            at: Instant::now(),
        }
    }
}

impl RepaintRestore {
    /// Whether anything is still waiting to be restored.
    pub(crate) fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// When the restore is due, so the caller can bound its wait and not
    /// oversleep past it.
    pub(crate) fn due_at(&self) -> Instant {
        self.at
    }

    /// Restore the real sizes once the settle window has passed.
    ///
    /// A no-op until then, and a no-op forever after: a pane that was closed
    /// in the meantime is simply skipped, since the restore is keyed by id.
    pub(crate) fn maybe_apply(&mut self, panes: &mut HashMap<PaneId, PtyPane>) -> bool {
        if self.pending.is_empty() || Instant::now() < self.at {
            return false;
        }
        for (id, size) in self.pending.drain(..) {
            if let Some(p) = panes.get_mut(&id) {
                let _ = p.master.resize(size);
            }
        }
        true
    }
}

/// Use the same drawable geometry before spawn and on every resize. A child
/// may measure its terminal immediately, before the first compositor frame.
pub(crate) fn pane_grid_sizes(
    layout: &Layout,
    host: GridSize,
    cfg: &Config,
) -> Vec<(PaneId, GridSize)> {
    layout
        .rows
        .iter()
        .flat_map(|r| &r.columns)
        .flat_map(|col| {
            let sizes =
                column_grid_sizes(col.width, col.panes.len(), host, 0, true, chrome_rows(cfg));
            col.panes.iter().copied().zip(sizes).map(|(pid, size)| {
                (
                    pid,
                    GridSize {
                        // Match TerminalGrid's minimum wide-glyph-safe width,
                        // even when only a clipped sliver is drawable.
                        cols: size.cols.max(2),
                        rows: size.rows.max(1),
                    },
                )
            })
        })
        .collect()
}

/// Kill any pane whose id is no longer in the layout, and spawn missing ones.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_panes(
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    tx: &Sender<PaneMsg>,
    geometry: (GridSize, CellPixels),
    agent_panes: &HashSet<PaneId>,
    agent_cmds: &HashMap<PaneId, String>,
    cwd: Option<&std::path::Path>,
    cfg: &Config,
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
    // Spawn missing panes. A pane with a resolved harness command runs it
    // directly (the `⌥+;` overlay and the fast paths); an agent pane with
    // none runs the gateway, which resolves, maybe prompts, and execs the
    // result, so the pane's process ends up *being* the harness.
    // Resolving inside the pane (rather than here) is what lets the "not
    // installed" case be a real interactive screen instead of a toast.
    for (pid, size) in pane_grid_sizes(layout, geometry.0, cfg) {
        if panes.contains_key(&pid) {
            continue;
        }
        let cmd = if let Some(direct) = agent_cmds.get(&pid) {
            direct.clone()
        } else if agent_panes.contains(&pid) {
            agent_gateway_cmd()
        } else {
            String::new()
        };
        let pane = spawn_pane(pid, &cmd, size.cols, size.rows, tx.clone(), cwd, geometry.1)?;
        panes.insert(pid, pane);
        tracing::debug!(pid, "spawned pane");
    }
    Ok(())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn owned_and_inherited_pty_resize_preserve_pixel_dimensions() {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let owned = PaneIo::Owned(pair.master);
        let size = CellPixels {
            width: 8,
            height: 16,
        }
        .pty_size(38, 22);
        owned.resize(size).unwrap();
        let PaneIo::Owned(ref master) = owned else {
            unreachable!()
        };
        assert_eq!(master.get_size().unwrap(), size);

        // The original owner keeps this fd alive. Exercise exactly the ioctl
        // path used by a pane adopted after exec, without spawning a child.
        let inherited = PaneIo::Inherited(owned.raw_fd().unwrap());
        let next = CellPixels {
            width: 10,
            height: 20,
        }
        .pty_size(78, 28);
        inherited.resize(next).unwrap();
        assert_eq!(master.get_size().unwrap(), next);
    }

    /// The reload repaint must not inject input into the child, and must put
    /// the pane back at exactly the size it started with.
    ///
    /// The nudge used to write `Ctrl-L` into every pane, which is a redraw in
    /// a plain shell but a real command in jcode (terminal-style clear): a hot
    /// reload wiped the agent transcript and left the pane black. It now asks
    /// for a repaint with a brief, observable size change and no bytes at all.
    #[test]
    fn reload_nudge_repaints_with_no_bytes_and_restores_the_size() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        pair.master.resize(size).expect("set initial size");
        let sink = Sink::default();
        let mut panes = HashMap::from([(
            1u64,
            PtyPane {
                master: PaneIo::Owned(pair.master),
                writer: Box::new(sink.clone()),
                child: PaneProc::Adopted(None),
                grid: Vt100Grid::new(GridSize { cols: 80, rows: 24 }),
                pty_size: size,
                alive: true,
                h_scroll: 0,
                last_output: Instant::now(),
                saw_osc133: false,
                graphics_stream: Default::default(),
                graphics: Default::default(),
                legacy_images: Default::default(),
                image_view: None,
                promote_streak: 0,
                image_activity: None,
            },
        )]);

        let mut restore = nudge_repaint(&mut panes);
        // Phase one: the pane is deliberately one row taller, which is the
        // only thing that makes a ratatui child redraw its whole surface.
        let grown = match &panes[&1].master {
            PaneIo::Owned(m) => m.get_size().expect("size after nudge"),
            #[cfg(unix)]
            PaneIo::Inherited(_) => unreachable!("owned in this test"),
        };
        assert_eq!(
            grown.rows,
            size.rows + 1,
            "the nudge must change the size for real; an identical size is a \
             no-op that leaves the pane blank"
        );
        assert!(restore.is_pending(), "the restore must still be owed");

        // Phase two is time-gated, so it does nothing yet and everything once
        // the window has passed.
        assert!(
            !restore.maybe_apply(&mut panes),
            "the restore must wait: applied immediately, the child never sees \
             two distinct sizes and never repaints"
        );
        std::thread::sleep(REPAINT_SETTLE + std::time::Duration::from_millis(20));
        assert!(restore.maybe_apply(&mut panes), "the restore must fire");
        let back = match &panes[&1].master {
            PaneIo::Owned(m) => m.get_size().expect("size after restore"),
            #[cfg(unix)]
            PaneIo::Inherited(_) => unreachable!("owned in this test"),
        };
        assert_eq!(
            (back.rows, back.cols),
            (size.rows, size.cols),
            "the pane must end at its true size, or the frame no longer fits"
        );
        assert!(!restore.is_pending(), "the restore must be consumed once");

        assert!(
            sink.0.lock().unwrap().is_empty(),
            "the reload repaint must send no input bytes to the child: any \
             byte here is a keystroke the child may act on (Ctrl-L cleared \
             jcode's transcript, which is the bug this replaced)"
        );
    }

    /// Every adopted pane is nudged, and a pane that dies mid-nudge is not a
    /// problem for the ones that live.
    ///
    /// A reload with four agents is the normal case this feature exists for,
    /// and a pane whose process exits during the settle window is closed by
    /// the loop, so the restore has to tolerate its id being gone rather than
    /// panicking or skipping the rest.
    #[test]
    fn the_restore_covers_every_pane_and_tolerates_one_disappearing() {
        fn pane(size: PtySize) -> PtyPane {
            let pair = native_pty_system().openpty(PtySize::default()).unwrap();
            pair.master.resize(size).expect("size");
            PtyPane {
                master: PaneIo::Owned(pair.master),
                writer: Box::new(std::io::sink()),
                child: PaneProc::Adopted(None),
                grid: Vt100Grid::new(GridSize {
                    cols: size.cols,
                    rows: size.rows,
                }),
                pty_size: size,
                alive: true,
                h_scroll: 0,
                last_output: Instant::now(),
                saw_osc133: false,
                graphics_stream: Default::default(),
                graphics: Default::default(),
                legacy_images: Default::default(),
                image_view: None,
                promote_streak: 0,
                image_activity: None,
            }
        }

        let size = PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut panes: HashMap<PaneId, PtyPane> = (1u64..=4).map(|id| (id, pane(size))).collect();

        let mut restore = nudge_repaint(&mut panes);
        for id in 1u64..=4 {
            let grown = match &panes[&id].master {
                PaneIo::Owned(m) => m.get_size().expect("size"),
                #[cfg(unix)]
                PaneIo::Inherited(_) => unreachable!(),
            };
            assert_eq!(
                grown.rows,
                size.rows + 1,
                "pane {id} must be nudged too: a reload with several agents \
                 must not repaint only the first"
            );
        }

        // A pane exits during the settle window and is dropped by the loop.
        panes.remove(&3);
        std::thread::sleep(REPAINT_SETTLE + std::time::Duration::from_millis(20));
        assert!(
            restore.maybe_apply(&mut panes),
            "a closed pane must not cancel the restore for the others"
        );
        for id in [1u64, 2, 4] {
            let back = match &panes[&id].master {
                PaneIo::Owned(m) => m.get_size().expect("size"),
                #[cfg(unix)]
                PaneIo::Inherited(_) => unreachable!(),
            };
            assert_eq!(
                (back.rows, back.cols),
                (size.rows, size.cols),
                "pane {id} must be restored to its true size"
            );
        }
    }

    /// A session that never reloaded owes no restore and must not be nudged.
    ///
    /// `RepaintRestore::default()` is what a normal launch carries, so it has
    /// to be inert: a stray resize on a fresh session would reflow every
    /// pane's child for no reason.
    #[test]
    fn a_fresh_session_owes_no_repaint_restore() {
        let mut restore = RepaintRestore::default();
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        assert!(!restore.is_pending());
        assert!(
            !restore.maybe_apply(&mut panes),
            "a default restore must never claim to have done anything"
        );
    }

    // Exercise the actual pane event path without launching a child or writing
    // to the host terminal. The inert inherited handles are never accessed.
    #[cfg(unix)]
    mod graphics_feed_tests {
        use super::super::super::input::focused_pane;
        use super::super::super::render::tests::no_map;
        use super::super::super::render::{render_frame, render_frame_with_images};
        use super::*;
        use crate::geometry::CellPixels;
        use crate::theme::Palette;
        use gwae_term::{Size as GridSize, TermGrid, Vt100Grid};
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        #[derive(Clone, Default)]
        struct Replies(Arc<Mutex<Vec<u8>>>);

        impl Replies {
            fn bytes(&self) -> Vec<u8> {
                self.0.lock().unwrap().clone()
            }
        }

        impl Write for Replies {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        fn pane_with_replies() -> (PtyPane, Replies) {
            let replies = Replies::default();
            let mut grid = Vt100Grid::new(GridSize { cols: 20, rows: 10 });
            grid.set_cell_size(8, 16);
            (
                PtyPane {
                    master: PaneIo::Inherited(-1),
                    writer: Box::new(replies.clone()),
                    child: PaneProc::Adopted(None),
                    grid,
                    pty_size: CellPixels {
                        width: 8,
                        height: 16,
                    }
                    .pty_size(20, 10),
                    alive: true,
                    h_scroll: 0,
                    last_output: Instant::now(),
                    saw_osc133: false,
                    graphics_stream: Default::default(),
                    graphics: Default::default(),
                    legacy_images: Default::default(),
                    image_view: None,
                    promote_streak: 0,
                    image_activity: None,
                },
                replies,
            )
        }

        #[test]
        fn native_overlay_is_visible_over_styled_text_and_no_host_means_no_placeholders() {
            let (mut pane, _) = pane_with_replies();
            feed_pane_output(
                &mut pane,
                b"\x1b[?25l\x1b[1;4;7mX\x1b[H\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            let layout = Layout::new(1);
            let pid = focused_pane(&layout).unwrap();
            let mut panes = HashMap::from([(pid, pane)]);
            let mut out = Vec::new();
            let mut host = crate::graphics_host::Host::default();
            host.begin();
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
                Some(&mut host),
            );
            let cell = out[81];
            assert_eq!(cell.ch, crate::graphics_host::PLACEHOLDER);
            assert!(!cell.style.bold && !cell.style.underline && !cell.style.inverse);
            // Unmapped child placeholders must never be interpreted as a host ID,
            // including disabled graphics and test/preview renders without a host.
            panes
                .get_mut(&pid)
                .unwrap()
                .grid
                .feed("\x1b[H\u{10eeee}\u{305}\u{305}".as_bytes());
            render_frame(
                &mut out,
                &layout,
                &mut panes,
                80,
                24,
                0,
                &Palette::default(),
                &no_map(),
                None,
            );
            assert!(out
                .iter()
                .all(|c| c.ch != crate::graphics_host::PLACEHOLDER));
        }

        #[test]
        fn image_activity_token_tracks_commits_placements_and_clears() {
            let (mut pane, _) = pane_with_replies();
            // Fresh pane: no image traffic, so `prepare_cached` skips it.
            assert_eq!(pane.image_activity, None);
            // Plain text never advances the token.
            feed_pane_output(&mut pane, b"hello", true, true);
            assert_eq!(pane.image_activity, None);
            // A native source commit advances it once.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(pane.image_activity, Some(1));
            // A no-op query changes no state: token holds.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=q,i=7,f=24,s=1,v=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(pane.image_activity, Some(1));
            assert!(pane.graphics.source(7).is_some());
            // A re-display placement advances it again.
            feed_pane_output(&mut pane, b"\x1b_Ga=p,i=7,C=1\x1b\\", true, true);
            assert_eq!(pane.image_activity, Some(2));
            // Alternate-screen entry clears graphics state: token resets so
            // the host cache cannot serve stale tiles.
            feed_pane_output(&mut pane, b"\x1b[?1049h", true, true);
            assert_eq!(pane.image_activity, None);
        }

        #[test]
        fn promotion_disabled_never_promotes_even_on_sustained_commits() {
            let (mut pane, _) = pane_with_replies();
            // `image_pane = "off"`: commits still land (image_activity
            // advances, tiles still paint inline), but no promotion fires.
            for id in [7, 8, 9] {
                feed_pane_output(
                    &mut pane,
                    format!("\x1b_Ga=T,i={id},f=24,s=1,v=1,C=1;AQID\x1b\\").as_bytes(),
                    true,
                    false,
                );
            }
            assert_eq!(pane.image_view, None);
            assert_eq!(pane.promote_streak(), 0);
            assert!(pane.image_activity.is_some());
            assert!(pane.graphics.source(9).is_some());
        }

        #[test]
        fn sustained_native_commits_promote_to_image_view() {
            let (mut pane, _) = pane_with_replies();
            assert_eq!(pane.image_view, None);
            // First commit: streak 1, below threshold, no promotion.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(pane.image_view, None);
            assert_eq!(pane.promote_streak(), 1);
            // Second commit with no text-screen change between: promoted.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=8,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(
                pane.image_view,
                Some(ImageView {
                    promoted_at: pane.image_activity.unwrap(),
                    commits: 2,
                })
            );
            // Further commits keep the promotion; the fired-at count stays.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=9,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(pane.image_view.unwrap().commits, 2);
        }

        #[test]
        fn queries_placements_and_single_thumbnails_never_promote() {
            let (mut pane, _) = pane_with_replies();
            // One commit alone is a thumbnail, not a viewer.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            // Queries and placements change image state but are not commits:
            // the streak holds at 1 and no promotion fires.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=q,i=7,f=24,s=1,v=1;AQID\x1b\\",
                true,
                true,
            );
            feed_pane_output(&mut pane, b"\x1b_Ga=p,i=7,C=1\x1b\\", true, true);
            assert_eq!(pane.image_view, None);
            assert_eq!(pane.promote_streak(), 1);
            // Legacy placeholder traffic never promotes either.
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,U=1,q=2,i=7,p=1,f=24,s=1,v=1,c=1,r=1;AQID\x1b\\",
                true,
                true,
            );
            assert_eq!(pane.image_view, None);
        }

        #[test]
        fn text_screen_change_demotes_and_resets_the_streak() {
            let (mut pane, _) = pane_with_replies();
            for id in [7, 8] {
                feed_pane_output(
                    &mut pane,
                    format!("\x1b_Ga=T,i={id},f=24,s=1,v=1,C=1;AQID\x1b\\").as_bytes(),
                    true,
                    true,
                );
            }
            assert!(pane.image_view.is_some());
            // Alternate-screen entry clears graphics, demotes, resets streak.
            feed_pane_output(&mut pane, b"\x1b[?1049h", true, true);
            assert_eq!(pane.image_view, None);
            assert_eq!(pane.promote_streak(), 0);
            assert_eq!(pane.image_activity, None);
            // A fresh streak can promote again after demotion.
            for id in [7, 8] {
                feed_pane_output(
                    &mut pane,
                    format!("\x1b_Ga=T,i={id},f=24,s=1,v=1,C=1;AQID\x1b\\").as_bytes(),
                    true,
                    true,
                );
            }
            assert!(pane.image_view.is_some());
        }

        #[test]
        fn native_eviction_pressure_leaves_legacy_images_alone() {
            // Legacy (quiet Unicode-placeholder) images live in a separate
            // store with their own budget. Native quota-pressure eviction must
            // neither drop legacy images nor be blocked by them: fill the
            // native store with the tdf shape (fresh id per page + soft
            // deletes), commit one legacy image, and confirm both survive
            // with the native page still placed.
            let (mut pane, _) = pane_with_replies();
            for id in 2..2 + crate::graphics::MAX_IMAGES as u32 {
                feed_pane_output(&mut pane, b"\x1b_Ga=d,d=a\x1b\\", true, true);
                feed_pane_output(
                    &mut pane,
                    format!("\x1b_Ga=T,i={id},f=24,s=1,v=1,C=1;AQID\x1b\\").as_bytes(),
                    true,
                    true,
                );
            }
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,U=1,q=2,i=7,p=1,f=24,s=1,v=2,c=1,r=1,m=1;AAAA\x1b\\",
                true,
                true,
            );
            feed_pane_output(&mut pane, b"\x1b_Gm=0;BAUG\x1b\\", true, true);
            assert!(pane
                .graphics
                .source(2 + crate::graphics::MAX_IMAGES as u32 - 1)
                .is_some());
            assert_eq!(pane.graphics.placements().len(), 1);
        }

        #[test]
        fn dispatcher_replaces_native_source_only_after_successful_legacy_commit() {
            let (mut pane, _) = pane_with_replies();
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,U=1,q=2,i=7,p=1,f=24,s=1,v=2,c=1,r=1,m=1;AAAA\x1b\\",
                true,
                true,
            );
            assert!(pane.graphics.source(7).is_some());
            feed_pane_output(&mut pane, b"\x1b_Gm=0;BAUG\x1b\\", true, true);
            assert!(pane.graphics.source(7).is_none());
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            assert!(pane.graphics.source(7).is_some());
            pane.grid
                .feed("\x1b[H\x1b[38;2;0;0;7m\x1b[58;2;0;0;1m\u{10eeee}\u{305}\u{305}".as_bytes());
            let mut host = crate::graphics_host::Host::default();
            host.begin();
            pane.legacy_images.begin_row();
            assert_eq!(
                pane.legacy_images.cell(pane.grid.cell(0, 0), &mut host).ch,
                ' '
            );
            assert!(
                host.pending.is_empty(),
                "replaced legacy source cannot be uploaded"
            );
        }

        #[test]
        fn mixed_text_and_apcs_use_each_command_cursor_at_every_split() {
            let input = concat!(
                "aé",
                "\x1b_Ga=T,i=1,f=24,s=1,v=1,C=1;AAAA\x1b\\",
                "XY\x1b[4;6H",
                "\x1b_Ga=T,i=2,f=24,s=1,v=1,C=1;AQID\x1b\\",
                "Z"
            )
            .as_bytes();
            for split in 0..=input.len() {
                let (mut pane, replies) = pane_with_replies();
                feed_pane_output(&mut pane, &input[..split], true, true);
                feed_pane_output(&mut pane, &input[split..], true, true);
                let placements = pane.graphics.placements();
                assert_eq!(placements.len(), 2, "split {split}");
                assert_eq!(
                    (placements[0].row, placements[0].col),
                    (0, 2),
                    "split {split}"
                );
                assert_eq!(
                    (placements[1].row, placements[1].col),
                    (3, 5),
                    "split {split}"
                );
                assert_eq!(pane.grid.cursor_position(), (3, 6), "split {split}");
                assert_eq!(pane.grid.cell(0, 0).ch, 'a');
                assert_eq!(pane.grid.cell(1, 0).ch, 'é');
                assert_eq!(pane.grid.cell(2, 0).ch, 'X');
                assert_eq!(pane.grid.cell(3, 0).ch, 'Y');
                assert_eq!(pane.grid.cell(5, 3).ch, 'Z');
                assert_eq!(&**pane.graphics.source(2).unwrap().pixels, &[1, 2, 3]);
                assert_eq!(
                    replies.bytes(),
                    b"\x1b_Gi=1;OK\x1b\\\x1b_Gi=2;OK\x1b\\",
                    "split {split}"
                );
                assert!(pane.grid.take_pty_replies().is_empty());
            }
        }

        #[test]
        fn graphics_cursor_advance_precedes_following_text_and_cursor_query() {
            let input = b"ab\x1b_Ga=T,i=4,f=24,s=1,v=1,c=3,r=2;AAAA\x1b\\Z\x1b[6n";
            for split in 0..=input.len() {
                let (mut pane, replies) = pane_with_replies();
                feed_pane_output(&mut pane, &input[..split], true, true);
                feed_pane_output(&mut pane, &input[split..], true, true);
                let placement = &pane.graphics.placements()[0];
                assert_eq!((placement.row, placement.col), (0, 2), "split {split}");
                assert_eq!((placement.pixel_width, placement.pixel_height), (24, 32));
                assert_eq!(pane.grid.cell(5, 2).ch, 'Z', "split {split}");
                assert_eq!(pane.grid.cursor_position(), (2, 6), "split {split}");
                assert_eq!(
                    replies.bytes(),
                    b"\x1b_Gi=4;OK\x1b\\\x1b[3;7R",
                    "split {split}"
                );
            }
        }

        #[test]
        fn final_image_chunk_uses_final_cursor_and_only_then_acknowledges() {
            let first = b"\x1b[2;3H\x1b_Ga=T,i=9,f=24,s=2,v=2,C=1,m=1;AAAA\x1b\\BEFORE";
            let last = b"\x1b[8;12H\x1b_Gm=0;/wAAAP8AAAD/\x1b\\!\x1b[6n";
            for split in 0..=last.len() {
                let (mut pane, replies) = pane_with_replies();
                feed_pane_output(&mut pane, first, true, true);
                assert!(replies.bytes().is_empty());
                assert!(pane.graphics.source(9).is_none());
                assert!(pane.graphics.placements().is_empty());
                feed_pane_output(&mut pane, &last[..split], true, true);
                feed_pane_output(&mut pane, &last[split..], true, true);
                let placement = &pane.graphics.placements()[0];
                assert_eq!((placement.row, placement.col), (7, 11), "split {split}");
                assert_eq!(pane.graphics.source(9).unwrap().pixels.len(), 12);
                assert_eq!(pane.grid.cell(11, 7).ch, '!');
                assert_eq!(pane.grid.cursor_position(), (7, 12));
                assert_eq!(
                    replies.bytes(),
                    b"\x1b_Gi=9;OK\x1b\\\x1b[8;13R",
                    "split {split}"
                );
            }
        }

        #[test]
        fn graphics_and_terminal_query_replies_keep_exact_stream_order() {
            let input = concat!(
                "\x1b[14t",
                "\x1b_Ga=q,i=31,f=24,s=1,v=1;AAAA\x1b\\",
                "\x1b[16t\x1b[18t\x1b[c\x1b[5n\x1b[3;7H\x1b[6n",
                "\x1b_Ga=t,i=2,f=24,s=1,v=1;AQID\x1b\\",
                "\x1b[16tXY\x1b[6n",
                "\x1b_Ga=q,i=32,f=24,s=1,v=1;AAAA\x1b\\"
            )
            .as_bytes();
            let expected = concat!(
                "\x1b[4;160;160t\x1b_Gi=31;OK\x1b\\",
                "\x1b[6;16;8t\x1b[8;10;20t\x1b[?6c\x1b[0n\x1b[3;7R",
                "\x1b_Gi=2;OK\x1b\\\x1b[6;16;8t\x1b[3;9R\x1b_Gi=32;OK\x1b\\"
            )
            .as_bytes();
            for split in 0..=input.len() {
                let (mut pane, replies) = pane_with_replies();
                feed_pane_output(&mut pane, &input[..split], true, true);
                feed_pane_output(&mut pane, &input[split..], true, true);
                assert_eq!(replies.bytes(), expected, "split {split}");
                assert!(pane.grid.take_pty_replies().is_empty());
            }
            let (mut pane, replies) = pane_with_replies();
            for byte in input {
                feed_pane_output(&mut pane, std::slice::from_ref(byte), true, true);
            }
            assert_eq!(replies.bytes(), expected, "one byte per read");
        }

        #[test]
        fn exact_tdf_probe_is_acknowledged_only_when_graphics_are_enabled() {
            let input = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c\x1b[16t\x1b[5n";
            for enabled in [false, true] {
                for split in 0..=input.len() {
                    let (mut pane, replies) = pane_with_replies();
                    feed_pane_output(&mut pane, &input[..split], enabled, true);
                    feed_pane_output(&mut pane, &input[split..], enabled, true);
                    let expected: &[u8] = if enabled {
                        b"\x1b_Gi=31;OK\x1b\\\x1b[?6c\x1b[6;16;8t\x1b[0n"
                    } else {
                        b"\x1b[?6c\x1b[6;16;8t\x1b[0n"
                    };
                    assert_eq!(
                        replies.bytes(),
                        expected,
                        "enabled={enabled}, split {split}"
                    );
                    assert!(
                        pane.graphics.source(31).is_none(),
                        "a query never stores pixels"
                    );
                    assert!(pane.graphics.placements().is_empty());
                    assert_eq!(pane.grid.cursor_position(), (0, 0));
                    assert_eq!(pane.grid.visible_text(), "");
                }
            }
        }

        #[test]
        fn separate_panes_reuse_ids_without_cross_routing_partial_streams_or_replies() {
            let (first, first_replies) = pane_with_replies();
            let (second, second_replies) = pane_with_replies();
            let mut panes = HashMap::from([(11u64, first), (22u64, second)]);
            feed_pane_output(
                panes.get_mut(&11).unwrap(),
                b"\x1b_Ga=T,i=7,f=24,s=1,v=2,C=1,m=1;AA",
                true,
                true,
            );
            feed_pane_output(
                panes.get_mut(&22).unwrap(),
                b"\x1b[5;7H\x1b_Ga=T,i=7,f=24,s=1,v=1,C=1;AQID\x1b\\\x1b[6n",
                true,
                true,
            );
            assert!(first_replies.bytes().is_empty());
            assert!(panes[&11].graphics.source(7).is_none());
            let second_expected = b"\x1b_Gi=7;OK\x1b\\\x1b[5;7R";
            assert_eq!(second_replies.bytes(), second_expected);
            feed_pane_output(panes.get_mut(&11).unwrap(), b"AA\x1b\\", true, true);
            assert!(
                first_replies.bytes().is_empty(),
                "a non-final chunk never acknowledges"
            );
            feed_pane_output(
                panes.get_mut(&11).unwrap(),
                b"\x1b[3;4H\x1b_Gm=0;BAUG\x1b\\\x1b[6n",
                true,
                true,
            );
            assert_eq!(first_replies.bytes(), b"\x1b_Gi=7;OK\x1b\\\x1b[3;4R");
            assert_eq!(second_replies.bytes(), second_expected);
            assert_eq!(
                &**panes[&11].graphics.source(7).unwrap().pixels,
                &[0, 0, 0, 4, 5, 6]
            );
            assert_eq!(&**panes[&22].graphics.source(7).unwrap().pixels, &[1, 2, 3]);
            let first_placement = &panes[&11].graphics.placements()[0];
            let second_placement = &panes[&22].graphics.placements()[0];
            assert_eq!((first_placement.row, first_placement.col), (2, 3));
            assert_eq!((second_placement.row, second_placement.col), (4, 6));
            feed_pane_output(
                panes.get_mut(&11).unwrap(),
                b"\x1b_Ga=d,d=A;\x1b\\",
                true,
                true,
            );
            assert!(panes[&11].graphics.source(7).is_none());
            assert!(panes[&11].graphics.placements().is_empty());
            assert!(panes[&22].graphics.source(7).is_some());
            assert_eq!(panes[&22].graphics.placements().len(), 1);
            assert_eq!(second_replies.bytes(), second_expected);
        }

        #[test]
        fn alternate_screen_enter_and_leave_in_one_text_event_clear_graphics() {
            for mode in [47, 1047, 1049] {
                let (mut pane, _) = pane_with_replies();
                feed_pane_output(
                    &mut pane,
                    b"\x1b_Ga=T,i=1,f=24,s=1,v=1,C=1;AAAA\x1b\\",
                    true,
                    true,
                );
                feed_pane_output(
                    &mut pane,
                    b"\x1b_Ga=T,i=9,f=24,s=1,v=2,C=1,m=1;AAAA\x1b\\",
                    true,
                    true,
                );
                assert!(pane.graphics.source(1).is_some());
                assert_eq!(pane.graphics.placements().len(), 1);
                let text = format!("\x1b[?{mode}hALT\x1b[?{mode}l");
                let events = crate::graphics_stream::Stream::default().feed(text.as_bytes());
                assert_eq!(
                    events,
                    vec![crate::graphics_stream::Event::Text(
                        text.as_bytes().to_vec()
                    )]
                );
                let epoch = pane.grid.screen_epoch();
                feed_pane_output(&mut pane, text.as_bytes(), true, true);
                assert!(!pane.grid.alternate_screen(), "mode {mode}");
                assert_ne!(pane.grid.screen_epoch(), epoch, "mode {mode}");
                assert!(pane.graphics.source(1).is_none(), "mode {mode}");
                assert!(pane.graphics.placements().is_empty(), "mode {mode}");
                // Alternate-screen invalidation must discard partial transfers,
                // not just remove the already-visible placement.
                feed_pane_output(&mut pane, b"\x1b_Gm=0;AQID\x1b\\", true, true);
                assert!(pane.graphics.source(9).is_none(), "mode {mode}");
                assert!(pane.graphics.placements().is_empty(), "mode {mode}");
            }
        }

        #[test]
        fn ris_clears_visible_and_pending_graphics_at_every_split() {
            let reset = b"\x1bc";
            for split in 0..=reset.len() {
                let (mut pane, _) = pane_with_replies();
                feed_pane_output(
                    &mut pane,
                    b"\x1b_Ga=T,i=1,f=24,s=1,v=1,C=1;AAAA\x1b\\",
                    true,
                    true,
                );
                feed_pane_output(
                    &mut pane,
                    b"\x1b_Ga=T,i=9,f=24,s=1,v=2,C=1,m=1;AAAA\x1b\\",
                    true,
                    true,
                );
                assert!(pane.graphics.source(1).is_some());
                feed_pane_output(&mut pane, &reset[..split], true, true);
                feed_pane_output(&mut pane, &reset[split..], true, true);
                assert!(pane.graphics.source(1).is_none(), "split {split}");
                assert!(pane.graphics.placements().is_empty(), "split {split}");
                feed_pane_output(&mut pane, b"\x1b_Gm=0;AQID\x1b\\", true, true);
                assert!(pane.graphics.source(9).is_none(), "split {split}");
                assert!(pane.graphics.placements().is_empty(), "split {split}");
            }
        }

        #[test]
        fn cancelling_an_opaque_apc_keeps_grid_and_graphics_parser_synchronized() {
            let input = b"A\x1b_Xopaque\x1b_Ga=T,i=3,f=24,s=1,v=1,C=1;AAAA\x1b\\B\x1b[6n";
            for split in 0..=input.len() {
                let (mut pane, replies) = pane_with_replies();
                feed_pane_output(&mut pane, &input[..split], true, true);
                feed_pane_output(&mut pane, &input[split..], true, true);
                assert_eq!(pane.grid.visible_text(), "AB", "split {split}");
                let placement = &pane.graphics.placements()[0];
                assert_eq!((placement.row, placement.col), (0, 1));
                assert_eq!(
                    replies.bytes(),
                    b"\x1b_Gi=3;OK\x1b\\\x1b[1;3R",
                    "split {split}"
                );
            }
        }

        #[test]
        fn discarded_oversized_continuation_cannot_commit_a_partial_image() {
            let (mut pane, replies) = pane_with_replies();
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=T,i=1,f=24,s=1,v=1,C=1;AQID\x1b\\",
                true,
                true,
            );
            feed_pane_output(
                &mut pane,
                b"\x1b_Ga=t,i=9,f=24,s=1,v=1,m=1;\x1b\\",
                true,
                true,
            );
            let mut oversized = b"\x1b_Gm=1;".to_vec();
            oversized.extend(std::iter::repeat_n(b'A', 64 * 1024));
            oversized.extend_from_slice(b"\x1b\\");
            feed_pane_output(&mut pane, &oversized, true, true);
            feed_pane_output(&mut pane, b"\x1b_Gm=0;AAAA\x1b\\Z\x1b[6n", true, true);
            assert!(
                pane.graphics.source(9).is_none(),
                "dropping an oversized continuation must invalidate the pending transfer"
            );
            assert!(
                !replies
                    .bytes()
                    .windows(b"\x1b_Gi=9;OK".len())
                    .any(|s| s == b"\x1b_Gi=9;OK"),
                "an image with discarded payload cannot be acknowledged as successful"
            );
            assert_eq!(&**pane.graphics.source(1).unwrap().pixels, &[1, 2, 3]);
            assert_eq!(pane.graphics.placements().len(), 1);
            assert_eq!(pane.grid.visible_text(), "Z");
            assert!(replies.bytes().ends_with(b"\x1b[1;2R"));
        }

        #[test]
        fn scroll_pane_forwards_arrows_to_fullscreen_child_instead_of_panning() {
            // Grey ghost-text in nvim pushes the cursor past the pane edge.
            // Hitting `⌥+←/→` there must move the child (sidescroll), never
            // gwae's own h_scroll window, or the pane visibly shoves sideways
            // underneath the editor.
            let (mut pane, replies) = pane_with_replies();
            // Plain shell: pans gwae's window and needs a repaint.
            assert!(pane.scroll_pane(16));
            assert_eq!(pane.h_scroll, 16);
            assert!(replies.bytes().is_empty());
            assert!(pane.scroll_pane(-16));
            assert_eq!(pane.h_scroll, 0);
            // Full-screen nvim: h_scroll stays put, child gets arrows.
            feed_pane_output(&mut pane, b"\x1b[?1049h", true, true);
            assert!(pane.grid.alternate_screen());
            assert!(!pane.scroll_pane(16));
            assert_eq!(pane.h_scroll, 0, "fullscreen child owns horizontal pan");
            assert_eq!(replies.bytes(), b"\x1b[C".repeat(16));
            assert!(!pane.scroll_pane(-1));
            assert_eq!(
                replies.bytes(),
                b"\x1b[C"
                    .repeat(16)
                    .iter()
                    .chain(b"\x1b[D")
                    .cloned()
                    .collect::<Vec<u8>>()
            );
            // Leaving the alt screen restores the multiplexer pan.
            feed_pane_output(&mut pane, b"\x1b[?1049l", true, true);
            assert!(!pane.grid.alternate_screen());
            assert!(pane.scroll_pane(1));
            assert_eq!(pane.h_scroll, 1);
        }
    }
}
