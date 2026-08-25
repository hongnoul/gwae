//! End-to-end: the onboarding title card must animate on a real terminal, and
//! must survive being replayed inside a strimux pane.
//!
//! The unit tests in `splash` prove every frame is well-formed, but they call
//! internal functions. This drives the *shipped executable* through a PTY
//! twice:
//!
//! 1. `strimux init` on a bare PTY: does the card actually animate (many
//!    distinct frames), and does it get out of the way of question 1?
//! 2. `strimux run "strimux init"` - the same card drawn *inside* a hosted
//!    pane, which is the real question: a strimux pane is a vt100 grid
//!    recomposited by strimux's own renderer, so animation that relies on
//!    anything beyond plain SGR + repaint would be dropped or shear.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// The block glyph the wordmark is drawn with.
const INK: char = '\u{2588}';

/// Drop CSI/OSC escapes, leaving the glyphs that land on the screen.
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        let _ = it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A scratch `XDG_CONFIG_HOME` with `body` as the config.
struct Sandbox {
    dir: std::path::PathBuf,
}

impl Sandbox {
    fn new(body: &str) -> Sandbox {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "strimux-splash-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        std::fs::write(dir.join("strimux/strimux.toml"), body).expect("write config");
        Sandbox { dir }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run the real binary with `args`, capturing output until `done` is happy or
/// the deadline passes. Returns the raw bytes painted.
fn capture(
    sb: &Sandbox,
    args: &[&str],
    cols: u16,
    rows: u16,
    done: impl Fn(&str) -> bool,
) -> String {
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
    cmd.env("XDG_CONFIG_HOME", &sb.dir);
    cmd.env("TERM", "xterm-256color");
    for a in args {
        cmd.arg(a);
    }
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
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if done(&String::from_utf8_lossy(&out)) {
            break;
        }
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) {
            out.extend_from_slice(&b);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    String::from_utf8_lossy(&out).into_owned()
}

/// Count screen-clears, which is how many frames the card repainted: every
/// splash frame (and every question) begins with a clear-and-home.
fn clears(s: &str) -> usize {
    s.matches("\x1b[2J").count()
}

#[test]
fn init_animates_the_title_card_before_the_first_question() {
    let sb = Sandbox::new("");
    // Stop once the flow has reached question 1: that proves both that the
    // card played and that it handed over.
    let out = capture(&sb, &["init"], 80, 24, |s| strip_ansi(s).contains("[1/"));
    let plain = strip_ansi(&out);

    assert!(
        plain.contains(INK),
        "the wordmark never reached the terminal"
    );
    assert!(
        clears(&out) > 10,
        "only {} repaints: the card is not animating",
        clears(&out)
    );
    assert!(
        plain.contains("scrolling panes"),
        "the tagline never resolved"
    );
    assert!(
        plain.contains("[1/"),
        "onboarding never got past the splash"
    );
    // Order matters: the card must come before the questions, not over them.
    let first_ink = plain.find(INK).unwrap();
    let first_q = plain.find("[1/").unwrap();
    assert!(first_ink < first_q, "the card painted after question 1");
}

#[test]
fn a_keypress_skips_the_card_immediately() {
    // The card must never be an obstacle: a user who is already typing gets
    // the questions at once, and the keystroke is not also eaten as an answer.
    let sb = Sandbox::new("");
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
    cmd.env("XDG_CONFIG_HOME", &sb.dir);
    cmd.env("TERM", "xterm-256color");
    cmd.arg("init");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn strimux");
    drop(pair.slave);
    let mut writer = pair.master.take_writer().expect("writer");
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
    // Bang on a key that is not a valid answer, so if it leaked through into
    // the question it would be visibly ignored rather than selecting.
    std::thread::sleep(Duration::from_millis(60));
    let _ = writer.write_all(b"z");
    let _ = writer.flush();

    let mut out = Vec::new();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(20);
    while Instant::now() < deadline {
        if strip_ansi(&String::from_utf8_lossy(&out)).contains("[1/") {
            break;
        }
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
            out.extend_from_slice(&b);
        }
    }
    let elapsed = started.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    let plain = strip_ansi(&String::from_utf8_lossy(&out));
    assert!(plain.contains("[1/"), "never reached question 1");
    assert!(
        elapsed < Duration::from_secs(2),
        "keypress did not cut the card short ({elapsed:?})"
    );
}

#[test]
fn the_card_renders_inside_a_strimux_pane() {
    // The actual question this feature raises: a strimux pane is a hosted
    // vt100 grid recomposited by strimux's own renderer, so the animation has
    // to survive a round trip through emulation *and* re-drawing. Run the
    // card as a pane program and look for the wordmark on the outer terminal.
    //
    // The outer terminal is deliberately large: a pane is only a fraction of
    // it, and the card draws its block art only when the *pane* is wide
    // enough (see `narrow_panes_fall_back_to_the_plain_word`).
    let sb = Sandbox::new("[minimap]\nshow = false\nmode = \"off\"\n");
    let inner = format!("{} init", env!("CARGO_BIN_EXE_strimux"));
    let out = capture(&sb, &["run", &inner], 200, 50, |s| {
        let p = strip_ansi(s);
        p.matches(INK).count() > 40 && p.contains("catppuccin-mocha")
    });
    let plain = strip_ansi(&out);
    assert!(
        plain.contains(INK),
        "the pane never painted the wordmark; got {} bytes",
        out.len()
    );
    // The host wraps every repaint in synchronized-update markers, so a card
    // animating inside a pane produces many of them: the animation is not
    // collapsing into a single static frame.
    let frames = out.matches("\x1b[?2026h").count();
    assert!(frames > 3, "pane only repainted {frames} times");
    // ...and the flow still gets to its questions inside the pane.
    assert!(
        plain.contains("catppuccin-mocha"),
        "onboarding never reached the theme question inside the pane"
    );
}

#[test]
fn narrow_panes_fall_back_to_the_plain_word() {
    // A pane narrower than the block art must degrade to the plain wordmark
    // rather than wrapping the art (which would scroll the pane and shear the
    // questions underneath it).
    let sb = Sandbox::new("[minimap]\nshow = false\nmode = \"off\"\n");
    let inner = format!("{} init", env!("CARGO_BIN_EXE_strimux"));
    let out = capture(&sb, &["run", &inner], 100, 30, |s| {
        strip_ansi(s).contains("catppuccin-mocha")
    });
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("catppuccin-mocha"),
        "onboarding never ran in the narrow pane"
    );
    assert!(
        !plain.contains(INK),
        "block art was drawn in a pane too narrow for it"
    );
}
