//! End-to-end: `⌥+v` claims the clipboard chord in plain and real fish panes.
//!
//! The real executable runs under a host PTY with an isolated HOME/config and
//! a stub `pbpaste` reading exact fixture bytes. Readiness and completion are
//! observable terminal/file states, never a quiet interval in the output stream.
//! Fish execution is checked with a file sentinel, not repaint-sensitive counts.
//!
//! macOS-only because the clipboard helper table differs on other platforms.
#![cfg(target_os = "macos")]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const HOST: Size = Size {
    rows: 24,
    cols: 120,
};
const TIMEOUT: Duration = Duration::from_secs(15);
const OPT_V: &[u8] = b"\x1bv";
const FISH_BARRIER: &[u8] = b"\x07";

// Noncanonical input makes every pasted byte reach tee immediately. Retaining
// ICRNL/ONLCR lets the terminal show newlines normally, without duplicate echo.
// tee never enables bracketed paste, and captures any unwanted escape markers.
const PLAIN_HELPER: &str = "#!/bin/sh\n\
    stty -echo -icanon min 1 time 0\n\
    printf 'PLAIN_READY\\r\\n'\n\
    exec /usr/bin/tee received-paste\n";

// This binding is an input-processing barrier. Once its sentinel advances,
// fish has processed everything preceding Ctrl+G, including the paste's closing
// bracket. It does not execute or alter the command buffer.
const FISH_INIT: &str = "set -g fish_greeting\n\
    set -g fish_autosuggestion_enabled 0\n\
    function fish_prompt\n    printf 'FISH_READY> '\nend\n\
    function fish_right_prompt\nend\n\
    function paste_probe\n    printf 'EXECUTED\\n' >> paste-executions\nend\n\
    bind \\cg 'printf x >> paste-barrier'\n";

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    screen: Vt100Grid,
    dir: PathBuf,
}

impl Session {
    fn start(clipboard_text: &str) -> Session {
        Self::start_with(clipboard_text, "/bin/sh plain-helper.sh", "PLAIN_READY")
    }

    fn start_with(clipboard_text: &str, pane_cmd: &str, ready: &str) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-paste-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "startup_panes = 1\ndefault_column_width = \"quarter\"\ncontent_width = 0\n\
             center_focus = false\nkeep_awake = false\ncell_labels = false\n\
             [minimap]\nshow = false\n[cowsay]\nenabled = false\n\
             [update]\ncheck = false\n",
        )
        .expect("write config");
        std::fs::write(dir.join("clipboard"), clipboard_text).expect("write clipboard bytes");
        std::fs::write(dir.join("plain-helper.sh"), PLAIN_HELPER).expect("write plain helper");
        std::fs::write(dir.join("fish-init.fish"), FISH_INIT).expect("write fish init");

        // Read a data file rather than interpolating clipboard text into shell
        // syntax. Quotes, actual newlines, and literal backslashes stay exact.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("temp bin dir");
        std::fs::write(
            bin.join("pbpaste"),
            "#!/bin/sh\nexec /bin/cat \"$GWAE_TEST_CLIPBOARD\"\n",
        )
        .expect("write stub pbpaste");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("pbpaste"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub pbpaste");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: HOST.rows,
                cols: HOST.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        // No user shell/editor/history, debug logging, reload handover, terminal
        // identity, or clipboard overrides may leak into this test.
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("ENV", "/dev/null");
        cmd.env("BASH_ENV", "/dev/null");
        cmd.env("LC_ALL", "C");
        cmd.env("GWAE_LOG", "off");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        cmd.env("GWAE_KITTY_GRAPHICS", "0");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_TEST_CLIPBOARD", dir.join("clipboard"));
        cmd.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        cmd.arg("run");
        cmd.arg(pane_cmd);
        let child = pair.slave.spawn_command(cmd).expect("spawn actual gwae");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut s = Session {
            rx,
            writer,
            child,
            _master: pair.master,
            screen: Vt100Grid::new(HOST),
            dir,
        };
        s.wait_for("child prompt/readiness", |s| s.shown().contains(ready));
        s
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    fn shown(&self) -> String {
        self.screen.visible_text()
    }

    fn file(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap_or_default()
    }

    fn wait_for(&mut self, label: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if ready(self) {
                return;
            }
            if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(20)) {
                self.screen.feed(&bytes);
            }
        }
        panic!(
            "{label}: deadline expired\nreceived paste: {:?}\nexecutions: {:?}\nbarrier: {:?}\nhost screen:\n{}",
            self.file("received-paste"),
            self.file("paste-executions"),
            self.file("paste-barrier"),
            self.shown()
        );
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn option_v_pastes_the_clipboard_into_a_plain_pane() {
    let mut s = Session::start("hello-paste");
    s.send(OPT_V);
    s.wait_for(
        "plain clipboard bytes, rendered text, and confirmation",
        |s| {
            let shown = s.shown();
            s.file("received-paste") == b"hello-paste"
                && shown.contains("hello-paste")
                && shown.contains("pasted 1 line")
        },
    );
}

#[test]
fn option_v_multiline_paste_arrives_as_one_block() {
    // Actual newline, not the old literal backslash-n fixture. tee's exact
    // captured bytes also reject unwanted bracket markers or dropped lines.
    let mut s = Session::start("one\ntwo");
    s.send(OPT_V);
    s.wait_for("both clipboard lines and multiline confirmation", |s| {
        let shown = s.shown();
        s.file("received-paste") == b"one\ntwo"
            && shown.contains("one")
            && shown.contains("two")
            && shown.contains("pasted 2 lines")
    });
}

#[test]
fn option_v_in_a_real_fish_pane_pastes_instead_of_opening_an_editor() {
    let fish = [
        "/opt/homebrew/bin/fish",
        "/usr/local/bin/fish",
        "/usr/bin/fish",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file());
    let Some(fish) = fish else {
        eprintln!("skipping: no fish binary found");
        return;
    };
    let pane_cmd = format!(
        "{fish} --no-config --private --interactive --init-command 'source fish-init.fish'"
    );
    // A trailing newline would execute this if pasted without brackets. The
    // sentinel proves actual execution, independently of shell/TUI repaints.
    let command = "paste_probe";
    let mut s = Session::start_with(&format!("{command}\n"), &pane_cmd, "FISH_READY> ");
    s.send(OPT_V);
    s.wait_for("fish buffers the clipboard on its command line", |s| {
        s.shown().contains(&format!("FISH_READY> {command}"))
    });
    s.send(FISH_BARRIER);
    s.wait_for("fish processed the complete paste without executing", |s| {
        s.file("paste-barrier") == b"x"
    });
    assert!(
        !s.dir.join("paste-executions").exists(),
        "paste must not execute before Enter: {:?}\n{}",
        s.file("paste-executions"),
        s.shown()
    );
    assert!(!s.shown().contains("External editor"), "{}", s.shown());

    s.send(b"\r");
    s.send(FISH_BARRIER);
    s.wait_for(
        "Enter executes once and fish returns to reading input",
        |s| s.file("paste-barrier") == b"xx",
    );
    assert_eq!(
        s.file("paste-executions"),
        b"EXECUTED\n",
        "Enter must execute the pasted command exactly once\n{}",
        s.shown()
    );
}
