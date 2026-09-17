//! Optional acceptance test for image-pane responsiveness.
//! Requires tdf >= 0.5 and Ghostty-class Kitty graphics, then:
//! cargo test -p gwae --test pdf_e2e -- --ignored --nocapture
//!
//! Real tdf renders a real PDF inside a real gwae PTY. gwae paints every pane
//! from one thread, so the bytes one image pane pushes to the host per redraw
//! are the whole session's frame budget. This measures that volume directly.
#![cfg(unix)]

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A multi-page PDF whose pages differ, so page turns are genuine new content
/// rather than a cached texture.
fn write_pdf(path: &PathBuf) {
    write_pdf_pages(path, 6);
}

/// Same as [`write_pdf`] with an explicit page count, for tests that turn
/// more pages than the default document holds.
fn write_pdf_pages(path: &PathBuf, pages: usize) {
    const DEFAULT_PAGES: usize = 6;
    let pages = if pages == 0 { DEFAULT_PAGES } else { pages };
    let mut objects: Vec<String> = Vec::new();
    // 1: catalog, 2: pages, then per page a page object and its content.
    let kids: Vec<String> = (0..pages).map(|n| format!("{} 0 R", 3 + n * 2)).collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {pages} >>",
        kids.join(" ")
    ));
    let font_obj = 3 + pages * 2;
    for n in 0..pages {
        let mut text = String::new();
        for line in 0..40 {
            text.push_str(&format!(
                "BT /F1 11 Tf 40 {} Td (Page {} line {line}: ordinary paragraph text for rendering.) Tj ET\n",
                740 - line * 18,
                n + 1
            ));
        }
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font_obj} 0 R >> >> /Contents {} 0 R >>",
            4 + n * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{text}endstream",
            text.len()
        ));
    }
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (n, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", n + 1));
    }
    let start = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
        objects.len() + 1
    ));
    std::fs::write(path, pdf).unwrap();
}

struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    _dir: PathBuf,
}

impl Session {
    /// gwae runs tdf in one pane, on a PTY that reports pixel dimensions so
    /// the image path (not the text-cell fallback) is the one exercised.
    fn start() -> Self {
        Self::start_with_pages(6)
    }

    /// Same as [`start`](Self::start) with an explicit PDF page count, for
    /// tests that turn more pages than the default document holds.
    fn start_with_pages(pages: usize) -> Self {
        let dir = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("gwae-pdf-e2e-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gwae")).unwrap();
        let pdf = dir.join("page.pdf");
        write_pdf_pages(&pdf, pages);
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "default_column_width = 'full'\nstartup_panes = 1\n\
             [minimap]\nshow = false\n\
             [update]\ncheck = false\n",
        )
        .unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 140,
                pixel_width: 1400,
                pixel_height: 1000,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into()),
        );
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("PATH", std::env::var_os("PATH").unwrap());
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TMPDIR", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("LC_ALL", "en_US.UTF-8");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_KITTY_KEYBOARD", "0");
        // The whole point: the host image path must be live.
        cmd.env("GWAE_KITTY_GRAPHICS", "1");
        cmd.env("GWAE_LOG", "off");
        cmd.arg("run");
        cmd.arg(format!("tdf -f {}", pdf.display()));
        eprintln!("PDF acceptance command: {:?}", cmd.get_argv());
        let child = pair
            .slave
            .spawn_command(cmd)
            .expect("tdf >= 0.5 must be installed on PATH");
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut buf = [0; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            writer,
            rx,
            _dir: dir,
        }
    }

    /// Total host output while quiet for `quiet`, giving up after TIMEOUT.
    fn drain_until_quiet(&self, quiet: Duration) -> Vec<u8> {
        let deadline = Instant::now() + TIMEOUT;
        let mut all = Vec::new();
        loop {
            match self.rx.recv_timeout(quiet) {
                Ok(bytes) => all.extend_from_slice(&bytes),
                Err(_) => return all,
            }
            if Instant::now() >= deadline {
                return all;
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore]
fn real_tdf_page_redraws_stay_within_a_streamable_frame_budget() {
    let mut s = Session::start();
    // Startup: gwae's splash, tdf's own render, and the first page upload.
    let startup = s.drain_until_quiet(Duration::from_millis(1500));
    assert!(
        !startup.is_empty(),
        "gwae produced no output at all; tdf never started"
    );
    let image_packets = startup
        .windows(6)
        .filter(|w| **w == b"\x1b_Ga=T"[..])
        .count();
    assert!(
        image_packets > 0,
        "no host image transfer was emitted, so this ran the text fallback \
         and measures nothing: {} bytes of startup output",
        startup.len()
    );
    // Every transfer we emit must be zlib, otherwise a page costs megabytes.
    let compressed = startup.windows(4).filter(|w| **w == b",o=z"[..]).count();
    assert_eq!(
        compressed, image_packets,
        "some image transfers were sent uncompressed"
    );

    // A page turn is the interactive case that felt laggy. Measure the bytes
    // and the wall time gwae needs to absorb and repaint it.
    for n in 0..3 {
        let start = Instant::now();
        s.writer
            .write_all(if n % 2 == 0 { b"l" } else { b"h" })
            .unwrap();
        s.writer.flush().unwrap();
        let frame = s.drain_until_quiet(Duration::from_millis(700));
        let elapsed = start.elapsed();
        eprintln!("page turn: {} bytes in {elapsed:?}", frame.len());
        assert!(
            frame.len() < 256 * 1024,
            "{} bytes per page turn is not streamable at interactive rates \
             (this cost ~818 KB before uploads were compressed)",
            frame.len()
        );
    }
}

/// CPU seconds a process has used so far, via `ps`.
fn cpu_seconds(pid: u32) -> Option<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut seconds = 0.0;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(seconds)
}

/// A PDF left open on screen must be as cheap as an idle session. The texture
/// cache is keyed by content, so an unchanging page must not be re-rastered or
/// re-uploaded every frame.
#[test]
#[ignore]
fn a_pdf_left_open_on_screen_costs_almost_nothing() {
    let s = Session::start();
    let pid = s.child.process_id().expect("gwae pid");
    // Startup rendering is real work and must not count against steady state.
    std::thread::sleep(Duration::from_secs(4));
    let Some(start) = cpu_seconds(pid) else {
        return;
    };
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(5));
    let end = cpu_seconds(pid).expect("gwae still running");
    let wall = t0.elapsed().as_secs_f64();
    let pct = (end - start) / wall * 100.0;
    eprintln!("idle with a PDF on screen: {pct:.2}% of a core");
    assert!(
        pct < 5.0,
        "a still PDF burned {pct:.2}% of a core; it is being re-rendered every frame"
    );
}

