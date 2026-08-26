//! End-to-end: pasting into a pane of the real binary.
//!
//! The unit tests in `select.rs` prove the encoding (bracketing, newline
//! normalization, marker stripping) in isolation. This is the acceptance
//! check for the thing a user actually does: select some text in their
//! terminal, hit ⌘/Ctrl+V over a gwae pane, and expect the whole thing to
//! land in the focused pane as *one* paste.
//!
//! What used to happen instead is the bug this guards: gwae never enabled
//! bracketed paste, so the payload arrived as ordinary key events, each `\r`
//! decoded to `KeyCode::Enter`, and a five-line paste ran five commands (or,
//! in an agent harness, submitted five half-written prompts).
//!
//! The child here is a script that reads raw bytes from its own stdin and
//! writes them to a file, so the assertion is on exactly what reached the
//! pane — not on how a shell chose to render it.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// Frames are wrapped in synchronized-update markers, so counting the opening
/// marker counts painted frames.
fn frame_count(out: &[u8]) -> usize {
    out.windows(8).filter(|w| *w == b"\x1b[?2026h").count()
}

/// Read from `rx` until the paint has been quiet for a moment.
fn drain_until_quiet(rx: &Receiver<Vec<u8>>, out: &mut Vec<u8>, quiet: usize) {
    let mut idle = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && idle < quiet {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(b) => {
                out.extend_from_slice(&b);
                idle = 0;
            }
            Err(_) => idle += 1,
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// A pasted payload reaches the focused pane as one delivery, with its
/// newlines intact as carriage returns rather than as N separate Enters.
#[test]
fn a_multiline_paste_reaches_the_pane_as_one_block() {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-paste-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
    std::fs::write(dir.join("gwae/gwae.toml"), "[minimap]\nshow = false\n").expect("write config");

    // The pane: announce readiness, then copy raw stdin to a file. `cat` with
    // the terminal in raw mode hands us exactly the bytes gwae wrote, which is
    // the level the assertion needs to be at.
    let received = dir.join("received.bin");
    let script = dir.join("pane.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nstty raw -echo 2>/dev/null\nprintf 'READY-FOR-PASTE\\n'\ncat > {}\n",
            received.display()
        ),
    )
    .expect("write pane script");
    #[cfg(unix)]
    make_executable(&script);

    let pty = native_pty_system();
    let pair = pty
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
    cmd.arg(script.display().to_string());
    let mut child = pair.slave.spawn_command(cmd).expect("spawn gwae");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
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

    let mut boot = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && frame_count(&boot) < 1 {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) {
            boot.extend_from_slice(&b);
        }
    }
    assert!(frame_count(&boot) >= 1, "gwae never painted a frame");

    // gwae must have asked the host to bracket pastes. Without this request
    // the host sends a paste as plain keystrokes and none of the rest can
    // work, so assert on the request itself rather than only its consequences.
    let boot_str = String::from_utf8_lossy(&boot).into_owned();
    assert!(
        boot_str.contains("\x1b[?2004h"),
        "gwae never enabled bracketed paste on the host; got:\n{}",
        boot_str.escape_debug()
    );

    writer.write_all(b"\x1b").expect("dismiss HUD");
    writer.flush().ok();
    drain_until_quiet(&rx, &mut boot, 4);
    assert!(
        String::from_utf8_lossy(&boot).contains("READY-FOR-PASTE"),
        "the pane never started; got:\n{}",
        String::from_utf8_lossy(&boot).escape_debug()
    );

    // The paste, exactly as a terminal delivers one: the payload wrapped in
    // the host's own bracket markers.
    writer
        .write_all(b"\x1b[200~echo one\necho two\necho three\x1b[201~")
        .expect("paste");
    writer.flush().ok();
    let mut after = Vec::new();
    drain_until_quiet(&rx, &mut after, 6);

    // Let the pane's `cat` flush what it was given.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !received.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    // `cat` writes when its buffer fills or on EOF; killing gwae closes the
    // PTY, which is the EOF that makes it flush.
    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && std::fs::metadata(&received).map(|m| m.len()).unwrap_or(0) == 0
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let got = std::fs::read(&received).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    let got = String::from_utf8_lossy(&got).into_owned();

    assert!(
        !got.is_empty(),
        "nothing reached the pane at all; the paste was dropped"
    );
    // Every line arrived, in order, in one delivery.
    assert!(
        got.contains("echo one") && got.contains("echo two") && got.contains("echo three"),
        "the whole payload should reach the pane; got:\n{}",
        got.escape_debug()
    );
    // Newlines became carriage returns (what Return is on a PTY), and no bare
    // \n survived to leave a line that never ends.
    assert!(
        got.contains("echo one\recho two\recho three"),
        "lines should be joined by \\r, exactly once each; got:\n{}",
        got.escape_debug()
    );
    // The child (a plain `cat`) never asked for bracketed paste, so gwae must
    // not have invented markers it cannot parse: they would print as literal
    // `[200~` garbage.
    assert!(
        !got.contains("\x1b[200~") && !got.contains("\x1b[201~"),
        "a child that never enabled bracketed paste must not receive markers; got:\n{}",
        got.escape_debug()
    );
}

