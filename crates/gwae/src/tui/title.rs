//! Host window title (OSC 2) helpers (verbatim move from `tui/mod.rs`).

use std::io::Write;

/// Strip control characters that could escape an OSC title sequence and clip
/// the result to a reasonable window-title length. Prevents a malicious child
/// title from running state-changing escapes on the host terminal.
pub(crate) fn sanitize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        if (c as u32) < 0x20 || c == '\x7f' {
            continue;
        }
        out.push(c);
        if out.chars().count() >= 256 {
            break;
        }
    }
    out
}

/// Tell the host terminal what title to display by writing OSC 2 (window
/// title) terminated with ST. Forwarding the focused pane's inner title makes
/// gwae effectively transparent to the host's title/status bar: the outer
/// window shows e.g. a jcode session title instead of "gwae".
pub(crate) fn emit_title(stdout: &mut impl Write, title: &str) -> std::io::Result<()> {
    write!(stdout, "\x1b]2;{}\x1b\\", sanitize_title(title))?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emit_title_writes_osc2_st() {
        let mut out = Vec::new();
        emit_title(&mut out, "jcode: my session").unwrap();
        assert_eq!(out, b"\x1b]2;jcode: my session\x1b\\");
    }

    #[test]
    fn sanitize_title_strips_control_and_clips() {
        // Ordinary text passes through untouched.
        assert_eq!(sanitize_title("abc 123"), "abc 123");
        // Control characters (ESC/BEL/CR/LF) are dropped so a child cannot
        // smuggle state-changing escapes out through the title; printable
        // characters inside the OSC payload are preserved verbatim.
        assert_eq!(sanitize_title("a\x1b]0;evil\x07b"), "a]0;evilb");
        assert_eq!(sanitize_title("\x01\x02"), "");
        // Over-long titles are clipped to a sane window-title length.
        let long = "x".repeat(1000);
        assert_eq!(sanitize_title(&long).chars().count(), 256);
    }
}
