//! Drag-to-copy selection and text paste inside a PTY pane.
//!
//! gwae captures the mouse (for click-to-focus, and to forward events to a
//! child that asked for mouse reporting), which takes native selection away
//! from the host terminal. A left drag copies the selected text on release.
//! Native clipboard helpers are preferred, with OSC 52 for remote terminals.
//! The host terminal owns clipboard reads. Native paste (Cmd+V or the host's
//! equivalent) arrives as text, encoded here for the child's bracketed-paste
//! mode. There is no separate Option+V shortcut or second clipboard read.

use gwae_term::{Cell, TermGrid};

/// One cell address inside a pane's grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    /// Grid row (0 = top of the visible pane).
    pub y: u16,
    /// Grid column.
    pub x: u16,
}

impl Point {
    pub fn new(x: u16, y: u16) -> Self {
        Point { x, y }
    }
}

/// An in-progress or completed selection inside one pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<P: Copy + Eq> {
    pub pane: P,
    pub anchor: Point,
    pub cursor: Point,
    /// True while the button is still held.
    pub dragging: bool,
}

impl<P: Copy + Eq> Selection<P> {
    /// The selection ends in document order (start <= end).
    pub fn ends(&self) -> (Point, Point) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// True when the selection covers no cell at all (a plain click).
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Whether `(x, y)` in `pane`'s grid is inside the selection. The range is
    /// inclusive of both ends, matching how terminals highlight a drag: the
    /// cell under the press and the cell under the release are both selected.
    pub fn contains(&self, pane: P, x: u16, y: u16) -> bool {
        if pane != self.pane {
            return false;
        }
        let (s, e) = self.ends();
        let p = Point::new(x, y);
        p >= s && p <= e
    }
}

/// Extract the selected text from a grid.
///
/// Lines are joined with `\n`, trailing blanks on each line are trimmed (a
/// terminal grid is space-padded to the full width, and pasting that padding
/// back is never what anyone wants), and wide-glyph continuation cells are
/// skipped so a CJK character is copied once, not twice.
pub fn selected_text<G: TermGrid>(grid: &G, sel: &Selection<impl Copy + Eq>) -> String {
    let size = grid.size();
    if size.cols == 0 || size.rows == 0 {
        return String::new();
    }
    let (start, end) = sel.ends();
    let mut out = String::new();
    let mut first = true;
    for y in start.y..=end.y.min(size.rows.saturating_sub(1)) {
        let x0 = if y == start.y { start.x } else { 0 };
        let x1 = if y == end.y {
            end.x
        } else {
            size.cols.saturating_sub(1)
        };
        let mut line = String::new();
        let mut x = x0;
        while x <= x1.min(size.cols.saturating_sub(1)) {
            let cell: Cell = grid.cell(x, y);
            if cell.width != 0 {
                cell.push_codepoints(&mut line);
            }
            x += 1;
        }
        let line = line.trim_end();
        if !first {
            out.push('\n');
        }
        out.push_str(line);
        first = false;
    }
    out
}

/// OSC 52 has no acknowledgement: successfully sending a request is not proof
/// that the terminal permitted a clipboard write. Keep that distinct from a
/// native helper's confirmed success in the user-facing feedback.
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
pub fn copy_to_clipboard(text: &str, output: &mut impl std::io::Write) -> CopyOutcome {
    #[cfg(target_os = "macos")]
    let helpers: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(windows)]
    let helpers: &[(&str, &[&str])] = &[("clip", &[])];
    #[cfg(not(any(windows, target_os = "macos")))]
    let helpers: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
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
    #[cfg(not(windows))]
    let bytes = text.as_bytes().to_vec();
    // clip.exe recognizes UTF-16LE with a BOM independently of the code page.
    #[cfg(windows)]
    let bytes: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
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
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Largest chunk written to a pane in one go. A paste can be a whole file,
/// and a PTY's buffer is not: writing it all at once blocks the event loop
/// until the child drains, which freezes every *other* pane's paint.
/// Chunking keeps the loop responsive.
pub const PASTE_CHUNK: usize = 4096;

/// Encode pasted `text` as the bytes a pane should receive.
///
/// `bracketed` is the focused child's own `DECSET 2004` state, not the host's:
/// a child that asked for bracketed paste (fish, zsh, jcode) needs the
/// markers re-emitted around the payload, or it cannot tell a multi-line
/// paste from typing — the difference between fish buffering a command and
/// running each line of it. A child that did not ask gets the bytes verbatim.
///
/// Two normalizations apply either way:
///
/// * `\r\n` and lone `\n` become `\r`, which is what a terminal delivers for
///   Return. A child reading a bare `\n` from a PTY sees a line that never ends.
/// * Any embedded end marker is removed. A payload containing `ESC[201~`
///   would otherwise close the bracket early and let its tail run as
///   keystrokes — the standard paste-injection hole.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let body = sanitize_paste(text);
    if body.is_empty() {
        return Vec::new();
    }
    if !bracketed {
        return body;
    }
    let mut out = Vec::with_capacity(body.len() + PASTE_START.len() + PASTE_END.len());
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(&body);
    out.extend_from_slice(PASTE_END);
    out
}

