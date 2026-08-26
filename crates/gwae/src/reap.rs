//! Guaranteed teardown of every process gwae started.
//!
//! The render loop's normal exit path already walks each pane's process tree
//! and SIGKILLs it (`tui::kill_pane_tree`). That covers the *graceful* exits:
//! `⌥+Shift+q`, the last pane closing, closing a pane. It covers nothing else.
//!
//! The ways gwae can leave without running that code are exactly the ways that
//! leak a multiplexer's worth of background work onto the machine:
//!
//! * a signal — `SIGTERM` from a `kill`, `SIGHUP` when the host terminal
//!   window is closed, `SIGINT`/`SIGQUIT` from a stray chord in the host;
//! * a panic in the loop, or an early `return Err(..)` from setup;
//! * `std::process::exit` anywhere above us.
//!
//! In all of those the PTY master fds are closed by the kernel, the panes get
//! a hangup, and *well-behaved* children die. Anything that escaped its
//! process group on purpose (`nohup ... &`, a `setsid` daemon, a language
//! server a pane's editor spawned) survives, invisible, forever. That is the
//! "I killed gwae, why is this still running" report.
//!
//! So this module keeps a registry of live pane root pids that is readable
//! from a signal handler, and reaps from three places:
//!
//! * [`install`] — signal handlers plus a panic hook;
//! * [`Guard`] — a drop guard covering every early return out of the TUI;
//! * [`reap_all`] — the explicit call the normal teardown path makes.
//!
//! Reaping is idempotent: killing an already-dead pid returns `ESRCH`, and
//! pids are deregistered when a pane is reaped normally.
//!
//! ## Why process *groups*, not just pids
//!
//! Each pane's child is spawned on a PTY slave, so it is a session leader with
//! its own process group; its jobs land in that group's descendants. Signalling
//! the group (`killpg`) reaches jobs the `ps`-based tree walk could miss
//! because they were reparented to init between the snapshot and the kill.
//! Both are used: the group catches the racy ones, the tree walk catches the
//! ones that deliberately left the group.

#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Maximum simultaneously tracked panes. Signal handlers may not allocate, so
/// the registry is a fixed array rather than a `Vec`. gwae's layout tops out
/// far below this; overflow degrades to "not tracked by the signal path",
/// which is the old behaviour, never a crash.
#[cfg(unix)]
const MAX_PANES: usize = 256;

/// Registered pane root pids. `0` means the slot is empty.
///
/// `AtomicI32` because a signal handler must not take a lock: `pthread_mutex`
/// is not async-signal-safe, and a handler that interrupts the thread already
/// holding the registry lock would deadlock the process while it is trying to
/// die. Plain atomic loads are safe from a handler.
#[cfg(unix)]
static PIDS: [AtomicUsize; MAX_PANES] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicUsize = AtomicUsize::new(0);
    [Z; MAX_PANES]
};

