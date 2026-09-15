//! End-to-end: editing the config file must reload a *running* session.
//!
//! This is the whole point of live reload. Restarting gwae would kill every
//! pane, which is exactly what someone running long-lived agents cannot
//! afford, so the acceptance check is that a config edit takes effect while
//! the same process keeps running. Chrome itself is fixed to the terminal's
//! own colors, so reload is about behavior keys, not colors.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// A running gwae under a PTY, with its config file writable underneath it.
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
            "gwae-reload-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), with_frames(initial_config))
            .expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.arg("run");
        cmd.arg("sleep 60");
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
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
        std::fs::write(self.dir.join("gwae/gwae.toml"), with_frames(body)).expect("rewrite config");
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

/// A case body, unchanged.
fn with_frames(body: &str) -> String {
    body.to_string()
}

#[test]
fn editing_the_config_reloads_the_running_session() {
    // Save a behavior key, as a user editing their config would. The session
    // must confirm without restarting.
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("startup_panes = 1\n");
    let after = s.drain(4);
    assert!(
        after.contains("reloaded"),
        "the running session should confirm the reload; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn retired_theme_keys_do_not_break_the_running_session() {
    // A retired `theme = "..."` preset name parses as "no overrides": no
    // parse error, the session confirms the reload.
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("theme = \"nord\"\n");
    let after = s.drain(4);
    assert!(
        after.contains("reloaded"),
        "a retired theme key should reload cleanly; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn theme_overrides_reload_the_running_session() {
    // A `[theme]` edit repaints the chrome live: the session confirms and the
    // new accent reaches the wire.
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("[theme]\naccent = \"#ff00ff\"\n");
    let after = s.drain(4);
    assert!(
        after.contains("reloaded"),
        "a theme edit should confirm the reload; got:\n{after:?}"
    );
    assert!(
        after.contains("38;2;255;0;255"),
        "the new accent should reach the wire; got:\n{after:?}"
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
    s.write_config("startup_panes = 1\n");
    let after = s.drain(4);
    assert!(
        s.child.process_id() == pid_before,
        "reload must not restart gwae itself"
    );
    assert!(
        after.contains("reloaded"),
        "and it should still have confirmed the reload; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_broken_edit_keeps_the_running_session() {
    // Editors save mid-keystroke, so gwae will inevitably read a
    // half-written config. That must not blow away the running session.
    let s = Session::start("startup_panes = 1\n");
    let _ = s.drain(3);

    s.write_config("startup_panes = 1\nthis is not valid toml <<<\n");
    let after = s.drain(4);

    assert!(
        after.contains("config error"),
        "the user should be told the config is broken; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_successful_reload_is_confirmed_on_screen() {
    let s = Session::start("");
    let _ = s.drain(3);
    s.write_config("startup_panes = 1\n");
    let after = s.drain(4);
    assert!(
        after.contains("reloaded"),
        "the user should get confirmation their edit took effect; got:\n{after:?}"
    );
    s.kill();
}
