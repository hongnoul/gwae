//! End-to-end: quitting gwae must actually stop what was running in the panes.
//!
//! The force-quit overlay promises "running commands are terminated
//! immediately", and quitting a multiplexer is the one moment a user trusts
//! that nothing is left behind. It was not entirely true. `Child::kill`
//! signals only the process gwae itself spawned, so the pane's shell and the
//! jobs sharing its process group died, but anything that had deliberately
//! escaped that group did not:
//!
//!   nohup long-running-thing &
//!
//! survived the quit and kept running invisibly, with no window left to find
//! it in and no entry in any job table the user still had access to. On a
//! laptop that means a process burning CPU for hours after its session is
//! gone. These tests drive real quits against a real gwae and assert on the
//! actual process table afterwards.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A running gwae whose single pane is a plain `sh`, so the tests can type
/// shell syntax (`nohup ... &`) into it and know exactly what will run.
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    dir: std::path::PathBuf,
    /// Everything gwae has written to its PTY. The signal tests read the tail
    /// to prove the terminal was actually handed back (alt screen left, cursor
    /// shown) rather than merely assuming the handler ran.
    output: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    fn start() -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gwae-teardown-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), "").expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("GWAE_NO_INSTALL", "1");
        // A predictable POSIX shell, not whatever the developer running the
        // tests happens to use.
        cmd.env("SHELL", "/bin/sh");
        cmd.arg("run");
        cmd.arg("sh -i");
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut b = [0u8; 8192];
            while let Ok(n) = reader.read(&mut b) {
                if n == 0 {
                    break;
                }
                sink.lock().expect("lock").extend_from_slice(&b[..n]);
            }
        });

        // Let the mux paint and the pane's shell reach its prompt before
        // anything is typed at it.
        std::thread::sleep(Duration::from_secs(3));
        Session {
            child,
            _master: pair.master,
            writer,
            dir,
            output,
        }
    }

    fn type_line(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).expect("write");
        self.writer.write_all(b"\r").expect("write newline");
        self.writer.flush().expect("flush");
    }

    /// Send the force-quit chord and confirm it.
    fn force_quit(&mut self) {
        // ⌥+⇧+q opens the confirmation; Enter commits it.
        self.writer.write_all(b"\x1bQ").expect("write chord");
        self.writer.flush().expect("flush");
        std::thread::sleep(Duration::from_millis(600));
        self.writer.write_all(b"\r").expect("write confirm");
        self.writer.flush().expect("flush");
    }

    /// Send a signal straight to the gwae process, bypassing every key-driven
    /// teardown path.
    fn signal(&mut self, sig: libc::c_int) {
        let pid = self.child.process_id().expect("gwae pid");
        // Safety: `kill(2)` on a pid we just spawned and still own.
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Is any process whose command line contains `needle` currently running?
fn any_running(needle: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-Ao", "command="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        // Skip the `ps` invocation itself and any shell wrapper quoting it.
        .filter(|l| !l.contains("-Ao"))
        .any(|l| l.contains(needle))
}

/// Wait for `needle` to appear, so a test never races the shell's startup.
fn wait_until_running(needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if any_running(needle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{needle} never started; the test cannot prove anything about teardown");
}

/// Wait for `needle` to disappear, then report whether it did.
fn gone_within(needle: &str, how_long: Duration) -> bool {
    let deadline = Instant::now() + how_long;
    while Instant::now() < deadline {
        if !any_running(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Make sure a leaked process from a failed run can never outlive the test
/// and confuse the next one (or sit on the developer's machine).
struct Reaper(&'static str);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", self.0])
            .output();
    }
}

/// The headline case: a job that escaped its process group must still die.
///
/// `nohup ... &` is the ordinary way a user detaches something, and it is
/// precisely what used to survive the quit.
#[test]
fn force_quit_kills_a_nohupped_background_job() {
    const MARKER: &str = "sleep 24601";
    let _reap = Reaper(MARKER);

    let mut s = Session::start();
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    s.force_quit();

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "a nohupped job survived the force quit; \
         quitting promises running commands are terminated immediately"
    );
}

/// Teardown must reach all the way down, not just one generation.
///
/// A detached child that itself detached a grandchild is the shape most
/// real tools have (a supervisor and its worker), so stopping at depth one
/// would still leak the process actually doing the work.
#[test]
fn force_quit_kills_a_whole_detached_tree() {
    const PARENT: &str = "sleep 24602";
    const CHILD: &str = "sleep 24603";
    let _reap_p = Reaper(PARENT);
    let _reap_c = Reaper(CHILD);

    let mut s = Session::start();
    s.type_line(&format!(
        "nohup sh -c 'nohup {CHILD} >/dev/null 2>&1 & {PARENT}' >/dev/null 2>&1 &"
    ));
    wait_until_running(PARENT);
    wait_until_running(CHILD);

    s.force_quit();

    assert!(
        gone_within(PARENT, Duration::from_secs(10)),
        "the detached parent survived the force quit"
    );
    assert!(
        gone_within(CHILD, Duration::from_secs(10)),
        "the detached grandchild survived the force quit; \
         teardown must walk the whole tree, not just one generation"
    );
}

/// Closing a pane is the same promise, scoped to that pane: its work stops.
///
/// This session has one pane, so ⌥+q closes it and gwae exits, which is the
/// path a user takes far more often than the force-quit chord.
#[test]
fn closing_the_last_pane_kills_its_detached_work() {
    const MARKER: &str = "sleep 24604";
    let _reap = Reaper(MARKER);

    let mut s = Session::start();
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    // ⌥+q closes the focused pane.
    s.writer.write_all(b"\x1bq").expect("write chord");
    s.writer.flush().expect("flush");

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "closing the pane left its detached job running"
    );
}

/// The case the graceful paths never covered: gwae is *killed*, not quit.
///
/// `kill gwae` (SIGTERM) is what a supervisor, a script, or an impatient user
/// sends. Before the signal handler existed, this bypassed every teardown
/// path: gwae vanished, the PTY masters closed, well-behaved children got a
/// hangup and died, and anything detached kept running with no window left to
/// find it in. The promise is the same whichever way gwae leaves.
#[test]
fn sigterm_kills_detached_work() {
    const MARKER: &str = "sleep 24605";
    let _reap = Reaper(MARKER);

    let mut s = Session::start();
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    s.signal(libc::SIGTERM);

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "a detached job survived SIGTERM to gwae; being killed must tear down \
         the panes exactly like quitting does"
    );
}

/// Closing the host terminal window sends SIGHUP. It is the most common
/// abnormal exit there is, and it must not strand a pane's work.
#[test]
fn sighup_kills_detached_work() {
    const MARKER: &str = "sleep 24606";
    let _reap = Reaper(MARKER);

    let mut s = Session::start();
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    s.signal(libc::SIGHUP);

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "a detached job survived SIGHUP; closing the terminal window must not \
         leave background processes behind"
    );
}