/// A child that *did* enable bracketed paste gets the markers put back, so it
/// can tell a paste from typing. This is the half that makes an agent harness
/// buffer a long prompt instead of submitting its first line.
#[test]
fn a_child_that_asked_for_bracketing_gets_its_markers_back() {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-paste-brk-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
    std::fs::write(dir.join("gwae/gwae.toml"), "[minimap]\nshow = false\n").expect("write config");

    // Same as the first pane, but it turns *on* bracketed paste (DECSET 2004)
    // first, exactly as a shell or an agent's line editor does.
    let received = dir.join("received.bin");
    let script = dir.join("pane.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nstty raw -echo 2>/dev/null\nprintf '\\033[?2004hREADY-FOR-PASTE\\n'\ncat > {}\n",
            received.display()
        ),
    )
    .expect("write pane script");
    #[cfg(unix)]
    make_executable(&script);

    let pty = native_pty_system();
    let pair = pty
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
    cmd.arg(script.display().to_string());
    let mut child = pair.slave.spawn_command(cmd).expect("spawn gwae");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
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

    let mut boot = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && frame_count(&boot) < 1 {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(200)) {
            boot.extend_from_slice(&b);
        }
    }
    assert!(frame_count(&boot) >= 1, "gwae never painted a frame");
    writer.write_all(b"\x1b").expect("dismiss HUD");
    writer.flush().ok();
    drain_until_quiet(&rx, &mut boot, 4);
    assert!(
        String::from_utf8_lossy(&boot).contains("READY-FOR-PASTE"),
        "the pane never started; got:\n{}",
        String::from_utf8_lossy(&boot).escape_debug()
    );

    writer
        .write_all(b"\x1b[200~echo one\necho two\x1b[201~")
        .expect("paste");
    writer.flush().ok();
    let mut after = Vec::new();
    drain_until_quiet(&rx, &mut after, 6);

    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && std::fs::metadata(&received).map(|m| m.len()).unwrap_or(0) == 0
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let got = String::from_utf8_lossy(&std::fs::read(&received).unwrap_or_default()).into_owned();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        got.contains("echo one\recho two"),
        "the payload should arrive intact; got:\n{}",
        got.escape_debug()
    );
    // The point of this test: the child asked, so it gets the markers, and it
    // gets exactly one pair around the whole payload.
    assert_eq!(
        got.matches("\x1b[200~").count(),
        1,
        "exactly one start marker; got:\n{}",
        got.escape_debug()
    );
    assert_eq!(
        got.matches("\x1b[201~").count(),
        1,
        "exactly one end marker; got:\n{}",
        got.escape_debug()
    );
    let start = got.find("\x1b[200~").unwrap();
    let end = got.find("\x1b[201~").unwrap();
    assert!(
        start < end && got[start..end].contains("echo one"),
        "the payload must sit inside the bracket; got:\n{}",
        got.escape_debug()
    );
}
