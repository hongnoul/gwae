//! Hot reload: replace gwae's own binary in place, keeping every pane alive.
//!
//! gwae is daemon-free (ADR-003 reversed, ADR-011), so there is no server to
//! keep panes running while the client restarts. That made every code change
//! cost a full restart: quit, lose four agents mid-task, relaunch, re-open
//! everything. The usual fix is a dev-only daemon, which is worse than it
//! looks: the dev build and the shipped build stop being the same program, so
//! bugs hide in whichever path you are not running that day.
//!
//! The mechanism used instead needs no daemon at all. A PTY master fd is just
//! a file descriptor, and file descriptors survive `execve` when their
//! close-on-exec flag is cleared. So gwae can hand its own panes to a *new
//! image of itself*:
//!
//! ```text
//! old image                              new image (same pid)
//! ─────────                              ────────────────────
//! serialize layout + fds + pids  ──┐
//! clear FD_CLOEXEC on each master  │
//! restore the terminal             │
//! execve(own path) ────────────────┴──>  read handover, adopt fds,
//!                                        reinstall signal handlers,
//!                                        repaint from the children's
//!                                        own scrollback
//! ```
//!
//! The pid never changes, the children are never signalled, and the shell in
//! each pane does not learn that anything happened.
//!
//! ## What is verified, and what is merely hoped
//!
//! Three facts this design rests on were measured before it was written, not
//! assumed (see `tests/reload_e2e.rs`, which asserts them against the real
//! binary):
//!
//! 1. A PTY master fd survives `execve` and still reads, writes, and accepts
//!    `TIOCSWINSZ` afterwards.
//! 2. `Layout` already round-trips through serde, so the pane tree needs no
//!    parallel representation.
//! 3. **Signal handlers do not survive `execve`.** They are reset to
//!    `SIG_DFL`. This is the dangerous one: a reloaded gwae that forgets to
//!    reinstall them still *looks* fine, and then leaks every pane's
//!    background jobs the next time the host window is closed. [`adopt`] is
//!    therefore responsible for re-arming [`crate::reap`] before anything
//!    else can go wrong, and a teardown test covers exactly that.
//!
//! ## Why this is gated
//!
//! Enabled by `GWAE_DEV_RELOAD=1` only. The failure mode of a subtly wrong
//! reload is not a bad frame, it is orphaned agent processes on a user's
//! machine, so the shipped default stays "restart like before" until the
//! teardown tests have lived in CI for a while. The same mechanism is what
//! upgrade-in-place will use once it does ship.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Environment variable carrying the handover file path across the `execve`.
///
/// The state travels in a file rather than in the variable itself because it
/// contains a pane tree; the variable is just the pointer. Its presence is
/// also the signal that this process *is* a reload, which is why [`handover`]
/// removes it from the environment as soon as it is read: a pane that later
/// spawns a child must not inherit a stale pointer.
pub const HANDOVER_VAR: &str = "GWAE_RELOAD_HANDOVER";

/// Environment variable that opts a session into hot reload at all.
pub const ENABLE_VAR: &str = "GWAE_DEV_RELOAD";

/// One pane, as it must be described to the next image of gwae.
///
/// Deliberately tiny: a raw fd, the pid it belongs to, and the size it was
/// last given. Everything else about a pane (its grid contents, its scroll
/// position, its OSC 133 status) is *recoverable* or cheap to lose, and
/// carrying it would mean versioning a much larger structure across builds
/// that are by definition different code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneHandover {
    /// The layout's id for this pane, so the tree and the fds can be rejoined.
    pub id: u64,
    /// The PTY master file descriptor, inherited across the exec.
    pub fd: i32,
    /// The pane's root process, re-registered with the reaper on the far side.
    pub pid: Option<u32>,
    /// Logical grid size, so the new image can rebuild a grid of the right
    /// shape without resizing the child (which would reflow its output).
    pub cols: u16,
    pub rows: u16,
    /// Whether this pane runs the agent gateway, so `⌥+;` panes stay agent
    /// panes when they are later respawned.
    pub is_agent: bool,
}

/// Everything the next image of gwae needs to continue this session.
///
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handover {
    /// The serialized pane tree.
    pub layout: gwae_layout::Layout,
    /// The panes, by layout id.
    pub panes: Vec<PaneHandover>,
    /// The spawn directory in force when the reload happened, so a `⌥+d`
    /// choice is not silently forgotten by the new image.
    pub spawn_dir: Option<PathBuf>,
    /// The binary that was running, purely so the new image can log what it
    /// replaced when a reload misbehaves.
    pub from: PathBuf,
}