/// Rapid page turns are the interaction that felt laggy. Each turn is genuine
/// new content, so this measures the real cost of the raster+upload path under
/// sustained input rather than the cached steady state.
#[test]
#[ignore]
fn sustained_page_turning_keeps_up_with_input() {
    let mut s = Session::start();
    let pid = s.child.process_id().expect("gwae pid");
    std::thread::sleep(Duration::from_secs(4));
    let _ = s.drain_until_quiet(Duration::from_millis(500));
    let Some(start) = cpu_seconds(pid) else {
        return;
    };
    let t0 = Instant::now();
    // Alternate forward/back so every step is a different page, defeating the
    // texture cache the way a reader flipping through a document does.
    let mut bytes = 0;
    for n in 0..20 {
        s.writer
            .write_all(if n % 2 == 0 { b"l" } else { b"h" })
            .unwrap();
        s.writer.flush().unwrap();
        std::thread::sleep(Duration::from_millis(120));
        while let Ok(chunk) = s.rx.try_recv() {
            bytes += chunk.len();
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    let end = cpu_seconds(pid).expect("gwae still running");
    let pct = (end - start) / wall * 100.0;
    eprintln!("20 page turns: {pct:.2}% of a core, {bytes} bytes to the host");
    // Turning pages costs real work, but it must stay well inside one core:
    // a saturated render thread is exactly what made the whole UI lag.
    assert!(
        pct < 30.0,
        "page turning burned {pct:.2}% of a core; the render thread is saturating"
    );
    assert!(
        bytes < 4 * 1024 * 1024,
        "{bytes} bytes for 20 page turns is not streamable \
         (this cost ~16.4 MB before uploads were compressed)"
    );
}

/// Phase 1 image isolation: once a page is up, idle frames must not push
/// image bytes. The per-pane prepare cache (keyed by the pane's
/// image-activity token) means a still page reuses its host tiles instead of
/// rewalking sources and reuploading. Without the cache every frame would
/// carry tile traffic and one image pane would eat the whole session budget.
#[test]
#[ignore]
fn still_page_emits_no_image_bytes_on_idle_frames() {
    let mut s = Session::start();
    let startup = s.drain_until_quiet(Duration::from_millis(1500));
    let startup_packets = startup
        .windows(6)
        .filter(|w| **w == b"\x1b_Ga=T"[..])
        .count();
    assert!(
        startup_packets > 0,
        "no host image transfer was emitted, so this ran the text fallback \
         and measures nothing: {} bytes of startup output",
        startup.len()
    );
    // Settle, then capture a quiet window with no input. Any image transfer
    // here is a re-upload of an unchanged page: the cache failed.
    let _ = s.drain_until_quiet(Duration::from_millis(500));
    let idle = s.drain_until_quiet(Duration::from_millis(2000));
    let idle_packets = idle.windows(6).filter(|w| **w == b"\x1b_Ga=T"[..]).count();
    eprintln!(
        "idle 2s window: {} bytes, {idle_packets} image transfers",
        idle.len()
    );
    assert_eq!(
        idle_packets, 0,
        "a still page re-uploaded {idle_packets} image transfers on idle frames"
    );
    // A genuine page turn must still produce exactly one page worth of new
    // image traffic: the cache isolates idle frames, it does not freeze them.
    s.writer.write_all(b"l").unwrap();
    s.writer.flush().unwrap();
    let turn = s.drain_until_quiet(Duration::from_millis(700));
    let turn_packets = turn.windows(6).filter(|w| **w == b"\x1b_Ga=T"[..]).count();
    eprintln!(
        "page turn: {} bytes, {turn_packets} image transfers",
        turn.len()
    );
    assert!(
        turn_packets > 0,
        "page turn produced no image traffic; the viewer did not navigate"
    );
}

/// Deep navigation past the old 64-image quota wedge. tdf allocates a fresh
/// image id per page and clears placements each turn with lowercase `d=a`,
/// which keeps image data for re-display; gwae used to accumulate one stored
/// source per page until the quota rejected every later transmit with ENOSPC,
/// leaving no placements, no host tiles, and a pitch-black viewer. Turning
/// 70 pages must keep producing exactly one page worth of host image traffic
/// per turn: the cache isolates idle frames, and quota-pressure eviction of
/// unplaced images keeps the viewer alive.
#[test]
#[ignore]
fn deep_page_navigation_keeps_rendering_past_sixty_turns() {
    let mut s = Session::start_with_pages(80);
    let startup = s.drain_until_quiet(Duration::from_millis(1500));
    assert!(
        startup.windows(6).any(|w| w == b"\x1b_Ga=T"),
        "no host image transfer was emitted, so this ran the text fallback \
         and measures nothing: {} bytes of startup output",
        startup.len()
    );
    let _ = s.drain_until_quiet(Duration::from_millis(500));
    let mut dark_turns = Vec::new();
    for n in 1..=70 {
        s.writer.write_all(b"l").unwrap();
        s.writer.flush().unwrap();
        let frame = s.drain_until_quiet(Duration::from_millis(700));
        let uploads = frame.windows(6).filter(|w| **w == b"\x1b_Ga=T"[..]).count();
        if uploads == 0 {
            dark_turns.push(n);
        }
    }
    assert!(
        dark_turns.is_empty(),
        "pages went black (no host image upload) on turns {dark_turns:?}; \
         the child image quota wedged again"
    );
}

/// Time from a keystroke to the first byte of the resulting redraw. This is
/// what "tactile" means to a reader: not throughput, but how soon the screen
/// starts responding to each key.
fn input_latency(s: &mut Session, key: &[u8]) -> Option<Duration> {
    // Settle first so we time this key's response, not the previous one's.
    let _ = s.drain_until_quiet(Duration::from_millis(400));
    let sent = Instant::now();
    s.writer.write_all(key).unwrap();
    s.writer.flush().unwrap();
    let first = s.rx.recv_timeout(Duration::from_secs(5)).ok()?;
    assert!(!first.is_empty());
    Some(sent.elapsed())
}

/// Zoom and scroll are the continuous interactions where re-rasterizing in the
/// middle would be felt. Each step is new content that defeats the texture
/// cache, so this measures the full decode/raster/encode/transmit path.
#[test]
#[ignore]
fn continuous_zoom_and_scroll_stay_responsive() {
    let mut s = Session::start();
    std::thread::sleep(Duration::from_secs(4));
    let mut worst = Duration::ZERO;
    let mut samples = Vec::new();
    // `z` toggles fit/fill, which re-scales the page: the most expensive
    // non-page-turn redraw, and a different raster every time.
    for _ in 0..10 {
        if let Some(d) = input_latency(&mut s, b"z") {
            worst = worst.max(d);
            samples.push(d);
        }
    }
    assert!(!samples.is_empty(), "tdf never responded to zoom input");
    let total: Duration = samples.iter().sum();
    let mean = total / samples.len() as u32;
    eprintln!(
        "zoom toggle: mean {:?}, worst {:?} over {} samples",
        mean,
        worst,
        samples.len()
    );
    // A redraw that starts within ~150 ms reads as immediate. Measured means
    // are 5-6 ms with a worst case near 14 ms, comfortably inside one 60 Hz
    // frame, so gwae's re-raster/re-encode is not what a reader feels. The
    // bound is deliberately loose: it catches a real regression (the path
    // becoming perceptible) without failing on scheduling noise.
    assert!(
        worst < Duration::from_millis(150),
        "worst zoom response was {worst:?}; the re-raster path is felt"
    );
}
