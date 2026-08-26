//! End-to-end: which directory a new pane starts in.
//!
//! The unit tests in `spawndir` pin the resolution rules; these pin the thing
//! the user actually experiences, by making the pane's *first command* print
//! its working directory. A pane that opens in the wrong tree is the entire
//! bug this feature exists to fix, so the assertion is on real `pwd` output
//! from a real PTY child, not on a resolved `PathBuf`.

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
    /// Temp dir holding the config and the fake project tree.
    home: PathBuf,
}

/// A unique temp root per test, holding `gwae/gwae.toml` plus whatever
/// project directories the case needs.
fn temp_root() -> PathBuf {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-spawndir-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    dir
}

impl Session {
    /// Launch gwae with `config`, running `pane_cmd` in the first pane, from
    /// the working directory `cwd`.
    fn start(root: &Path, config: &str, pane_cmd: &str, cwd: &Path, args: &[&str]) -> Session {
        std::fs::write(root.join("gwae/gwae.toml"), config).expect("write config");
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
        cmd.env("TERM", "xterm-256color");
        // The picker's candidate scan and `~` expansion both key off HOME;
        // pointing it at the temp tree keeps the test off the real machine.
        cmd.env("HOME", root);
        cmd.cwd(cwd);
        for a in args {
            cmd.arg(a);
        }
        cmd.arg("run");
        cmd.arg(pane_cmd);
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
            home: root.to_path_buf(),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

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
                    if idle >= 3 {
                        break;
                    }
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// A pane is ~23 columns wide, so a temp path printed by `pwd` is wrapped and
/// interleaved with frame escapes: unreadable as an assertion. Instead the
/// pane *writes* its cwd to a file, and the test reads that. The file is proof
/// the child really started there, which is the property under test; the
/// screen rendering of it is the theme tests' business.
fn probe_script(root: &Path, name: &str) -> PathBuf {
    let script = root.join(format!("{name}.sh"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\npwd > \"{}\"\nsleep 60\n",
            root.join(name).display()
        ),
    )
    .expect("write probe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("probe is executable");
    }
    script
}

/// Wait for a probe file to appear and return its contents, trimmed.
fn probe_result(root: &Path, name: &str) -> String {
    let path = root.join(name);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(t) = std::fs::read_to_string(&path) {
            if !t.trim().is_empty() {
                return t.trim().to_string();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    String::new()
}

const OPT_D: &[u8] = b"\x1bd";
const OPT_S: &[u8] = b"\x1bs";
const DOWN: &[u8] = b"\x1b[B";
const ENTER: &[u8] = b"\r";

#[test]
fn a_pane_inherits_gwae_cwd_when_nothing_is_configured() {
    let root = temp_root();
    let work = root.join("inherited-marker");
    std::fs::create_dir_all(&work).unwrap();
    let script = probe_script(&root, "where");
    let s = Session::start(&root, "", script.to_str().unwrap(), &work, &[]);
    let got = probe_result(&root, "where");
    assert!(
        got.ends_with("inherited-marker"),
        "with no agent_dir the pane keeps gwae's own cwd; got {got:?}"
    );
    s.kill();
}

#[test]
fn agent_dir_moves_the_pane_and_expands_tilde() {
    let root = temp_root();
    let proj = root.join("configured-marker");
    std::fs::create_dir_all(&proj).unwrap();
    let script = probe_script(&root, "where");
    let s = Session::start(
        &root,
        // `~` must expand: it is how a config file is actually written, and
        // an unexpanded one would fail spawn with a literal `~` directory.
        "agent_dir = \"~/configured-marker\"\n",
        script.to_str().unwrap(),
        &root,
        &[],
    );
    let got = probe_result(&root, "where");
    assert!(
        got.ends_with("configured-marker"),
        "agent_dir should place the pane; got {got:?}"
    );
    let _ = proj;
    s.kill();
}

#[test]
fn dir_flag_beats_the_config_file() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("configured-marker")).unwrap();
    let flagged = root.join("flagged-marker");
    std::fs::create_dir_all(&flagged).unwrap();
    let script = probe_script(&root, "where");
    let s = Session::start(
        &root,
        "agent_dir = \"~/configured-marker\"\n",
        script.to_str().unwrap(),
        &root,
        &["--dir", flagged.to_str().unwrap()],
    );
    let got = probe_result(&root, "where");
    assert!(
        got.ends_with("flagged-marker"),
        "--dir is this-run intent and must win; got {got:?}"
    );
    assert!(
        !got.ends_with("configured-marker"),
        "and the config value must not win; got {got:?}"
    );
    s.kill();
}

#[test]
fn a_missing_agent_dir_still_opens_the_pane_and_says_so() {
    let root = temp_root();
    let work = root.join("fallback-marker");
    std::fs::create_dir_all(&work).unwrap();
    let script = probe_script(&root, "where");
    let s = Session::start(
        &root,
        "agent_dir = \"~/definitely-not-here\"\n",
        script.to_str().unwrap(),
        &work,
        &[],
    );
    let got = probe_result(&root, "where");
    let out = s.drain();
    assert!(
        got.ends_with("fallback-marker"),
        "a bad agent_dir must never stop a pane from opening; got {got:?}"
    );
    assert!(
        out.contains("agent_dir"),
        "and the typo should be surfaced, not silently ignored; got:\n{out}"
    );
    s.kill();
}

#[test]
fn the_picker_finds_projects_by_marker_whatever_the_layout() {
    let root = temp_root();
    // Deliberately *not* under a directory called git/code/src: discovery
    // must key off the marker, not off a name gwae could have guessed.
    std::fs::create_dir_all(root.join("wherever/alpha-proj/.git")).unwrap();
    std::fs::create_dir_all(root.join("Documents/clients/beta-proj/.hg")).unwrap();
    std::fs::create_dir_all(root.join("wherever/plain-dir")).unwrap();
    let mut s = Session::start(&root, "", "sleep 60", &root, &[]);
    let _ = s.drain();
    s.send(OPT_D);
    let out = s.drain();
    assert!(
        out.contains("alpha-proj") && out.contains("beta-proj"),
        "⌥+d should discover projects by marker, whatever the layout; got:\n{out}"
    );
    assert!(
        out.contains("spawn dir"),
        "and the panel should name itself; got:\n{out}"
    );
    assert!(
        !out.contains("plain-dir"),
        "a directory with no project marker is not a candidate; got:\n{out}"
    );
    s.kill();
}

#[test]
fn picking_a_directory_moves_the_next_pane_there() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("some/where/picked-proj/.git")).unwrap();
    let mut s = Session::start(&root, "", "sleep 60", &root, &[]);
    let _ = s.drain();
    s.send(OPT_D);
    let _ = s.drain();
    // Type the filter, then take the top match for this session.
    s.send(b"picked");
    let filtered = s.drain();
    assert!(
        filtered.contains("picked-proj"),
        "typing should filter to the match; got:\n{filtered}"
    );
    s.send(ENTER);
    let noted = s.drain();
    assert!(
        noted.contains("picked-proj"),
        "the choice should be confirmed on screen; got:\n{noted}"
    );
    // A new column spawns a shell; ask it where it is.
    s.send(b"\x1b\r"); // ⌥+Enter: new column
    let _ = s.drain();
    s.send(format!("pwd > {}\r", root.join("after").display()).as_bytes());
    let got = probe_result(&root, "after");
    assert!(
        got.ends_with("picked-proj"),
        "panes spawned after the pick should start there; got {got:?}"
    );
    s.kill();
}

