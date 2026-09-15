//! Clipboard writes: native helpers with OSC 52 fallback.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    Copied,
    SentToTerminal,
    Empty,
    Unavailable,
}

impl CopyOutcome {
    pub fn note(self, text: &str) -> String {
        match self {
            Self::Copied => {
                let lines = text.lines().count().max(1);
                format!("copied {lines} line{}", if lines == 1 { "" } else { "s" })
            }
            Self::SentToTerminal => "copy sent to terminal".into(),
            Self::Empty => "nothing to copy".into(),
            Self::Unavailable => "clipboard unavailable".into(),
        }
    }
}

/// Copy only in response to a user selection. Over SSH, local helpers would
/// write the remote computer's clipboard, so use the host terminal instead.
///
/// macOS-only: `pbcopy`, with OSC 52 fallback.
pub fn copy_to_clipboard(text: &str, output: &mut impl std::io::Write) -> CopyOutcome {
    let helpers: &[(&str, &[&str])] = &[("pbcopy", &[])];
    let remote =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    copy_with_helpers(text, if remote { &[] } else { helpers }, output)
}

fn copy_with_helpers(
    text: &str,
    helpers: &[(&str, &[&str])],
    output: &mut impl std::io::Write,
) -> CopyOutcome {
    if text.trim().is_empty() {
        return CopyOutcome::Empty;
    }
    if helpers
        .iter()
        .any(|(program, args)| spawn_copy(program, args, text))
    {
        return CopyOutcome::Copied;
    }
    if output.write_all(osc52_sequence(text).as_bytes()).is_ok() && output.flush().is_ok() {
        CopyOutcome::SentToTerminal
    } else {
        CopyOutcome::Unavailable
    }
}

/// Bound both pipe writes and process exit. A missing display or stalled helper
/// must not hang the event loop indefinitely, even when text exceeds pipe capacity.
fn spawn_copy(program: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let mut stdin = child.stdin.take().expect("piped clipboard stdin");
    let bytes = text.as_bytes().to_vec();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let written = stdin.write_all(&bytes).is_ok();
        drop(stdin); // helpers commit only after EOF
        let _ = tx.send(written);
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status.success()
                    && rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .unwrap_or(false);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn osc52_sequence(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("\x1b]52;c;");
    for chunk in text.as_bytes().chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out.push('\x07');
    out
}

// --- paste ---------------------------------------------------------------

/// The bracketed-paste delimiters (`DECSET 2004`).
#[cfg(test)]
mod tests {
    use super::super::selection::{selected_text, Point, Selection};
    use super::*;
    use gwae_term::Size;

    #[test]
    fn osc52_encodes_utf8_padding_and_control_bytes_as_data() {
        for (text, payload) in [
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("日本語", "5pel5pys6Kqe"),
            ("a\nb", "YQpi"),
            ("\x1b]", "G10="),
        ] {
            assert_eq!(osc52_sequence(text), format!("\x1b]52;c;{payload}\x07"));
        }
    }

    #[test]
    fn blank_copy_leaves_all_clipboard_outputs_untouched() {
        for text in ["", "\n\n", " \t\n"] {
            let mut output = Vec::new();
            assert_eq!(
                copy_with_helpers(text, &[], &mut output),
                CopyOutcome::Empty
            );
            assert!(output.is_empty());
        }
    }

    #[test]
    fn empty_grid_dimensions_have_no_text_to_copy() {
        for size in [Size { cols: 0, rows: 3 }, Size { cols: 3, rows: 0 }] {
            let grid = gwae_testkit::FakeTerminal::new(size);
            assert_eq!(selected_text(&grid, &sel((0, 0), (2, 2))), "");
        }
    }

    #[test]
    fn absent_helper_sends_a_terminal_request_not_a_confirmed_copy() {
        let mut output = Vec::new();
        let outcome = copy_with_helpers(
            "foo",
            &[("/nonexistent/gwae-copy-helper", &[])],
            &mut output,
        );
        assert_eq!(outcome, CopyOutcome::SentToTerminal);
        assert_eq!(output, b"\x1b]52;c;Zm9v\x07");
        assert_eq!(outcome.note("foo"), "copy sent to terminal");
        assert_eq!(CopyOutcome::Copied.note("foo"), "copied 1 line");
        assert_eq!(CopyOutcome::Copied.note("foo\nbar"), "copied 2 lines");
    }

    #[test]
    fn failed_terminal_write_or_flush_reports_unavailable() {
        struct FailedOutput(bool);
        impl std::io::Write for FailedOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0 {
                    Ok(bytes.len())
                } else {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        for fails_on_flush in [false, true] {
            assert_eq!(
                copy_with_helpers("foo", &[], &mut FailedOutput(fails_on_flush)),
                CopyOutcome::Unavailable
            );
        }
        assert_eq!(
            CopyOutcome::Unavailable.note("foo"),
            "clipboard unavailable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_helper_consumes_utf8_and_eof_without_a_second_terminal_write() {
        let mut output = Vec::new();
        let helpers: &[(&str, &[&str])] = &[
            ("/nonexistent/gwae-copy-helper", &[]),
            ("/bin/sh", &["-c", "test \"$(/bin/cat)\" = '日本語'"]),
        ];
        assert_eq!(
            copy_with_helpers("日本語", helpers, &mut output),
            CopyOutcome::Copied
        );
        assert!(output.is_empty());
        assert!(!spawn_copy("/bin/sh", &["-c", "exit 1"], "foo"));
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_that_never_reads_cannot_block_a_pipe_sized_copy_forever() {
        let started = std::time::Instant::now();
        assert!(!spawn_copy(
            "/bin/sh",
            &["-c", "exec /bin/sleep 30"],
            &"x".repeat(256 * 1024)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    fn sel(anchor: (u16, u16), cursor: (u16, u16)) -> Selection<u8> {
        Selection {
            pane: 1,
            anchor: Point::new(anchor.0, anchor.1),
            cursor: Point::new(cursor.0, cursor.1),
            dragging: false,
        }
    }
}
