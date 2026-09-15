//! End-to-end: gwae paints the host terminal's own colors.
//!
//! gwae has no theming. Chrome is always the terminal's default
//! foreground/background pair plus ANSI 0-15 indices, so this drives the
//! *shipped executable* through a PTY with a config file it discovers on its
//! own via `XDG_CONFIG_HOME`, and asserts on the SGR sequences that actually
//! reach the terminal: ANSI indices present, no 24-bit chrome colors, and no
//! config key able to change that.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::channel;
use std::time::Duration;

/// How many frames have been painted.
///
/// gwae wraps each frame in the synchronized-update markers
/// `ESC[?2026h ... ESC[?2026l`, so counting the opening marker counts frames.
/// The tests need this because gwae emits startup log lines *before* it
/// paints and paints incrementally afterwards: "some bytes arrived and then
/// it went quiet" can be satisfied by the logs alone, or by a first frame
/// that has not yet drawn the cells under the startup HUD.
fn frame_count(out: &[u8]) -> usize {
    out.windows(8).filter(|w| *w == b"\x1b[?2026h").count()
}

/// Run the real gwae binary with `config_body` as its config file and
/// return every byte it painted before it settled.
fn paint_with_config(config_body: &str) -> String {
    // The frames are the chrome these cases read the colors off: they are
    // drawn as glyphs, so every palette key arrives as a foreground code.
    paint_with_config_raw(&format!("{config_body}\n"))
}

/// As [`paint_with_config`], but writes `config_body` to disk verbatim.
///
/// Used by the malformed-config case, which must not have anything appended
/// to it (and cannot have the HUD disabled, since gwae discards the whole
/// unparseable file and falls back to defaults).
fn paint_with_config_raw(config_body: &str) -> String {
    // A per-case config directory. The counter is what actually guarantees
    // uniqueness: these tests run as threads of one process, so the pid is
    // shared, and a wall-clock timestamp is not reliably distinct between
    // threads that start together. Two cases landing on the same directory
    // means one overwrites the other's config and asserts against the wrong
    // theme.
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-theme-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    std::fs::write(dir.join("gwae/gwae.toml"), config_body).expect("write config");

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
    // Keep the child's own pane trivial and quiet so the only colored cells on
    // screen are gwae's own chrome.
    cmd.arg("run");
    cmd.arg("sleep 30");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn gwae");
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

    // Collect until a real frame has landed and the paint has then gone quiet.
    //
    // gwae shows a centered startup HUD that persists until a key press,
    // and the painter only emits *changed* cells, so chrome sitting under the
    // HUD may never reach the wire while it is up. Once the first frame is
    // seen, dismiss the HUD with a bare Escape and keep reading: the repaint
    // that follows redraws the cells it was covering, so every chrome color
    // is guaranteed to appear in the capture.
    // Collect until the paint goes quiet. The test config disables the
    // startup HUD, so the first frame already contains the full chrome and
    // nothing later covers it.
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
        "gwae never repainted after the resize; captured {} bytes",
        out.len()
    );
    String::from_utf8_lossy(&out).into_owned()
}

/// The SGR *foreground* sequence emitted for a 24-bit color.
///
/// The skeleton frames are the chrome that is always on screen, and they are
/// drawn as glyphs, so any hardcoded RGB would arrive as a foreground code.
fn fg_seq(r: u8, g: u8, b: u8) -> String {
    format!("38;2;{r};{g};{b}")
}

/// All distinct 24-bit foreground colors in a capture, for failure messages.
fn fgs_in(painted: &str) -> Vec<String> {
    let mut v: Vec<String> = painted
        .split("38;2;")
        .skip(1)
        .filter_map(|t| t.split('m').next().map(|s| s.to_string()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Colors no terminal-native chrome may ever emit: the old presets'
/// accents and overlays. If any of these appear, gwae painted a hardcoded
/// RGB instead of the host terminal's palette.
const RETIRED_RGB: &[(&str, (u8, u8, u8))] = &[
    ("Mocha accent", (0x74, 0xc7, 0xec)),
    ("Mocha overlay", (0x6c, 0x70, 0x86)),
    ("Nord accent", (0x88, 0xc0, 0xd0)),
    ("Nord overlay", (0x4c, 0x56, 0x6a)),
    ("Latte accent", (0x20, 0x9f, 0xb5)),
    ("white phosphor", (0xd8, 0xd8, 0xd0)),
    ("keep-awake red", (0xff, 0x40, 0x40)),
];

fn assert_terminal_native(painted: &str, ctx: &str) {
    for (name, (r, g, b)) in RETIRED_RGB {
        assert!(
            !painted.contains(&fg_seq(*r, *g, *b)),
            "{ctx} must not paint the retired hardcoded {name}; saw {fgs:?}",
            fgs = fgs_in(painted),
        );
    }
    assert!(
        painted.contains("38;5;"),
        "{ctx} should paint chrome as ANSI palette indices; saw {fgs:?}",
        fgs = fgs_in(painted),
    );
}

#[test]
fn default_config_paints_terminal_native_chrome() {
    let painted = paint_with_config("");
    assert_terminal_native(&painted, "a default run");
}

#[test]
fn retired_theme_keys_do_not_change_the_painted_chrome() {
    // Old configs name presets and overrides gwae no longer reads. They
    // must be ignored, not fatal, and the screen must look exactly like a
    // default run: terminal-native chrome.
    for config in [
        "theme = \"nord\"\n",
        "[theme]\npreset = \"nord\"\naccent = \"#010203\"\n",
        "focus_color = \"#040506\"\nskeleton_color = \"#070809\"\n",
        "theme = \"white-phosphor\"\n",
    ] {
        let painted = paint_with_config(config);
        assert_terminal_native(&painted, &format!("config {config:?}"));
    }
}

#[test]
fn an_unparseable_config_still_starts_with_terminal_chrome() {
    // Bad TOML falls back to defaults rather than refusing to launch.
    let painted = paint_with_config_raw("this is not valid toml <<<\n");
    assert_terminal_native(&painted, "a broken config");
}
