//! Narrow compatibility for private modes handled by the previous core.
//!
//! Plain DECSET/DECRST 47 is deliberately mapped to 1049: protecting the
//! primary screen matters more than retaining the alternate buffer on reentry.
//! This adopts 1049's clear/save/restore semantics, not exact historical 47
//! semantics. Mode 9 maps to 1000 to preserve the facade's boolean mouse
//! ownership, not to implement exact X10 event filtering.

const MAX_CSI_BYTES: usize = 256;

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    CsiPassthrough,
    String {
        osc: bool,
        escape: bool,
    },
}

/// Streaming normalizer for the 7-bit CSI syntax used by child terminals.
/// Other bytes, including UTF-8 and opaque control-string payloads, are kept.
/// Oversized or non-plain CSI sequences pass through without translation.
#[derive(Default)]
pub(super) struct LegacyCsiNormalizer {
    state: State,
    pending: Vec<u8>,
}

impl LegacyCsiNormalizer {
    /// Normalize a chunk, retaining at most 256 incomplete CSI body bytes.
    /// Keep this instance across calls and feed every returned byte to the core.
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + self.pending.len());
        for &byte in bytes {
            match self.state {
                State::Ground => {
                    out.push(byte);
                    if byte == 0x1b {
                        self.state = State::Escape;
                    }
                }
                State::Escape => {
                    out.push(byte);
                    self.state = match byte {
                        b'[' => State::Csi,
                        b']' => State::String {
                            osc: true,
                            escape: false,
                        },
                        b'P' | b'_' | b'^' | b'X' => State::String {
                            osc: false,
                            escape: false,
                        },
                        // Executed/ignored controls do not finish an escape.
                        0x00..=0x17 | 0x19 | 0x1b | 0x1c..=0x1f | 0x7f => State::Escape,
                        _ => State::Ground,
                    };
                }
                State::Csi => match byte {
                    0x20..=0x3f if self.pending.len() < MAX_CSI_BYTES => {
                        self.pending.push(byte);
                    }
                    0x40..=0x7e => {
                        self.emit_body(byte, &mut out);
                        self.pending.clear();
                        out.push(byte);
                        self.state = State::Ground;
                    }
                    _ => {
                        out.append(&mut self.pending);
                        self.passthrough_csi(byte, &mut out);
                    }
                },
                State::CsiPassthrough => self.passthrough_csi(byte, &mut out),
                State::String { osc, escape } => {
                    out.push(byte);
                    self.state = if matches!(byte, 0x18 | 0x1a)
                        || (osc && byte == 0x07)
                        || (escape && byte == b'\\')
                    {
                        State::Ground
                    } else {
                        State::String {
                            osc,
                            escape: byte == 0x1b,
                        }
                    };
                }
            }
        }
        out
    }

    fn passthrough_csi(&mut self, byte: u8, out: &mut Vec<u8>) {
        out.push(byte);
        self.state = match byte {
            0x1b => State::Escape,
            0x18 | 0x1a | 0x40..=0x7e => State::Ground,
            _ => State::CsiPassthrough,
        };
    }

    fn emit_body(&self, final_byte: u8, out: &mut Vec<u8>) {
        let body = &self.pending;
        if !matches!(final_byte, b'h' | b'l')
            || body.first() != Some(&b'?')
            || !body[1..].iter().all(|b| b.is_ascii_digit() || *b == b';')
        {
            out.extend_from_slice(body);
            return;
        }

        out.push(b'?');
        for (i, param) in body[1..].split(|b| *b == b';').enumerate() {
            if i > 0 {
                out.push(b';');
            }
            // Ignore leading zeroes without integer parsing or overflow.
            let significant = param.iter().position(|b| *b != b'0').unwrap_or(param.len());
            out.extend_from_slice(match &param[significant..] {
                b"47" => b"1049",
                b"9" => b"1000",
                _ => param,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_stream(input: &[u8], expected: &[u8]) {
        for split in 0..=input.len() {
            let mut normalizer = LegacyCsiNormalizer::default();
            let mut output = normalizer.feed(&input[..split]);
            output.extend(normalizer.feed(&input[split..]));
            assert_eq!(output, expected, "split {split}");
        }
        for chunk_size in 1..=input.len().max(1) {
            let mut normalizer = LegacyCsiNormalizer::default();
            let output: Vec<u8> = input
                .chunks(chunk_size)
                .flat_map(|b| normalizer.feed(b))
                .collect();
            assert_eq!(output, expected, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn translates_set_reset_and_multiple_parameters_at_every_boundary() {
        assert_stream(
            b"MAIN\x1b[?47hALT\x1b[?47l\x1b[?9h\x1b[?9l",
            b"MAIN\x1b[?1049hALT\x1b[?1049l\x1b[?1000h\x1b[?1000l",
        );
        assert_stream(
            b"\x1b[?25;047;;0009;1049;1006h\x1b[?9;47;0l",
            b"\x1b[?25;1049;;1000;1049;1006h\x1b[?1000;1049;0l",
        );
    }

    #[test]
    fn preserves_unrelated_and_non_plain_csi() {
        let bytes =
            b"\x1b[47h\x1b[?47m\x1b[>47h\x1b[??47h\x1b[?47:9h\x1b[?47$h\x1b[?947;49;1047h\x1b[?h";
        assert_stream(bytes, bytes);
        let utf8 = "é你👩\u{200d}💻\u{009b}[?47h\x1b[?47h";
        let expected = "é你👩\u{200d}💻\u{009b}[?47h\x1b[?1049h";
        assert_stream(utf8.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn does_not_rewrite_osc_dcs_apc_or_other_control_strings() {
        for start in [b']', b'P', b'_', b'^', b'X'] {
            let mut input = vec![0x1b, start];
            input.extend_from_slice(b"payload\x1b[?47;9h\x1b[?9l\x1b\\");
            let mut expected = input.clone();
            input.extend_from_slice(b"\x1b[?47h");
            expected.extend_from_slice(b"\x1b[?1049h");
            assert_stream(&input, &expected);
        }
        assert_stream(
            b"\x1b]2;title\x1b[?47h\x07\x1b[?9h",
            b"\x1b]2;title\x1b[?47h\x07\x1b[?1000h",
        );
        // BEL terminates OSC, but must not end DCS/APC/SOS/PM protection.
        for start in [b'P', b'_', b'^', b'X'] {
            let mut input = vec![0x1b, start];
            input.extend_from_slice(b"data\x07\x1b[?47h\x1b\\");
            assert_stream(&input, &input);
        }
    }

    #[test]
    fn cancellation_and_new_escape_recover_without_losing_bytes() {
        assert_stream(
            b"\x1b[?47\x18\x1b[?9h\x1b[?47\x1a\x1b[?47l\x1b[?47\x1b[?9l",
            b"\x1b[?47\x18\x1b[?1000h\x1b[?47\x1a\x1b[?1049l\x1b[?47\x1b[?1000l",
        );
        for cancel in [0x18, 0x1a] {
            let mut input = b"\x1b_payload".to_vec();
            input.push(cancel);
            let mut expected = input.clone();
            input.extend_from_slice(b"\x1b[?9h");
            expected.extend_from_slice(b"\x1b[?1000h");
            assert_stream(&input, &expected);
        }
        // Controls within a CSI are preserved in order, without translation.
        assert_stream(b"\x1b[?4\x079h\x1b[?9h", b"\x1b[?4\x079h\x1b[?1000h");
        assert_stream(b"\x1b\x07[?47h", b"\x1b\x07[?1049h");
    }

    #[test]
    fn incomplete_csi_is_retained_but_empty_input_does_not_flush_it() {
        let mut normalizer = LegacyCsiNormalizer::default();
        assert_eq!(normalizer.feed(b"text\x1b[?4"), b"text\x1b[");
        assert!(normalizer.feed(b"").is_empty());
        assert_eq!(normalizer.feed(b"7h"), b"?1049h");
        assert!(normalizer.pending.is_empty());
    }

    #[test]
    fn oversized_csi_passes_through_and_memory_stays_bounded() {
        let mut input = b"\x1b[?".to_vec();
        input.extend(std::iter::repeat_n(b'0', MAX_CSI_BYTES * 4));
        input.extend_from_slice(b"47h");
        let mut expected = input.clone();
        input.extend_from_slice(b"\x1b[?47;9l");
        expected.extend_from_slice(b"\x1b[?1049;1000l");

        let mut normalizer = LegacyCsiNormalizer::default();
        let mut output = Vec::new();
        for byte in input {
            output.extend(normalizer.feed(&[byte]));
            assert!(normalizer.pending.len() <= MAX_CSI_BYTES);
            assert!(normalizer.pending.capacity() <= MAX_CSI_BYTES);
        }
        assert_eq!(output, expected);
        assert!(normalizer.pending.is_empty());
    }

    #[test]
    fn exact_buffer_limit_translates_and_overflow_recovers_at_escape() {
        let mut at_limit = b"\x1b[?".to_vec();
        at_limit.extend(std::iter::repeat_n(b'0', MAX_CSI_BYTES - 3));
        at_limit.extend_from_slice(b"47h");
        assert_stream(&at_limit, b"\x1b[?1049h");

        let mut overflow = b"\x1b[?".to_vec();
        overflow.extend(std::iter::repeat_n(b'0', MAX_CSI_BYTES));
        let mut expected = overflow.clone();
        overflow.extend_from_slice(b"\x1b[?9h");
        expected.extend_from_slice(b"\x1b[?1000h");
        assert_stream(&overflow, &expected);
    }

    #[test]
    fn huge_control_strings_are_not_buffered() {
        let mut normalizer = LegacyCsiNormalizer::default();
        assert_eq!(normalizer.feed(b"\x1b_"), b"\x1b_");
        let payload = vec![b'a'; 32 * 1024];
        assert_eq!(normalizer.feed(&payload), payload);
        assert!(normalizer.pending.is_empty());
        assert_eq!(normalizer.feed(b"\x1b\\\x1b[?9h"), b"\x1b\\\x1b[?1000h");
    }
}
