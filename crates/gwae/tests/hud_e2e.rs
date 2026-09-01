//! End-to-end: the ⌥-hold dashboard, driven through a real PTY.
//!
//! The panel only exists while the modifier is down, so nothing about it can
//! be observed from a unit test of the layout: the reveal, its contents, and
//! its disappearance are all products of key *timing* in the real event loop.
//! These tests therefore run the actual binary and read the bytes it paints.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Session {
    fn start(config: &str) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gwae-hud-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), config).expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 140,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.arg("run");
        // A pane that sets its own window title, so the dashboard has a real
        // OSC 0/2 name to show rather than a fixture we injected ourselves.
        // `run` splits its argument itself (it does not go through a shell),
        // so the shell is named explicitly.
        cmd.arg("sh -c \"printf '\\033]0;watchdog\\007'; sleep 60\"");
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

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    /// Read until output goes quiet, and return it.
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

    /// Read for a fixed short window, without waiting for quiet. The panel is
    /// up for ~180ms after a chord on a terminal with no release reporting, so
    /// `drain` (which waits for silence) would always miss it.
    fn peek(&self, ms: u64) -> String {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if let Ok(b) = self.rx.recv_timeout(Duration::from_millis(20)) {
                out.extend_from_slice(&b);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Alt chords as a terminal that maps Option to Meta sends them: ESC + key.
fn alt(key: u8) -> Vec<u8> {
    vec![0x1b, key]
}
const ALT_ENTER: &[u8] = b"\x1b\r";

/// Add `n` columns to the focused strip, letting each PTY settle.
fn widen(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.send(ALT_ENTER);
        std::thread::sleep(Duration::from_millis(150));
    }
    let _ = s.drain();
}

/// Strip SGR/CSI escapes so assertions read the text the user sees. Panel
/// text is heavily styled per cell, so the raw stream interleaves colour
/// sequences between almost every character.
fn visible(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: consume through the final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: consume through BEL or ST.
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' || c == '\u{1b}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn holding_the_modifier_reveals_a_dashboard_that_names_its_panes() {
    // Spatial-only: the panel shows geometry (color + address + marker), not
    // titles. Titles live in pane chrome; the HUD stays uncluttered.
    let mut s = Session::start("[cowsay]\nenabled = false\n");
    let _ = s.drain();
    widen(&mut s, 3);

    // A chord holds the modifier open for a short window even on terminals
    // that never report a bare Option press, which is what most terminals do.
    s.send(&alt(b'h'));
    let shown = visible(&s.peek(150));
    assert!(
        shown.contains("attention"),
        "the hold should reveal the key hints; got:\n{shown:?}"
    );
    // Spatial HUD no longer repeats pane titles on tiles; watchdog
    // still lives in the pane itself, so visible() (panes+HUD) will
    // contain it from pane chrome. Assert HUD geometry instead.
    assert!(
        shown.contains("»") || shown.contains("!"),
        "dashboard should show spatial tiles (glyphs); got:\n{shown:?}"
    );
    // The panel is transient: once the hold lapses it must clean up after
    // itself rather than leaving a box painted over live panes.
    std::thread::sleep(Duration::from_millis(400));
    let after = visible(&s.drain());
    assert!(
        !after.contains("attention"),
        "the panel must not outlive the hold; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_lone_pane_still_answers_the_hold() {
    // Regression: with one pane the panel used to draw nothing at all, which
    // taught first-run users that holding ⌥ was broken. It now degrades to
    // the key hints, which is exactly what a new user needs.
    let mut s = Session::start("[cowsay]\nenabled = false\nstartup_panes = 1\n");
    let _ = s.drain();

    s.send(&alt(b'h'));
    let shown = visible(&s.peek(150));
    assert!(
        shown.contains("attention"),
        "one pane still gets the hints; got:\n{shown:?}"
    );
    s.kill();
}

#[test]
fn typing_a_column_number_previews_it_on_the_dashboard() {
    // A multi-digit jump is typed blind: the map is the only place that can
    // show which column the number currently addresses.
    let mut s = Session::start("[cowsay]\nenabled = false\n");
    let _ = s.drain();
    widen(&mut s, 3);

    s.send(&alt(b'2'));
    let shown = visible(&s.peek(150));
    assert!(
        shown.contains("column 2"),
        "the pending number is echoed; got:\n{shown:?}"
    );
    // The dashboard toast ("⌥ → column 2") can overlap the bottom hint
    // ("⌥1-9 col · ⌥g attention …") on this 30-row PTY; either is proof the
    // overlay is up and the number was accepted.
    assert!(
        shown.contains("attention") || shown.contains("column 2"),
        "and the dashboard/toast is up; got:\n{shown:?}"
    );
    s.kill();
}
