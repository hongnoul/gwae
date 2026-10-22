//! End-to-end: the host terminal's real cursor is the anchor every OS input
//! method (Hangul, Japanese, Chinese, dead-key accents) uses for its preedit
//! overlay and candidate window — and screen readers speak the cell under it.
//! gwae must park it on the focused pane's cursor cell, the same cursor
//! contract tmux honors.
//!
//! The real executable runs under a host PTY; the host byte stream is fed to a
//! VT emulator whose cursor position after each frame is exactly what the OS
//! IME uses to place the composing text and the candidate window. Before this
//! fix the host cursor was wherever the last diff run finished (typically the
//! right edge), so composition painted over the frame. Set `GWAE_E2E_BIN` to
//! replay against a pre-fix executable: both assertions fail there.

// Unix-only end to end: drives a real PTY running sh syntax.
#![cfg(unix)]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const HOST: Size = Size {
    rows: 24,
    cols: 120,
};
const TIMEOUT: Duration = Duration::from_secs(15);

// Echo stays ON and the read is noncanonical: every typed byte is echoed by
// the inner tty immediately, so the pane's cursor advances per keystroke the
// same way a shell prompt does under composition-committed input.
const HELPER: &str = "#!/bin/sh\n\
    stty -icanon min 1 time 0\n\
    printf 'READY> '\n\
    exec /bin/cat > /dev/null\n";

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    screen: Vt100Grid,
    dir: PathBuf,
}

impl Session {
    fn start() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-ime-cursor-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("create isolated config");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "default_column_width = \"quarter\"\nstartup_panes = 1\n\
             keep_awake = false\n\
             [minimap]\nshow = false\n\
             [update]\ncheck = false\n",
        )
        .expect("write config");
        std::fs::write(dir.join("ime-helper.sh"), HELPER).expect("write shell helper");
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: HOST.rows,
                cols: HOST.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open host PTY");
        let executable =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        eprintln!("ime cursor acceptance executable: {executable:?}");
        let mut cmd = CommandBuilder::new(executable);
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env("ENV", "/dev/null");
        cmd.env("BASH_ENV", "/dev/null");
        cmd.env("LC_ALL", "C");
        cmd.env("GWAE_LOG", "off");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        cmd.env("GWAE_KITTY_GRAPHICS", "0");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.arg("run");
        cmd.arg("/bin/sh ime-helper.sh");
        let child = pair
            .slave
            .spawn_command(cmd)
            .expect("spawn actual gwae binary");
        drop(pair.slave);
        let writer = pair.master.take_writer().expect("host writer");
        let mut reader = pair.master.try_clone_reader().expect("host reader");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut buf = [0; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            _master: pair.master,
            writer,
            rx,
            screen: Vt100Grid::new(HOST),
            dir,
        }
    }

    fn send(&mut self, keys: &[u8]) {
        self.writer.write_all(keys).expect("write host keys");
        self.writer.flush().expect("flush host keys");
    }

    fn pump(&mut self) {
        if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(20)) {
            self.screen.feed(&bytes);
        }
    }

    /// Pump host output until `ready` holds, then settle: keep pumping until
    /// the stream stays quiet for 150ms so the *final* parked cursor position
    /// of the last frame is measured, not a mid-paint position.
    fn wait_settled(&mut self, label: &str, ready: impl Fn(&Vt100Grid) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if ready(&self.screen) {
                let mut quiet = Instant::now();
                while quiet.elapsed() < Duration::from_millis(150) {
                    if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(20)) {
                        self.screen.feed(&bytes);
                        quiet = Instant::now();
                    }
                }
                if ready(&self.screen) {
                    return;
                }
                continue;
            }
            self.pump();
        }
        panic!(
            "{label}: condition never held\nhost screen:\n{}",
            self.screen.visible_text()
        );
    }

    /// Screen (x, y) of the cell right after `text` on the host screen,
    /// in *columns* (wide chars occupy two, their continuation cell is
    /// width 0), never byte offsets.
    fn after_text(&self, text: &str) -> (u16, u16) {
        let want: Vec<char> = text.chars().collect();
        let size = self.screen.size();
        for y in 0..size.rows {
            'start: for x0 in 0..size.cols {
                let mut x = x0;
                for &wc in &want {
                    let c = self.screen.cell(x, y);
                    if c.width == 0 || c.ch != wc {
                        continue 'start;
                    }
                    x += c.width as u16;
                }
                return (x, y);
            }
        }
        panic!(
            "text {text:?} not on host screen:\n{}",
            self.screen.visible_text()
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

/// The host terminal's cursor (the preedit anchor for every composing input
/// method) must sit exactly where the focused pane's cursor is: right after
/// the prompt at startup, and it must track each typed character as the pane
/// echoes it.
#[test]
fn host_cursor_parks_on_the_focused_panes_typing_position() {
    let mut s = Session::start();
    // Startup: prompt visible, host cursor right after "READY> ".
    s.wait_settled("prompt appears", |g| g.visible_text().contains("READY>"));
    let expect = s.after_text("READY> ");
    let got = {
        let (row, col) = s.screen.cursor_position();
        (col, row)
    };
    assert_eq!(
        got,
        expect,
        "host cursor must park at the typing position (after the prompt), \
         not wherever the last diff run finished\nhost screen:\n{}",
        s.screen.visible_text()
    );

    // Typing: each echoed character moves the pane cursor; the parked host
    // cursor must follow so composing text renders at the caret. The echoed
    // text includes multi-byte UTF-8 (a committed Hangul syllable, standing in
    // for any wide CJK commit): the anchor must advance by *columns*, not
    // bytes.
    s.send("ab한".as_bytes());
    s.wait_settled("echo appears", |g| g.visible_text().contains("READY> ab한"));
    let expect = s.after_text("READY> ab한");
    let got = {
        let (row, col) = s.screen.cursor_position();
        (col, row)
    };
    assert_eq!(
        got, expect,
        "host cursor must track typed characters (wide Hangul advances 2 columns)\nhost screen:\n{}",
        s.screen.visible_text()
    );
}