#[test]
fn saving_from_the_picker_writes_agent_dir_to_the_config() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("some/where/saved-proj/.git")).unwrap();
    let cfg = root.join("gwae/gwae.toml");
    let mut s = Session::start(
        &root,
        // A comment and an unrelated key: the rewrite must preserve both,
        // since the config is a hand-edited file.
        "# my config\nscroll_margin = 4\n",
        "sleep 60",
        &root,
        &[],
    );
    let _ = s.drain();
    s.send(OPT_D);
    let _ = s.drain();
    s.send(b"saved");
    let _ = s.drain();
    s.send(OPT_S);
    let out = s.drain();
    assert!(
        out.contains("saved"),
        "it should confirm the save; got:\n{out}"
    );
    let text = std::fs::read_to_string(&cfg).expect("config still readable");
    assert!(
        text.contains("agent_dir = "),
        "the pick should be persisted; got:\n{text}"
    );
    assert!(
        text.contains("saved-proj"),
        "with the directory that was highlighted; got:\n{text}"
    );
    assert!(
        text.contains("# my config") && text.contains("scroll_margin = 4"),
        "and the rest of the file must survive; got:\n{text}"
    );
    s.kill();
}

#[test]
fn escape_cancels_the_picker_without_changing_anything() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("some/where/other-proj/.git")).unwrap();
    let start = root.join("start-marker");
    std::fs::create_dir_all(&start).unwrap();
    let mut s = Session::start(&root, "", "sleep 60", &start, &[]);
    let _ = s.drain();
    s.send(OPT_D);
    let _ = s.drain();
    s.send(DOWN);
    let _ = s.drain();
    s.send(b"\x1b");
    let _ = s.drain();
    s.send(b"\x1b\r");
    let _ = s.drain();
    s.send(format!("pwd > {}\r", root.join("after").display()).as_bytes());
    let got = probe_result(&root, "after");
    assert!(
        got.ends_with("start-marker"),
        "esc must leave the spawn dir alone; got {got:?}"
    );
    s.kill();
}
