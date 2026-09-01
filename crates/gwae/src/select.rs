//! Drag-to-copy selection inside a PTY pane.
//!
//! gwae captures the mouse (for click-to-focus, and to forward events to a
//! child that asked for mouse reporting), which takes native click-drag
//! selection away from the host terminal. This module gives it back: a left
//! drag inside a pane highlights cells and, on release, copies the selected
//! text to the system clipboard, exactly like jcode's own transcript
//! selection.
//!
//! Coordinates are the pane's own *grid* coordinates (the same `(gx, gy)`
//! `pane_at` returns), so the highlight lines up with what is rendered no
//! matter how the strip is scrolled horizontally.

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
            // A width-0 cell is the right half of the wide glyph already
            // emitted by the previous cell; emitting it would duplicate the
            // character.
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

/// Copy `text` to the system clipboard.
///
/// Native clipboards first (they work whatever the host terminal supports),
/// then OSC 52 as the remote/SSH fallback. OSC 52 alone is not enough:
/// Terminal.app and several Linux terminals silently ignore it while the
/// write still "succeeds", which would report a copy that never happened.
pub fn copy_to_clipboard(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        if spawn_copy("pbcopy", &[], text) {
            return true;
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if spawn_copy("wl-copy", &[], text) {
            return true;
        }
        if spawn_copy("xclip", &["-selection", "clipboard"], text) {
            return true;
        }
        if spawn_copy("xsel", &["--clipboard", "--input"], text) {
            return true;
        }
    }
    #[cfg(windows)]
    {
        if spawn_copy("clip", &[], text) {
            return true;
        }
    }
    copy_via_osc52(text)
}

/// Feed `text` to a clipboard helper program's stdin.
#[cfg_attr(test, allow(dead_code))]
fn spawn_copy(program: &str, args: &[&str], text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.kill();
            return false;
        }
    }
    drop(child.stdin.take());
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Ask the *host* terminal to set the clipboard (OSC 52). This is the only
/// path that works over SSH or inside a container, where no local clipboard
/// helper exists.
fn copy_via_osc52(text: &str) -> bool {
    use std::io::{IsTerminal, Write};

    let mut out = std::io::stdout();
    if !out.is_terminal() {
        return false;
    }
    let seq = osc52_sequence(text);
    out.write_all(seq.as_bytes()).is_ok() && out.flush().is_ok()
}

/// `ESC ] 52 ; c ; <base64> BEL`, the clipboard-set escape.
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Minimal standard base64 with padding (gwae has no base64 dependency and
/// this is the only place that needs one).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
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
    out
}

// --- paste ---------------------------------------------------------------

