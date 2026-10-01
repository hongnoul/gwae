//! End-to-end: session save/restore (`GWAE_SESSION_RESTORE=1`).
//!
//! gwae is daemon-free, so "attach" means resurrection: the saved layout +
//! cwd state must survive a force quit and rebuild on the next launch. These
//! tests drive the real binary in a real PTY and assert on what the user
//! sees: a saved state file after quitting, and a relaunched pane whose
//! `pwd` is the directory the old pane lived in.

// Unix-only end to end: real PTYs and sh syntax throughout.
#![cfg(unix)]

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

fn temp_root() -> PathBuf {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-session-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    std::fs::create_dir_all(dir.join("state")).expect("temp state dir");
    dir
}

impl Session {
    /// Launch gwae with session restore on, from `cwd`, with extra `args`.
    /// The state dir is pinned inside the temp root via XDG_STATE_HOME so
    /// tests never touch the real machine's saved session.
    fn start(root: &Path, cwd: &Path, args: &[&str]) -> Session {
        std::fs::write(root.join("gwae/gwae.toml"), "startup_panes = 1\n").expect("write config");
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", root);
        cmd.env("XDG_STATE_HOME", root.join("state"));
        cmd.env("XDG_DATA_HOME", root.join("data"));
        cmd.env("HOME", root);
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("GWAE_SESSION_RESTORE", "1");
        // No jcode daemon poll, no native modifier poll: headless CI.
        cmd.env("GWAE_NO_HARNESS_STATUS", "1");
        cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
        cmd.cwd(cwd);
        for a in args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let (tx, rx) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Session {
            rx,
            writer,
            child,
            _master: pair.master,
        }
    }

    fn send_line(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).expect("write line");
        self.writer.write_all(b"\r").expect("write newline");
        self.writer.flush().expect("flush");
    }

    /// Send the force-quit chord and confirm it.
    fn force_quit(&mut self) {
        self.writer.write_all(b"\x1bQ").expect("write chord");
        self.writer.flush().expect("flush");
        std::thread::sleep(Duration::from_millis(600));
        self.writer.write_all(b"\r").expect("write confirm");
        self.writer.flush().expect("flush");
    }

    /// Drain output until quiet (bounded), returning everything seen.
    fn drain(&self) -> String {
        let mut out = Vec::new();
        let mut idle = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(b) => {
                    out.extend_from_slice(&b);
                    idle = 0;
                }
                Err(_) => {
                    idle += 1;
                    if !out.is_empty() && idle >= 3 {
                        break;
                    }
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn wait_exit(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().expect("try_wait").is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("gwae did not exit in time");
    }
}

fn session_file(root: &Path) -> PathBuf {
    root.join("state/gwae/session.json")
}

/// The whole feature, user's-eye view: quit a session, relaunch, and the
/// pane is back in the directory the old one lived in.
#[test]
fn a_quit_session_resurrects_in_its_old_directory() {
    let root = temp_root();
    let proj = root.join("proj-alpha");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    // First life: one shell pane (`run sh` keeps the agent gateway out of
    // the test), started in `proj`, long enough for a save tick (SAVE_POLL
    // is 2s; wait past it) then force quit.
    let mut s = Session::start(&root, &proj, &["run", "sh"]);
    std::thread::sleep(Duration::from_millis(400));
    s.send_line("echo first-life-marker");
    std::thread::sleep(Duration::from_millis(3000));
    s.force_quit();
    s.wait_exit();

    let state = session_file(&root);
    assert!(
        state.exists(),
        "force quit must leave a saved session at {}",
        state.display()
    );
    let json = std::fs::read_to_string(&state).expect("read state");
    assert!(
        json.contains("proj-alpha"),
        "the saved pane cwd must name the project dir; got: {json}"
    );

    // Second life: launched from the *root* (not the project), so a correct
    // restore must come from the state file, not inheritance.
    let mut s2 = Session::start(&root, &root, &[]);
    std::thread::sleep(Duration::from_millis(800));
    s2.send_line("pwd");
    let out = s2.drain();
    assert!(
        out.contains("proj-alpha"),
        "restored pane must open in the saved cwd; saw: {}",
        out.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>()
    );
    s2.force_quit();
    s2.wait_exit();
    let _ = std::fs::remove_dir_all(&root);
}

/// `--new` is the discard path: the next launch must NOT resurrect, and the
/// old state must be archived, not deleted.
#[test]
fn dash_dash_new_discards_but_archives_the_saved_session() {
    let root = temp_root();
    let proj = root.join("proj-beta");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut s = Session::start(&root, &proj, &["run", "sh"]);
    std::thread::sleep(Duration::from_millis(3000));
    s.force_quit();
    s.wait_exit();
    assert!(session_file(&root).exists(), "state must exist after quit");

    // Relaunch with --new from the root: the old state must be archived
    // and the fresh pane must sit in the root, not in proj-beta.
    let mut s2 = Session::start(&root, &root, &["--new", "run", "sh"]);
    std::thread::sleep(Duration::from_millis(800));
    s2.send_line("pwd");
    let out = s2.drain();
    assert!(
        !out.contains("proj-beta"),
        "--new must not resurrect the old cwd; saw: {out}"
    );
    assert!(
        session_file(&root).with_extension("json.bak").exists(),
        "--new must archive the old state to .bak"
    );
    s2.force_quit();
    s2.wait_exit();
    let _ = std::fs::remove_dir_all(&root);
}

/// Exiting the last pane's process by hand ends the session for good: no
/// ghost strip on the next launch.
#[test]
fn exiting_the_last_pane_ends_the_session_not_saves_it() {
    let root = temp_root();
    let proj = root.join("proj-gamma");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut s = Session::start(&root, &proj, &["run", "sh"]);
    std::thread::sleep(Duration::from_millis(3000));
    // A save has happened by now; `exit` must then discard it.
    s.send_line("exit");
    s.wait_exit();
    assert!(
        !session_file(&root).exists(),
        "a deliberate exit must clear the saved session"
    );
    let _ = std::fs::remove_dir_all(&root);
}
