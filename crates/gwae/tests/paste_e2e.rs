//! End-to-end: native Cmd+V and optional `⌥+v` preserve pasted text in panes.
//!
//! The real executable runs under a host PTY with an isolated HOME/config and
//! a stub `pbpaste` reading exact fixture bytes. Readiness and completion are
//! observable terminal/file states, never a quiet interval in the output stream.
//! Fish execution is checked with a file sentinel, not repaint-sensitive counts.
//! Drag selection must copy the highlighted text on release, then `⌥+v` must
//! paste those exact bytes. Clipboard helpers are isolated from the user's OS.
//!
//! macOS-only because the clipboard helper table differs on other platforms.
#![cfg(target_os = "macos")]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const HOST: Size = Size {
    rows: 24,
    cols: 120,
};
const TIMEOUT: Duration = Duration::from_secs(15);
const OPT_V: &[u8] = b"\x1bv";
const OPT_Q: &[u8] = b"\x1bQ";
const FISH_BARRIER: &[u8] = b"\x07";

// Noncanonical input makes every pasted byte reach tee immediately. Retaining
// ICRNL/ONLCR lets the terminal show newlines normally, without duplicate echo.
// tee never enables bracketed paste, and captures any unwanted escape markers.
const PLAIN_HELPER: &str = "#!/bin/sh\n\
    stty -echo -icanon min 1 time 0\n\
    printf 'PLAIN_READY\\r\\n'\n\
    exec /usr/bin/tee received-paste\n";

// This binding is an input-processing barrier. Once its sentinel advances,
// fish has processed everything preceding Ctrl+G, including the paste's closing
// bracket. It does not execute or alter the command buffer.
const FISH_INIT: &str = "set -g fish_greeting\n\
    set -g fish_autosuggestion_enabled 0\n\
    function fish_prompt\n    printf 'FISH_READY> '\nend\n\
    function fish_right_prompt\nend\n\
    function paste_probe\n    printf 'EXECUTED\\n' >> paste-executions\nend\n\
    bind \\cg 'printf x >> paste-barrier'\n";

const ZSH_INIT: &str = "PROMPT='ZSH_READY> '\nRPROMPT=''\nPROMPT_EOL_MARK=''\n\
    bindkey -e\n\
    paste_probe() { print -r -- EXECUTED >> paste-executions; }\n\
    paste_barrier() { print -rn -- x >> paste-barrier; }\n\
    zle -N paste_barrier\nbindkey '^G' paste_barrier\n";

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    screen: Vt100Grid,
    raw: Vec<u8>,
    dir: PathBuf,
}

impl Session {
    fn start(clipboard_text: &str) -> Session {
        Self::start_with(clipboard_text, "/bin/sh plain-helper.sh", "PLAIN_READY")
    }

    fn start_with(clipboard_text: &str, pane_cmd: &str, ready: &str) -> Session {
        Self::start_with_env(clipboard_text, pane_cmd, ready, &[])
    }

