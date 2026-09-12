//! Optional acceptance tests for the bundled Yazi configuration.
//! Requires Yazi >= 26.8.15 on PATH, then:
//! cargo test -p gwae --test yazi_e2e -- --ignored --nocapture
//! Uses real Yazi, real PTY resizes, and the rendered terminal screen.
#![cfg(unix)]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);
const LONG_NAME: &str = "FILE_NAME_that_needs_more_horizontal_room.txt";

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    screen: Vt100Grid,
    dir: PathBuf,
    client_id: String,
}

impl Session {
    fn start(cols: u16, in_gwae: bool, pane_env: bool, setup: bool) -> Self {
        Self::configured(
            cols,
            in_gwae,
            pane_env,
            if setup {
                "require('gwae-responsive'):setup()\nrequire('gwae-responsive'):setup()\n"
            } else {
                ""
            },
            "",
        )
    }

    fn configured(cols: u16, in_gwae: bool, pane_env: bool, init: &str, config_text: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let client_id = (u64::from(std::process::id()) * 10000 + serial as u64).to_string();
        let dir = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("gwae-yazi-e2e-{}-{}", std::process::id(), serial));
        let config = dir.join("yazi");
        let cwd = dir.join("files/current");
        std::fs::create_dir_all(config.join("plugins/gwae-responsive.yazi")).unwrap();
        std::fs::create_dir_all(dir.join("gwae")).unwrap();
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(dir.join("files/UP"), "parent sibling\n").unwrap();
        std::fs::write(cwd.join("CURRENT.txt"), "PREVIEW\n").unwrap();
        // This unhovered filename can only appear in the file list, never
        // in the header, selected-file status, or selected-file preview.
        std::fs::write(cwd.join(LONG_NAME), "another file\n").unwrap();
        // Runtime loading lets packaged crates compile these ignored tests
        // without requiring the repository-level examples at compile time.
        let plugin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/yazi/gwae-responsive.yazi/main.lua");
        std::fs::copy(plugin, config.join("plugins/gwae-responsive.yazi/main.lua"))
            .expect("run this optional acceptance suite from the gwae repository");
        std::fs::write(config.join("init.lua"), init).unwrap();
        std::fs::write(config.join("yazi.toml"), config_text).unwrap();
        std::fs::write(
            config.join("keymap.toml"),
            "[[mgr.prepend_keymap]]\non = '<C-g>'\nrun = 'plugin gwae-responsive'\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "default_column_width = 'quarter'\ncontent_width = 0\nstartup_panes = 1\n\
             cell_labels = false\n[minimap]\nshow = false\n[cowsay]\nenabled = false\n\
             [update]\ncheck = false\n",
        )
        .unwrap();
        let pair = native_pty_system().openpty(pty_size(cols)).unwrap();
        let mut cmd = CommandBuilder::new(if in_gwae {
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into())
        } else {
            "yazi".into()
        });
        cmd.env_clear();
        cmd.cwd(&cwd);
        cmd.env("PATH", std::env::var_os("PATH").unwrap());
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("YAZI_CONFIG_HOME", &config);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_STATE_HOME", dir.join("state"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        // Never join a user's live Yazi DDS socket or other test instances.
        cmd.env("TMPDIR", dir.join("tmp"));
        cmd.env("XDG_RUNTIME_DIR", dir.join("tmp"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("LC_ALL", "en_US.UTF-8");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        cmd.env("GWAE_KITTY_GRAPHICS", "0");
        cmd.env("GWAE_LOG", "off");
        cmd.env("YAZI_LOG", "error");
        if pane_env {
            cmd.env("GWAE_PANE", "0");
        }
        if in_gwae {
            cmd.arg("run");
            cmd.arg(format!("yazi --client-id {client_id}"));
        } else {
            cmd.arg("--client-id");
            cmd.arg(&client_id);
        }
        eprintln!("Yazi acceptance command: {:?}", cmd.get_argv());
        let child = pair
            .slave
            .spawn_command(cmd)
            .expect("Yazi >= 26.8.15 must be installed on PATH");
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut buf = [0; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            master: pair.master,
            writer,
            rx,
            screen: Vt100Grid::new(Size { rows: 30, cols }),
            dir,
            client_id,
        }
    }

    fn pump(&mut self) {
        if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(30)) {
            self.screen.feed(&bytes);
        }
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "Yazi exited: {}",
            self.screen.visible_text()
        );
    }

    fn expect(&mut self, parent: bool, preview: bool, label: &str) {
        self.expect_text(label, |text| {
            text.contains("CURRENT.txt")
                && text.contains("UP") == parent
                && text.contains("PREVIEW") == preview
        });
        eprintln!("PASS {label}: parent={parent}, preview={preview}");
    }

    fn expect_text(&mut self, label: &str, predicate: impl Fn(&str) -> bool) {
        // Require a stable frame rather than accepting an intermediate paint.
        let deadline = Instant::now() + TIMEOUT;
        let mut matching_since = None;
        while Instant::now() < deadline {
            self.pump();
            let text = self.screen.visible_text();
            if predicate(&text) {
                let since = matching_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_millis(300) {
                    return;
                }
            } else {
                matching_since = None;
            }
        }
        panic!(
            "{label}: expected frame not observed\nscreen:\n{}\nlogs: {:?}",
            self.screen.visible_text(),
            self.dir.join("state/yazi")
        );
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn resize(&mut self, cols: u16) {
        self.master.resize(pty_size(cols)).unwrap();
        self.screen.resize(Size { rows: 30, cols });
    }

    fn text_column(&self, token: &str) -> u16 {
        let chars: Vec<_> = token.chars().collect();
        for y in 0..self.screen.size().rows {
            for x in 0..=self.screen.size().cols - chars.len() as u16 {
                if chars
                    .iter()
                    .enumerate()
                    .all(|(i, ch)| self.screen.cell(x + i as u16, y).ch == *ch)
                {
                    return x;
                }
            }
        }
        panic!("missing {token:?}: {}", self.screen.visible_text());
    }

    fn save_screen(&self, name: &str) {
        let path = self.dir.join(format!("{name}.txt"));
        std::fs::write(&path, self.screen.visible_text()).unwrap();
        eprintln!("Rendered evidence: {}", path.display());
    }

    fn activate_via_cli(&self) {
        // The documented command from a Yazi subshell, with this isolated
        // test instance's environment rather than the user's live session.
        let output = std::process::Command::new("ya")
            .args(["emit", "plugin", "gwae-responsive"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.dir)
            .env("YAZI_ID", &self.client_id)
            .env("YAZI_CONFIG_HOME", self.dir.join("yazi"))
            .env("XDG_CONFIG_HOME", &self.dir)
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_DATA_HOME", self.dir.join("data"))
            .env("XDG_RUNTIME_DIR", self.dir.join("tmp"))
            .env("TMPDIR", self.dir.join("tmp"))
            .output()
            .expect("ya CLI must be installed beside Yazi");
        assert!(
            output.status.success(),
            "ya emit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.writer.write_all(b"q");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pty_size(cols: u16) -> PtySize {
    PtySize {
        rows: 30,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[test]
#[ignore = "requires installed Yazi >= 26.8.15"]
fn yazi_progressively_reveals_panels_in_real_gwae() {
    let mut s = Session::start(215, true, false, true);
    s.expect(false, false, "quarter width");
    s.send(b"\x1br");
    s.expect(false, true, "third width");
    s.send(b"\x1br");
    s.expect(true, true, "half width");
    s.send(b"\x1br");
    s.expect(false, false, "back to quarter");
    s.send(b"\x1bf");
    s.expect(true, true, "fullscreen");
    s.send(b"\x1bf");
    s.expect(false, false, "restore quarter");
    s.resize(400);
    s.expect(true, true, "wider host terminal");
    s.resize(215);
    s.expect(false, false, "restore host terminal");
}

#[test]
#[ignore = "requires installed Yazi >= 26.8.15"]
fn yazi_breakpoints_and_live_activation_are_reversible() {
    let mut s = Session::start(63, false, true, false);
    s.expect(true, true, "before live activation");
    s.send(b"\x07");
    s.expect(false, false, "live activation");
    for (cols, parent, preview) in [
        (64, false, true),
        (95, false, true),
        (96, true, true),
        (95, false, true),
        (64, false, true),
        (63, false, false),
        (40, false, false),
        (160, true, true),
        (63, false, false),
    ] {
        s.resize(cols);
        s.expect(parent, preview, &format!("{cols}-column boundary"));
    }
    s.send(b"\x07");
    s.expect(false, false, "repeated activation");
    s.resize(96);
    s.expect(true, true, "full ratio survives repeated activation");
}

#[test]
#[ignore = "requires installed Yazi >= 26.8.15"]
fn yazi_outside_gwae_is_unchanged() {
    let mut baseline = Session::start(53, false, false, false);
    baseline.expect(true, true, "standalone unmodified narrow baseline");
    let narrow = baseline.screen.visible_text();
    baseline.resize(107);
    baseline.expect(true, true, "standalone unmodified wide baseline");
    let wide = baseline.screen.visible_text();
    drop(baseline);
    let mut s = Session::start(53, false, false, true);
    s.expect(true, true, "standalone narrow terminal");
    assert_eq!(s.screen.visible_text(), narrow);
    s.send(b"\x07");
    s.expect(true, true, "standalone plugin activation");
    assert_eq!(s.screen.visible_text(), narrow);
    s.resize(107);
    s.expect(true, true, "standalone wide terminal");
    assert_eq!(s.screen.visible_text(), wide);
    s.resize(53);
    s.expect(true, true, "standalone restored narrow terminal");
    assert_eq!(s.screen.visible_text(), narrow);
    eprintln!("PASS standalone: complete rendered text matches unmodified narrow and wide baselines byte-for-byte");
}

#[test]
#[ignore = "requires installed Yazi >= 26.8.15"]
fn yazi_default_pane_displays_more_of_the_actual_file_list() {
    let mut s = Session::start(215, true, false, false);
    s.expect(true, true, "baseline quarter-width Yazi");
    assert!(
        !s.screen.visible_text().contains(LONG_NAME),
        "baseline must truncate the long filename"
    );
    s.save_screen("before-activation");
    let old_column = s.text_column("CURRENT.txt");
    s.activate_via_cli();
    s.expect(false, false, "responsive quarter-width Yazi");
    assert!(
        s.screen.visible_text().contains(LONG_NAME),
        "responsive file list must show the entire filename"
    );
    let new_column = s.text_column("CURRENT.txt");
    assert!(
        new_column < old_column,
        "file list must reclaim the parent panel's space"
    );
    s.save_screen("after-activation");
    eprintln!("PASS usability: {}-character unhovered filename changes from truncated to fully visible in the same 53-column pane; file-list text moves from column {old_column} to {new_column}", LONG_NAME.len());
    s.send(b"h");
    s.expect_text("navigate to parent while panels are hidden", |text| {
        text.contains("UP") && !text.contains("CURRENT.txt")
    });
    s.send(b"l");
    s.expect(false, false, "reenter directory while panels are hidden");
    assert!(s.screen.visible_text().contains(LONG_NAME));
    eprintln!("PASS live activation via ya emit and ordinary h/l directory navigation");
    // Plugin activation is not allowed to rewrite openers or keymaps.
    assert_eq!(
        std::fs::read_to_string(s.dir.join("yazi/yazi.toml")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(s.dir.join("yazi/keymap.toml")).unwrap(),
        "[[mgr.prepend_keymap]]\non = '<C-g>'\nrun = 'plugin gwae-responsive'\n"
    );
}

#[test]
#[ignore = "requires installed Yazi >= 26.8.15"]
fn yazi_custom_breakpoints_restore_the_configured_ratio() {
    let config = "[mgr]\nratio = [2, 3, 5]\n\
        [opener]\nfixture = [{ run = 'true', desc = 'Preserved opener' }]\n";
    let init = "require('gwae-responsive'):setup { preview_width = 80, parent_width = 120 }\n";
    let mut baseline = Session::configured(160, false, true, "", config);
    baseline.expect(true, true, "configured-ratio baseline");
    let columns = (
        baseline.text_column("CURRENT.txt"),
        baseline.text_column("PREVIEW"),
    );
    baseline.save_screen("configured-ratio-baseline");
    drop(baseline);
    let mut s = Session::configured(79, false, true, init, config);
    s.expect(false, false, "custom preview breakpoint minus one");
    for (cols, parent, preview) in [
        (80, false, true),
        (119, false, true),
        (120, true, true),
        (119, false, true),
        (80, false, true),
        (79, false, false),
        (160, true, true),
    ] {
        s.resize(cols);
        s.expect(parent, preview, &format!("custom {cols}-column boundary"));
    }
    let restored = (s.text_column("CURRENT.txt"), s.text_column("PREVIEW"));
    assert_eq!(
        restored, columns,
        "wide layout must restore configured 2:3:5, not hard-coded 1:4:3"
    );
    s.save_screen("configured-ratio-restored");
    assert_eq!(
        std::fs::read_to_string(s.dir.join("yazi/yazi.toml")).unwrap(),
        config
    );
    eprintln!("PASS configured ratio: current/preview text columns {restored:?} exactly match the unmodified 2:3:5 baseline; opener configuration is byte-identical");
    drop(s);
    // Removing setup and restarting is the documented rollback, even with
    // the plugin files still installed and GWAE_PANE still present.
    let mut reverted = Session::configured(79, false, true, "", config);
    reverted.expect(true, true, "setup removed and Yazi restarted");
}
