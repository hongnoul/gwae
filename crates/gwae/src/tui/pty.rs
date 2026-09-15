//! PTY pane ownership: types, spawning, adoption, teardown, sync (verbatim move from `tui/mod.rs`).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::time::Instant;

use gwae_layout::{Layout, PaneId};
use gwae_term::{Size as GridSize, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};

use crate::config::Config;
use crate::geometry::CellPixels;
use super::render::column_grid_sizes;
use crate::tui::chrome_rows;

use super::shell::agent_gateway_cmd;
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
}

/// Graphics placements use the cursor at their position in the byte stream,
/// never the cursor after a whole PTY read. Replies go only to this child.
pub(crate) fn feed_pane_output(pane: &mut PtyPane, bytes: &[u8], graphics_enabled: bool) {
    use crate::graphics_stream::Event;
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
        }
        let mut replies = pane.grid.take_pty_replies();
        if let Event::Apc(apc) = event {
            if graphics_enabled {
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
                replies.extend_from_slice(&outcome.replies);
            }
        }
        if !replies.is_empty() {
            let _ = pane.writer.write_all(&replies);
            let _ = pane.writer.flush();
        }
    }
}

/// Message a per-pane reader thread sends to the main loop.
pub(crate) enum PaneMsg {
    Output(PaneId, Vec<u8>),
    Exited(PaneId),
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
        pty_size: size,
        alive: true,
        h_scroll: 0,
        last_output: Instant::now(),
        saw_osc133: false,
        graphics_stream: Default::default(),
        graphics: Default::default(),
        legacy_images: Default::default(),
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
    })
}

/// Ask every adopted pane's child to repaint, so a reloaded screen is not
/// blank until the user types.
///
/// `Ctrl-L` is the closest thing to a universal "redraw" a terminal program
/// understands: shells redraw their prompt, and full-screen apps (vim, agent
/// TUIs) repaint their whole surface. Sending it costs nothing when the child
/// ignores it.
pub(crate) fn nudge_repaint(panes: &mut HashMap<PaneId, PtyPane>) {
    for p in panes.values_mut() {
        let _ = p.writer.write_all(b"\x0c");
        let _ = p.writer.flush();
    }
}

/// Use the same drawable geometry before spawn and on every resize. A child
/// may measure its terminal immediately, before the first compositor frame.
pub(crate) fn pane_grid_sizes(layout: &Layout, host: GridSize, cfg: &Config) -> Vec<(PaneId, GridSize)> {
    layout
        .rows
        .iter()
        .flat_map(|r| &r.columns)
        .flat_map(|col| {
            let sizes = column_grid_sizes(
                col.width,
                col.panes.len(),
                host,
                cfg.content_width,
                true,
                chrome_rows(cfg),
            );
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
pub(crate) fn sync_panes(
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    tx: &Sender<PaneMsg>,
    geometry: (GridSize, CellPixels),
    agent_panes: &HashSet<PaneId>,
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
    // Spawn missing panes. Agent panes (created via the spawn-agent verb) run
    // the agent gateway, which becomes the harness; everything else gets the
    // shell.
    for (pid, size) in pane_grid_sizes(layout, geometry.0, cfg) {
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
        let pane = spawn_pane(pid, &cmd, size.cols, size.rows, tx.clone(), cwd, geometry.1)?;
        panes.insert(pid, pane);
        tracing::debug!(pid, "spawned pane");
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
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

}