    fn start_with_env(
        clipboard_text: &str,
        pane_cmd: &str,
        ready: &str,
        extra_env: &[(&str, &str)],
    ) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-paste-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "startup_panes = 1\ndefault_column_width = \"quarter\"\ncontent_width = 0\n\
             center_focus = false\nkeep_awake = false\ncell_labels = false\n\
             [minimap]\nshow = false\n[cowsay]\nenabled = false\n\
             [update]\ncheck = false\n",
        )
        .expect("write config");
        std::fs::write(dir.join("clipboard"), clipboard_text).expect("write clipboard bytes");
        std::fs::write(dir.join("plain-helper.sh"), PLAIN_HELPER).expect("write plain helper");
        std::fs::write(
            dir.join("bracket-helper.sh"),
            "#!/bin/sh\nstty raw -echo\n\
             printf '\x1b[?2004hBRACKET_READY\r\n'\n\
             exec /usr/bin/tee received-paste\n",
        )
        .expect("write bracket-aware helper");
        std::fs::write(
            dir.join("mode-helper.sh"),
            "#!/bin/sh\nstty raw -echo\n\
             printf '\x1b[?2004hMODE_READY\r\n'\n\
             dd bs=1 count=17 of=first-paste 2>/dev/null\n\
             printf '\x1b[?2004lMODE_OFF\r\n'\n\
             exec /usr/bin/tee received-paste\n",
        )
        .expect("write mode-changing helper");
        std::fs::write(
            dir.join("selection-helper.sh"),
            "#!/bin/sh\nstty -echo -icanon min 1 time 0\n\
             printf 'ROW_ONE 日本語 é\r\nROW_TWO 끝\r\n'\n\
             exec /usr/bin/tee received-paste\n",
        )
        .expect("write Unicode helper");
        std::fs::write(
            dir.join("mouse-helper.sh"),
            "#!/bin/sh\nstty -echo -icanon min 1 time 0\n\
             printf '\x1b[?1000h\x1b[?1006hMOUSE_READY\r\n'\n\
             exec /usr/bin/tee received-paste\n",
        )
        .expect("write mouse-reporting helper");
        std::fs::write(dir.join("fish-init.fish"), FISH_INIT).expect("write fish init");
        std::fs::write(dir.join(".zshrc"), ZSH_INIT).expect("write isolated zsh init");

        // Read a data file rather than interpolating clipboard text into shell
        // syntax. Quotes, actual newlines, and literal backslashes stay exact.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("temp bin dir");
        std::fs::write(
            bin.join("pbpaste"),
            "#!/bin/sh\nexec /bin/cat \"$GWAE_TEST_CLIPBOARD\"\n",
        )
        .expect("write stub pbpaste");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("pbpaste"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub pbpaste");
        // A regression must never overwrite the user's real clipboard. Record
        // every native copy and redirect it to the isolated clipboard.
        std::fs::write(
            bin.join("pbcopy"),
            "#!/bin/sh\nprintf x >> \"$GWAE_TEST_CLIPBOARD.calls\"\nexec /bin/cat > \"$GWAE_TEST_CLIPBOARD\"\n",
        )
        .expect("write stub pbcopy");
        std::fs::set_permissions(bin.join("pbcopy"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub pbcopy");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: HOST.rows,
                cols: HOST.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let binary =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        let mut cmd = CommandBuilder::new(binary);
        // No user shell/editor/history, debug logging, reload handover, terminal
        // identity, or clipboard overrides may leak into this test.
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("ENV", "/dev/null");
        cmd.env("BASH_ENV", "/dev/null");
        cmd.env("LC_ALL", "C");
        cmd.env("GWAE_LOG", "off");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        cmd.env("GWAE_KITTY_GRAPHICS", "0");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
        cmd.env("GWAE_TEST_CLIPBOARD", dir.join("clipboard"));
        cmd.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.arg("run");
        cmd.arg(pane_cmd);
        let child = pair.slave.spawn_command(cmd).expect("spawn actual gwae");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut s = Session {
            rx,
            writer,
            child,
            _master: pair.master,
            screen: Vt100Grid::new(HOST),
            raw: Vec::new(),
            dir,
        };
        s.wait_for("child prompt/readiness", |s| s.shown().contains(ready));
        s
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    // Emulate the host terminal's Cmd+V, not the local clipboard shortcut.
    // A real host only emits delimiters when gwae requests DECSET 2004.
    fn native_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.screen.wants_bracketed_paste() {
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn shown(&self) -> String {
        self.screen.visible_text()
    }

    fn file(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap_or_default()
    }

    fn find_text(&self, text: &str) -> (u16, u16) {
        (0..HOST.rows)
            .flat_map(|y| (0..=HOST.cols - text.chars().count() as u16).map(move |x| (x, y)))
            .find(|&(x, y)| {
                text.chars()
                    .enumerate()
                    .all(|(i, ch)| self.screen.cell(x + i as u16, y).ch == ch)
            })
            .unwrap_or_else(|| panic!("missing {text:?} in:\n{}", self.shown()))
    }

    fn mouse(&mut self, button: u8, x: u16, y: u16, release: bool) {
        let end = if release { 'm' } else { 'M' };
        self.send(format!("\x1b[<{button};{};{}{end}", x + 1, y + 1).as_bytes());
    }

    fn wait_for(&mut self, label: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if ready(self) {
                return;
            }
            if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(20)) {
                self.raw.extend_from_slice(&bytes);
                self.screen.feed(&bytes);
            }
        }
        panic!(
            "{label}: deadline expired\nreceived paste: {:?}\nexecutions: {:?}\nbarrier: {:?}\nclipboard: {:?}\ncopy calls: {:?}\nhost screen:\n{}",
            self.file("received-paste"),
            self.file("paste-executions"),
            self.file("paste-barrier"),
            self.file("clipboard"),
            self.file("clipboard.calls"),
            self.shown()
        );
    }
}

#[test]
fn drag_copies_on_release_then_option_v_pastes_selected_text() {
    let clipboard = "clipboard-before-drag";
    let mut s = Session::start(clipboard);
    let label = "PLAIN_READY";
    let (x, y) = (0..HOST.rows)
        .flat_map(|y| (0..=HOST.cols - label.len() as u16).map(move |x| (x, y)))
        .find(|&(x, y)| {
            label
                .chars()
                .enumerate()
                .all(|(i, ch)| s.screen.cell(x + i as u16, y).ch == ch)
        })
        .expect("locate actual child text rather than assuming pane coordinates");
    let baseline: Vec<_> = (0..label.len() as u16)
        .map(|i| s.screen.cell(x + i, y).style)
        .collect();
    let highlighted = |s: &Session, range: std::ops::RangeInclusive<usize>| {
        baseline.iter().enumerate().all(|(i, style)| {
            let mut expected = *style;
            if range.contains(&i) {
                expected.inverse = !expected.inverse;
            }
            s.screen.cell(x + i as u16, y).style == expected
        })
    };
    let mouse = |button, dx, end| format!("\x1b[<{button};{};{}{end}", x + dx + 1, y + 1);

    // Real SGR mouse input traverses crossterm, hit-testing, selection state,
    // painting, and the host terminal parser. Release deliberately extends the
    // range so a stale drag frame cannot satisfy the release assertion.
    s.send(mouse(0, 1, 'M').as_bytes());
    s.send(mouse(32, 4, 'M').as_bytes());
    s.wait_for("forward drag highlights only cells 1 through 4", |s| {
        highlighted(s, 1..=4)
    });
    assert_eq!(
        s.file("clipboard"),
        clipboard.as_bytes(),
        "not before release"
    );
    assert!(!s.dir.join("clipboard.calls").exists());
    s.send(mouse(0, 6, 'm').as_bytes());
    s.wait_for("release highlights and copies cells 1 through 6", |s| {
        highlighted(s, 1..=6)
            && s.file("clipboard") == b"LAIN_R"
            && s.shown().contains("copied 1 line")
    });
    assert_eq!(s.file("clipboard.calls"), b"x");
    eprintln!("observed forward release copied LAIN_R and confirmed it in the pane");

    s.send(mouse(0, 8, 'M').as_bytes());
    s.send(mouse(0, 8, 'm').as_bytes());
    s.wait_for("a plain click clears the completed selection", |s| {
        baseline
            .iter()
            .enumerate()
            .all(|(i, style)| s.screen.cell(x + i as u16, y).style == *style)
    });
    assert_eq!(s.file("clipboard"), b"LAIN_R", "plain click must not copy");

    s.send(mouse(0, 6, 'M').as_bytes());
    s.send(mouse(32, 3, 'M').as_bytes());
    s.wait_for("backward drag highlights cells 3 through 6", |s| {
        highlighted(s, 3..=6)
    });
    s.send(mouse(0, 0, 'm').as_bytes());
    s.wait_for(
        "backward release highlights and copies through cell 0",
        |s| highlighted(s, 0..=6) && s.file("clipboard") == b"PLAIN_R",
    );
    eprintln!("observed plain click preserves clipboard, backward release copied PLAIN_R");

    // A duplicate release without a live drag must not recopy a stale range.
    s.send(mouse(0, 8, 'm').as_bytes());
    // The received paste is an input-processing barrier after the releases.
    s.send(OPT_V);
    s.wait_for(
        "paste after selection delivers exactly the selected text",
        |s| s.file("received-paste") == b"PLAIN_R" && s.shown().contains("pasted 1 line"),
    );
    assert_eq!(s.file("clipboard"), b"PLAIN_R");
    assert_eq!(s.file("clipboard.calls"), b"xx", "one copy per real drag");
    assert!(
        !s.raw.windows(5).any(|w| w == b"\x1b]52;") && !s.raw.windows(4).any(|w| w == b"\x9d52;"),
        "a successful native copy must not also emit OSC 52"
    );
    eprintln!("observed selected text pasted intact and exactly two native copy calls");
}

#[test]
fn drag_copies_multiline_unicode_without_padding_or_duplicate_wide_cells() {
    let mut s = Session::start_with("old", "/bin/sh selection-helper.sh", "ROW_TWO");
    let (x, y) = s.find_text("ROW_ONE");
    let (end_x, end_y) = s.find_text("ROW_TWO");
    s.mouse(0, x, y, false);
    s.mouse(32, end_x + 9, end_y, false);
    s.mouse(0, end_x + 9, end_y, true);
    let expected = "ROW_ONE 日本語 é\nROW_TWO 끝";
    s.wait_for(
        "multiline Unicode copied exactly once without grid padding",
        |s| s.file("clipboard") == expected.as_bytes() && s.shown().contains("copied 2 lines"),
    );
    s.send(OPT_V);
    s.wait_for(
        "Unicode clipboard round trips through the real child PTY",
        |s| s.file("received-paste") == expected.as_bytes(),
    );
    assert_eq!(s.file("clipboard.calls"), b"x");
}

#[test]
fn drag_outside_the_pane_copies_to_its_edge_not_the_neighbor() {
    let mut s = Session::start("old");
    let (x, y) = s.find_text("PLAIN_READY");
    s.mouse(0, x + 2, y, false);
    s.mouse(32, HOST.cols - 1, y, false);
    s.mouse(0, HOST.cols - 1, y, true);
    s.wait_for("off-pane release clamps to the owning pane", |s| {
        s.file("clipboard") == b"AIN_READY" && s.shown().contains("copied 1 line")
    });
    assert_eq!(s.file("clipboard.calls"), b"x");
}

#[test]
fn empty_drag_does_not_replace_the_clipboard() {
    let mut s = Session::start("keep-this");
    let (x, y) = s.find_text("PLAIN_READY");
    s.mouse(0, x, y + 3, false);
    s.mouse(32, x + 5, y + 3, false);
    s.mouse(0, x + 5, y + 3, true);
    s.wait_for("blank selection reports nothing to copy", |s| {
        s.shown().contains("nothing to copy")
    });
    s.send(OPT_V);
    s.wait_for("blank selection leaves the old clipboard usable", |s| {
        s.file("received-paste") == b"keep-this"
    });
    assert!(!s.dir.join("clipboard.calls").exists());
    assert!(!s.raw.windows(5).any(|w| w == b"\x1b]52;"));
}

#[test]
fn child_owns_plain_drag_but_shift_drag_copies_even_if_shift_is_released_first() {
    let mut s = Session::start_with("old", "/bin/sh mouse-helper.sh", "MOUSE_READY");
    let (x, y) = s.find_text("MOUSE_READY");
    s.mouse(0, x, y, false);
    s.mouse(32, x + 4, y, false);
    s.mouse(0, x + 4, y, true);
    let forwarded = b"\x1b[<0;1;1M\x1b[<32;5;1M\x1b[<0;5;1m";
    s.wait_for(
        "mouse-reporting child receives all unmodified drag events",
        |s| s.file("received-paste") == forwarded,
    );
    assert_eq!(s.file("clipboard"), b"old");
    assert!(!s.dir.join("clipboard.calls").exists());

    s.mouse(4, x, y, false); // Shift+press gives this drag to gwae.
    s.mouse(36, x + 4, y, false);
    s.mouse(0, x + 6, y, true); // Shift released before mouse-up.
    s.wait_for("gwae completes and copies its captured Shift drag", |s| {
        s.file("clipboard") == b"MOUSE_R" && s.shown().contains("copied 1 line")
    });
    s.send(b"!");
    let mut expected = forwarded.to_vec();
    expected.push(b'!');
    s.wait_for("captured Shift drag never leaks a tail to the child", |s| {
        s.file("received-paste") == expected
    });
    assert_eq!(s.file("clipboard.calls"), b"x");
}

#[test]
fn failed_or_stalled_native_copy_falls_back_without_claiming_confirmed_success() {
    for script in ["#!/bin/sh\nexit 1\n", "#!/bin/sh\nexec /bin/sleep 30\n"] {
        let mut s = Session::start("old");
        std::fs::write(s.dir.join("bin/pbcopy"), script).expect("replace isolated helper");
        let (x, y) = s.find_text("PLAIN_READY");
        let started = Instant::now();
        s.mouse(0, x, y, false);
        s.mouse(32, x + 4, y, false);
        s.mouse(0, x + 4, y, true);
        let sequence = b"\x1b]52;c;UExBSU4=\x07"; // PLAIN
        s.wait_for(
            "failed helper emits exact OSC 52 and unconfirmed feedback",
            |s| {
                s.raw.windows(sequence.len()).any(|w| w == sequence)
                    && s.shown().contains("copy sent to terminal")
            },
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "stalled helper blocked the UI"
        );
        assert!(!s.shown().contains("copied 1 line"));
        assert_eq!(s.file("clipboard"), b"old");
        s.send(b"still-responsive");
        s.wait_for("pane input resumes after native-copy failure", |s| {
            s.file("received-paste") == b"still-responsive"
        });
    }
}

#[test]
fn ssh_copy_targets_the_host_terminal_without_touching_the_remote_clipboard() {
    for key in ["SSH_CONNECTION", "SSH_TTY"] {
        let mut s = Session::start_with_env(
            "remote-clipboard",
            "/bin/sh plain-helper.sh",
            "PLAIN_READY",
            &[(key, "fixture-remote-session")],
        );
        let (x, y) = s.find_text("PLAIN_READY");
        s.mouse(0, x, y, false);
        s.mouse(32, x + 4, y, false);
        s.mouse(0, x + 4, y, true);
        let sequence = b"\x1b]52;c;UExBSU4=\x07";
        s.wait_for(
            "SSH selection requests the host clipboard instead of local helpers",
            |s| {
                s.raw.windows(sequence.len()).any(|w| w == sequence)
                    && s.shown().contains("copy sent to terminal")
            },
        );
        assert_eq!(s.file("clipboard"), b"remote-clipboard");
        assert!(
            !s.dir.join("clipboard.calls").exists(),
            "{key} must bypass pbcopy"
        );
        assert!(!s.shown().contains("copied 1 line"));
        eprintln!(
            "observed {key}: exact host OSC 52 request, no remote helper or clipboard change"
        );
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn native_paste_mode_is_enabled_and_restored_on_exit() {
    let mut s = Session::start("");
    assert!(s.screen.wants_bracketed_paste(), "host must frame Cmd+V");
    s.send(OPT_Q);
    s.wait_for("quit confirmation", |s| {
        s.shown().contains("force quit gwae?")
    });
    s.send(b"\r");
    s.wait_for("bracketed paste disabled on exit", |s| {
        s.raw.windows(8).any(|w| w == b"\x1b[?2004l")
    });
    assert!(!s.screen.wants_bracketed_paste());
}

#[test]
fn native_paste_preserves_blank_lines_and_unicode_in_plain_panes() {
    // Delimiters must not leak into a program that did not enable them.
    let mut s = Session::start("not the host clipboard");
    s.native_paste("one\n\n日本語 é\r\n끝\n\n");
    s.send(b"PASTE_DONE");
    s.wait_for("complete native paste", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    assert_eq!(
        s.file("received-paste"),
        "one\n\n日本語 é\n끝\n\nPASTE_DONE".as_bytes()
    );
}

#[test]
fn native_paste_reframes_a_large_block_for_a_bracket_aware_child() {
    let mut s = Session::start_with(
        "wrong clipboard",
        "/bin/sh bracket-helper.sh",
        "BRACKET_READY",
    );
    // Larger than a PTY write chunk, with blank lines and literal Option-key
    // glyphs. Pasted √ must be text, never dispatched as gwae's paste shortcut.
    let text = format!("{}\n\n日本語 é √\r\n끝\n\n", "long row\n".repeat(600));
    s.native_paste(&text);
    s.send(b"PASTE_DONE");
    s.wait_for("complete bracketed native paste", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    let expected = format!(
        "\x1b[200~{}\x1b[201~PASTE_DONE",
        text.replace("\r\n", "\r").replace('\n', "\r")
    );
    assert_eq!(s.file("received-paste"), expected.as_bytes());
}

#[test]
fn native_paste_lf_input_preserves_consecutive_blank_lines() {
    let mut s = Session::start_with("", "/bin/sh bracket-helper.sh", "BRACKET_READY");
    // Some hosts retain LF rather than converting it to CR. Exercise both.
    s.send(b"\x1b[200~one\n\ntwo\r\n\n\x1b[201~PASTE_DONE");
    s.wait_for("LF native paste", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    assert_eq!(
        s.file("received-paste"),
        b"\x1b[200~one\r\rtwo\r\r\x1b[201~PASTE_DONE"
    );
}

#[test]
fn native_paste_tracks_child_mode_without_disabling_host_framing() {
    let mut s = Session::start_with("", "/bin/sh mode-helper.sh", "MODE_READY");
    s.native_paste("first");
    s.wait_for("child disabled its paste mode", |s| {
        s.shown().contains("MODE_OFF")
    });
    assert_eq!(s.file("first-paste"), b"\x1b[200~first\x1b[201~");
    assert!(
        s.screen.wants_bracketed_paste(),
        "host framing stays enabled"
    );
    s.native_paste("second\n\nlast");
    s.send(b"PASTE_DONE");
    s.wait_for("unbracketed delivery after mode change", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    assert_eq!(s.file("received-paste"), b"second\r\rlastPASTE_DONE");
}

#[test]
fn native_paste_cannot_confirm_quit_or_leak_to_the_pane() {
    let mut s = Session::start("");
    s.send(OPT_Q);
    s.wait_for("quit confirmation", |s| {
        s.shown().contains("force quit gwae?")
    });
    s.native_paste("\nshould-not-reach-child\n");
    s.send(b"PASTE_DONE");
    s.wait_for("paste cancels quit and normal input still works", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    assert_eq!(s.file("received-paste"), b"PASTE_DONE");
}

#[test]
fn native_paste_in_directory_picker_only_updates_the_filter() {
    let mut s = Session::start("");
    s.send(b"\x1bd");
    s.wait_for("directory picker", |s| s.shown().contains("spawn dir"));
    s.native_paste("zznativepath\nnot-a-second-path\n");
    s.wait_for("pasted directory filter", |s| {
        s.shown().contains("zznativepath")
    });
    assert!(!s.shown().contains("not-a-second-path"));
    s.send(b"\x1b");
    s.wait_for("picker closed", |s| !s.shown().contains("zznativepath"));
    s.send(b"PASTE_DONE");
    s.wait_for("normal input after closing picker", |s| {
        s.file("received-paste").ends_with(b"PASTE_DONE")
    });
    assert_eq!(s.file("received-paste"), b"PASTE_DONE");
}

#[test]
fn option_v_pastes_the_clipboard_into_a_plain_pane() {
    let mut s = Session::start("hello-paste");
    s.send(OPT_V);
    s.wait_for(
        "plain clipboard bytes, rendered text, and confirmation",
        |s| {
            let shown = s.shown();
            s.file("received-paste") == b"hello-paste"
                && shown.contains("hello-paste")
                && shown.contains("pasted 1 line")
        },
    );
}

#[test]
fn option_v_multiline_paste_arrives_as_one_block() {
    // Actual newline, not the old literal backslash-n fixture. tee's exact
    // captured bytes also reject unwanted bracket markers or dropped lines.
    let mut s = Session::start("one\ntwo");
    s.send(OPT_V);
    s.wait_for("both clipboard lines and multiline confirmation", |s| {
        let shown = s.shown();
        s.file("received-paste") == b"one\ntwo"
            && shown.contains("one")
            && shown.contains("two")
            && shown.contains("pasted 2 lines")
    });
}

#[test]
fn option_v_in_a_real_fish_pane_pastes_instead_of_opening_an_editor() {
    fish_paste_waits_for_enter(false);
}

#[test]
fn native_paste_in_a_real_fish_pane_waits_for_enter() {
    fish_paste_waits_for_enter(true);
}

#[test]
fn native_paste_in_a_real_zsh_pane_waits_for_enter() {
    let mut s = Session::start_with("not used", "/bin/zsh -d -i", "ZSH_READY> ");
    s.native_paste("paste_probe\n\npaste_probe\n");
    s.send(FISH_BARRIER);
    s.wait_for("zsh has processed the entire paste", |s| {
        s.file("paste-barrier") == b"x"
    });
    assert_eq!(
        s.file("paste-executions"),
        b"",
        "native paste must not execute before Enter"
    );
    s.send(b"\r");
    s.send(FISH_BARRIER);
    s.wait_for("zsh has processed Enter", |s| {
        s.file("paste-barrier") == b"xx"
    });
    assert_eq!(s.file("paste-executions"), b"EXECUTED\nEXECUTED\n");
}

fn fish_paste_waits_for_enter(native: bool) {
    let fish = [
        "/opt/homebrew/bin/fish",
        "/usr/local/bin/fish",
        "/usr/bin/fish",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file());
    let Some(fish) = fish else {
        eprintln!("skipping: no fish binary found");
        return;
    };
    let pane_cmd = format!(
        "{fish} --no-config --private --interactive --init-command 'source fish-init.fish'"
    );
    // A trailing newline would execute this if pasted without brackets. The
    // sentinel proves actual execution, independently of shell/TUI repaints.
    let command = "paste_probe\n\npaste_probe";
    let mut s = Session::start_with(&format!("{command}\n"), &pane_cmd, "FISH_READY> ");
    if native {
        s.native_paste(&format!("{command}\n"));
    } else {
        s.send(OPT_V);
    }
    s.send(FISH_BARRIER);
    s.wait_for("fish processed the complete paste without executing", |s| {
        s.file("paste-barrier") == b"x"
    });
    assert!(
        !s.dir.join("paste-executions").exists(),
        "paste must not execute before Enter: {:?}\n{}",
        s.file("paste-executions"),
        s.shown()
    );
    assert!(!s.shown().contains("External editor"), "{}", s.shown());

    s.send(b"\r");
    s.send(FISH_BARRIER);
    s.wait_for(
        "Enter executes once and fish returns to reading input",
        |s| s.file("paste-barrier") == b"xx",
    );
    assert_eq!(
        s.file("paste-executions"),
        b"EXECUTED\nEXECUTED\n",
        "Enter must execute each pasted command exactly once\n{}",
        s.shown()
    );
}
