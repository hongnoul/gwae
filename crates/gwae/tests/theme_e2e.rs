//! End-to-end: gwae paints the enforced retro chrome by default.
//!
//! The default session paints explicit RGB colors (true-black panels,
//! high-contrast functional colors) regardless of the host terminal scheme.
//! A `[theme]` override changes exactly the named keys. This drives the
//! *shipped executable* through a PTY with a config file it discovers on its
//! own via `XDG_CONFIG_HOME`, and asserts on the SGR sequences that actually
//! reach the terminal.

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

/// The SGR *background* sequence emitted for a 24-bit color.
fn bg_seq(r: u8, g: u8, b: u8) -> String {
    format!("48;2;{r};{g};{b}")
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

/// The enforced retro default: true-black panels with high-contrast
/// functional colors (white focus, blue running, amber idle, green done,
/// red failed, white text). A default run must paint these RGB values, not
/// the host terminal's palette indices.
fn assert_retro_chrome(painted: &str, ctx: &str) {
    // White focus accent and true-black panel background must reach the wire.
    assert!(
        painted.contains(&fg_seq(0xff, 0xff, 0xff)),
        "{ctx} must paint the retro white focus accent; saw {fgs:?}",
        fgs = fgs_in(painted),
    );
    // The focus ring is bold white on the wire, so focus reads by weight as
    // well as color; unfocused chrome is a dimmer gray for the delta.
    assert!(
        painted.contains("\x1b[1m"),
        "{ctx} must paint the focus ring bold; capture has no SGR bold",
    );
    assert!(
        painted.contains(&fg_seq(0x3f, 0x3f, 0x3f)),
        "{ctx} must paint dim gray unfocused chrome; saw {fgs:?}",
        fgs = fgs_in(painted),
    );
    assert!(
        painted.contains(&bg_seq(0x00, 0x00, 0x00)),
        "{ctx} must paint true-black panel backgrounds",
    );
    // The terminal-native chrome must be gone: no ANSI-index chrome colors.
    assert!(
        !painted.contains("38;5;6"),
        "{ctx} must not inherit the terminal cyan for focus; saw {fgs:?}",
        fgs = fgs_in(painted),
    );
}

#[test]
fn default_config_paints_retro_chrome() {
    let painted = paint_with_config("");
    assert_retro_chrome(&painted, "a default run");
}

#[test]
fn theme_overrides_repaint_only_the_named_keys() {
    // An accent override swaps the focus ring to magenta while the black
    // panels stay. Text is overridden too: white is both the retro focus
    // accent and the retro text color, so leaving text alone would keep
    // white on the wire and prove nothing about the accent swap.
    let painted = paint_with_config("[theme]\naccent = \"#ff00ff\"\ntext = \"#00ff00\"\n");
    assert!(
        painted.contains(&fg_seq(0xff, 0x00, 0xff)),
        "the accent override must reach the wire; saw {fgs:?}",
        fgs = fgs_in(&painted),
    );
    assert!(
        !painted.contains(&fg_seq(0xff, 0xff, 0xff)),
        "retro white must be gone once overridden; saw {fgs:?}",
        fgs = fgs_in(&painted),
    );
    assert!(
        painted.contains(&bg_seq(0x00, 0x00, 0x00)),
        "unrelated keys keep the retro default",
    );
}

#[test]
fn retired_preset_names_still_start_on_retro() {
    // Old configs naming retired presets must be ignored, not fatal, and the
    // screen must look exactly like a default run: retro chrome.
    for config in [
        "theme = \"nord\"\n",
        "[theme]\npreset = \"nord\"\n",
        "focus_color = \"#040506\"\n",
    ] {
        let painted = paint_with_config(config);
        assert_retro_chrome(&painted, &format!("config {config:?}"));
    }
}

#[test]
fn an_unparseable_config_still_starts_on_retro() {
    // Bad TOML falls back to defaults rather than refusing to launch.
    let painted = paint_with_config_raw("this is not valid toml <<<\n");
    assert_retro_chrome(&painted, "a broken config");
}
