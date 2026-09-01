//! End-to-end: drag-to-copy in a real pane of the real binary.
//!
//! The unit tests prove the geometry, the text extraction, and the highlight
//! styling in isolation. This is the acceptance check: it launches the
//! *shipped executable* on a real PTY, sends the SGR mouse sequences a
//! terminal emits for a click-drag across some pane text, and asserts on the
//! two things a user actually experiences:
//!
//!   1. the dragged cells are painted inverse (the selection highlight), and
//!   2. the selected text lands on the system clipboard when the button is
//!      released.
//!
//! (2) is verified without ever touching the developer's real clipboard: the
//! child runs with a `PATH` whose clipboard helper is a stub script that
//! writes to a file, so the assertion is on exactly the bytes gwae handed
//! to the platform clipboard tool.

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

/// The name of the clipboard helper gwae prefers on this platform. The stub
/// must shadow *that* program, since it is the first one gwae tries and the
/// only one it will run when it succeeds.
fn clipboard_helper() -> &'static str {
    if cfg!(target_os = "macos") {
        "pbcopy"
    } else if cfg!(windows) {
        "clip"
    } else {
        "wl-copy"
    }
}

/// An SGR (1006) mouse sequence as a terminal sends it: 1-based coordinates,
/// `M` for press/motion and `m` for release.
fn sgr(code: u16, col: u16, row: u16, release: bool) -> String {
    format!(
        "\x1b[<{};{};{}{}",
        code,
        col + 1,
        row + 1,
        if release { 'm' } else { 'M' }
    )
}

/// Left button press / drag / release at a screen cell (0-based).
fn press(col: u16, row: u16) -> String {
    sgr(0, col, row, false)
}
fn drag(col: u16, row: u16) -> String {
    sgr(32, col, row, false)
}
fn release(col: u16, row: u16) -> String {
    sgr(0, col, row, true)
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

#[test]
fn drag_highlights_pane_text_and_copies_it_to_the_clipboard() {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-select-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("stub bin dir");
    // Pane content is inset 1 cell inside its column frame, so pane grid cell
    // (x, y) is screen cell (x + 1, y + 1); the coordinates below are the ones
    // a user's mouse would really report.
    std::fs::write(dir.join("gwae/gwae.toml"), "[minimap]\nshow = false\n").expect("write config");

    // The stub clipboard helper: capture stdin instead of the real clipboard.
    let captured = dir.join("clipboard.txt");
    let stub = bin.join(clipboard_helper());
    std::fs::write(&stub, format!("#!/bin/sh\ncat > {}\n", captured.display()))
        .expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
    }

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
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd.arg("run");
    // A pane that prints a known line and then holds the terminal open. The
    // child never asks for mouse reporting, so the drag is ours to handle.
    // Written as a tiny script rather than an inline `sh -c '...'`: gwae's
    // own command splitting is not a shell, so nested quotes would not survive.
    let script = dir.join("pane.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'HELLO-GWAE WORLD\\n'\nsleep 30\n",
    )
    .expect("write pane script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod pane script");
    }
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

    // Wait for the first frame, then dismiss the startup HUD (it covers the
    // pane text we are about to select).
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
        String::from_utf8_lossy(&boot).contains("HELLO-GWAE"),
        "the pane text never reached the screen; cannot select it. got:\n{}",
        String::from_utf8_lossy(&boot).escape_debug()
    );

    // Drag across "HELLO-GWAE" (10 cells, grid columns 0..=9 of grid row
    // 0, i.e. screen columns 1..=10 of screen row 1).
    let mut painted = Vec::new();
    writer.write_all(press(1, 1).as_bytes()).expect("press");
    writer.flush().ok();
    writer.write_all(drag(5, 1).as_bytes()).expect("drag");
    writer.flush().ok();
    writer.write_all(drag(10, 1).as_bytes()).expect("drag end");
    writer.flush().ok();
    drain_until_quiet(&rx, &mut painted, 4);

    let mid_drag = String::from_utf8_lossy(&painted).into_owned();
    // The highlight is SGR 7 (reverse video): mid-drag the terminal must have
    // been told to invert the dragged cells.
    assert!(
        mid_drag.contains("\x1b[7m") || mid_drag.contains(";7m") || mid_drag.contains("[7;"),
        "no reverse-video highlight was painted during the drag; got:\n{}",
        mid_drag.escape_debug()
    );
    // The child must not have been fed the drag: nothing was typed into it.
    assert!(
        !captured.exists(),
        "the clipboard was written before the button was released"
    );

    // Release: this is what copies.
    writer
        .write_all(release(10, 1).as_bytes())
        .expect("release");
    writer.flush().ok();
    let mut after = Vec::new();
    drain_until_quiet(&rx, &mut after, 6);

    // The helper is spawned asynchronously from the render loop's point of
    // view; give it a moment to land on disk.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !captured.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let copied = std::fs::read_to_string(&captured).unwrap_or_default();

    let _ = child.kill();
    let _ = child.wait();
    drop(pair.master);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        copied.trim_end_matches('\n'),
        "HELLO-GWAE",
        "the dragged text should be exactly what reached the clipboard"
    );
    // And the user is told it happened.
    let note = String::from_utf8_lossy(&after);
    assert!(
        note.contains("copied"),
        "no copy confirmation was shown; got:\n{}",
        note.escape_debug()
    );
}

