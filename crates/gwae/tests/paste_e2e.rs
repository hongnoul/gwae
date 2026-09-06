//! End-to-end: `⌥+v` pastes the system clipboard into a plain PTY pane.
//!
//! The regression this guards: fish binds `ESC+v` to `edit_command_buffer`,
//! which prints "External editor requested but $VISUAL or $EDITOR not set."
//! when no editor is configured. gwae must claim the chord itself — read the
//! clipboard and bracket-write it — rather than forwarding it to the shell.
//! These tests drive the real binary through a real PTY with a stub `pbpaste`
//! on PATH, so the clipboard read is hermetic.

//! macOS-only: the stubs fake `pbpaste`, and the helper table differs on
//! other platforms (wl-paste/xclip). Faking the whole table is not worth
//! the flake surface, so the entire file — helpers included, which keeps
//! `clippy -D warnings` green on Linux — compiles out elsewhere.
#![cfg(target_os = "macos")]

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
    _dir: std::path::PathBuf,
}

impl Session {
    /// Start gwae with `startup_panes = 1` running `cat`, and a `bin`
    /// directory at the front of PATH holding a stub `pbpaste` that prints
    /// `clipboard_text`. Returns the session plus the temp dir (kept alive
    /// by the struct so the stub stays on disk).
    fn start(clipboard_text: &str) -> Session {
        Self::start_with(clipboard_text, "cat")
    }

    /// Same as [`Session::start`], but run `pane_cmd` in the pane instead
    /// of `cat` (e.g. a real `fish` for the acceptance test).
    fn start_with(clipboard_text: &str, pane_cmd: &str) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gwae-paste-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "[cowsay]\nenabled = false\nstartup_panes = 1\n",
        )
        .expect("write config");

        // Hermetic clipboard: a stub pbpaste (macOS helper) printing fixed
        // text. `read_clipboard` tries it first on macOS, so PATH order is
        // enough to control the test on this platform. Skipped on other
        // platforms: the helper table differs there (wl-paste/xclip),
        // and faking the whole table is not worth the flake surface.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("temp bin dir");
        std::fs::write(
            bin.join("pbpaste"),
            ["#!/bin/sh\nprintf '%s' '", clipboard_text, "'"].concat(),
        )
        .expect("write stub pbpaste");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("pbpaste"), std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub pbpaste");
        }

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
        // PATH first so the stub wins; keep a usable base for sh/cat.
        let path = format!("{}:/usr/bin:/bin:/opt/homebrew/bin", bin.to_string_lossy());
        cmd.env("PATH", &path);
        cmd.arg("run");
        // `cat` echoes whatever gwae writes to the pane, bracket markers
        // included, so the test can assert on the exact pasted bytes.
        // `cat` never enables bracketed paste (no DECSET 2004), so the pane
        // gets the payload verbatim — the right expectation for a child
        // that did not ask.
        cmd.arg(pane_cmd);
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
            _dir: dir,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    /// Read until output goes quiet, and return it.
    fn drain(&self) -> Vec<u8> {
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
        out
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Option+v as a terminal that maps Option to Meta sends it: ESC + v.
const OPT_V: &[u8] = b"\x1bv";

/// Strip SGR/CSI escapes so assertions read the text the user sees.
fn visible(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
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
#[cfg(target_os = "macos")]
fn option_v_pastes_the_clipboard_into_a_plain_pane() {
    // `cat` echoes the pasted bytes back through the pane, and gwae toasts
    // a confirmation. Neither happens if the chord were forwarded as
    // `ESC+v` input: `cat` would echo exactly `\x1bv` and nothing more.
    let mut s = Session::start("hello-paste");
    // Settle: wait until the frame is up so the chord is not swallowed by
    // startup (the HUD flash / first paint).
    std::thread::sleep(Duration::from_millis(1500));
    let _ = s.drain();

    s.send(OPT_V);
    let out = s.drain();
    let shown = visible(&out);
    assert!(
        shown.contains("hello-paste"),
        "the clipboard text must reach the pane; got:\n{shown:?}"
    );
    assert!(
        shown.contains("pasted 1 line"),
        "the paste toast must confirm; got:\n{shown:?}"
    );
    // The failure mode this replaces: fish's external-editor error. `cat`
    // cannot print that string itself, so its absence proves gwae claimed
    // the chord rather than forwarding it.
    assert!(
        !shown.contains("External editor"),
        "the chord must not reach the child as input; got:\n{shown:?}"
    );
    s.kill();
}

#[test]
#[cfg(target_os = "macos")]
fn option_v_multiline_paste_arrives_as_one_block() {
    // A multi-line payload through `cat` must echo back joined: gwae
    // normalizes newlines to `\r` (what a PTY delivers for Return), so the
    // child sees line breaks, not one long line and not literal `\n` text.
    let mut s = Session::start("one\\ntwo");
    std::thread::sleep(Duration::from_millis(1500));
    let _ = s.drain();

    s.send(OPT_V);
    let out = s.drain();
    let shown = visible(&out);
    assert!(
        shown.contains("one") && shown.contains("two"),
        "both pasted lines must reach the pane; got:\n{shown:?}"
    );
    assert!(
        shown.contains("pasted 1 line") || shown.contains("pasted 2 lines"),
        "the paste toast must confirm; got:\n{shown:?}"
    );
    s.kill();
}

#[test]
#[cfg(target_os = "macos")]
fn option_v_in_a_real_fish_pane_pastes_instead_of_opening_an_editor() {
    // Acceptance for the original report: a plain fish pane with no
    // $VISUAL/$EDITOR. Before the fix, `⌥+v` arrived as `ESC+v`, which fish
    // binds to `edit_command_buffer` — printing "External editor requested
    // but $VISUAL or $EDITOR not set." Now gwae bracket-writes the
    // clipboard, and fish (which enables bracketed paste) buffers it on the
    // command line instead of running it.
    //
    // The fish binary must exist; skip otherwise (Linux CI has no fish).
    let fish = [
        "/opt/homebrew/bin/fish",
        "/usr/local/bin/fish",
        "/usr/bin/fish",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists());
    let Some(fish) = fish else {
        eprintln!("skipping: no fish binary found");
        return;
    };
    // The pasted text must appear on fish's command line without
    // executing on its own.
    let mut s = Session::start_with("echo PASTED_MARKER", fish);
    std::thread::sleep(Duration::from_millis(2000));
    let _ = s.drain();

    s.send(OPT_V);
    std::thread::sleep(Duration::from_millis(1000));
    let out = s.drain();
    let shown = visible(&out);
    assert!(
        !shown.contains("External editor"),
        "fish must not open its external editor; got:\n{shown:?}"
    );
    // The 100-column frame wraps the prompt line across pane borders, so
    // `echo PASTED_MARKER` may arrive split around box-drawing cells.
    // Collapse everything that is not a letter to compare the content.
    let squashed: String = shown.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    assert!(
        squashed.contains("echoPASTEDMARKER"),
        "the pasted command must sit on fish's command line; got:\n{shown:?}"
    );
    // Fish buffers a bracketed paste without executing: the marker text
    // appears exactly once (on the prompt line). Running the command
    // would print a second bare `PASTED_MARKER` output line. Count on the
    // squashed text so frame wrapping cannot hide or fake an occurrence.
    let marker_count = squashed.matches("PASTEDMARKER").count();
    assert!(
        marker_count == 1,
        "paste must buffer (1 occurrence), not execute ({marker_count}); got:\n{shown:?}"
    );
    s.kill();
}