impl Handover {
    /// Write the handover to a temp file and return its path.
    ///
    /// A file in the temp dir rather than a pipe or an env blob: it survives
    /// the exec without a reader on the other end, it is trivially
    /// inspectable when a reload goes wrong, and it is bounded in size in a
    /// way an environment variable is not.
    ///
    /// JSON rather than the config file's TOML: `Layout` keys its pane map by
    /// integer pane id, which TOML cannot express at all. The alternative was
    /// to reshape the layout model to suit a transport format, which would be
    /// the tail wagging the dog. `serde_json` is already in the tree (it is
    /// how `gwae-layout` verifies its own round trip) and the "std + serde
    /// only" rule binds that pure library, not this binary.
    pub fn write(&self) -> Result<PathBuf, String> {
        let path = std::env::temp_dir().join(format!("gwae-reload-{}.json", std::process::id()));
        let text = serde_json::to_string(self).map_err(|e| format!("encode handover: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("write handover: {e}"))?;
        Ok(path)
    }

    /// Read and delete the handover left by the previous image, if this
    /// process is a reload at all.
    ///
    /// The file is removed immediately: it names file descriptors, which are
    /// meaningless to any later process, and leaving it behind would let a
    /// crash-and-restart loop adopt fds that have since been recycled.
    pub fn take() -> Option<Handover> {
        let path = std::env::var_os(HANDOVER_VAR)?;
        // Remove from the environment before anything can spawn a child that
        // would otherwise inherit a pointer to a consumed handover.
        std::env::remove_var(HANDOVER_VAR);
        let text = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_file(&path);
        match text.as_deref().map(serde_json::from_str::<Handover>) {
            Some(Ok(h)) => Some(h),
            Some(Err(e)) => {
                tracing::error!("reload handover is unreadable ({e}); starting fresh");
                None
            }
            None => {
                tracing::error!("reload handover file vanished; starting fresh");
                None
            }
        }
    }
}

/// Whether hot reload is enabled for this session.
///
/// Off unless explicitly asked for. See the module docs: a wrong reload leaks
/// processes, so this stays opt-in until the teardown tests have earned it.
pub fn enabled() -> bool {
    matches!(
        std::env::var(ENABLE_VAR).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Clear the close-on-exec flag on `fd` so it survives `execve`.
///
/// `portable-pty` sets `FD_CLOEXEC` on every master it opens, which is the
/// right default (a pane's child must not inherit other panes' fds). Reload
/// is the one moment we want the opposite, and only for the process that is
/// about to replace itself.
#[cfg(unix)]
pub fn make_inheritable(fd: i32) -> Result<(), String> {
    use std::io::Error;
    // Safety: `fcntl` with a fd this process owns; failures are reported.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(format!("F_GETFD on {fd}: {}", Error::last_os_error()));
    }
    let cleared = flags & !libc::FD_CLOEXEC;
    if unsafe { libc::fcntl(fd, libc::F_SETFD, cleared) } == -1 {
        return Err(format!("F_SETFD on {fd}: {}", Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn make_inheritable(_fd: i32) -> Result<(), String> {
    Err("hot reload is unix-only".into())
}

/// Whether `exe` is a binary this machine will actually let us `exec`.
///
/// This exists because of a failure mode that is fatal and completely silent.
/// On macOS, a Mach-O binary carries a code signature (ad-hoc, for a locally
/// built one). Overwriting that file in place — which is exactly what
/// `cargo build` and `make install` do — can leave the on-disk image with a
/// signature the kernel rejects. `execve` then does not fail with an errno
/// that could be reported; **the kernel SIGKILLs the process mid-exec**. The
/// old image is already gone, so a reload that hits this takes the whole
/// session and every pane with it, with nothing in any log:
///
/// ```text
/// AMFI: '/path/to/gwae' has no CMS blob?
/// proc 1234: load code signature error 2 for file "gwae"
/// ASP: Security policy would not allow process: 1234
/// ```
///
/// (Found the hard way: the reload appeared to "do nothing", and the session
/// died. The kernel log above was the only evidence it happened at all.)
///
/// So the new image is *proved loadable in a throwaway child* before this
/// process commits to becoming it. A child that dies costs one `fork`; the
/// process that skips this check costs the user their whole session.
#[cfg(unix)]
pub fn is_loadable(exe: &std::path::Path) -> Result<(), String> {
    let out = std::process::Command::new(exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run new binary: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = out.status.signal() {
            // 9 here is the code-signing kill described above, not a crash in
            // gwae's own startup.
            return Err(format!(
                "new binary was killed by signal {sig} on exec                  (code signature invalid? try `codesign -f -s - {}`)",
                exe.display()
            ));
        }
    }
    Err(format!(
        "new binary exited with {} on --version",
        out.status
    ))
}

#[cfg(not(unix))]
pub fn is_loadable(_exe: &std::path::Path) -> Result<(), String> {
    Err("hot reload is unix-only".into())
}

/// The binary to exec: this process's own path.
///
/// Resolved fresh rather than remembered from argv, because the whole point
/// is that the file at this path has *changed* since we started.
pub fn own_path() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("current_exe: {e}"))
}

/// The mtime of the running binary, used to notice a rebuild.
pub fn binary_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Replace this process with a fresh image of `exe`, carrying `handover`.
///
/// On success this function does not return: the process's code is replaced
/// while its pid, its open file descriptors, and its children all stay put.
/// On failure it returns an error and the caller must carry on running, since
/// the panes are still perfectly alive.
///
/// The caller is responsible for restoring the terminal first (leaving raw
/// mode and the alternate screen). Terminal modes are kernel tty state, not
/// process state, so they are *not* reset by the exec: a new image that
/// assumed cooked mode would inherit raw mode and behave bizarrely.
#[cfg(unix)]
pub fn exec_into(
    exe: &std::path::Path,
    handover: &Handover,
) -> Result<std::convert::Infallible, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // Prove the new image can actually be loaded *before* this process
    // commits to becoming it. See [`is_loadable`]: the failure mode is a
    // silent SIGKILL during `execve`, which would take every pane with it.
    is_loadable(exe)?;
    for p in &handover.panes {
        make_inheritable(p.fd)?;
    }
    let path = handover.write()?;
    std::env::set_var(HANDOVER_VAR, &path);

    let cexe = CString::new(exe.as_os_str().as_bytes()).map_err(|e| format!("exe path: {e}"))?;
    // argv[0] only; every other input is either in the handover or in the
    // environment, and re-passing the original arguments would re-run
    // one-shot startup behaviour (`run <cmd>` would spawn the command again).
    let argv = [cexe.as_ptr(), std::ptr::null()];
    // Safety: `execvp` with a NUL-terminated argv. It only returns on error.
    // Safety: `execv` with a NUL-terminated argv. It only returns on error.
    unsafe {
        libc::execv(cexe.as_ptr(), argv.as_ptr());
    }
    let err = std::io::Error::last_os_error();
    // The exec failed, so this image is still running and still owns the
    // panes. Clean up the handover we were about to use, and un-set the
    // pointer, so a later successful reload does not find this stale file.
    std::env::remove_var(HANDOVER_VAR);
    let _ = std::fs::remove_file(&path);
    Err(format!("execv {}: {err}", exe.display()))
}

#[cfg(not(unix))]
pub fn exec_into(
    _exe: &std::path::Path,
    _handover: &Handover,
) -> Result<std::convert::Infallible, String> {
    Err("hot reload is unix-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Handover {
        Handover {
            layout: gwae_layout::Layout::default(),
            panes: vec![PaneHandover {
                id: 7,
                fd: 11,
                pid: Some(4242),
                cols: 80,
                rows: 24,
                is_agent: true,
            }],
            spawn_dir: Some(PathBuf::from("/tmp")),
            from: PathBuf::from("/usr/local/bin/gwae"),
        }
    }

    #[test]
    fn handover_round_trips_through_the_file() {
        let h = sample();
        let path = h.write().expect("write");
        std::env::set_var(HANDOVER_VAR, &path);
        let back = Handover::take().expect("take");
        assert_eq!(back, h);
        // Consumed exactly once: the file is gone and so is the pointer, so a
        // later process cannot adopt file descriptors that no longer mean
        // anything.
        assert!(!path.exists(), "handover file should be consumed");
        assert!(std::env::var_os(HANDOVER_VAR).is_none());
        assert!(Handover::take().is_none());
    }

    #[test]
    fn a_corrupt_handover_starts_fresh_instead_of_dying() {
        // Losing a session is bad; refusing to start is worse. A damaged
        // handover must degrade to a normal launch.
        let path =
            std::env::temp_dir().join(format!("gwae-reload-bad-{}.json", std::process::id()));
        std::fs::write(&path, "{not json").unwrap();
        std::env::set_var(HANDOVER_VAR, &path);
        assert!(Handover::take().is_none());
        assert!(!path.exists(), "even a bad handover is cleaned up");
    }

    #[test]
    fn a_missing_handover_file_starts_fresh() {
        std::env::set_var(
            HANDOVER_VAR,
            std::env::temp_dir().join("gwae-reload-nope.json"),
        );
        assert!(Handover::take().is_none());
        assert!(std::env::var_os(HANDOVER_VAR).is_none());
    }

    #[test]
    fn reload_is_off_unless_asked_for() {
        std::env::remove_var(ENABLE_VAR);
        assert!(!enabled());
        std::env::set_var(ENABLE_VAR, "0");
        assert!(!enabled());
        std::env::set_var(ENABLE_VAR, "1");
        assert!(enabled());
        std::env::remove_var(ENABLE_VAR);
    }

    #[cfg(unix)]
    #[test]
    fn make_inheritable_clears_cloexec() {
        // A pipe is a stand-in for a PTY master: same fd semantics.
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let fd = fds[0];
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        make_inheritable(fd).expect("clear cloexec");
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0,
            "the fd must survive execve"
        );
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[cfg(unix)]
    #[test]
    fn make_inheritable_reports_a_bad_fd() {
        assert!(make_inheritable(-1).is_err());
    }
}
