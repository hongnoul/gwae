//! End-to-end: the live mockup, on a real terminal, driven by real keystrokes.
//!
//! The unit tests in `preview` and `onboard` prove the *rendering* is correct,
//! but they call internal functions with hand-built state. They cannot answer
//! the question that actually matters to a user: when I press `↓` at a real
//! `strimux init`, does the picture on my screen change to show me the thing I
//! just highlighted, and can I still see the question I am answering?
//!
//! So this drives the **shipped executable** through a PTY, types the keys a
//! person would type, and asserts on the bytes that land on the terminal:
//!
//! 1. The preview is there at all, at the default terminal size.
//! 2. Moving the highlight repaints it differently (it previews, rather than
//!    being a picture of the default).
//! 3. Earlier answers persist into later questions' previews.
//! 4. The question is never scrolled off the screen by its own illustration,
//!    at any size, which is the one way this feature could make setup *worse*.
//! 5. The config written at the end matches what the previews showed.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

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

/// A scratch `XDG_CONFIG_HOME`.
struct Sandbox {
    dir: std::path::PathBuf,
}

impl Sandbox {
    fn new(body: &str) -> Sandbox {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "strimux-preview-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        std::fs::write(dir.join("strimux/strimux.toml"), body).expect("write config");
        Sandbox { dir }
    }