#[test]
fn copy_mode_enters_reports_view_and_session_to_clipboard() {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    fn frame_count(out: &[u8]) -> usize { out.windows(8).filter(|w| *w == b"\x1b[?2026h").count() }
    fn clipboard_helper() -> &'static str {
        if cfg!(target_os = "macos") { "pbcopy" } else if cfg!(windows) { "clip" } else { "wl-copy" }
    }

    static NEXT: AtomicUsize = AtomicUsize::new(999);
    let dir = std::env::temp_dir().join(format!("gwae-copy-mode-e2e-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(dir.join("gwae/gwae.toml"), "[minimap]\nshow = false\n").expect("cfg");
    let captured = dir.join("clipboard.txt");
    let stub = bin.join(clipboard_helper());
    std::fs::write(&stub, format!("#!/bin/sh\ncat > {}\n", captured.display())).expect("stub");
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap(); }

    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 24, cols: 100, pixel_width: 0, pixel_height: 0 }).expect("pty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd.env("TERM", "xterm-256color");
    cmd.env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    cmd.arg("run");
    let script = dir.join("pane.sh");
    std::fs::write(&script, "#!/bin/sh\nfor i in $(seq 1 60); do echo VIEW-$i; done\nprintf 'TAIL-LINE\n'\nsleep 30\n").expect("script");
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap(); }
    cmd.arg(script.display().to_string());
    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || { let mut buf=[0u8;8192]; loop { match reader.read(&mut buf) { Ok(0)|Err(_)=>break, Ok(n)=> if tx.send(buf[..n].to_vec()).is_err(){break} } } });

    let mut boot=Vec::new();
    let deadline=Instant::now()+Duration::from_secs(20);
    while Instant::now()<deadline && frame_count(&boot)<1 { if let Ok(b)=rx.recv_timeout(Duration::from_millis(200)) { boot.extend_from_slice(&b);} }
    assert!(frame_count(&boot)>=1, "no frame");
    writer.write_all(b"\x1b").unwrap(); writer.flush().ok();
    // drain until pane content visible
    for _ in 0..20 { if let Ok(b)=rx.recv_timeout(Duration::from_millis(200)) { boot.extend_from_slice(&b);} std::thread::sleep(Duration::from_millis(50)); }
    // Enter copy mode via glyph (Option+c on macOS sends ç)
    writer.write_all("ç".as_bytes()).unwrap(); writer.flush().ok();
    std::thread::sleep(Duration::from_millis(400));
    let mut out=Vec::new(); while let Ok(b)=rx.recv_timeout(Duration::from_millis(200)) { out.extend_from_slice(&b); if out.windows(9).any(|w| w==b"copy mode") { break; } }
    assert!(String::from_utf8_lossy(&out).contains("copy mode"), "copy mode not entered: {}", String::from_utf8_lossy(&out).escape_debug());
    // Press a plain 'a' to copy session (should capture scrollback)
    let _ = std::fs::remove_file(&captured);
    writer.write_all(b"a").unwrap(); writer.flush().ok();
    let deadline=Instant::now()+Duration::from_secs(5);
    while Instant::now()<deadline && !captured.exists() { std::thread::sleep(Duration::from_millis(50)); }
    let sess = std::fs::read_to_string(&captured).unwrap_or_default();
    assert!(sess.contains("VIEW-1"), "session missing scrollback VIEW-1, got {:?}", &sess[..sess.len().min(300)]);
    assert!(sess.contains("TAIL-LINE"), "session missing tail");

    // Re-enter and copy view (Enter) should be smaller than session
    std::thread::sleep(Duration::from_millis(200));
    writer.write_all("ç".as_bytes()).unwrap(); writer.flush().ok();
    std::thread::sleep(Duration::from_millis(300));
    let _ = std::fs::remove_file(&captured);
    writer.write_all(b"\r").unwrap(); writer.flush().ok();
    let deadline=Instant::now()+Duration::from_secs(5);
    while Instant::now()<deadline && !captured.exists() { std::thread::sleep(Duration::from_millis(50)); }
    let view = std::fs::read_to_string(&captured).unwrap_or_default();
    assert!(!view.is_empty(), "view empty");
    assert!(view.len() < sess.len(), "view not smaller than session: view {} session {}", view.len(), sess.len());

    let _=child.kill(); let _=child.wait(); drop(pair.master); let _=std::fs::remove_dir_all(&dir);
}
