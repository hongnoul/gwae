//! Real-executable regression coverage for pane resizing.
//!
//! A shell in gwae's inner PTY reads `stty size`, logs actual SIGWINCH
//! deliveries, and redraws a width ruler and bottom-row marker. The outer
//! portable_pty is the host terminal, not a mocked layout or resize callback.
//! The primary-screen case deliberately does NOT redraw on SIGWINCH, so it
//! separately exercises emulator reflow rather than a cooperative TUI.
//! Set `GWAE_E2E_BIN` to replay against a release or pre-fix executable.
#![cfg(unix)]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const HOST: Size = Size {
    rows: 30,
    cols: 120,
};
const ALT_R: &[u8] = b"\x1br";
const ALT_F: &[u8] = b"\x1bf";
const TIMEOUT: Duration = Duration::from_secs(10);

// /bin/sh and stty are sufficient on both macOS and Linux. A blocked builtin
// read runs the WINCH trap without polling or needing Python. A signal can
// interrupt read, so its failure must not terminate the loop. `p` explicitly
// probes the geometry, but resize assertions wait for WINCH WITHOUT probing.
const HELPER: &str = r#"#!/bin/sh
stty -echo
seq=0
report() {
    # WINCH can interrupt START inside stty. Keep recursive trap invocations
    # from overwriting the outer report's reason, dimensions and sequence.
    local event rows cols n stamp
    event=$1
    seq=$((seq + 1))
    stamp=$seq
    set -- $(stty size < /dev/tty)
    rows=$1
    cols=$2
    if [ -z "$GWAE_RESIZE_TEXT" ]; then
        printf '\033[2J\033[H%03d %s %sx%s' "$stamp" "$event" "$rows" "$cols"
        printf '\033[3;1H'
        n=2
        while [ "$n" -lt "$cols" ]; do printf '.'; n=$((n + 1)); done
        printf 'WX'
        printf '\033[%s;1HBOTTOM %03d' "$rows" "$stamp"
    elif [ "$event" = START ]; then
        printf '\033[2J\033[H%s\r\nHARD-END\r\n' "$GWAE_RESIZE_TEXT"
    fi
    printf '%s %s %s %s\n' "$stamp" "$event" "$rows" "$cols" >> "$GWAE_RESIZE_LOG"
}
trap 'report WINCH' WINCH
trap 'exit 0' HUP TERM
if [ -z "$GWAE_RESIZE_TEXT" ]; then printf '\033[?1049h'; fi
report START
while :; do
    if read -r key; then
        case "$key" in
            p) report PROBE ;;
            q) exit 0 ;;
        esac
    fi
done
"#;

#[derive(Debug)]
struct Event {
    seq: usize,
    reason: String,
    size: Size,
}

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    screen: Vt100Grid,
    dir: PathBuf,
    last_seq: usize,
}

