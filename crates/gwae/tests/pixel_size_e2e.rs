//! Real-executable pixel geometry and terminal-query regression tests.
//!
//! The outer portable_pty supplies the host's actual TIOCGWINSZ metrics. A
//! guarded invocation of this test executable is the inner application: its
//! first terminal operation records ioctl geometry, then it issues queries
//! and logs the bytes actually returned on its own stdin. No Python, GUI,
//! mocked layout, injected resize events, or fabricated terminal replies.
//! Set GWAE_E2E_BIN to replay against a release or pre-fix executable.
#![cfg(unix)]

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const HOST: PtySize = PtySize {
    rows: 30,
    cols: 120,
    pixel_width: 960,
    pixel_height: 480,
};
const TIMEOUT: Duration = Duration::from_secs(10);

fn ioctl_size() -> PtySize {
    // SAFETY: winsize is a plain C structure and ioctl writes through its
    // valid pointer. The guarded helper runs with stdin attached to its PTY.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) },
        0,
        "child TIOCGWINSZ: {}",
        std::io::Error::last_os_error()
    );
    PtySize {
        rows: size.ws_row,
        cols: size.ws_col,
        pixel_width: size.ws_xpixel,
        pixel_height: size.ws_ypixel,
    }
}

fn dimensions(size: PtySize) -> String {
    format!(
        "{} {} {} {}",
        size.rows, size.cols, size.pixel_width, size.pixel_height
    )
}

fn inner(host: PtySize, divisor: u16) -> PtySize {
    let rows = host.rows - 2;
    let cols = host.cols.div_ceil(divisor) - 1;
    PtySize {
        rows,
        cols,
        pixel_width: cols * (host.pixel_width / host.cols),
        pixel_height: rows * (host.pixel_height / host.rows),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn record(log: &mut File, line: &str) {
    writeln!(log, "{line}").expect("append child observation");
    log.flush().expect("flush child observation");
}

/// Read actual terminal responses, including any unexpected extra bytes.
/// A quiet interval after the requested replies detects duplicated responses.
fn read_replies(expected_frames: usize) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let mut last_byte = Instant::now();
    while Instant::now() < deadline {
        let frames = bytes.iter().filter(|&&b| b == 0x1b).count();
        if frames >= expected_frames && last_byte.elapsed() >= Duration::from_millis(150) {
            break;
        }
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one initialized, valid pollfd.
        let ready = unsafe { libc::poll(&mut fd, 1, 20) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            panic!("child poll: {error}");
        }
        if ready > 0 {
            let mut buf = [0; 1024];
            match std::io::stdin().read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.extend_from_slice(&buf[..n]);
                    last_byte = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("child read: {e}"),
            }
        }
    }
    bytes
}

#[test]
#[ignore = "guarded subprocess helper, launched only by pixel_size_e2e tests"]
fn pixel_size_helper() {
    if std::env::var_os("GWAE_PIXEL_HELPER").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    // No raw-mode setup, query, sleep, or startup synchronization before this
    // observation. A later corrected geometry cannot replace the START record.
    let initial = ioctl_size();
    let dir = PathBuf::from(std::env::var_os("GWAE_PIXEL_DIR").expect("helper directory"));
    let pane = std::env::var("GWAE_PANE").expect("inner pane identity");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("pane-{pane}.log")))
        .expect("open helper log");
    record(&mut log, &format!("START {}", dimensions(initial)));
    record(&mut log, &format!("PID {}", std::process::id()));
    crossterm::terminal::enable_raw_mode().expect("raw child stdin for terminal replies");
    record(&mut log, "READY");
    let mut previous_size = initial;
    let mut previous_command = String::new();
    // A hard deadline also prevents an orphan helper from surviving a failed
    // outer test indefinitely. Parent Drop explicitly requests normal exit.
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline && !dir.join("stop").exists() {
        let size = ioctl_size();
        if size != previous_size {
            record(&mut log, &format!("SIZE {}", dimensions(size)));
            previous_size = size;
        }
        let command =
            std::fs::read_to_string(dir.join(format!("pane-{pane}.command"))).unwrap_or_default();
        if !command.is_empty() && command != previous_command {
            previous_command = command.clone();
            let fields: Vec<_> = command.split_whitespace().collect();
            if let [sequence, mode] = fields.as_slice() {
                let (query, frames): (&[u8], usize) = match *mode {
                    "pixels" => (b"\x1b[14t", 1),
                    "fragmented" => (b"\x1b[14t\x1b[16t\x1b[18t", 3),
                    "coalesced" => (b"\x1b[14t\x1b[16t\x1b[18t\x1b[14t", 4),
                    "status" => (b"\x1b[3;7H\x1b[c\x1b[5n\x1b[6n", 3),
                    "cursor" => (b"\x1b[6n", 1),
                    _ => panic!("unknown helper command {command}"),
                };
                let mut stdout = std::io::stdout().lock();
                if *mode == "fragmented" {
                    // Separate writes, each flushed and spaced across multiple
                    // renderer ticks, exercise streaming parser state.
                    for byte in query {
                        stdout.write_all(&[*byte]).expect("fragmented query");
                        stdout.flush().expect("flush query fragment");
                        std::thread::sleep(Duration::from_millis(35));
                    }
                } else {
                    stdout.write_all(query).expect("write child query batch");
                    stdout.flush().expect("flush child query batch");
                }
                drop(stdout);
                let reply = read_replies(frames);
                record(
                    &mut log,
                    &format!("PROCESS {sequence} {}", std::process::id()),
                );
                record(
                    &mut log,
                    &format!(
                        "QUERY {sequence} {mode} {} {}",
                        dimensions(ioctl_size()),
                        hex(&reply)
                    ),
                );
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = crossterm::terminal::disable_raw_mode();
    record(&mut log, "EXIT");
}

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    output: Vec<u8>,
    dir: PathBuf,
    sequence: usize,
    panes: usize,
    reload_binary: Option<(PathBuf, PathBuf)>,
}

/// Replace only a test-owned executable, never the selected installed/build
/// binary. Atomic replacement and ad-hoc signing match hotreload_e2e.rs.
fn install_binary(source: &Path, destination: &Path) {
    let temporary = destination.with_extension("new");
    std::fs::copy(source, &temporary).expect("copy executable for real hot reload");
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
        .expect("make copied executable runnable");
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["-f", "-s", "-"])
            .arg(&temporary)
            .status()
            .expect("ad-hoc sign copied executable");
        assert!(status.success(), "copied executable must be validly signed");
    }
    std::fs::rename(temporary, destination).expect("atomically replace test executable");
}

