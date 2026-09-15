//! Streaming extractor for Kitty graphics APC sequences.

/// Streaming extractor for Kitty graphics APC sequences (`ESC _ G ... ESC \`).
///
/// The terminal core parses and *drops* APC sequences, so
/// a child's Kitty image transmissions die inside the mux and panes show
/// nothing where an image should be. The fix is passthrough: gwae scans
/// each pane's raw PTY output and forwards complete graphics sequences
/// verbatim to the host terminal.
///
/// This is safe to do out-of-band because modern emitters (ratatui-image,
/// jcode) use *virtual placements* (`U=1`) addressed by U+10EEEE placeholder
/// cells: the APC only carries pixel data + an image id, and on-screen
/// position comes entirely from where the placeholder cells are painted. The
/// grid keeps those placeholder cells (see `Cell::combining`), so images land
/// exactly inside their pane and are cropped by pane clipping for free.
///
/// The extractor is a byte-level state machine so it survives PTY chunk
/// boundaries (a 1 MB PNG arrives as hundreds of 4 KB reads, and a chunk can
/// split even the 3-byte `ESC _ G` introducer). Non-graphics APCs are
/// swallowed, and a sequence over [`KittyApcExtractor::MAX_SEQ`] is discarded
/// rather than buffered forever (Kitty itself chunks payloads at 4 KB, so a
/// bigger "sequence" means a malformed or hostile stream).
#[derive(Default)]
pub struct KittyApcExtractor {
    state: ApcState,
    seq: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum ApcState {
    /// Ordinary output.
    #[default]
    Ground,
    /// Seen ESC.
    Esc,
    /// Seen ESC `_` (APC opener), kind not yet known.
    ApcOpen,
    /// Inside a graphics APC (`ESC _ G`), buffering into `seq`.
    Graphics,
    /// Inside a graphics APC, seen ESC (maybe ST).
    GraphicsEsc,
    /// Inside a non-graphics or oversized APC, discarding until ST.
    Skip,
    /// Inside a discarded APC, seen ESC (maybe ST).
    SkipEsc,
}

impl KittyApcExtractor {
    /// Upper bound for one buffered APC sequence. Kitty chunks image payloads
    /// at 4096 bytes of base64, so well-formed sequences are tiny; the bound
    /// only exists so a malformed stream cannot grow the buffer unboundedly.
    pub const MAX_SEQ: usize = 64 * 1024;

    pub fn new() -> Self {
        Self::default()
    }

    /// Scan `bytes`, returning every complete Kitty graphics APC sequence
    /// (introducer and ST terminator included) ready to write to the host.
    /// Partial sequences are carried across calls.
    pub fn extract(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.state {
                ApcState::Ground => {
                    if b == 0x1b {
                        self.state = ApcState::Esc;
                    }
                }
                ApcState::Esc => {
                    self.state = match b {
                        b'_' => ApcState::ApcOpen,
                        0x1b => ApcState::Esc,
                        _ => ApcState::Ground,
                    };
                }
                ApcState::ApcOpen => match b {
                    b'G' => {
                        self.seq.clear();
                        self.seq.extend_from_slice(b"\x1b_G");
                        self.state = ApcState::Graphics;
                    }
                    0x1b => self.state = ApcState::Esc,
                    _ => self.state = ApcState::Skip,
                },
                ApcState::Graphics => {
                    if b == 0x1b {
                        self.state = ApcState::GraphicsEsc;
                    } else if self.seq.len() >= Self::MAX_SEQ {
                        self.seq.clear();
                        self.state = ApcState::Skip;
                    } else {
                        self.seq.push(b);
                    }
                }
                ApcState::GraphicsEsc => {
                    if b == b'\\' {
                        self.seq.extend_from_slice(b"\x1b\\");
                        // Queries (a=q) are dropped: the host's reply would
                        // arrive on gwae's stdin, not the child's, so
                        // forwarding them can only desync both sides.
                        if is_graphics_query(&self.seq) {
                            self.seq.clear();
                        } else {
                            out.append(&mut self.seq);
                        }
                        self.state = ApcState::Ground;
                    } else {
                        // ESC inside a graphics payload is malformed (payloads
                        // are base64 + ASCII keys); drop the sequence and
                        // re-treat this byte from the ESC state.
                        self.seq.clear();
                        self.state = if b == 0x1b {
                            ApcState::Esc
                        } else if b == b'_' {
                            ApcState::ApcOpen
                        } else {
                            ApcState::Ground
                        };
                    }
                }
                ApcState::Skip => {
                    if b == 0x1b {
                        self.state = ApcState::SkipEsc;
                    }
                }
                ApcState::SkipEsc => {
                    self.state = match b {
                        b'\\' => ApcState::Ground,
                        0x1b => ApcState::SkipEsc,
                        _ => ApcState::Skip,
                    };
                }
            }
        }
        out
    }
}

