//! End-to-end: a `theme = "..."` in a real config file on disk must change the
//! bytes the real binary paints to a real terminal.
//!
//! This is the acceptance check for the theming feature. The `Palette` unit
//! tests prove the resolution rules; this drives the *shipped executable*
//! through a PTY with a config file it discovers on its own via
//! `XDG_CONFIG_HOME`, and asserts on the SGR sequences that actually reach the
//! terminal.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::channel;
use std::time::Duration;

/// How many frames have been painted.
///
/// strimux wraps each frame in the synchronized-update markers
/// `ESC[?2026h ... ESC[?2026l`, so counting the opening marker counts frames.
/// The tests need this because strimux emits startup log lines *before* it
/// paints and paints incrementally afterwards: "some bytes arrived and then
/// it went quiet" can be satisfied by the logs alone, or by a first frame
/// that has not yet drawn the cells under the startup HUD.
fn frame_count(out: &[u8]) -> usize {
    out.windows(8).filter(|w| *w == b"\x1b[?2026h").count()
}

/// Run the real strimux binary with `config_body` as its config file and
/// return every byte it painted before it settled.
fn paint_with_config(config_body: &str) -> String {
    paint_with_config_raw(&format!(
        // The HUD switch goes *after* the case's own body: TOML tables
        // swallow every key that follows them, so prepending `[minimap]`
        // would capture the case's top-level keys into that table instead.
        "{config_body}\n[minimap]\nhud_on_attention_ms = 0\n"
    ))
}

/// As [`paint_with_config`], but writes `config_body` to disk verbatim.
///
/// Used by the malformed-config case, which must not have anything appended
/// to it (and cannot have the HUD disabled, since strimux discards the whole
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
        "strimux-theme-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
    std::fs::write(dir.join("strimux/strimux.toml"), config_body).expect("write config");

    let pty = native_pty_system();
    let pair = pty
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
    // Keep the child's own pane trivial and quiet so the only colored cells on
    // screen are strimux's own chrome.
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

    // Collect until a real frame has landed and the paint has then gone quiet.
    //
    // strimux shows a centered startup HUD that persists until a key press,
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
        "strimux never repainted after the resize; captured {} bytes",
        out.len()
    );
    String::from_utf8_lossy(&out).into_owned()
}

/// The SGR *foreground* sequence emitted for a 24-bit color.
///
/// The skeleton frames are the chrome that is always on screen, and they are
/// drawn as glyphs, so the accent and overlay colors arrive as foreground
/// codes. `base` only shows through in genuinely uncovered cells, which a
/// default skeleton layout does not have, so these tests key off the frames.
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

/// The SGR background sequence emitted for a 24-bit color.
fn bg_seq(r: u8, g: u8, b: u8) -> String {
    format!("48;2;{r};{g};{b}")
}

/// Mocha's accent (sapphire) and overlay0, the default focus ring and
/// skeleton frame colors.
const MOCHA_ACCENT: (u8, u8, u8) = (0x74, 0xc7, 0xec);
const MOCHA_OVERLAY: (u8, u8, u8) = (0x6c, 0x70, 0x86);
/// Nord's accent and overlay.
const NORD_ACCENT: (u8, u8, u8) = (0x88, 0xc0, 0xd0);
const NORD_OVERLAY: (u8, u8, u8) = (0x4c, 0x56, 0x6a);

#[test]
fn default_config_paints_the_catppuccin_mocha_chrome() {
    let painted = paint_with_config("");
    let (r, g, b) = MOCHA_ACCENT;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "the focused box frame should be the Mocha accent; saw {:?}",
        fgs_in(&painted)
    );
    let (r, g, b) = MOCHA_OVERLAY;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "unfocused skeleton frames should be the Mocha overlay"
    );
}