impl Session {
    fn start(host: PtySize, panes: usize) -> Self {
        Self::start_with_reload(host, panes, false)
    }

    fn start_with_reload(host: PtySize, panes: usize, reload: bool) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-pixel-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("create isolated config");
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            format!(
                "default_column_width = \"quarter\"\nstartup_panes = {panes}\ncontent_width = 0\n\
                 center_focus = false\nkeep_awake = false\ncell_labels = false\n\
                 [minimap]\nshow = false\n[cowsay]\nenabled = false\n\
                 [update]\ncheck = false\n"
            ),
        )
        .expect("write isolated config");
        // All panes run the same guarded Rust helper, including unfocused
        // startup panes that normally execute $SHELL without arguments.
        let helper = dir.join("pixel-helper.sh");
        std::fs::write(
            &helper,
            "#!/bin/sh\nexec \"$GWAE_PIXEL_TEST_EXE\" --ignored --exact pixel_size_helper --nocapture --test-threads=1\n",
        )
        .expect("write helper launcher");
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))
            .expect("make helper launcher executable");
        let pair = native_pty_system().openpty(host).expect("open host PTY");
        let executable =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        eprintln!("pixel acceptance executable: {executable:?}, host {host:?}");
        let source = std::fs::canonicalize(executable).expect("selected gwae executable");
        let reload_binary = reload.then(|| {
            let destination = dir.join("gwae-bin");
            install_binary(&source, &destination);
            (source.clone(), destination)
        });
        let mut cmd = CommandBuilder::new(
            reload_binary
                .as_ref()
                .map(|(_, path)| path)
                .unwrap_or(&source),
        );
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", &helper);
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env("ENV", "/dev/null");
        cmd.env("BASH_ENV", "/dev/null");
        cmd.env("LC_ALL", "C");
        cmd.env("GWAE_LOG", "off");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        cmd.env("GWAE_KITTY_GRAPHICS", "0");
        cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_PIXEL_HELPER", "1");
        cmd.env("GWAE_PIXEL_DIR", &dir);
        cmd.env("TMPDIR", &dir);
        if reload {
            cmd.env("GWAE_DEV_RELOAD", "1");
        }
        cmd.env(
            "GWAE_PIXEL_TEST_EXE",
            std::env::current_exe().expect("test executable"),
        );
        cmd.arg("run");
        cmd.arg("/bin/sh pixel-helper.sh");
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
        let mut session = Self {
            child,
            master: pair.master,
            writer,
            rx,
            output: Vec::new(),
            dir,
            sequence: 0,
            panes,
            reload_binary,
        };
        for pane in 0..panes {
            let start = session.wait_line(pane, "START ");
            assert_eq!(
                start,
                format!("START {}", dimensions(inner(host, 4))),
                "pane {pane}: first child ioctl must already be pane-sized"
            );
            session.wait_line(pane, "READY");
        }
        session
    }

    fn log(&self, pane: usize) -> String {
        std::fs::read_to_string(self.dir.join(format!("pane-{pane}.log"))).unwrap_or_default()
    }

    fn pump(&mut self) {
        if let Ok(bytes) = self.rx.recv_timeout(Duration::from_millis(20)) {
            self.output.extend(bytes);
            if self.output.len() > 65536 {
                self.output.drain(..self.output.len() - 65536);
            }
        }
    }

    fn wait_line(&mut self, pane: usize, prefix: &str) -> String {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(line) = self
                .log(pane)
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(|line| line.trim_end_matches('\n'))
                .find(|line| line.starts_with(prefix))
            {
                return line.to_owned();
            }
            self.pump();
        }
        panic!(
            "pane {pane}: no {prefix:?}\nchild log:\n{}\nhost output:\n{}",
            self.log(pane),
            String::from_utf8_lossy(&self.output)
        );
    }

    fn request(&mut self, pane: usize, mode: &str) -> usize {
        self.sequence += 1;
        let command = self.dir.join(format!("pane-{pane}.command"));
        let temporary = command.with_extension("pending");
        std::fs::write(&temporary, format!("{} {mode}\n", self.sequence))
            .expect("write helper command");
        std::fs::rename(temporary, command).expect("atomically publish helper command");
        self.sequence
    }

    fn assert_reply(&mut self, pane: usize, sequence: usize, mode: &str, size: PtySize) {
        let pixels = format!("\x1b[4;{};{}t", size.pixel_height, size.pixel_width);
        let cell_pixels = format!(
            "\x1b[6;{};{}t",
            size.pixel_height / size.rows,
            size.pixel_width / size.cols
        );
        let cells = format!("\x1b[8;{};{}t", size.rows, size.cols);
        let expected = match mode {
            "pixels" => pixels,
            "fragmented" => format!("{pixels}{cell_pixels}{cells}"),
            "coalesced" => format!("{pixels}{cell_pixels}{cells}{pixels}"),
            "status" => "\x1b[?6c\x1b[0n\x1b[3;7R".to_owned(),
            _ => unreachable!(),
        };
        let line = self.wait_line(pane, &format!("QUERY {sequence} "));
        assert_eq!(
            line,
            format!(
                "QUERY {sequence} {mode} {} {}",
                dimensions(size),
                hex(expected.as_bytes())
            ),
            "pane {pane}: exact {mode} terminal replies and ioctl must agree"
        );
        eprintln!("pane {pane}: {line}");
    }

    fn query(&mut self, pane: usize, mode: &str, size: PtySize) {
        let sequence = self.request(pane, mode);
        self.assert_reply(pane, sequence, mode, size);
    }

    fn wait_size(&mut self, pane: usize, size: PtySize) {
        self.wait_line(pane, &format!("SIZE {}", dimensions(size)));
    }

    fn keys(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("host key input");
        self.writer.flush().expect("flush host keys");
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Let all helpers leave normally before killing gwae, even when an
        // assertion unwinds. SIGKILL of the host alone could orphan children.
        let _ = std::fs::write(self.dir.join("stop"), "stop\n");
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            if (0..self.panes).all(|pane| self.log(pane).lines().any(|line| line == "EXIT")) {
                break;
            }
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            self.pump();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn initial_child_ioctl_and_fragmented_coalesced_queries_match_pane_pixels() {
    let mut session = Session::start(HOST, 1);
    let size = inner(HOST, 4);
    // Concrete non-square cell metrics catch width/height transposition and
    // accidentally returning the full host's dimensions.
    assert_eq!(dimensions(size), "28 29 232 448");
    for mode in ["pixels", "fragmented", "coalesced", "status"] {
        session.query(0, mode, size);
    }
}

#[test]
fn host_resize_and_pixel_only_font_change_update_ioctl_and_query_replies() {
    let mut session = Session::start(HOST, 1);
    for host in [
        // Non-integral host padding must be floored per cell, not scaled as
        // a floating fraction of the whole host window.
        PtySize {
            rows: 36,
            cols: 144,
            pixel_width: 1303,
            pixel_height: 619,
        },
        // The same rows and columns with a different font is still a resize
        // of the child pixel area, even if crossterm coalesces cell events.
        PtySize {
            rows: 36,
            cols: 144,
            pixel_width: 1735,
            pixel_height: 727,
        },
        HOST,
    ] {
        session.master.resize(host).expect("real host TIOCSWINSZ");
        let size = inner(host, 4);
        session.wait_size(0, size);
        session.query(0, "coalesced", size);
    }
    session.keys(b"\x1br");
    session.wait_size(0, inner(HOST, 3));
    session.query(0, "fragmented", inner(HOST, 3));
    session.keys(b"\x1bf");
    session.wait_size(0, inner(HOST, 1));
    session.query(0, "coalesced", inner(HOST, 1));
}

#[test]
fn differently_sized_panes_receive_only_their_own_query_responses() {
    let mut session = Session::start(HOST, 2);
    // Keep pane 0 focused and make it wider than unfocused pane 1. Both
    // independently emit queries, so routing everything to focus cannot pass.
    session.keys(b"\x1br");
    session.wait_size(0, inner(HOST, 3));
    for _ in 0..3 {
        let left = session.request(0, "coalesced");
        let right = session.request(1, "fragmented");
        session.assert_reply(0, left, "coalesced", inner(HOST, 3));
        session.assert_reply(1, right, "fragmented", inner(HOST, 4));
    }
}

#[test]
fn newly_spawned_pane_first_ioctl_uses_current_host_pixels() {
    let mut session = Session::start(HOST, 1);
    let host = PtySize {
        rows: 36,
        cols: 144,
        pixel_width: 1735,
        pixel_height: 727,
    };
    session
        .master
        .resize(host)
        .expect("resize before adding pane");
    let size = inner(host, 4);
    session.wait_size(0, size);
    session.query(0, "coalesced", size);
    // Real Alt+Enter opens a shell column through sync_panes, rather than
    // exercising the initial startup-panes loop again.
    session.panes = 2;
    session.keys(b"\x1b\r");
    let start = session.wait_line(1, "START ");
    assert_eq!(start, format!("START {}", dimensions(size)));
    eprintln!("new pane 1 first ioctl: {start}");
    session.wait_line(1, "READY");
    session.query(1, "coalesced", size);
    session.query(0, "fragmented", size);
}

#[test]
fn hot_reload_inherits_live_pty_and_preserves_pixel_query_parity() {
    let mut session = Session::start_with_reload(HOST, 1, true);
    let size = inner(HOST, 4);
    let helper_pid = session.wait_line(0, "PID ");
    let helper_pid = helper_pid.strip_prefix("PID ").unwrap();
    let host_pid = session.child.process_id().expect("gwae pid");
    // Reload reconstructs the emulator at 1;1, but keeps the child/PTY.
    // A deliberately non-default cursor gives observable evidence that the
    // new image adopted the PTY, rather than merely waiting after a copy.
    session.query(0, "status", size);
    let (source, destination) = session.reload_binary.as_ref().unwrap();
    install_binary(source, destination);
    let deadline = Instant::now() + TIMEOUT;
    let mut saw_repaint = false;
    loop {
        let sequence = session.request(0, "cursor");
        let line = session.wait_line(0, &format!("QUERY {sequence} "));
        let prefix = format!("QUERY {sequence} cursor {} ", dimensions(size));
        let reply = line
            .strip_prefix(&prefix)
            .expect("geometry survives reload");
        // The real adoption path also sends Ctrl+L to repaint the child. A
        // probe in flight during exec may be lost, leaving just this input.
        // Observe it explicitly rather than pretending it is a query reply.
        if reply.contains("0c") {
            assert!(!saw_repaint, "only one reload repaint is expected");
            assert_eq!(reply.matches("0c").count(), 1);
            saw_repaint = true;
            eprintln!("observed real adoption repaint input: {line}");
        }
        let reply = reply.replace("0c", "");
        if reply == hex(b"\x1b[1;1R") {
            assert!(saw_repaint, "new grid must follow real PTY adoption");
            assert_eq!(
                session.wait_line(0, &format!("PROCESS {sequence} ")),
                format!("PROCESS {sequence} {helper_pid}"),
                "the same helper process must continue across exec/adoption"
            );
            eprintln!("observed reconstructed emulator after reload: {line}");
            break;
        }
        assert!(
            reply == hex(b"\x1b[3;7R") || (reply.is_empty() && saw_repaint),
            "unexpected transition response: {line}"
        );
        assert!(Instant::now() < deadline, "never observed a real reload");
    }
    assert_eq!(session.child.process_id(), Some(host_pid));
    assert!(session.child.try_wait().expect("check live gwae").is_none());
    assert_eq!(
        session
            .log(0)
            .lines()
            .filter(|line| line.starts_with("START "))
            .count(),
        1,
        "reload must not restart the helper"
    );
    session.query(0, "coalesced", size);
    // Both pixel-only and cell-count changes now exercise PaneIo::Inherited,
    // not the Owned master used before exec. Every query records a fresh ioctl.
    for host in [
        PtySize {
            pixel_width: 1200,
            pixel_height: 600,
            ..HOST
        },
        PtySize {
            rows: 36,
            cols: 144,
            pixel_width: 1735,
            pixel_height: 727,
        },
    ] {
        session
            .master
            .resize(host)
            .expect("resize inherited pane's host");
        let size = inner(host, 4);
        session.wait_size(0, size);
        session.query(0, "coalesced", size);
    }
}
