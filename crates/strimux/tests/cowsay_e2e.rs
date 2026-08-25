//! End-to-end: the cowsay hints in empty placeholder boxes must reach a real
//! terminal, painted by the real binary reading a real config file.
//!
//! The unit tests in `cowsay` prove the wrapping and the `tui` tests prove the
//! placement, but both call internal functions. This drives the *shipped
//! executable* through a PTY so the whole path is covered: config discovery
//! via `XDG_CONFIG_HOME`, the render, and the bytes on the wire.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::channel;
use std::time::Duration;

/// How many frames have been painted. strimux wraps each frame in the
/// synchronized-update markers, so counting the opening marker counts frames.
fn frame_count(out: &[u8]) -> usize {
    out.windows(8).filter(|w| *w == b"\x1b[?2026h").count()
}

/// Run the real binary with `config_body` on a `cols`x`rows` terminal and
/// return everything it painted.
fn paint(config_body: &str, cols: u16, rows: u16) -> String {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "strimux-cowsay-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
    std::fs::write(dir.join("strimux/strimux.toml"), config_body).expect("write config");

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd.env("TERM", "xterm-256color");
    cmd.arg("run");
    cmd.arg("sleep 30");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn strimux");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut out = Vec::new();
    let mut idle = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if frame_count(&out) >= 1 && idle >= 4 {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(b) => {
                out.extend_from_slice(&b);
                idle = 0;
            }
            Err(_) => idle += 1,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        frame_count(&out) >= 1,
        "strimux never painted; captured {} bytes",
        out.len()
    );
    String::from_utf8_lossy(&out).into_owned()
}

/// The startup HUD is centered over the grid and would cover the placeholder
/// boxes, so these cases turn the chrome off and rely on the first frame.
const NO_CHROME: &str = "[minimap]\nshow = false\nmode = \"off\"\n";

#[test]
fn configured_cow_message_reaches_the_terminal() {
    // A user-supplied message must appear verbatim in an empty box. The
    // string is distinctive so it cannot be confused with anything else
    // strimux paints.
    let cfg = format!("[cowsay]\nenabled = true\nmessages = [\"zebrafish\"]\n{NO_CHROME}");
    let painted = paint(&cfg, 120, 30);
    assert!(
        painted.contains("zebrafish"),
        "configured cowsay message never reached the terminal"
    );
    assert!(
        painted.contains("^__^"),
        "the cow itself never reached the terminal"
    );
}

#[test]
fn cow_can_be_disabled() {
    let cfg = format!("[cowsay]\nenabled = false\n{NO_CHROME}");
    let painted = paint(&cfg, 120, 30);
    assert!(
        !painted.contains("^__^"),
        "cow painted despite enabled = false"
    );
}

#[test]
fn empty_message_list_disables_the_cow() {
    let cfg = format!("[cowsay]\nmessages = []\n{NO_CHROME}");
    let painted = paint(&cfg, 120, 30);
    assert!(!painted.contains("^__^"), "cow painted with no messages");
}

#[test]
fn default_config_shows_a_keybinding_hint() {
    // Out of the box, with no `[cowsay]` section at all, empty boxes should
    // document the keybindings. This is the actual shipped default.
    let painted = paint(NO_CHROME, 120, 30);
    assert!(
        painted.contains("^__^"),
        "default config painted no cow in the empty grid"
    );
}
