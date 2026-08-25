//! End-to-end: editing the config file must re-theme a *running* session.
//!
//! This is the whole point of live reload. Restarting strimux to change a
//! color would kill every pane, which is exactly what someone running
//! long-lived agents cannot afford, so the acceptance check is that the
//! colors on the wire change while the same process keeps running.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// A running strimux under a PTY, with its config file writable underneath it.
struct Session {
    dir: std::path::PathBuf,
    rx: Receiver<Vec<u8>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Session {
    fn start(initial_config: &str) -> Session {
        // Unique per case: these tests are threads of one process, so a
        // shared directory would let cases clobber each other's config.
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "strimux-reload-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        std::fs::write(dir.join("strimux/strimux.toml"), initial_config).expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.arg("run");
        cmd.arg("sleep 60");
        let child = pair.slave.spawn_command(cmd).expect("spawn strimux");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
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
            dir,
            rx,
            child,
            _master: pair.master,
        }
    }

    /// Rewrite the config file, as an editor's save would.
    fn write_config(&self, body: &str) {
        std::fs::write(self.dir.join("strimux/strimux.toml"), body).expect("rewrite config");
    }

    /// Read output until it goes quiet, and return it.
    fn drain(&self, quiet_polls: usize) -> String {
        let mut out = Vec::new();
        let mut idle = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(b) => {
                    out.extend_from_slice(&b);
                    idle = 0;
                }
                Err(_) => {
                    idle += 1;
                    if idle >= quiet_polls {
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
    }
}

/// The SGR foreground sequence for a 24-bit color. The skeleton frames are
/// drawn as glyphs, so theme colors arrive as foreground codes.
fn fg(r: u8, g: u8, b: u8) -> String {
    format!("38;2;{r};{g};{b}")
}

const MOCHA_ACCENT: (u8, u8, u8) = (0x74, 0xc7, 0xec);
const NORD_ACCENT: (u8, u8, u8) = (0x88, 0xc0, 0xd0);
const NORD_OVERLAY: (u8, u8, u8) = (0x4c, 0x56, 0x6a);

#[test]
fn editing_the_config_rethemes_the_running_session() {
    let s = Session::start("");
    let before = s.drain(3);
    let (r, g, b) = MOCHA_ACCENT;
    assert!(
        before.contains(&fg(r, g, b)),
        "should start on the default Mocha accent; got:\n{before:?}"
    );

    // Save a new theme, as a user editing their config would.
    s.write_config("theme = \"nord\"\n");
    let after = s.drain(4);

    let (r, g, b) = NORD_ACCENT;
    assert!(
        after.contains(&fg(r, g, b)),
        "the running session should repaint in Nord without a restart; got:\n{after:?}"
    );
    let (r, g, b) = NORD_OVERLAY;
    assert!(
        after.contains(&fg(r, g, b)),
        "the whole palette should switch, not just the accent; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn the_panes_survive_a_reload() {
    // The entire reason for live reload: the child processes must keep
    // running. If reload restarted anything, the pane's PID would change or
    // the process would die.
    let s = Session::start("");
    let _ = s.drain(3);
    let pid_before = s.child.process_id();
    s.write_config("theme = \"gruvbox\"\n");
    let after = s.drain(4);
    assert!(
        s.child.process_id() == pid_before,
        "reload must not restart strimux itself"
    );
    assert!(
        after.contains(&fg(0x83, 0xa5, 0x98)),
        "and it should still have applied the new theme; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_broken_edit_keeps_the_working_theme() {
    // Editors save mid-keystroke, so strimux will inevitably read a
    // half-written config. That must not blow away a working theme.
    let s = Session::start("theme = \"nord\"\n");
    let before = s.drain(3);
    let (r, g, b) = NORD_ACCENT;
    assert!(before.contains(&fg(r, g, b)), "should start on Nord");

    s.write_config("theme = \"nord\"\nthis is not valid toml <<<\n");
    let after = s.drain(4);

    let (mr, mg, mb) = MOCHA_ACCENT;
    assert!(
        !after.contains(&fg(mr, mg, mb)),
        "a broken edit must not silently revert to the default theme; got:\n{after:?}"
    );
    assert!(
        after.contains("config error"),
        "and the user should be told the config is broken; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn an_unknown_theme_name_is_reported_on_reload() {
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("theme = \"tokyonight-storm\"\n");
    let after = s.drain(4);
    assert!(
        after.contains("unknown theme"),
        "a typo'd theme name should be reported, not silently ignored; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_successful_reload_is_confirmed_on_screen() {
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("theme = \"dracula\"\n");
    let after = s.drain(4);
    assert!(
        after.contains("reloaded"),
        "the user should get confirmation their edit took effect; got:\n{after:?}"
    );
    s.kill();
}
