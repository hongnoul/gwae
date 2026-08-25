//! End-to-end: reading back through a pane's scrollback with `⌥+↑`/`⌥+↓`.
//!
//! strimux used to capture the mouse wheel for this. That is gone: the wheel
//! now only ever reaches a child that asked for mouse reporting, so the
//! keyboard is the *only* route into a pane's history. This suite exists
//! because that makes it load-bearing - a regression here would leave
//! scrollback with nothing able to reach it, which is exactly the kind of
//! removal that goes unnoticed until a user tries to scroll.
//!
//! It drives the real binary through a PTY and reconstructs the screen from
//! the frames it paints, because strimux repaints *incrementally*: the bytes
//! for one frame carry only the cells that changed, so asserting on the raw
//! stream would be asserting on a diff rather than on what a user sees.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const ROWS: u16 = 30;
const COLS: u16 = 100;

/// Alt+Up / Alt+Down as a terminal with a modifier-aware CSI sends them.
const ALT_UP: &[u8] = b"\x1b[1;3A";
const ALT_DOWN: &[u8] = b"\x1b[1;3B";

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    /// The reconstructed screen: `ROWS` rows of `COLS` chars.
    grid: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
}

impl Session {
    fn start() -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "strimux-scrollback-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("strimux")).expect("temp config dir");
        // One full-width column and no minimap, so the pane owns the screen
        // and the line numbers below are not competing with chrome.
        std::fs::write(
            dir.join("strimux/strimux.toml"),
            "onboarded = true\ndefault_column_width = \"full\"\n[minimap]\nshow = false\n",
        )
        .expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        // Setup must not run, and must never install anything.
        cmd.env("STRIMUX_NO_INSTALL", "1");
        cmd.arg("run");
        cmd.arg("sh");
        let child = pair.slave.spawn_command(cmd).expect("spawn strimux");
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
            grid: vec![vec![' '; COLS as usize]; ROWS as usize],
            cx: 0,
            cy: 0,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    /// Read for `secs`, folding everything that arrives into the screen.
    fn settle(&mut self, secs: f32) {
        let deadline = Instant::now() + Duration::from_secs_f32(secs);
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(b) => {
                    let s = String::from_utf8_lossy(&b).into_owned();
                    self.apply(&s);
                }
                Err(_) => continue,
            }
        }
    }

    /// Fold one chunk of output into the screen.
    ///
    /// Only the subset of ANSI strimux actually paints with: absolute cursor
    /// positioning, erase-in-display, newlines, and printable text. Anything
    /// else (SGR colors, synchronized-update markers, OSC titles) is skipped,
    /// since this suite asserts on *glyphs*, not on styling.
    fn apply(&mut self, s: &str) {
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\x1b' => match it.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut final_byte = ' ';
                        for c in it.by_ref() {
                            if c.is_ascii_alphabetic() {
                                final_byte = c;
                                break;
                            }
                            params.push(c);
                        }
                        self.csi(&params, final_byte);
                    }
                    Some(']') => {
                        // OSC: runs to BEL or ST.
                        while let Some(c) = it.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' && it.peek() == Some(&'\\') {
                                it.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => self.cx = 0,
                '\n' => {
                    self.cy = (self.cy + 1).min(ROWS as usize - 1);
                    self.cx = 0;
                }
                c if (c as u32) >= 0x20 => {
                    if self.cy < ROWS as usize && self.cx < COLS as usize {
                        self.grid[self.cy][self.cx] = c;
                    }
                    self.cx += 1;
                }
                _ => {}
            }
        }
    }

    fn csi(&mut self, params: &str, final_byte: char) {
        let nums: Vec<usize> = params
            .trim_start_matches('?')
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        match final_byte {
            'H' => {
                self.cy = nums.first().copied().unwrap_or(1).max(1) - 1;
                self.cx = nums.get(1).copied().unwrap_or(1).max(1) - 1;
                self.cy = self.cy.min(ROWS as usize - 1);
                self.cx = self.cx.min(COLS as usize - 1);
            }
            'J' => {
                // Erase in display; strimux uses this on entering the alt
                // screen, so treat every form as "clear it all".
                self.grid = vec![vec![' '; COLS as usize]; ROWS as usize];
                self.cx = 0;
                self.cy = 0;
            }
            'K' => {
                for x in self.cx..COLS as usize {
                    self.grid[self.cy][x] = ' ';
                }
            }
            _ => {}
        }
    }

    /// Every `LINE-<n>` currently on screen.
    fn visible_lines(&self) -> Vec<u32> {
        let mut v = Vec::new();
        for row in &self.grid {
            let text: String = row.iter().collect();
            let mut rest = text.as_str();
            while let Some(at) = rest.find("LINE-") {
                rest = &rest[at + 5..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<u32>() {
                    v.push(n);
                }
            }
        }
        v.sort_unstable();
        v
    }

    /// The `(first, last)` line number on screen, for range assertions.
    fn span(&self) -> (u32, u32) {
        let v = self.visible_lines();
        assert!(
            !v.is_empty(),
            "no LINE-n on screen; got:\n{}",
            self.render()
        );
        (v[0], v[v.len() - 1])
    }

    /// The screen as text, for failure messages.
    fn render(&self) -> String {
        self.grid
            .iter()
            .map(|r| r.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Fill the pane's scrollback with numbered lines.
    fn emit_history(&mut self) {
        self.settle(3.0);
        // Dismiss the startup cheat-sheet HUD, which covers the middle of the
        // screen until a key is pressed.
        self.send(b"\r");
        self.settle(0.8);
        self.send(b"for i in $(seq 1 60); do echo LINE-$i; done\n");
        self.settle(2.5);
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn alt_up_reads_back_through_history_and_alt_down_returns_to_live() {
    // The acceptance behavior for removing the wheel: everything it used to do
    // must still be reachable, from the keyboard, in the shipped binary.
    let mut s = Session::start();
    s.emit_history();

    let live = s.span();
    assert_eq!(
        live.1,
        60,
        "should start at the live bottom; got:\n{}",
        s.render()
    );

    // Eight notches of three rows each: far enough that the top of the screen
    // is well above where it was, and the bottom no longer shows the last line.
    for _ in 0..8 {
        s.send(ALT_UP);
        std::thread::sleep(Duration::from_millis(60));
    }
    s.settle(2.0);
    let back = s.span();
    assert!(
        back.0 < live.0 && back.1 < live.1,
        "Alt+Up did not scroll back: was {live:?}, now {back:?}\n{}",
        s.render()
    );

    // And back down to exactly where we started: scrollback that cannot
    // return to live is a trap, not a feature.
    for _ in 0..8 {
        s.send(ALT_DOWN);
        std::thread::sleep(Duration::from_millis(60));
    }
    s.settle(2.0);
    assert_eq!(
        s.span(),
        live,
        "Alt+Down did not return to the live bottom\n{}",
        s.render()
    );
    s.kill();
}

#[test]
fn typing_snaps_a_scrolled_back_pane_to_the_live_bottom() {
    // Scrolling up then typing must show you the prompt you are typing at,
    // not leave you looking at history while your keystrokes land off screen.
    let mut s = Session::start();
    s.emit_history();
    let live = s.span();

    for _ in 0..8 {
        s.send(ALT_UP);
        std::thread::sleep(Duration::from_millis(60));
    }
    s.settle(2.0);
    assert!(s.span().1 < live.1, "did not scroll back\n{}", s.render());

    s.send(b"echo BACK-AT-PROMPT");
    s.settle(2.0);
    // Compared with the newlines, frame glyphs and padding stripped: the
    // echoed text wraps at the pane edge, so it is one string on screen but
    // several rows in the grid, each row separated by the column frames.
    let flat: String = s
        .render()
        .chars()
        .filter(|c| !c.is_whitespace() && !"│─╭╮╰╯├┤┬┴┼".contains(*c))
        .collect();
    assert!(
        flat.contains("BACK-AT-PROMPT"),
        "typing did not snap the pane back to live\n{}",
        s.render()
    );
    // ...and the live bottom is what is on screen again.
    assert_eq!(
        s.span().1,
        live.1,
        "snapped somewhere other than the live bottom\n{}",
        s.render()
    );
    s.kill();
}

#[test]
fn the_wheel_no_longer_scrolls_a_pane_that_did_not_ask_for_it() {
    // The removal itself, asserted rather than assumed: a plain shell does not
    // request mouse reporting, so wheel events must change nothing at all.
    let mut s = Session::start();
    s.emit_history();
    let before = s.span();

    // SGR (1006) wheel-up reports over the middle of the pane.
    for _ in 0..10 {
        s.send(b"\x1b[<64;40;10M");
        std::thread::sleep(Duration::from_millis(40));
    }
    s.settle(1.5);
    assert_eq!(
        s.span(),
        before,
        "the wheel still moved the pane's scrollback\n{}",
        s.render()
    );
    s.kill();
}