/// Whether a complete graphics APC (`ESC _ G <controls> ; <payload> ESC \`) is
/// a capability query (`a=q`). Only the control section before any `;` is
/// inspected, so base64 payload bytes can never false-positive.
fn is_graphics_query(seq: &[u8]) -> bool {
    let body = seq.strip_prefix(b"\x1b_G").unwrap_or(seq);
    let controls = match body.iter().position(|&b| b == b';') {
        Some(i) => &body[..i],
        None => body,
    };
    controls
        .split(|&b| b == b',')
        .any(|kv| kv == b"a=q" || kv == b"a=+q")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apc_extractor_passes_graphics_and_survives_chunking() {
        let mut e = KittyApcExtractor::new();
        let seq = b"\x1b_Gi=42,a=T,U=1,f=32,s=2,v=1;AAAA\x1b\\";
        // Whole sequence in one chunk, surrounded by ordinary output.
        let out = e.extract(b"hello\x1b_Gi=42,a=T,U=1,f=32,s=2,v=1;AAAA\x1b\\world");
        assert_eq!(out, seq.to_vec());
        // Split at every possible byte boundary, including mid-introducer
        // and mid-terminator.
        for cut in 1..seq.len() {
            let mut e = KittyApcExtractor::new();
            let mut out = e.extract(&seq[..cut]);
            out.extend(e.extract(&seq[cut..]));
            assert_eq!(out, seq.to_vec(), "split at {cut}");
        }
    }


    #[test]
    fn apc_extractor_swallows_non_graphics_and_queries() {
        let mut e = KittyApcExtractor::new();
        // Non-graphics APC: swallowed.
        assert!(e.extract(b"\x1b_Xsomething\x1b\\").is_empty());
        // Graphics query (a=q): dropped, the host reply cannot be routed back.
        assert!(e
            .extract(b"\x1b_Ga=q,i=1,f=24,s=1,v=1;AAAA\x1b\\")
            .is_empty());
        // State machine returns to ground: a following display APC still passes.
        let seq = b"\x1b_Gi=7,a=T;AAAA\x1b\\";
        assert_eq!(e.extract(seq), seq.to_vec());
    }


    #[test]
    fn apc_extractor_bounds_runaway_sequences() {
        let mut e = KittyApcExtractor::new();
        // An unterminated "graphics" stream larger than MAX_SEQ is discarded,
        // not buffered forever.
        let big = vec![b'A'; KittyApcExtractor::MAX_SEQ + 1024];
        assert!(e.extract(b"\x1b_G").is_empty());
        assert!(e.extract(&big).is_empty());
        // Terminate the (now discarded) sequence; nothing comes out.
        assert!(e.extract(b"\x1b\\").is_empty());
        // And the extractor still works afterwards.
        let seq = b"\x1b_Gi=7,a=T;AAAA\x1b\\";
        assert_eq!(e.extract(seq), seq.to_vec());
    }
}
