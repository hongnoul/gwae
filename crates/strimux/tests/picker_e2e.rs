//! End-to-end: the `⌥+t` theme picker previews presets on the live screen.
//!
//! The picker's value is that the preview is the *actual running UI*, not a
//! swatch, so these tests assert on the colors the real binary paints while
//! stepping through it.

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
            "strimux-picker-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        // `skeleton = true` first: these cases read the live theme off the
        // frame glyphs, which carry every palette key as a *foreground* color.
        // strimux ships full-bleed (focus is a background tint and there are
        // no frames), so the picker cases opt the frames back on. It goes
        // before `config` because a body opening a TOML table would otherwise
        // swallow it.
        std::fs::write(
            dir.join("strimux/strimux.toml"),
            format!("skeleton = true\n{config}"),
        )
        .expect("write config");

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

fn fg(r: u8, g: u8, b: u8) -> String {
    format!("38;2;{r};{g};{b}")
}

/// Option+t, as a terminal that maps Option to Meta sends it.
const OPT_T: &[u8] = b"\x1bt";
const RIGHT: &[u8] = b"\x1b[C";
const LEFT: &[u8] = b"\x1b[D";

const MOCHA_ACCENT: (u8, u8, u8) = (0x74, 0xc7, 0xec);
const LATTE_ACCENT: (u8, u8, u8) = (0x20, 0x9f, 0xb5);

#[test]
fn opens_on_the_current_theme() {
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let out = s.drain();
    assert!(
        out.contains("catppuccin-mocha"),
        "the picker should open on the theme in use; got:\n{out:?}"
    );
    assert!(
        out.contains("keep") && out.contains("cancel"),
        "it should say how to keep or cancel; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn stepping_previews_the_next_theme_on_the_live_screen() {
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let _ = s.drain();
    s.send(RIGHT);
    let out = s.drain();
    assert!(
        out.contains("catppuccin-latte"),
        "stepping should name the next preset; got:\n{out:?}"
    );
    let (r, g, b) = LATTE_ACCENT;
    assert!(
        out.contains(&fg(r, g, b)),
        "and the live chrome should repaint in it, not just the label; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn stepping_wraps_backwards_to_the_last_theme() {
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let _ = s.drain();
    s.send(LEFT);
    let out = s.drain();
    assert!(
        out.contains("terminal"),
        "stepping back from the first preset should wrap to the last; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn cancelling_restores_the_configured_theme() {
    // The preview must never be sticky: escaping puts back exactly what the
    // config says, since the picker never writes to the config file.
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let _ = s.drain();
    s.send(RIGHT);
    let previewed = s.drain();
    let (lr, lg, lb) = LATTE_ACCENT;
    assert!(
        previewed.contains(&fg(lr, lg, lb)),
        "preview should be Latte"
    );

    s.send(b"\x1b\x1b"); // Escape (doubled: the first may be eaten as a chord preamble)
    let out = s.drain();
    let (mr, mg, mb) = MOCHA_ACCENT;
    assert!(
        out.contains(&fg(mr, mg, mb)),
        "cancelling should restore the configured Mocha; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn keeping_tells_the_user_how_to_persist_it() {
    // The picker deliberately does not rewrite the user's config (that would
    // mean owning their file's formatting and comments), so it has to say
    // what to write.
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let _ = s.drain();
    s.send(RIGHT);
    let _ = s.drain();
    s.send(b"\r");
    let out = s.drain();
    assert!(
        out.contains("theme =") && out.contains("catppuccin-latte"),
        "keeping should show the config line to add; got:\n{out:?}"
    );
    s.kill();
}

#[test]
fn the_picker_does_not_leak_keys_into_the_focused_pane() {
    // While open the picker owns the keyboard. If arrows reached the pane,
    // the shell would receive them and the picker would be unusable.
    let mut s = Session::start("");
    let _ = s.drain();
    s.send(OPT_T);
    let _ = s.drain();
    s.send(RIGHT);
    s.send(RIGHT);
    let out = s.drain();
    assert!(
        out.contains("theme 3/"),
        "two steps should land on the third preset; got:\n{out:?}"
    );
    s.kill();
}