/// Normalize newlines to `\r` and strip embedded paste-end markers.
fn sanitize_paste(text: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    let mut previous_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\n' => {
                // Only an original CR forms a CRLF pair. Looking at `out`
                // instead would mistake a normalized LF for CR and collapse
                // consecutive blank lines (\n\n -> \r instead of \r\r).
                if !previous_was_cr {
                    out.push(b'\r');
                }
            }
            '\r' => out.push(b'\r'),
            _ => {
                let mut b = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
            }
        }
        previous_was_cr = ch == '\r';
    }
    strip_marker(&mut out, PASTE_END);
    strip_marker(&mut out, PASTE_START);
    out
}

/// Remove every occurrence of `marker` from `buf`, in place.
fn strip_marker(buf: &mut Vec<u8>, marker: &[u8]) {
    if buf.len() < marker.len() {
        return;
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(marker) {
            i += marker.len();
        } else {
            out.push(buf[i]);
            i += 1;
        }
    }
    *buf = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::{Size, Vt100Grid};

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

    fn grid(lines: &[&str]) -> Vt100Grid {
        let mut g = Vt100Grid::new(Size { cols: 20, rows: 5 });
        g.feed(lines.join("\r\n").as_bytes());
        g
    }

    #[test]
    fn ends_are_ordered_regardless_of_drag_direction() {
        let forward = sel((2, 0), (5, 1));
        let backward = sel((5, 1), (2, 0));
        assert_eq!(forward.ends(), backward.ends());
        assert_eq!(forward.ends().0, Point::new(2, 0));
    }

    #[test]
    fn contains_spans_whole_intermediate_rows() {
        let s = sel((5, 0), (2, 2));
        assert!(!s.contains(2, 5, 0));
        assert!(!s.contains(1, 4, 0));
        assert!(s.contains(1, 5, 0));
        assert!(s.contains(1, 0, 1));
        assert!(s.contains(1, 19, 1));
        assert!(s.contains(1, 2, 2));
        assert!(!s.contains(1, 3, 2));
    }

    #[test]
    fn single_cell_selection_is_empty() {
        assert!(sel((3, 1), (3, 1)).is_empty());
        assert!(!sel((3, 1), (4, 1)).is_empty());
    }

    #[test]
    fn selected_text_trims_grid_padding_and_spans_rows() {
        let g = grid(&["hello world", "second line"]);
        let s = sel((0, 0), (5, 1));
        assert_eq!(selected_text(&g, &s), "hello world\nsecond");
    }

    #[test]
    fn selected_text_takes_a_partial_single_row() {
        let g = grid(&["hello world"]);
        assert_eq!(selected_text(&g, &sel((6, 0), (10, 0))), "world");
    }

    #[test]
    fn selected_text_skips_wide_glyph_continuation_cells() {
        let g = grid(&["日本語"]);
        assert_eq!(selected_text(&g, &sel((0, 0), (5, 0))), "日本語");
    }

    #[test]
    fn paste_is_bracketed_only_when_the_child_asked() {
        assert_eq!(paste_bytes("ls -la", true), b"\x1b[200~ls -la\x1b[201~");
        // A child that did not ask gets the bytes verbatim: injecting markers
        // into a program that doesn't parse them prints `[200~` as text.
        assert_eq!(paste_bytes("ls -la", false), b"ls -la");
    }

    #[test]
    fn newlines_become_carriage_returns() {
        // A PTY delivers Return as \r; a bare \n leaves a line that never
        // ends. \r\n must not become two.
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\r\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a\rb", false), b"a\rb");
        assert_eq!(
            paste_bytes("one\ntwo\nthree", true),
            b"\x1b[200~one\rtwo\rthree\x1b[201~"
        );
    }

    #[test]
    fn blank_lines_survive_paste_normalization() {
        for text in ["\n\na\n\nb\n\n", "\r\r\na\r\n\nb\r\r\n"] {
            assert_eq!(paste_bytes(text, false), b"\r\ra\r\rb\r\r");
            assert_eq!(paste_bytes(text, true), b"\x1b[200~\r\ra\r\rb\r\r\x1b[201~");
        }
    }

    #[test]
    fn an_embedded_end_marker_cannot_escape_the_bracket() {
        // The paste-injection hole: a payload carrying the end marker would
        // otherwise close the bracket early and run its tail as keystrokes.
        let hostile = "safe\x1b[201~rm -rf /\r";
        let out = paste_bytes(hostile, true);
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s.matches("\x1b[201~").count(),
            1,
            "exactly one end marker, the one we appended: {s:?}"
        );
        assert!(
            s.ends_with("\x1b[201~"),
            "the marker is the last thing sent"
        );
        // The text survives; only the marker is removed.
        assert!(s.contains("saferm -rf /"));
        // A start marker in the payload is stripped too: doubling it lets a
        // payload survive a downstream strip of the outer pair.
        let s = String::from_utf8(paste_bytes("a\x1b[200~b", true)).unwrap();
        assert_eq!(s.matches("\x1b[200~").count(), 1);
    }

    #[test]
    fn an_empty_paste_writes_nothing_at_all() {
        // Not even the markers: an empty bracketed paste still makes some
        // shells redraw their prompt, which reads as a glitch.
        assert!(paste_bytes("", true).is_empty());
        assert!(paste_bytes("", false).is_empty());
    }
}
