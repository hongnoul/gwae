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
#[allow(dead_code)]
pub const ENABLE_VAR: &str = "GWAE_DEV_RELOAD";

/// Environment variable that opts a dev session into *automatic* rebuilds:
/// the running image watches its own sources and runs the build itself, so
/// `make dev` needs no manual `make dev-build` step. Implies nothing on its
/// own: the session still only execs into a binary that builds cleanly and
/// proves loadable, so a broken tree never disturbs the live panes.
#[allow(dead_code)]
pub const WATCH_VAR: &str = "GWAE_DEV_WATCH";

/// Whether this session rebuilds its own binary when sources change.
///
/// Off unless explicitly asked for. Gated separately from [`ENABLE_VAR`]
/// because the exec path is the dangerous part (orphaned processes on a
/// wrong reload); the watch only *builds*, and a failed build is invisible.
#[allow(dead_code)]
pub fn watch_enabled() -> bool {
    matches!(
        std::env::var(WATCH_VAR).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Shared, bounded buffer of a build child's output.
///
/// `Arc<Mutex<..>>` because the drain threads write it and the event loop
/// reads it when the child exits.
#[allow(dead_code)]
pub type BuildLog = std::sync::Arc<std::sync::Mutex<String>>;

/// Largest tail kept from a build's output, in bytes.
///
/// The overlay only ever shows the last lines, and a long build (or a crate
/// that warns in a loop) would otherwise grow this without bound for the life
/// of the session.
#[allow(dead_code)]
const BUILD_LOG_CAP: usize = 64 * 1024;

/// Continuously drain a build child's stdout and stderr into a shared buffer.
///
/// **This is what keeps the build from deadlocking, and the deadlock is not
/// hypothetical.** A pipe is a fixed kernel buffer (64KB on macOS). The event
/// loop polls the child with `try_wait` and never reads, so a build that
/// out-talks that buffer blocks in `write` and stays blocked: the child never
/// exits, so `try_wait` returns `None` forever, so nothing ever drains the
/// pipe. Worse, the wedged `cargo` still holds the target lock, so every later
/// build in the workspace blocks behind it, and the only symptom in the UI is
/// a HUD pill that spins forever. (Found one stuck 30+ minutes at 0.06s CPU
/// with no `rustc` children.)
///
/// Both pipes must be drained, not just stderr: cargo writes to both, and
/// either one filling is enough to stop the child.
///
/// Returns immediately; the threads end at EOF, which the child's exit
/// guarantees.
#[allow(dead_code)]
pub fn drain_build_output(child: &mut std::process::Child) -> BuildLog {
    let log: BuildLog = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut readers: Vec<Box<dyn std::io::Read + Send>> = Vec::new();
    if let Some(e) = child.stderr.take() {
        readers.push(Box::new(e));
    }
    if let Some(o) = child.stdout.take() {
        readers.push(Box::new(o));
    }
    for mut reader in readers {
        let log = std::sync::Arc::clone(&log);
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut s) = log.lock() {
                            s.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if s.len() > BUILD_LOG_CAP {
                                // Keep the tail, and cut on a char boundary:
                                // slicing mid-UTF-8 would panic, and build
                                // output is full of non-ASCII (arrows in
                                // rustc diagnostics, paths).
                                let mut cut = s.len() - BUILD_LOG_CAP / 2;
                                while cut < s.len() && !s.is_char_boundary(cut) {
                                    cut += 1;
                                }
                                *s = s[cut..].to_string();
                            }
                        }
                    }
                }
            }
        });
    }
    log
}

/// The last `max_lines` lines of a drained build log, for the error overlay.
#[allow(dead_code)]
pub fn build_log_tail(log: &BuildLog, max_lines: usize) -> String {
    let Ok(text) = log.lock() else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

/// Human phase of an in-flight dev rebuild, for the HUD status pill.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPhase {
    Building,
    Linking,
    Signing,
}

/// Outcome of one finished `cargo build` attempt.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum BuildOutcome {
    /// The binary is newer and `is_loadable` passed: safe to exec.
    Ready,
    /// The build failed or the image is unloadable: stay on this image.
    /// Carries the first error line for the on-demand overlay.
    Failed(String),
}

