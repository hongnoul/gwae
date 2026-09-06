//! Selection geometry inside a PTY pane.
//!
//! gwae captures the mouse (for click-to-focus, and to forward events to a
//! child that asked for mouse reporting), which takes native selection away
//! from the host terminal. Text paste (`⌥+v`) is handled here: gwae reads the
//! system clipboard itself and delivers it to the focused pane, bracketed the
//! way that child expects. The host terminal's native paste (Cmd+V / Ctrl+V)
//! still works for shells; `⌥+v` is the route that also works when the child
//! grabbed the key (fish's `edit_command_buffer`, jcode's own smart paste).

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
#[allow(dead_code)]
pub fn selected_text<G: TermGrid>(grid: &G, sel: &Selection<impl Copy + Eq>) -> String {
    let size = grid.size();
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
    for ch in text.chars() {
        match ch {
            '\n' => {
                // \r\n already pushed the \r; don't double it.
                if out.last() != Some(&b'\r') {
                    out.push(b'\r');
                }
            }
            '\r' => out.push(b'\r'),
            _ => {
                let mut b = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
            }
        }
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

/// Read the system clipboard (text only).
///
/// There is deliberately no OSC 52 *read* path: the terminal's reply would
/// arrive on gwae's own stdin in the middle of a frame, and most terminals
/// refuse clipboard reads anyway. Over SSH this returns `None` and the
/// caller says so out loud.
pub fn read_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    let helpers: &[(&str, &[&str])] = &[("pbpaste", &[])];
    #[cfg(windows)]
    let helpers: &[(&str, &[&str])] = &[(
        "powershell",
        &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
    )];
    #[cfg(not(any(windows, target_os = "macos")))]
    let helpers: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-o", "-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    helpers
        .iter()
        .find_map(|(program, args)| spawn_paste(program, args))
}

/// Run a clipboard-read helper and capture its stdout.
#[cfg_attr(test, allow(dead_code))]
fn spawn_paste(program: &str, args: &[&str]) -> Option<String> {
    use std::process::{Command, Stdio};

    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::{Size, Vt100Grid};

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
