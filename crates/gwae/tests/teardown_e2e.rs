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
use std::time::{Duration, Instant};

/// A running gwae whose single pane is a plain `sh`, so the tests can type
/// shell syntax (`nohup ... &`) into it and know exactly what will run.
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    dir: std::path::PathBuf,
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
        std::thread::spawn(move || {
            let mut b = [0u8; 8192];
            while let Ok(n) = reader.read(&mut b) {
                if n == 0 {
                    break;
                }
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