    fn config(&self) -> String {
        std::fs::read_to_string(self.dir.join("strimux/strimux.toml")).unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A running `strimux init` on a PTY, that keys can be typed into and whose
/// screen can be read one repaint at a time.
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    buf: String,
}

impl Session {
    fn start(sb: &Sandbox, cols: u16, rows: u16) -> Session {
        let pair = native_pty_system()
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
        // Never touch the machine running the tests.
        cmd.env("STRIMUX_NO_INSTALL", "1");
        cmd.arg("init");
        let child = pair.slave.spawn_command(cmd).expect("spawn strimux");
        drop(pair.slave);
        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let (tx, rx) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut b = [0u8; 8192];
            loop {
                match reader.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(b[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Session {
            child,
            master: pair.master,
            writer,
            rx,
            buf: String::new(),
        }
    }

    /// Read until `pred` holds on the accumulated *stripped* text, or give up.
    fn wait_for(&mut self, what: &str, pred: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if pred(&strip_ansi(&self.buf)) {
                return;
            }
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(b) => self.buf.push_str(&String::from_utf8_lossy(&b)),
                Err(_) => continue,
            }
        }
        panic!(
            "timed out waiting for {what}; screen was:\n{}",
            strip_ansi(&self.buf)
        );
    }

    /// Wait for question `n` of the flow to be on screen.
    fn wait_for_question(&mut self, n: usize) {
        let tag = format!("[{n}/");
        self.wait_for(&format!("question {n}"), |s| s.contains(&tag));
    }

    /// Everything painted since the last screen-clear: one full repaint, which
    /// is what a user is actually looking at.
    fn screen(&self) -> String {
        match self.buf.rfind("\x1b[2J") {
            Some(i) => self.buf[i..].to_string(),
            None => self.buf.clone(),
        }
    }

    /// The current screen with escapes removed.
    fn plain(&self) -> String {
        strip_ansi(&self.screen())
    }

    /// Type keys, then wait for the repaint they cause.
    fn press(&mut self, keys: &str) {
        let before = self.buf.len();
        self.writer.write_all(keys.as_bytes()).expect("write keys");
        self.writer.flush().expect("flush");
        // A repaint always begins with a clear, so wait for a new one rather
        // than for a fixed sleep: no timing assumptions, no flake.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.buf[before..].contains("\x1b[2J") {
                // Let the rest of the frame arrive.
                while let Ok(b) = self.rx.recv_timeout(Duration::from_millis(120)) {
                    self.buf.push_str(&String::from_utf8_lossy(&b));
                }
                return;
            }
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(b) => self.buf.push_str(&String::from_utf8_lossy(&b)),
                Err(_) => continue,
            }
        }
        panic!("no repaint after pressing {keys:?}");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The mockup's own frame corners, which nothing else on the screen draws.
const TOP_LEFT: char = '\u{256d}';
const BOT_RIGHT: char = '\u{256f}';

/// The rows of the mockup, as painted: from its top frame to its bottom.
fn mockup(plain: &str) -> Vec<String> {
    let lines: Vec<&str> = plain.lines().collect();
    let start = lines.iter().position(|l| l.contains(TOP_LEFT));
    let end = lines.iter().position(|l| l.contains(BOT_RIGHT));
    match (start, end) {
        (Some(a), Some(b)) if b >= a => lines[a..=b].iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Skip the title card and land on question 1.
fn to_first_question(s: &mut Session) {
    // Any key dismisses the card; the flow then draws question 1.
    s.press("\x1b[B");
    s.wait_for_question(1);
}

#[test]
fn the_first_question_is_drawn_with_a_live_mockup() {
    let sb = Sandbox::new("");
    let mut s = Session::start(&sb, 100, 40);
    to_first_question(&mut s);
    let m = mockup(&s.plain());
    assert!(
        !m.is_empty(),
        "no mockup on screen for question 1:\n{}",
        s.plain()
    );
    // It is a grid of panes, not a decorative box.
    assert!(
        m.iter().any(|l| l.contains('\u{250c}')),
        "the mockup has no pane boxes in it:\n{}",
        m.join("\n")
    );
    // And it is in color: the whole point of previewing a *theme*.
    assert!(
        s.screen().contains("\x1b[48;2;"),
        "the mockup painted no truecolor background"
    );
}

#[test]
fn moving_the_highlight_repaints_the_mockup_in_the_new_theme() {
    let sb = Sandbox::new("");
    let mut s = Session::start(&sb, 100, 40);
    to_first_question(&mut s);

    // The theme question: the mockup's *colors* must change, while its shape
    // (which theme does not affect) stays put.
    let before = s.screen();
    let before_shape = mockup(&s.plain());
    s.press("\x1b[B"); // down: catppuccin-latte
    let after = s.screen();
    let after_shape = mockup(&s.plain());

    assert_ne!(
        before, after,
        "highlighting a different theme repainted identical bytes"
    );
    assert_eq!(
        before_shape, after_shape,
        "a theme change altered the layout, which it does not do"
    );
    assert!(!before_shape.is_empty(), "no mockup to compare");
}

#[test]
fn a_width_choice_changes_the_shape_of_the_previewed_grid() {
    let sb = Sandbox::new("");
    let mut s = Session::start(&sb, 100, 40);
    to_first_question(&mut s);
    s.press("\r"); // accept theme -> panes
    s.wait_for_question(2);
    s.press("\r"); // accept panes -> width
    s.wait_for_question(3);

    let quarter = mockup(&s.plain());
    // Option 3 is `half`: two columns instead of four.
    s.press("\x1b[B\x1b[B");
    let half = mockup(&s.plain());

    assert!(!quarter.is_empty() && !half.is_empty(), "no mockup drawn");
    assert_ne!(
        quarter, half,
        "highlighting `half` drew the same grid as `quarter`"
    );
    // Concretely: fewer, wider boxes. Count the box corners on a pane row.
    let corners = |m: &[String]| {
        m.iter()
            .map(|l| l.matches('\u{250c}').count())
            .max()
            .unwrap_or(0)
    };
    assert!(
        corners(&half) < corners(&quarter),
        "`half` should show fewer columns than `quarter`: {} vs {}",
        corners(&half),
        corners(&quarter)
    );
}

#[test]
fn an_earlier_theme_answer_is_still_visible_in_a_later_questions_mockup() {
    // Pick a theme on question 1, then check a later question's preview is
    // wearing it. This is the property that makes each answer judged in the
    // setup it will actually live in.
    let mut screens = Vec::new();
    for theme_key in ["1", "4"] {
        // catppuccin-mocha, gruvbox
        let sb = Sandbox::new("");
        let mut s = Session::start(&sb, 100, 40);
        to_first_question(&mut s);
        s.press(theme_key); // a digit answers and advances
        s.wait_for_question(2);
        s.press("\r");
        s.wait_for_question(3);
        s.press("\r");
        s.wait_for_question(4);
        screens.push(s.screen());
    }
    assert_ne!(
        screens[0], screens[1],
        "question 4 looked identical under two different themes, so the \
         earlier answer never reached the preview"
    );
}

#[test]
fn the_question_is_never_pushed_off_screen_by_its_own_mockup() {
    // The one way this feature could make setup worse. Checked at the classic
    // default size and at a deliberately cramped one.
    for (cols, rows) in [(100u16, 40u16), (80, 24), (80, 20), (70, 16)] {
        let sb = Sandbox::new("");
        let mut s = Session::start(&sb, cols, rows);
        to_first_question(&mut s);
        let plain = s.plain();
        // The question, its options and the key hints must all be present.
        assert!(
            plain.contains("[1/"),
            "{cols}x{rows}: the question header is missing:\n{plain}"
        );
        assert!(
            plain.contains("catppuccin-mocha") && plain.contains("terminal"),
            "{cols}x{rows}: options were pushed off screen:\n{plain}"
        );
        assert!(
            plain.contains("pick"),
            "{cols}x{rows}: the key hints were pushed off screen:\n{plain}"
        );
        // Whatever is drawn must fit the terminal it was drawn for.
        let painted = plain.lines().filter(|l| !l.trim().is_empty()).count();
        assert!(
            painted <= rows as usize,
            "{cols}x{rows}: painted {painted} non-blank lines into {rows} rows:\n{plain}"
        );
    }
}

#[test]
fn a_small_terminal_shrinks_the_mockup_rather_than_breaking_the_flow() {
    let big = {
        let sb = Sandbox::new("");
        let mut s = Session::start(&sb, 100, 40);
        to_first_question(&mut s);
        mockup(&s.plain()).len()
    };
    let small = {
        let sb = Sandbox::new("");
        let mut s = Session::start(&sb, 100, 22);
        to_first_question(&mut s);
        mockup(&s.plain()).len()
    };
    assert!(big > 0, "no mockup on a large terminal");
    assert!(
        small < big,
        "a 22-row terminal drew the same {big}-row mockup as a 40-row one"
    );
}

#[test]
fn what_the_preview_showed_is_what_gets_written_to_the_config() {
    // The preview is only trustworthy if it agrees with the file. Answer with
    // digits (unambiguous), then read back the config the flow wrote.
    let sb = Sandbox::new("");
    let mut s = Session::start(&sb, 100, 40);
    to_first_question(&mut s);
    s.press("4"); // theme: gruvbox
    s.wait_for_question(2);
    s.press("2"); // startup_panes: 2
    s.wait_for_question(3);
    s.press("3"); // default_column_width: half

    // The preview at question 3 must already show two live panes in a
    // half-width grid, which is exactly what we just answered.
    let m = mockup(&s.plain());
    assert!(
        m.iter().any(|l| l.contains("agent")) && m.iter().any(|l| l.contains("shell")),
        "two panes were answered but the mockup shows only one:\n{}",
        m.join("\n")
    );

    // Take defaults for the rest and finish.
    s.press("\x1b"); // esc: defaults for the remainder -> summary
    s.wait_for("the summary", |t| t.contains("strimux is configured"));
    s.press("\r");
    s.wait_for("the process to write its config", |_| {
        !sb.config().trim().is_empty()
    });

    let cfg = sb.config();
    assert!(cfg.contains("gruvbox"), "theme not written:\n{cfg}");
    assert!(
        cfg.contains("startup_panes = 2"),
        "panes not written:\n{cfg}"
    );
    assert!(
        cfg.contains("default_column_width = \"half\""),
        "width not written:\n{cfg}"
    );
    // And the real parser must accept the whole thing: a preview that led to
    // an unloadable config would be worse than no preview.
    toml::from_str::<toml::Value>(&cfg).expect("the config the flow wrote does not parse");
}
