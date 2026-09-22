//! OSC 133 shell-integration scanning (verbatim move from `tui/mod.rs`).

use gwae_layout::PaneStatus;

/// Scan a PTY output chunk for OSC 133 shell-integration markers and return
/// the status implied by the *last* one present. The protocol (emitted by
/// fish/zsh integrations and agent harnesses like jcode):
///   `133;A`   prompt shown  -> the pane is waiting for input (Idle)
///   `133;C`   command start -> the pane is working (Running)
///   `133;D;n` command done  -> Done when n == 0 (or omitted), Failed else
/// `133;B` (prompt end / input start) is ignored: focus-wise it is still the
/// prompt. Sequences may be terminated by BEL or ST and may split across
/// reads; a marker whose terminator hasn't arrived yet is picked up on a
/// later chunk (the payload we need sits right after the `133;` prefix).
pub(crate) fn scan_osc133(bytes: &[u8]) -> Option<PaneStatus> {
    let mut status = None;
    let mut i = 0;
    while i + 6 <= bytes.len() {
        // ESC ] 1 3 3 ;
        if bytes[i] == 0x1b && bytes[i + 1] == b']' && bytes[i + 2..i + 6] == *b"133;" {
            let rest = &bytes[i + 6..];
            match rest.first() {
                Some(b'A') => status = Some(PaneStatus::Idle),
                Some(b'C') => status = Some(PaneStatus::Running),
                Some(b'D') => {
                    // Exit code follows as `;n` up to BEL/ESC; absent means 0.
                    let code: u32 = rest
                        .get(1)
                        .filter(|c| **c == b';')
                        .map(|_| {
                            rest[2..]
                                .iter()
                                .take_while(|c| c.is_ascii_digit())
                                .fold(0u32, |a, c| a.saturating_mul(10) + (*c - b'0') as u32)
                        })
                        .unwrap_or(0);
                    status = Some(if code == 0 {
                        PaneStatus::Done
                    } else {
                        PaneStatus::Failed
                    });
                }
                _ => {}
            }
            i += 6;
        } else {
            i += 1;
        }
    }
    status
}

/// What a pane's tile actually claims after an OSC 133 marker.
///
/// `Running` is an *agent* claim: the HUD triages harness work, and a plain
/// pane mid-command (an editor, a pager, a long `git log`) is not work the
/// user is waiting on. A shell-integrated plain shell still gets its prompt
/// (`Idle`) and completion (`Done`/`Failed`) statuses, since those are the
/// user-facing facts the minimap and smart-jump act on; only the in-flight
/// `133;C` is demoted to neutral for non-agent panes.
pub(crate) fn osc_status(st: PaneStatus, is_agent: bool) -> PaneStatus {
    if st == PaneStatus::Running && !is_agent {
        PaneStatus::Plain
    } else {
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_layout::PaneStatus;

    #[test]
    fn scan_osc133_maps_protocol_to_status() {
        // Prompt marker -> waiting for input.
        assert_eq!(scan_osc133(b"\x1b]133;A\x07"), Some(PaneStatus::Idle));
        // Command start -> running.
        assert_eq!(scan_osc133(b"\x1b]133;C\x07"), Some(PaneStatus::Running));
        // Command done, exit 0 (and the bare form) -> done.
        assert_eq!(scan_osc133(b"\x1b]133;D;0\x07"), Some(PaneStatus::Done));
        assert_eq!(scan_osc133(b"\x1b]133;D\x1b\\"), Some(PaneStatus::Done));
        // Non-zero exit -> failed.
        assert_eq!(scan_osc133(b"\x1b]133;D;127\x07"), Some(PaneStatus::Failed));
        // The *last* marker in a chunk wins (C then D;1 -> failed).
        assert_eq!(
            scan_osc133(b"\x1b]133;C\x07output\x1b]133;D;1\x07"),
            Some(PaneStatus::Failed)
        );
        // Ordinary output and other OSCs carry no status.
        assert_eq!(scan_osc133(b"plain output"), None);
        assert_eq!(scan_osc133(b"\x1b]2;title\x07"), None);
        // B (input start) is not a status change.
        assert_eq!(scan_osc133(b"\x1b]133;B\x07"), None);
    }

    #[test]
    fn running_is_an_agent_only_claim() {
        // A harness pane's command-start marker means real agent work.
        assert_eq!(
            osc_status(PaneStatus::Running, true),
            PaneStatus::Running
        );
        // A plain pane running lazyvim (its shell emitted 133;C) is not
        // "working" in the HUD sense: the tile stays neutral.
        assert_eq!(osc_status(PaneStatus::Running, false), PaneStatus::Plain);
        // Every other marker is trusted from any integrated shell.
        for st in [PaneStatus::Idle, PaneStatus::Done, PaneStatus::Failed] {
            assert_eq!(osc_status(st, false), st);
            assert_eq!(osc_status(st, true), st);
        }
    }
}