/// The newest mtime under the watched source roots, if any are readable.
///
/// Roots are resolved from the current exe (`target/debug/gwae` ->
/// workspace root): `crates/`, `Cargo.toml`, `Cargo.lock`. No watcher
/// dependency: one walk per poll at the config-poll rate is negligible
/// next to the render loop, mirroring the config mtime approach.
#[allow(dead_code)]
pub fn source_mtime() -> Option<std::time::SystemTime> {
    let exe = std::env::current_exe().ok()?;
    // `.../target/debug/gwae` -> `...` (workspace root).
    let root = exe
        .parent()? // debug/
        .parent()? // target/
        .parent()?; // workspace root
    let mut newest: Option<std::time::SystemTime> = None;
    fn consider(p: &std::path::Path, newest: &mut Option<std::time::SystemTime>) {
        if let Ok(md) = std::fs::metadata(p) {
            if let Ok(m) = md.modified() {
                *newest = Some(newest.map_or(m, |n: std::time::SystemTime| n.max(m)));
            }
            if md.is_dir() {
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        let path = e.path();
                        // Skip build artifacts and VCS state: they churn
                        // without meaning anything changed.
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name == "target" || name == ".git" {
                                continue;
                            }
                        }
                        consider(&path, newest);
                    }
                }
            }
        }
    }
    for rel in ["crates", "Cargo.toml", "Cargo.lock"] {
        consider(&root.join(rel), &mut newest);
    }
    newest
}

/// Classify build stderr into a HUD phase: dependency compilation reads as
/// building, the final link step as linking. Best-effort only.
#[allow(dead_code)]
pub fn classify_phase(line: &str) -> BuildPhase {
    if line.contains("Linking") || line.contains("linking") {
        BuildPhase::Linking
    } else {
        BuildPhase::Building
    }
}

/// First meaningful error line from `cargo build` output, for the overlay.
#[allow(dead_code)]
pub fn first_error_line(output: &str) -> String {
    output
        .lines()
        .find(|l| l.contains("error"))
        .map(|l| {
            let l = l.trim();
            l.chars().take(120).collect::<String>()
        })
        .unwrap_or_else(|| "build failed".to_string())
}

/// Outcome of one finished dev rebuild: the worst failure mode is exec'ing
/// a half-written or unloadable binary, so every path that is not "fresh
/// mtime plus loadable image" reports failure and the caller stays put.
#[allow(dead_code)]
pub fn build_outcome(
    build_ok: bool,
    stderr_tail: &str,
    exe: &std::path::Path,
    before: Option<std::time::SystemTime>,
) -> BuildOutcome {
    if !build_ok {
        return BuildOutcome::Failed(first_error_line(stderr_tail));
    }
    let after = binary_mtime(exe);
    let advanced = match (before, after) {
        (Some(b), Some(a)) => a > b,
        (None, Some(_)) => true,
        _ => false,
    };
    if !advanced {
        // Nothing changed on disk (cached build): not a failure, but
        // nothing to exec into either. Report it as a quiet non-event.
        return BuildOutcome::Failed(String::new());
    }
    match is_loadable(exe) {
        Ok(()) => BuildOutcome::Ready,
        Err(e) => BuildOutcome::Failed(first_line(&e)),
    }
}

/// First line of an error, for one-line toasts and overlays.
#[allow(dead_code)]
fn first_line(e: &str) -> String {
    e.lines().next().unwrap_or(e).trim().to_string()
}

/// Small text badge stamped onto the bottom frame row of the Option HUD
/// while dev mode is on (`GWAE_DEV_RELOAD=1`), mirroring the keep-awake
/// badge on the top frame row. Plain text, not a palette change, so stable
/// sessions (no env) render a plain frame.
pub const DEV_BADGE: &str = " DEV ";

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
    /// Whether this pane was created by `⌥+;`.
    ///
    /// Carried so the reloaded image's agent-pane set still describes the
    /// same panes. It does not resurrect anything: a pane whose process dies
    /// is closed rather than respawned.
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
    /// Milliseconds the old image had been up when it exec'd. The new
    /// image shifts its quiet timers by this gap so panes do not flap
    /// to attention across a reload. Defaults to 0 for old handovers.
    #[serde(default)]
    pub boot_ago_ms: u64,
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
    #[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn is_loadable(_exe: &std::path::Path) -> Result<(), String> {
    Err("hot reload is unix-only".into())
}

/// The binary to exec: this process's own path.
///
/// Resolved fresh rather than remembered from argv, because the whole point
/// is that the file at this path has *changed* since we started.
#[allow(dead_code)]
pub fn own_path() -> Result<PathBuf, String> {
    let path = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    Ok(undeleted(path))
}

