//! End-to-end: the launch-time config keys `startup_command` and
//! `startup_hud`, driven through a real PTY running the real binary.
//!
//! Both keys are only observable at init: `startup_command` decides what the
//! very first pane execs (replacing the agent harness/gateway), and
//! `startup_hud` decides whether the cheat-sheet flashes before the user has
//! pressed anything. Neither can be seen from a unit test of the config, so
//! these tests read the bytes the binary actually paints on its first frames.

#![cfg(unix)]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

struct Session {
    rx: Receiver<Vec<u8>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    screen: Vt100Grid,
    dir: std::path::PathBuf,
}

impl Session {
    /// Launch `gwae` with no subcommand (so nothing more specific than the
    /// config can claim pane 1.1) in a throwaway HOME/XDG root.
    fn start(config: &str) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-startup-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), config).expect("write config");
        // A stand-in for the configured tool: it announces its own argv, so
        // the test can tell both *that* it ran and *what* it was handed.
        let stub = dir.join("stub-tool");
        std::fs::write(
            &stub,
            "#!/bin/sh\nstty -echo\nprintf 'STUBTOOL ARGS=[%s]' \"$*\"\nexec sleep 60\n",
        )
        .expect("write stub tool");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700))
            .expect("executable stub");

        let rows = 30;
        let cols = 140;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let executable =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        let mut cmd = CommandBuilder::new(executable);
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("PATH", format!("{}:/usr/bin:/bin", dir.display()));
        cmd.env("ENV", "/dev/null");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_LOG", "off");
        cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
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
            rx,
            child,
            _master: pair.master,
            screen: Vt100Grid::new(Size { rows, cols }),
            dir,
        }
    }

    /// Read for a fixed window, feeding the emulator, and return the screen.
    fn peek(&mut self, ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            if let Ok(b) = self.rx.recv_timeout(Duration::from_millis(20)) {
                self.screen.feed(&b);
            }
        }
        self.screen.visible_text()
    }

    /// Read (bounded) until the screen satisfies `done`, then return it.
    fn wait_for(&mut self, secs: u64, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let text = self.peek(100);
            if done(&text) || Instant::now() >= deadline {
                return text;
            }
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

/// A line of the startup cheat-sheet that nothing else paints.
fn hud_showing(text: &str) -> bool {
    text.contains("focus left")
}

#[test]
fn startup_command_runs_in_pane_one_with_tildes_expanded() {
    // The configured tool replaces the agent pane, and a configured `~` must
    // arrive as the home path: panes exec directly, with no shell to expand
    // it, so an unexpanded `~` would be a literal directory name.
    let mut s = Session::start("startup_command = \"stub-tool ~\"\n");
    let text = s.wait_for(15, |t| t.contains("STUBTOOL"));
    assert!(
        text.contains("STUBTOOL"),
        "configured startup_command owns pane 1.1; got:\n{text:?}"
    );
    let home = s.dir.display().to_string();
    // A quarter-width pane wraps the long path across rows, and the chrome
    // paints a border on each one, so compare with the layout removed.
    let squashed: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '│')
        .collect();
    assert!(
        squashed.contains(&format!("ARGS=[{home}]")),
        "the tilde reached the tool as {home}; got:\n{text:?}"
    );
}

#[test]
fn startup_hud_false_suppresses_the_launch_cheat_sheet() {
    // Opt-out: a clean first screen, with no key pressed to dismiss it.
    let mut s = Session::start("startup_hud = false\nstartup_command = \"stub-tool\"\n");
    let text = s.wait_for(15, |t| t.contains("STUBTOOL"));
    assert!(
        !hud_showing(&text),
        "startup_hud = false paints no cheat-sheet; got:\n{text:?}"
    );
}

#[test]
fn startup_hud_defaults_to_showing_the_cheat_sheet() {
    // The control for the test above: without the opt-out the flash is still
    // there, so a passing suppression test cannot be a false positive from
    // the HUD having been removed or renamed.
    let mut s = Session::start("startup_command = \"stub-tool\"\n");
    let text = s.wait_for(15, hud_showing);
    assert!(
        hud_showing(&text),
        "default launch still flashes the cheat-sheet; got:\n{text:?}"
    );
}
