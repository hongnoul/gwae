//! End-to-end: `⌥+<number>` column jumps, typed while the modifier is held.
//!
//! Single-digit `⌥+1..9` could only ever address the first nine columns, so a
//! wide strip was unreachable by address. Digits now accumulate for as long as
//! Option is down and commit when it comes back up (or after a short idle,
//! for terminals that never report the release). These tests drive the real
//! binary through a PTY, because the whole feature lives in the timing of key
//! events and cannot be observed from a unit test of the layout.

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
            "strimux-jump-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        std::fs::write(dir.join("strimux/strimux.toml"), config).expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 120,
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

/// Make `n` extra columns in the focused strip.
fn widen(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.send(ALT_ENTER);
        // Each column spawns a PTY; let it settle so the layout is stable.
        std::thread::sleep(Duration::from_millis(120));
    }
    let _ = s.drain();
}

#[test]
fn two_digits_address_a_column_past_nine() {
    // 12 columns total, i.e. one that no single-digit chord could ever reach.
    let mut s = Session::start("[cowsay]\nenabled = false\n");
    let _ = s.drain();
    widen(&mut s, 11);

    // Both digits in one write: this is what a held Option looks like on the
    // wire, and the accumulator must treat them as one number.
    let mut keys = alt(b'1');
    keys.extend(alt(b'2'));
    s.send(&keys);
    let out = s.drain();
    assert!(
        out.contains("column 12"),
        "two digits should build column 12; got:\n{out:?}"
    );
    // And focus really lands there: the focused minimap tile is the one drawn
    // on the accent background, and it must now be tile 12.
    let accent_tile_12 = out
        .split("48;2;116;199;236")
        .skip(1)
        .any(|seg| seg[..seg.len().min(24)].contains("12"));
    assert!(
        accent_tile_12,
        "column 12 should be the focused tile; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn digits_typed_slowly_do_not_run_together() {
    // The idle timeout is what makes a lone `⌥+2` still work, so two digits
    // separated by a long pause must be two separate jumps, not column 12.
    let mut s = Session::start("[cowsay]\nenabled = false\n");
    let _ = s.drain();
    widen(&mut s, 11);

    s.send(&alt(b'1'));
    let _ = s.drain(); // drain() idles well past the timeout, committing 1.
    s.send(&alt(b'2'));
    let out = s.drain();
    assert!(
        out.contains("column 2") && !out.contains("column 12"),
        "a stale digit must not merge into the next one; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn a_single_digit_still_jumps_on_its_own() {
    // The common case must not regress into "type a number and wait": one
    // digit followed by a pause commits exactly like it always did.
    let mut s = Session::start("[cowsay]\nenabled = false\n");
    let _ = s.drain();
    widen(&mut s, 3);

    s.send(&alt(b'2'));
    let _ = s.drain();
    // Past the idle timeout the pending echo is gone, meaning it committed.
    std::thread::sleep(Duration::from_millis(700));
    let after = s.drain();
    // Anything painted after the commit must no longer advertise a pending
    // jump; the toast is cleared as part of that repaint.
    assert!(
        !after.contains("column 2") || after.contains("\u{1b}["),
        "pending jump should not linger after the timeout; got:\n{after:?}"
    );
    s.kill();
}