#[test]
fn theme_preset_changes_the_painted_chrome() {
    // Naming a preset must put Nord's colors on the wire and remove Mocha's.
    let painted = paint_with_config("theme = \"nord\"\n");
    let (r, g, b) = NORD_ACCENT;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "theme = nord should paint the Nord accent; saw {:?}",
        fgs_in(&painted)
    );
    let (r, g, b) = NORD_OVERLAY;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "theme = nord should paint the Nord overlay"
    );
    let (r, g, b) = MOCHA_ACCENT;
    assert!(
        !painted.contains(&fg_seq(r, g, b)),
        "a Nord run must not paint the Mocha accent"
    );
}

#[test]
fn theme_table_override_reaches_the_screen() {
    // Start from Nord but override just the accent: the override must win and
    // the preset's own accent must be gone, while its overlay is untouched.
    let painted = paint_with_config("[theme]\npreset = \"nord\"\naccent = \"#010203\"\n");
    assert!(
        painted.contains(&fg_seq(1, 2, 3)),
        "the [theme] accent override should reach the screen; saw {:?}",
        fgs_in(&painted)
    );
    let (r, g, b) = NORD_ACCENT;
    assert!(
        !painted.contains(&fg_seq(r, g, b)),
        "the overridden Nord accent must not be painted"
    );
    let (r, g, b) = NORD_OVERLAY;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "the rest of the Nord preset must survive the override"
    );
}

#[test]
fn legacy_color_keys_still_work() {
    // A pre-theme config file must keep painting exactly what it did before
    // the theme system existed.
    let painted = paint_with_config("focus_color = \"#040506\"\nskeleton_color = \"#070809\"\n");
    assert!(
        painted.contains(&fg_seq(4, 5, 6)),
        "the legacy focus_color key must still be honored; saw {:?}",
        fgs_in(&painted)
    );
    assert!(
        painted.contains(&fg_seq(7, 8, 9)),
        "the legacy skeleton_color key must still be honored"
    );
}

#[test]
fn legacy_keys_beat_a_named_preset() {
    let painted = paint_with_config("theme = \"nord\"\nfocus_color = \"#040506\"\n");
    assert!(
        painted.contains(&fg_seq(4, 5, 6)),
        "an explicit legacy key must win over the preset; saw {:?}",
        fgs_in(&painted)
    );
    let (r, g, b) = NORD_ACCENT;
    assert!(
        !painted.contains(&fg_seq(r, g, b)),
        "the preset accent must not be painted when overridden"
    );
    let (r, g, b) = NORD_OVERLAY;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "keys the user did not override still come from the preset"
    );
}

#[test]
fn background_key_paints_uncovered_cells() {
    // `base` is only visible where nothing covers it, so turn the skeleton
    // off: the placeholder boxes go away and the empty right side is bare
    // background.
    let painted = paint_with_config("skeleton = false\nbackground = \"#040506\"\n");
    assert!(
        painted.contains(&bg_seq(4, 5, 6)),
        "the uncovered background should be painted with the configured base; saw {:?}",
        fgs_in(&painted)
    );
}

#[test]
fn terminal_theme_paints_no_hardcoded_colors() {
    // `theme = "terminal"` inherits the host scheme, so strimux must not emit
    // any 24-bit color of its own; the frames come out as ANSI indices.
    let painted = paint_with_config("theme = \"terminal\"\n");
    for (name, (r, g, b)) in [
        ("Mocha accent", MOCHA_ACCENT),
        ("Mocha overlay", MOCHA_OVERLAY),
        ("Nord accent", NORD_ACCENT),
    ] {
        assert!(
            !painted.contains(&fg_seq(r, g, b)),
            "terminal theme must not paint the hardcoded {name}"
        );
    }
    assert!(
        painted.contains("38;5;"),
        "terminal theme should paint chrome as ANSI palette indices"
    );
}

#[test]
fn an_unparseable_config_still_starts_with_default_colors() {
    // Bad TOML falls back to defaults rather than refusing to launch.
    let painted = paint_with_config_raw("this is not valid toml <<<\n");
    let (r, g, b) = MOCHA_ACCENT;
    assert!(
        painted.contains(&fg_seq(r, g, b)),
        "a broken config should fall back to the default Mocha chrome; saw {:?}",
        fgs_in(&painted)
    );
}