/// Set once the reaper is armed, so `install` is idempotent.
#[cfg(unix)]
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Track `pid` as a pane root that must not outlive this process.
pub fn register(pid: u32) {
    #[cfg(unix)]
    {
        if pid == 0 {
            return;
        }
        for slot in PIDS.iter() {
            if slot
                .compare_exchange(0, pid as usize, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
        tracing::warn!(
            pid,
            "pane registry full; pane not covered by signal teardown"
        );
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Stop tracking `pid` (its pane was torn down through the normal path).
///
/// Unix only, like every caller: the registry exists to feed `kill(2)`, and
/// Windows has no signal path to feed. Defining it there anyway would be dead
/// code, which `-D warnings` rightly rejects.
#[cfg(unix)]
pub fn unregister(pid: u32) {
    for slot in PIDS.iter() {
        let _ = slot.compare_exchange(pid as usize, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// Currently registered pids, for tests and for the normal teardown sweep.
#[cfg(unix)]
pub fn tracked() -> Vec<u32> {
    PIDS.iter()
        .map(|s| s.load(Ordering::SeqCst))
        .filter(|&p| p != 0)
        .map(|p| p as u32)
        .collect()
}

/// SIGKILL every registered pane root, its process group, and its descendants.
///
/// Safe to call from normal code (it shells out to `ps` for the tree walk).
/// The signal path uses [`reap_all_signal_safe`] instead.
pub fn reap_all() {
    #[cfg(unix)]
    for pid in tracked() {
        let kids = crate::tui::descendants(pid);
        unsafe {
            // Negative pid = "the whole process group", which is where a
            // pane's own jobs live.
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        for k in kids {
            unsafe {
                libc::kill(k as libc::pid_t, libc::SIGKILL);
            }
        }
        unregister(pid);
    }
}

/// The subset of [`reap_all`] that is legal inside a signal handler: atomic
/// loads and `kill(2)` only. No `ps`, no allocation, no locks.
#[cfg(unix)]
unsafe fn reap_all_signal_safe() {
    for slot in PIDS.iter() {
        let pid = slot.swap(0, Ordering::SeqCst);
        if pid == 0 {
            continue;
        }
        let pid = pid as libc::pid_t;
        libc::kill(-pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL);
    }
}

/// Set by the handler to the signal it caught, read by the reaper thread.
#[cfg(unix)]
static PENDING_SIGNAL: AtomicUsize = AtomicUsize::new(0);
/// Set by the reaper thread once the deep sweep is finished.
#[cfg(unix)]
static SWEEP_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Signal handler: hand off to the reaper thread, wait for it, then re-raise.
///
/// The handler cannot do the deep sweep itself. Killing the pane's process
/// *group* is not enough: an interactive shell runs job control, so `cmd &`
/// gets a **new** process group, and `killpg` on the pane's group misses it
/// entirely. Finding those jobs requires walking the real process table, which
/// means `fork`/`exec`/allocation, none of which are async-signal-safe.
///
/// So the handler only does signal-safe work (atomic stores, `write`), and a
/// thread parked since `install` does the walk. The handler then waits, with a
/// bound, for that thread: if the signal happened to interrupt a thread holding
/// an allocator lock the reaper could block forever, and a mux that refuses to
/// die when told to is worse than one that leaks. On timeout it falls back to
/// the signal-safe group kill and dies anyway.
///
/// Re-raising with the disposition reset to `SIG_DFL` is what makes the parent
/// see the true cause of death (`128+signo`) rather than a synthetic exit
/// code, which shells and supervisors branch on.
#[cfg(unix)]
extern "C" fn on_signal(signo: libc::c_int) {
    unsafe {
        SWEEP_DONE.store(false, Ordering::SeqCst);
        PENDING_SIGNAL.store(signo as usize, Ordering::SeqCst);
        // Wake the reaper. One byte on a pipe: `write(2)` is signal-safe.
        let fd = WAKE_WRITE.load(Ordering::SeqCst);
        if fd >= 0 {
            let byte = 1u8;
            let _ = libc::write(
                fd as libc::c_int,
                &byte as *const u8 as *const libc::c_void,
                1,
            );
            // Bounded wait: 100 x 20ms = 2s, then give up and die regardless.
            let mut spins = 0;
            while !SWEEP_DONE.load(Ordering::SeqCst) && spins < 100 {
                let ts = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 20_000_000,
                };
                libc::nanosleep(&ts, std::ptr::null_mut());
                spins += 1;
            }
        }
        // Whatever the reaper managed, this is cheap, safe, and idempotent.
        reap_all_signal_safe();
        restore_terminal_signal_safe();
        libc::signal(signo, libc::SIG_DFL);
        libc::raise(signo);
    }
}

/// Write end of the handler-to-reaper wakeup pipe; -1 until `install` runs.
#[cfg(unix)]
static WAKE_WRITE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Minimal terminal restore usable from a handler: leave the alternate screen,
/// pop the Kitty keyboard flags, re-enable autowrap, show the cursor, disable
/// mouse reporting and bracketed paste. `write(2)` is async-signal-safe;
/// `crossterm` is not.
///
/// Raw mode itself is restored by the shell (it resets termios when it takes
/// the terminal back after a fatal signal); the escape sequences here are the
/// part the shell does *not* undo, and leaving them set is what strands a user
/// in a black alternate screen with no cursor.
#[cfg(unix)]
unsafe fn restore_terminal_signal_safe() {
    // `\x1b[?2004l` is bracketed paste: leaving it on means the user's shell
    // receives `ESC[200~` wrappers it never asked for, which it then echoes as
    // literal text on the next paste.
    const RESTORE: &[u8] =
        b"\x1b[<u\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?7h\x1b[?1049l\x1b[?25h";
    let mut off = 0usize;
    while off < RESTORE.len() {
        let n = libc::write(
            libc::STDOUT_FILENO,
            RESTORE[off..].as_ptr() as *const libc::c_void,
            RESTORE.len() - off,
        );
        if n <= 0 {
            break;
        }
        off += n as usize;
    }
}

/// Park a thread on the wakeup pipe so the deep sweep can run outside handler
/// context. Started before any pane exists, so by the time a signal can
/// arrive the thread is already blocked in `read` with nothing left to
/// allocate.
#[cfg(unix)]
fn spawn_reaper_thread() {
    let mut fds = [0 as libc::c_int; 2];
    // Safety: `pipe(2)` into a two-element array, the documented contract.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        tracing::warn!("reaper pipe failed; signal teardown falls back to group kill");
        return;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    WAKE_WRITE.store(write_fd, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("gwae-reaper".into())
        .spawn(move || loop {
            let mut byte = [0u8; 1];
            // Safety: blocking `read(2)` on the pipe this thread owns.
            let n = unsafe { libc::read(read_fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
            if n <= 0 {
                // EINTR: a signal was delivered to this thread; go round again.
                if n < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                return;
            }
            // The full sweep, including the `ps` tree walk that finds jobs the
            // shell put in their own process group.
            reap_all();
            SWEEP_DONE.store(true, Ordering::SeqCst);
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!("reaper thread: {e}"));
}

/// Arm the teardown paths. Call once, before the first pane is spawned.
///
/// Handles the fatal signals a terminal program actually receives:
/// `SIGHUP` (host window closed), `SIGTERM` (`kill`), `SIGINT`, `SIGQUIT`.
/// `SIGKILL` cannot be caught; nothing can be done there, which is why panes
/// are killed by group too, so a `kill -9` of gwae still lets the PTY hangup
/// take the well-behaved children.
pub fn install() {
    #[cfg(unix)]
    {
        if INSTALLED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        spawn_reaper_thread();
        for &sig in &[libc::SIGHUP, libc::SIGTERM, libc::SIGINT, libc::SIGQUIT] {
            unsafe {
                libc::signal(sig, on_signal as *const () as libc::sighandler_t);
            }
        }
        // A panic unwinds past the teardown code in `run_tui` only if it is
        // caught; with `panic = "abort"`, or a panic in a pane reader thread,
        // nothing runs. Reap from the hook, which runs first in both cases,
        // then defer to the previous hook for the message.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            reap_all();
            unsafe { restore_terminal_signal_safe() };
            prev(info);
        }));
    }
}

/// Drop guard: reaps whatever is still registered when it goes out of scope.
///
/// This is the net under every early `return` in `run_tui` (setup failures,
/// `?` on a render error) and under an unwinding panic. The normal exit path
/// has already unregistered every pane by the time this drops, so it is a
/// no-op there rather than a double kill.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        reap_all();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    /// A pid is only "gone" once it is reaped; a killed child of *this*
    /// process lingers as a zombie until waited on, and a zombie still answers
    /// `kill(pid, 0)`. Spawn through `sh` so the thing we poll is a
    /// grandchild: nobody waits on it, so signal 0 reports the truth.
    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    fn wait_gone(pid: u32) -> bool {
        for _ in 0..200 {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn register_then_unregister_leaves_no_trace() {
        // A pid no real process can have, so a parallel test cannot collide.
        register(4242);
        assert!(tracked().contains(&4242), "registered pid must be tracked");
        unregister(4242);
        assert!(!tracked().contains(&4242), "unregistered pid must be gone");
    }

    #[test]
    fn register_ignores_pid_zero() {
        register(0);
        assert!(!tracked().contains(&0), "pid 0 is not a pane");
    }

    /// The core promise: a registered process is dead after `reap_all`, and
    /// the registry is empty so a second reap is a no-op.
    #[test]
    fn reap_all_kills_a_registered_process() {
        // `sh -c 'exec sleep 300'` so the pid we track is the sleep itself.
        let mut child = Command::new("sh")
            .args(["-c", "exec sleep 300"])
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        register(pid);
        reap_all();
        // Reap the zombie so the assertion below sees a truly gone pid.
        let _ = child.wait();
        assert!(!alive(pid), "reap_all must kill the registered process");
        assert!(
            !tracked().contains(&pid),
            "a reaped pid must be deregistered so a second reap is a no-op"
        );
    }

    /// The leak this module exists for: a background job that outlives the
    /// pane's own shell. Killing only the tracked pid would orphan it.
    #[test]
    fn reap_all_kills_a_backgrounded_grandchild() {
        let dir = std::env::temp_dir().join(format!("gwae-reap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let pidfile = dir.join("bg.pid");
        // The shell backgrounds a sleep, records its pid, and then blocks. The
        // sleep is in the shell's process group, so killpg reaches it.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!("sleep 300 & echo $! > {}; wait", pidfile.display()))
            .spawn()
            .expect("spawn shell");
        let shell_pid = child.id();
        // Wait for the pid file to appear and be complete.
        let mut bg_pid = 0u32;
        for _ in 0..200 {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(p) = s.trim().parse::<u32>() {
                    bg_pid = p;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(bg_pid != 0, "background job must report its pid");
        register(shell_pid);
        reap_all();
        let _ = child.wait();
        assert!(
            wait_gone(bg_pid),
            "a backgrounded grandchild ({bg_pid}) must not survive teardown"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The drop guard is the net under early returns and unwinding panics.
    #[test]
    fn guard_reaps_on_drop() {
        let mut child = Command::new("sh")
            .args(["-c", "exec sleep 300"])
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        register(pid);
        {
            let _g = Guard;
        }
        let _ = child.wait();
        assert!(!alive(pid), "dropping the guard must reap live panes");
    }

    #[test]
    fn install_is_idempotent() {
        install();
        install();
        // Reaching here without aborting is the assertion: a second install
        // must not re-wrap the panic hook or re-register handlers forever.
        assert!(INSTALLED.load(std::sync::atomic::Ordering::SeqCst));
    }
}
