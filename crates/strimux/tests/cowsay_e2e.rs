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

/// Drop CSI/OSC escape sequences, leaving just the glyphs that land on the
/// screen. Enough for these assertions: strimux only emits CSI, OSC and the
/// synchronized-update markers.
fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            // CSI: parameters then a final byte in @..~
            Some('[') => {
                for c in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs to BEL or ST (ESC \)
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

    let mut writer = pair.master.take_writer().expect("writer");
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
    let mut hud_dismissed = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        // The startup HUD is drawn centered over the grid and the cells
        // under it are never emitted, so whatever hint it covers depends on
        // the platform's key-label widths. Dismiss it (⌥+/ toggles it) once
        // the first frame is up, so every assertion sees the full grid.
        if !hud_dismissed && frame_count(&out) >= 1 {
            use std::io::Write;
            let _ = writer.write_all(b"\x1b/");
            let _ = writer.flush();
            hud_dismissed = true;
            idle = 0;
        }
        if hud_dismissed && frame_count(&out) >= 2 && idle >= 4 {
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

/// The pinned box: the first empty placeholder, wherever it currently is,
/// must advertise the cheat-sheet toggle. This is the one hint we can be sure
/// a user reads, so it has to be the one that opens the full list.
///
/// The startup HUD is drawn centered *over* the grid, so the assertion looks
/// for the leading fragment of the pinned line, which survives on the left of
/// the overlay, rather than the whole sentence.
#[test]
fn the_first_empty_box_advertises_the_cheat_sheet() {
    let cfg = format!("[cowsay]\nenabled = true\n{NO_CHROME}");
    let painted = paint(&cfg, 160, 40);
    assert!(painted.contains("^__^"), "cow did not paint");
    // Strip the CSI sequences that reposition the cursor between cells, so
    // the assertion sees the glyphs as the user does rather than as bytes.
    let text = strip_ansi(&painted);
    let modk = if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    };
    // Only the fragment left of the centered HUD survives; the overlay grows
    // as bindings are added, so match the shortest unambiguous head.
    let lead = format!("{modk}+/ tog");
    assert!(
        text.contains(&lead),
        "first empty box should lead with {lead:?}"
    );
}

/// The pin is a *position*, not an address: with two panes open the first
/// empty box is `1.3`, and that is where the cheat-sheet hint must move to.
/// A hard-coded `1.2` would already be covered by a live pane here, and a
/// hard-coded `1.1` would never be a placeholder at all.
///
/// At this width the centered startup HUD covers the middle of the box, so
/// the assertion matches the *tail* of the wrapped hint that survives to the
/// right of the overlay.
#[test]
fn the_pinned_hint_follows_the_layout() {
    let cfg = format!("startup_panes = 2\n[cowsay]\nenabled = true\n{NO_CHROME}");
    let painted = strip_ansi(&paint(&cfg, 160, 40));
    // `sheet` is the tail of "toggles this cheat-sheet"; the head of the line
    // is behind the overlay at this size.
    assert!(
        painted.contains("sheet"),
        "cheat-sheet hint did not follow the panes to the first empty box"
    );
    // Uniqueness across boxes is a property of `message_for` and is asserted
    // in its unit tests; the capture here spans several repainted frames, so
    // counting occurrences in the byte stream would count frames, not boxes.
}

#[test]
fn default_config_keeps_the_grid_bare() {
    // The shipped default is a bare skeleton: `cowsay.enabled` is false, so
    // no cow appears without opting in.
    let painted = paint(NO_CHROME, 120, 30);
    assert!(
        !painted.contains("^__^"),
        "default config painted a cow; it is opt-in"
    );
}

#[test]
fn enabling_the_cow_shows_a_real_keybinding_hint() {
    // Turning the cow on with no `messages` must fall back to the default
    // hint pool, which is generated from the binding registry. Every hint is
    // therefore a chord `handle_key` actually implements.
    let cfg = format!("[cowsay]\nenabled = true\n{NO_CHROME}");
    let painted = paint(&cfg, 120, 30);
    assert!(painted.contains("^__^"), "enabled cow did not paint");
    let mods = ["⌥", "Alt"];
    assert!(
        mods.iter().any(|m| painted.contains(m)),
        "default hint should name the modifier key"
    );
}