/// Strip Linux's `" (deleted)"` marker from a `/proc/self/exe` path.
///
/// A rebuild replaces the file, which unlinks the inode this process is
/// running. Linux then resolves `/proc/self/exe` to `"<path> (deleted)"`,
/// which is not the name of any file: stat'ing it fails, so the watcher
/// never sees the new build's mtime and a reload never fires. macOS
/// resolves by path and never shows this. The suffix is only dropped when
/// the literal path does not exist but the trimmed one does, so a real file
/// whose name ends in `" (deleted)"` is still handled correctly.
#[allow(dead_code)]
fn undeleted(path: PathBuf) -> PathBuf {
    const MARKER: &str = " (deleted)";
    if path.exists() {
        return path;
    }
    let Some(text) = path.to_str() else {
        return path;
    };
    let Some(trimmed) = text.strip_suffix(MARKER) else {
        return path;
    };
    let candidate = PathBuf::from(trimmed);
    if candidate.exists() {
        candidate
    } else {
        path
    }
}

/// The mtime of the running binary, used to notice a rebuild.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn exec_into(
    _exe: &std::path::Path,
    _handover: &Handover,
) -> Result<std::convert::Infallible, String> {
    Err("hot reload is unix-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Serializes the tests that touch process-wide environment variables.
    ///
    /// `HANDOVER_VAR` and `ENABLE_VAR` are global state, and cargo runs tests
    /// in threads of one process, so two of these racing produce failures
    /// that look like real bugs and vanish when run alone. (They did exactly
    /// that: green in isolation, red in the full workspace run.)
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take [`ENV_LOCK`], ignoring poisoning: a panic in one env test must not
    /// cascade into unrelated failures in the others.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

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
            boot_ago_ms: 0,
        }
    }

    #[test]
    fn old_handover_without_clock_field_still_parses() {
        // `boot_ago_ms` defaults: a handover written by the previous
        // image loads with zero gap rather than failing.
        let text = r#"{"layout":{"rows":[],"panes":{},"focus":{"row":0,"column":0,"pane":0},"next_pane":0,"next_row":0},"panes":[],"spawn_dir":null,"from":"/bin/gwae"}"#;
        let back: Handover = serde_json::from_str(text).expect("old handover parses");
        assert_eq!(back.boot_ago_ms, 0);
    }

    #[test]
    fn build_outcome_demands_fresh_mtime_and_loadable_image() {
        // A failed build never execs, whatever the mtime says.
        let exe = std::path::Path::new("/bin/sh");
        let before = binary_mtime(exe);
        assert!(matches!(
            build_outcome(false, "error: boom", exe, before),
            BuildOutcome::Failed(_)
        ));
        // A "successful" build that changed nothing is a quiet non-event
        // (empty message), not a failure and not a reload.
        assert!(matches!(
            build_outcome(true, "", exe, before),
            BuildOutcome::Failed(m) if m.is_empty()
        ));
    }

    #[test]
    fn first_error_line_extracts_the_signal() {
        assert_eq!(
            first_error_line("compiling foo\nerror: expected `;`\nmore"),
            "error: expected `;`"
        );
        assert_eq!(first_error_line("all good"), "build failed");
    }

    /// A child that out-talks the pipe buffer must still finish.
    ///
    /// This is a regression test for a real deadlock, so it asserts against
    /// the failing shape first. `try_wait` never reads, so an undrained child
    /// writing more than one pipe buffer (64KB) blocks in `write` forever:
    /// it never exits, so `try_wait` never reports it, so nothing ever drains
    /// the pipe. The wedged `cargo` keeps the target lock the whole time,
    /// which is what blocked every other build in the workspace.
    ///
    /// ~256KB of output, several times the buffer, so the undrained case
    /// cannot pass by luck on a platform with a roomier pipe.
    #[test]
    fn a_chatty_build_finishes_only_when_its_output_is_drained() {
        fn spawn_chatty() -> std::process::Child {
            std::process::Command::new("sh")
                .arg("-c")
                // Both streams, since draining only one still wedges.
                .arg(
                    "i=0; while [ $i -lt 2000 ]; do \
                     echo 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'; \
                     echo 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' >&2; \
                     i=$((i+1)); done",
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn chatty child")
        }

        fn finishes_within(child: &mut std::process::Child, how_long: Duration) -> bool {
            let deadline = std::time::Instant::now() + how_long;
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        }

        // The bug: polled but never drained, it wedges.
        let mut undrained = spawn_chatty();
        let undrained_finished = finishes_within(&mut undrained, Duration::from_secs(5));
        let _ = undrained.kill();
        let _ = undrained.wait();
        assert!(
            !undrained_finished,
            "precondition: an undrained chatty child is supposed to deadlock, \
             so this test can prove the drain is what fixes it. It finished, \
             which means the pipe buffer here is large enough to swallow the \
             test's output and the test is no longer proving anything"
        );

        // The fix: drained continuously, it completes.
        let mut drained = spawn_chatty();
        let log = drain_build_output(&mut drained);
        assert!(
            finishes_within(&mut drained, Duration::from_secs(20)),
            "a drained build must finish; if this hangs, the dev session's \
             auto-build wedges and holds cargo's target lock"
        );
        let _ = drained.wait();

        // And the output is actually captured, since the error overlay reads
        // it. Give the drain threads a moment to see EOF.
        std::thread::sleep(Duration::from_millis(300));
        let tail = build_log_tail(&log, 40);
        assert!(
            tail.contains("aaaa") && tail.contains("bbbb"),
            "both streams must be captured for the overlay; got {} chars",
            tail.len()
        );
    }

    /// The captured log is bounded, and never split mid-character.
    ///
    /// A long build would otherwise grow this for the life of the session.
    /// The truncation keeps the tail, so it must cut on a `char` boundary:
    /// slicing a `String` inside a UTF-8 sequence panics, and build output is
    /// full of non-ASCII (rustc's arrows, box drawing, paths).
    #[test]
    fn the_build_log_stays_bounded_without_splitting_a_character() {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            // Multi-byte characters, so a byte-offset cut would panic.
            .arg("i=0; while [ $i -lt 3000 ]; do printf '→→→→→→→→→→→→→→→→→→→→→→→→→→→→→→→→\\n'; i=$((i+1)); done")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        let log = drain_build_output(&mut child);
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(300));

        let len = log.lock().expect("log").len();
        assert!(
            len <= BUILD_LOG_CAP,
            "the build log must stay bounded: {len} > {BUILD_LOG_CAP}"
        );
        // Reaching here without a panic is the UTF-8 assertion: an unaligned
        // cut would have aborted the drain thread inside the mutex.
        assert!(
            build_log_tail(&log, 5).contains('→'),
            "the tail must still be readable text after truncation"
        );
    }

    /// A build with nothing to say is not an error.
    #[test]
    fn draining_a_silent_child_yields_an_empty_tail() {
        let mut child = std::process::Command::new("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        let log = drain_build_output(&mut child);
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(build_log_tail(&log, 40), "");
    }

    #[test]
    fn watch_flag_is_opt_in() {
        let _env = env_guard();
        std::env::remove_var(WATCH_VAR);
        assert!(!watch_enabled());
        std::env::set_var(WATCH_VAR, "1");
        assert!(watch_enabled());
        std::env::remove_var(WATCH_VAR);
    }

    #[test]
    fn handover_round_trips_through_the_file() {
        let _env = env_guard();
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
        let _env = env_guard();
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
        let _env = env_guard();
        std::env::set_var(
            HANDOVER_VAR,
            std::env::temp_dir().join("gwae-reload-nope.json"),
        );
        assert!(Handover::take().is_none());
        assert!(std::env::var_os(HANDOVER_VAR).is_none());
    }

    #[test]
    fn reload_is_off_unless_asked_for() {
        let _env = env_guard();
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

#[cfg(test)]
mod deleted_path_tests {
    use super::*;

    /// A replaced binary is still watchable by its real path.
    ///
    /// Linux reports `/proc/self/exe` as `"<path> (deleted)"` after a rebuild
    /// unlinks the running image, and stat'ing that name fails, so the hot
    /// reload watcher would never see the new build.
    #[test]
    fn a_replaced_binary_resolves_back_to_its_real_path() {
        let dir = std::env::temp_dir().join(format!("gwae-deleted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture directory");
        let real = dir.join("gwae-bin");
        std::fs::write(&real, b"new build").expect("write replacement build");

        let reported = PathBuf::from(format!("{} (deleted)", real.display()));
        assert!(!reported.exists(), "the reported name is not a real file");
        assert_eq!(undeleted(reported), real);
        assert!(
            binary_mtime(&undeleted(PathBuf::from(format!(
                "{} (deleted)",
                real.display()
            ))))
            .is_some(),
            "the watcher can stat the resolved path"
        );

        // An existing path is never rewritten, including a real file whose
        // own name ends in the marker.
        assert_eq!(undeleted(real.clone()), real);
        let literal = dir.join("odd (deleted)");
        std::fs::write(&literal, b"real file").expect("write literal file");
        assert_eq!(undeleted(literal.clone()), literal);

        // Nothing on disk: keep the reported name rather than inventing one.
        let missing = dir.join("absent (deleted)");
        assert_eq!(undeleted(missing.clone()), missing);
        std::fs::remove_dir_all(&dir).ok();
    }
}
