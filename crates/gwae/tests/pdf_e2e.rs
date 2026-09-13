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

/// A minimal single-page PDF with enough ink that its raster is a realistic
/// page rather than a flat fill.
fn write_pdf(path: &PathBuf) {
    let mut text = String::new();
    for line in 0..40 {
        text.push_str(&format!(
            "BT /F1 11 Tf 40 {} Td (Line {line} of an ordinary paragraph of page text.) Tj ET\n",
            740 - line * 18
        ));
    }
    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into());
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .into(),
    );
    objects.push(format!(
        "<< /Length {} >>\nstream\n{text}endstream",
        text.len()
    ));
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
        let dir = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("gwae-pdf-e2e-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gwae")).unwrap();
        let pdf = dir.join("page.pdf");
        write_pdf(&pdf);
        std::fs::write(
            dir.join("gwae/gwae.toml"),
            "default_column_width = 'full'\ncontent_width = 0\nstartup_panes = 1\n\
             cell_labels = false\n[minimap]\nshow = false\n[cowsay]\nenabled = false\n\
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
        cmd.env("GWAE_NO_INSTALL", "1");
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
    for _ in 0..3 {
        let start = Instant::now();
        s.writer.write_all(b" ").unwrap();
        s.writer.flush().unwrap();
        let frame = s.drain_until_quiet(Duration::from_millis(700));
        let elapsed = start.elapsed();
        eprintln!("page turn: {} bytes in {elapsed:?}", frame.len());
        assert!(
            frame.len() < 2 * 1024 * 1024,
            "{} bytes per page turn is not streamable at interactive rates",
            frame.len()
        );
    }
}