impl Session {
    fn start(reflow_text: Option<&str>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-resize-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("create isolated config");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "default_column_width = \"quarter\"\nstartup_panes = 1\ncontent_width = 0\n\
             center_focus = false\nkeep_awake = false\ncell_labels = false\n\
             [minimap]\nshow = false\n[cowsay]\nenabled = false\n\
             [update]\ncheck = false\n",
        )
        .expect("write config");
        std::fs::write(dir.join("resize-helper.sh"), HELPER).expect("write shell helper");
        let pair = native_pty_system()
            .openpty(pty_size(HOST))
            .expect("open host PTY");
        let executable =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        eprintln!("resize acceptance executable: {executable:?}");
        let mut cmd = CommandBuilder::new(executable);
        // Developer logging/reload overrides and the user's terminal identity
        // must not leak into this deterministic, headless host terminal.
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
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_RESIZE_LOG", dir.join("events.log"));
        cmd.env("GWAE_RESIZE_TEXT", reflow_text.unwrap_or(""));
        cmd.arg("run");
        cmd.arg("/bin/sh resize-helper.sh");
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
        let mut s = Self {
            child,
            master: pair.master,
            writer,
            rx,
            screen: Vt100Grid::new(HOST),
            dir,
            last_seq: 0,
        };
        s.wait_event("START", inner(HOST, 4), "initial quarter width");
        // Dismiss any startup HUD and prove normal input reaches this helper.
        s.send(b"p\r");
        if reflow_text.is_some() {
            s.wait_event("PROBE", inner(HOST, 4), "initial explicit stty probe");
        } else {
            s.assert_redraw("PROBE", inner(HOST, 4), "initial explicit stty probe");
        }
        s
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

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("events.log")).unwrap_or_default()
    }

    fn wait_event(&mut self, reason: &str, size: Size, label: &str) -> Event {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            for line in self.log().lines().rev() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() != 4 {
                    continue;
                }
                let (Ok(seq), Ok(rows), Ok(cols)) =
                    (fields[0].parse(), fields[2].parse(), fields[3].parse())
                else {
                    continue;
                };
                let event = Event {
                    seq,
                    reason: fields[1].into(),
                    size: Size { rows, cols },
                };
                if event.seq > self.last_seq && event.reason == reason && event.size == size {
                    self.last_seq = event.seq;
                    eprintln!("{label}: child {event:?}");
                    return event;
                }
            }
            self.pump();
        }
        panic!(
            "{label}: expected fresh {reason} at {size:?} after event {}\nchild stty/WINCH log:\n{}\nhost screen:\n{}",
            self.last_seq, self.log(), self.screen.visible_text()
        );
    }

    fn wait_screen(&mut self, label: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if ready(self) {
                return;
            }
            self.pump();
        }
        panic!(
            "{label}: redraw/reflow not visible\nchild log:\n{}\nhost screen:\n{}",
            self.log(),
            self.screen.visible_text()
        );
    }

    fn row(&self, y: u16, width: u16) -> String {
        (1..=width).map(|x| self.screen.cell(x, y).ch).collect()
    }

    fn assert_redraw(&mut self, reason: &str, size: Size, label: &str) {
        let event = self.wait_event(reason, size, label);
        let header = format!(
            "{:03} {} {}x{}",
            event.seq, event.reason, size.rows, size.cols
        );
        let bottom = format!("BOTTOM {:03}", event.seq);
        // Logical cols subtract ONLY the left inset. A full-width pane has
        // one less visible cell because the final right frame is on-screen.
        // Its final X is clipped, but W must still reach the visible edge.
        let visible_cols = size.cols.min(self.screen.size().cols - 2);
        let ruler: String = format!("{}WX", ".".repeat(size.cols as usize - 2))
            .chars()
            .take(visible_cols as usize)
            .collect();
        self.wait_screen(label, |s| {
            s.row(1, header.len() as u16) == header
                && s.row(3, visible_cols) == ruler
                && s.row(size.rows, bottom.len() as u16) == bottom
        });
        eprintln!("{label}: observed header {header:?}, {visible_cols}-column ruler and {bottom:?} on bottom row {}", size.rows);
    }

    fn cycle(&mut self, divisor: u16, label: &str) {
        self.send(ALT_R);
        self.assert_redraw("WINCH", inner(self.screen.size(), divisor), label);
    }

    fn fullscreen(&mut self, divisor: u16, label: &str) {
        self.send(ALT_F);
        self.assert_redraw("WINCH", inner(self.screen.size(), divisor), label);
    }

    fn resize_host(&mut self, host: Size, divisor: u16, label: &str) {
        // This ioctl delivers a real host SIGWINCH to gwae. Never inject a
        // fake crossterm event or signal the helper directly.
        self.master.resize(pty_size(host)).expect("resize host PTY");
        self.screen.resize(host);
        self.assert_redraw("WINCH", inner(host, divisor), label);
    }

    fn assert_reflow(&mut self, text: &str, size: Size, label: &str) {
        let lines: Vec<_> = text.as_bytes().chunks(size.cols as usize).collect();
        let visible_cols = size.cols.min(self.screen.size().cols - 2);
        self.wait_screen(label, |s| {
            // Reflow can preserve the cursor's vertical position by adding
            // empty rows above a paragraph as it gets shorter on widening.
            // Require exact new soft-wrap points and all content in order,
            // without requiring a particular legitimate viewport anchor.
            (1..=size.rows - lines.len() as u16).any(|top| {
                (1..=size.rows).all(|y| {
                    let expected = if y >= top && y < top + lines.len() as u16 {
                        String::from_utf8_lossy(lines[(y - top) as usize]).into_owned()
                    } else if y == top + lines.len() as u16 {
                        "HARD-END".to_owned()
                    } else {
                        String::new()
                    };
                    let padded = format!("{expected:<width$}", width = size.cols as usize);
                    s.row(y, visible_cols)
                        == padded
                            .chars()
                            .take(visible_cols as usize)
                            .collect::<String>()
                })
            })
        });
        eprintln!("{label}: observed expected viewport for {} characters in {} soft-wrapped rows at {} columns, followed by HARD-END, without child redraw", text.len(), lines.len(), size.cols);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn inner(host: Size, divisor: u16) -> Size {
    Size {
        rows: host.rows - 2,
        cols: host.cols.div_ceil(divisor) - 1,
    }
}

#[test]
fn width_cycles_and_fullscreen_deliver_sigwinch_and_redraw() {
    let mut s = Session::start(None);
    for round in 1..=3 {
        s.cycle(3, &format!("cycle {round}: quarter -> third"));
        s.cycle(2, &format!("cycle {round}: third -> half"));
        s.cycle(4, &format!("cycle {round}: half -> quarter"));
    }
    for round in 1..=2 {
        s.fullscreen(1, &format!("fullscreen {round}: quarter -> full"));
        s.fullscreen(4, &format!("fullscreen {round}: full -> quarter"));
    }
}

#[test]
fn host_resize_delivers_both_dimensions_in_tiled_and_fullscreen_panes() {
    let mut s = Session::start(None);
    for divisor in [4, 1] {
        if divisor == 1 {
            s.fullscreen(1, "enter full width before resizing host");
        }
        for host in [
            Size {
                rows: 36,
                cols: 144,
            },
            Size { rows: 24, cols: 96 },
            HOST,
        ] {
            s.resize_host(host, divisor, &format!("host {host:?}, width 1/{divisor}"));
        }
    }
    s.fullscreen(4, "restore quarter after host resize");
}

#[test]
fn primary_output_reflows_across_width_cycles_without_child_redraw() {
    // No new output after START, including from the WINCH trap. Numbered
    // chunks catch tail truncation, duplication, and incorrect ordering.
    let text: String = (0..23).map(|i| format!("word{i:02}:")).collect();
    let mut s = Session::start(Some(&text));
    s.assert_reflow(&text, inner(HOST, 4), "initial primary output");
    for round in 1..=3 {
        for divisor in [3, 2, 4] {
            let label = format!("primary cycle {round}, width 1/{divisor}");
            s.send(ALT_R);
            let size = inner(HOST, divisor);
            s.wait_event("WINCH", size, &label);
            s.assert_reflow(&text, size, &label);
        }
    }
    for divisor in [1, 4, 1, 4] {
        let label = format!("primary fullscreen round trip, width 1/{divisor}");
        s.send(ALT_F);
        let size = inner(HOST, divisor);
        s.wait_event("WINCH", size, &label);
        s.assert_reflow(&text, size, &label);
    }
    for host in [
        Size { rows: 24, cols: 96 },
        Size {
            rows: 36,
            cols: 144,
        },
        HOST,
    ] {
        let label = format!("primary host resize {host:?}");
        s.master.resize(pty_size(host)).expect("resize host PTY");
        s.screen.resize(host);
        let size = inner(host, 4);
        s.wait_event("WINCH", size, &label);
        s.assert_reflow(&text, size, &label);
    }
}
