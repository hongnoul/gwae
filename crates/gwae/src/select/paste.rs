//! Paste encoding: bracketed markers and newline normalization.

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
}