/// The bracketed-paste delimiters (`DECSET 2004`).
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Largest chunk written to a pane in one go. A paste can be megabytes (a
/// whole file), and a PTY's buffer is not: writing it all at once blocks the
/// event loop until the child drains, which freezes every *other* pane's
/// paint. Chunking keeps the loop responsive.
pub const PASTE_CHUNK: usize = 4096;

/// Encode pasted `text` as the bytes a pane should receive.
///
/// `bracketed` is the focused child's own `DECSET 2004` state, not the host's:
/// gwae decodes the host's markers away, so a child that asked for bracketed
/// paste needs them re-emitted or it cannot tell a 40-line paste from someone
/// typing 40 lines — the difference between an agent buffering a prompt and
/// submitting the first line of it.
///
/// Two normalizations apply either way:
///
/// * `\r\n` and lone `\n` become `\r`, which is what a terminal delivers for
///   Return. A child reading a bare `\n` from a PTY in canonical mode sees a
///   line that never ends.
/// * Any embedded end marker is removed. A payload containing `ESC[201~`
///   would otherwise close the bracket early and let its tail run as
///   keystrokes — the standard paste-injection hole, and the reason
///   bracketed paste has a security reputation at all.
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

/// Read the system clipboard, the mirror image of [`copy_to_clipboard`].
///
/// There is deliberately no OSC 52 *read* path: the terminal's reply would
/// arrive on gwae's own stdin in the middle of a frame, and most terminals
/// refuse clipboard reads anyway (a remote host being able to exfiltrate your
/// clipboard is a real attack, not a hypothetical one). Over SSH this returns
/// `None` and the caller says so out loud.
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

// --- image clipboard (P4) ---------------------------------------------

/// Write PNG bytes to the system image clipboard.
///
/// Tries native helpers first; returns false when none succeed so the caller
/// can fall back to writing a file and toasting the path (SSH / headless).
pub fn copy_image_to_clipboard(png: &[u8]) -> bool {
    if png.is_empty() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        if spawn_image_copy_macos(png) {
            return true;
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if spawn_image_bytes("wl-copy", &["-t", "image/png"], png) {
            return true;
        }
        if spawn_image_bytes(
            "xclip",
            &["-selection", "clipboard", "-t", "image/png"],
            png,
        ) {
            return true;
        }
    }
    #[cfg(windows)]
    {
        if spawn_image_bytes_windows(png) {
            return true;
        }
    }
    false
}

fn spawn_image_bytes(program: &str, args: &[&str], bytes: &[u8]) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(bytes).is_err() {
            let _ = child.kill();
            return false;
        }
    }
    drop(child.stdin.take());
    child.wait().map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn spawn_image_copy_macos(png: &[u8]) -> bool {
    use std::process::{Command, Stdio};
    // Use a temp file + osascript to set «class PNGf».
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("gwae-clip-{}.png", std::process::id()));
    if std::fs::write(&tmp, png).is_err() {
        return false;
    }
    let applescript = format!(
        "set theFile to POSIX file \"{}\"\n\
         set theData to read theFile as «class PNGf»\n\
         set the clipboard to theData\n",
        tmp.display()
    );
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", &applescript])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let ok = cmd.status().map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    if ok {
        return true;
    }
    spawn_image_fallback_macos(png)
}

#[cfg(target_os = "macos")]
fn spawn_image_fallback_macos(png: &[u8]) -> bool {
    spawn_image_bytes("pbcopy", &[], png)
}

#[cfg(windows)]
fn spawn_image_bytes_windows(png: &[u8]) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("gwae-clip-{}.png", std::process::id()));
    if std::fs::write(&tmp, png).is_err() {
        return false;
    }
    let ps = format!(
        "Set-Clipboard -Path \"{}\"",
        tmp.display().to_string().replace('\"', "\"\"")
    );
    let ok = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    if ok {
        return true;
    }
    let mut child = match Command::new("clip")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(png).is_err() {
            let _ = child.kill();
            return false;
        }
    }
    drop(child.stdin.take());
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// SSH/headless fallback: write PNG to a temp file and return its path.
pub fn write_image_fallback(png: &[u8]) -> Option<std::path::PathBuf> {
    let mut p = std::env::temp_dir();
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!(
        "gwae-shot-{}-{}.png",
        std::process::id(),
        n % 1_000_000
    ));
    std::fs::write(&p, png).ok()?;
    Some(p)
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
        // Same pane only.
        assert!(!s.contains(2, 5, 0));
        // Start row: only from the anchor column rightwards.
        assert!(!s.contains(1, 4, 0));
        assert!(s.contains(1, 5, 0));
        // Middle row: everything.
        assert!(s.contains(1, 0, 1));
        assert!(s.contains(1, 19, 1));
        // End row: up to and including the release column.
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
        // Six columns wide: three wide glyphs, each with a continuation cell.
        assert_eq!(selected_text(&g, &sel((0, 0), (5, 0))), "日本語");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn osc52_wraps_base64_payload() {
        assert_eq!(osc52_sequence("foo"), "\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn empty_text_is_never_copied() {
        assert!(!copy_to_clipboard(""));
    }

    #[test]
    fn paste_is_bracketed_only_when_the_child_asked() {
        // The child's DECSET 2004 decides. A shell that asked for it must see
        // the markers or a multi-line paste runs line by line.
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