/// A pane's *ordinary* foreground work (no `nohup`) must die too. This is the
/// common case, and asserting it separately keeps the signal path honest if
/// the group-kill is ever removed in favour of the tree walk alone.
#[test]
fn sigterm_kills_ordinary_pane_work() {
    const MARKER: &str = "sleep 24607";
    let _reap = Reaper(MARKER);

    let mut s = Session::start();
    s.type_line(&format!("{MARKER} &"));
    wait_until_running(MARKER);

    s.signal(libc::SIGTERM);

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "an ordinary background job in a pane survived SIGTERM to gwae"
    );
}

/// Being killed must also hand the terminal back usable.
///
/// A mux that dies in the alternate screen with the cursor hidden leaves the
/// user staring at a black rectangle they have to `reset` out of, so the
/// signal path writes the restore sequences before re-raising.
#[test]
fn signal_death_restores_the_terminal() {
    let mut s = Session::start();
    s.output.lock().expect("lock").clear();
    s.signal(libc::SIGTERM);
    std::thread::sleep(Duration::from_secs(1));
    let tail = String::from_utf8_lossy(&s.output.lock().expect("lock")).into_owned();
    assert!(
        tail.contains("\x1b[?1049l"),
        "dying by signal must leave the alternate screen; got {tail:?}"
    );
    assert!(
        tail.contains("\x1b[?25h"),
        "dying by signal must show the cursor again; got {tail:?}"
    );
}

/// The exit status must still say "killed by SIGTERM", not a synthetic code:
/// shells and supervisors branch on `128+signo`, and swallowing the signal
/// would make gwae look like it exited cleanly when it was actually killed.
///
/// `waitpid` directly rather than `Child::wait`: `portable_pty` flattens every
/// signal death into exit code 1, discarding exactly the fact under test. The
/// spawned gwae is a direct child of this process, so the raw wait status is
/// available as long as nothing has reaped it first.
#[test]
fn signal_death_preserves_the_exit_status() {
    let mut s = Session::start();
    let pid = s.child.process_id().expect("gwae pid") as libc::pid_t;
    s.signal(libc::SIGTERM);

    let mut status: libc::c_int = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reaped = false;
    while Instant::now() < deadline {
        // Safety: `waitpid` on a direct child, WNOHANG so the test can bound
        // its own wait rather than blocking forever on a wedged process.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if r == pid {
            reaped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        reaped,
        "gwae did not exit within 10s of SIGTERM; teardown must never wedge \
         a process that was told to die"
    );
    // WIFSIGNALED/WTERMSIG, open-coded: the libc crate does not export the
    // macros. Low 7 bits are the terminating signal; 0 means a normal exit.
    let termsig = status & 0x7f;
    assert_eq!(
        termsig,
        libc::SIGTERM,
        "gwae must die of the signal it was sent (raw wait status {status:#x}); \
         supervisors read 128+signo, so exiting normally here would lie"
    );
}
