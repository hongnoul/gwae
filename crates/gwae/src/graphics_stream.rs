//! Ordered, bounded extraction of Kitty graphics APCs from one pane's output.
//!
//! Only a literal `ESC _ G` introducer is intercepted. Everything else is
//! returned as text, including incomplete UTF-8, CSI, and opaque control
//! strings. The terminal parser, not this splitter, interprets those bytes.
//!
//! Consumers must feed both `Text` and complete `Apc` bytes to their terminal
//! parser in event order. The parser swallows APCs, but still needs their ESC
//! cancellation/ST transitions if an earlier CSI or control string was cut
//! short. Handle a graphics command after feeding its APC, before feeding the
//! next event. An `Oversized` event drops the packet instead: feed CAN to the
//! terminal parser to cancel any preceding partial sequence, never fake an ACK.

/// Maximum complete graphics APC length, including its five framing bytes.
pub const MAX_APC_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Ordinary bytes, unchanged and in order. May end inside UTF-8 or CSI.
    Text(Vec<u8>),
    /// One complete `ESC _ G ... ESC \\` packet, including query (`a=q`) packets.
    Apc(Vec<u8>),
    /// One packet exceeded the limit. No payload or success reply is supplied.
    /// Its remaining bytes are discarded through ST, or until CAN/SUB cancels
    /// it. Embedded escape sequences do not escape this discard mode.
    Oversized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opaque {
    Osc,
    Dcs,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    // true: one literal ESC is buffered. false: ESC plus an intervening C0
    // control/DEL was already emitted, so no literal ESC_G match is possible.
    Escape(bool),
    EscapeIntermediate,
    Csi,
    Opaque(Opaque),
    // Exactly ESC_ is buffered; the next byte decides graphics versus ordinary APC.
    ApcStart,
    Graphics,
    GraphicsEscape,
    Discard,
    DiscardEscape,
}

/// A streaming splitter belonging to exactly one pane. Retained storage never
/// exceeds 64 KiB plus the finite parser state; non-graphics strings are never
/// buffered. Returned owned events can of course be as large as the input text.
#[derive(Debug, Default)]
pub struct Stream {
    state: State,
    apc: Vec<u8>,
}

impl Stream {
    pub fn feed(&mut self, input: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        let mut text = Vec::new();
        for &byte in input {
            // A malformed APC's pending ESC starts a new escape sequence. The
            // same following byte must be interpreted again in Escape state.
            loop {
                match self.state {
                    State::Ground => {
                        if byte == 0x1b {
                            self.state = State::Escape(true);
                        } else {
                            text.push(byte);
                        }
                    }
                    State::Escape(buffered) => {
                        if byte == b'_' && buffered {
                            self.state = State::ApcStart;
                            break;
                        }
                        if buffered {
                            text.push(0x1b);
                        }
                        if byte == 0x1b {
                            self.state = State::Escape(true);
                            break;
                        }
                        text.push(byte);
                        self.state = match byte {
                            0x18 | 0x1a => State::Ground,
                            0x20..=0x2f => State::EscapeIntermediate,
                            b'[' => State::Csi,
                            b']' => State::Opaque(Opaque::Osc),
                            b'P' => State::Opaque(Opaque::Dcs),
                            b'X' | b'^' | b'_' => State::Opaque(Opaque::Other),
                            0x30..=0x7e => State::Ground,
                            _ => State::Escape(false),
                        };
                    }
                    State::EscapeIntermediate | State::Csi => {
                        if byte == 0x1b {
                            self.state = State::Escape(true);
                        } else {
                            text.push(byte);
                            let final_byte = if self.state == State::Csi {
                                (0x40..=0x7e).contains(&byte)
                            } else {
                                (0x30..=0x7e).contains(&byte)
                            };
                            if matches!(byte, 0x18 | 0x1a) || final_byte {
                                self.state = State::Ground;
                            }
                        }
                    }
                    State::Opaque(kind) => {
                        if byte == 0x1b {
                            // VT ESC cancels OSC/DCS/APC and starts a fresh
                            // escape. It is not opaque payload until a later ST.
                            self.state = State::Escape(true);
                        } else {
                            text.push(byte);
                            if matches!(byte, 0x18 | 0x1a) || (kind == Opaque::Osc && byte == 0x07)
                            {
                                self.state = State::Ground;
                            }
                        }
                    }
                    State::ApcStart => {
                        if byte == b'G' {
                            // Start at a power of two so Vec's geometric growth
                            // cannot overshoot the 64 KiB retained-storage cap.
                            if self.apc.capacity() == 0 {
                                self.apc = Vec::with_capacity(256);
                            }
                            self.apc.extend_from_slice(b"\x1b_G");
                            self.state = State::Graphics;
                        } else {
                            text.extend_from_slice(b"\x1b_");
                            self.state = State::Opaque(Opaque::Other);
                            // Includes ESC, CAN and SUB: apply their normal VT
                            // cancellation semantics rather than swallowing them.
                            continue;
                        }
                    }
                    State::Graphics => {
                        if matches!(byte, 0x18 | 0x1a) {
                            self.reject_into(&mut text);
                            text.push(byte);
                            self.state = State::Ground;
                        } else if self.append_graphics(byte, &mut text, &mut events) {
                            if byte == 0x1b {
                                self.state = State::GraphicsEscape;
                            }
                        } else {
                            self.state = if byte == 0x1b {
                                State::DiscardEscape
                            } else {
                                State::Discard
                            };
                        }
                    }
                    State::GraphicsEscape => {
                        if byte == b'\\' {
                            if self.append_graphics(byte, &mut text, &mut events) {
                                flush_text(&mut text, &mut events);
                                events.push(Event::Apc(std::mem::take(&mut self.apc)));
                            }
                            // ST is consumed even if its final byte exceeded
                            // the size limit. The following byte is ordinary.
                            self.state = State::Ground;
                        } else {
                            // This was not ST. Reject the old packet without an
                            // ACK, but preserve its bytes as ordinary terminal
                            // input. Reprocess the ESC and current byte, so a
                            // following CSI/OSC/new APC is not lost or corrupted.
                            let escape = self.apc.pop();
                            debug_assert_eq!(escape, Some(0x1b));
                            self.reject_into(&mut text);
                            self.state = State::Escape(true);
                            continue;
                        }
                    }
                    State::Discard => match byte {
                        0x1b => self.state = State::DiscardEscape,
                        0x18 | 0x1a => {
                            text.push(byte);
                            self.state = State::Ground;
                        }
                        _ => (),
                    },
                    State::DiscardEscape => match byte {
                        b'\\' => self.state = State::Ground,
                        0x1b => (),
                        0x18 | 0x1a => {
                            text.push(byte);
                            self.state = State::Ground;
                        }
                        _ => self.state = State::Discard,
                    },
                }
                break;
            }
        }
        flush_text(&mut text, &mut events);
        events
    }

    fn reject_into(&mut self, text: &mut Vec<u8>) {
        text.append(&mut self.apc);
    }

    fn append_graphics(&mut self, byte: u8, text: &mut Vec<u8>, events: &mut Vec<Event>) -> bool {
        if self.apc.len() == MAX_APC_BYTES {
            self.apc.clear();
            flush_text(text, events);
            events.push(Event::Oversized);
            false
        } else {
            self.apc.push(byte);
            true
        }
    }
}

fn flush_text(text: &mut Vec<u8>, events: &mut Vec<Event>) {
    if !text.is_empty() {
        events.push(Event::Text(std::mem::take(text)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> Event {
        Event::Text(bytes.to_vec())
    }

    fn apc(bytes: &[u8]) -> Event {
        Event::Apc(bytes.to_vec())
    }

    // Chunking can split Text events, never change their bytes or reorder an APC.
    fn normalize(events: impl IntoIterator<Item = Event>) -> Vec<Event> {
        let mut out = Vec::new();
        for event in events {
            if let Event::Text(bytes) = &event {
                assert!(!bytes.is_empty(), "never emit an empty Text event");
                if let Some(Event::Text(previous)) = out.last_mut() {
                    previous.extend_from_slice(bytes);
                    continue;
                }
            }
            out.push(event);
        }
        out
    }

    fn assert_all_splits(input: &[u8], expected: Vec<Event>) {
        for split in 0..=input.len() {
            let mut stream = Stream::default();
            let mut events = stream.feed(&input[..split]);
            assert!(stream.feed(b"").is_empty());
            events.extend(stream.feed(&input[split..]));
            assert_eq!(normalize(events), expected, "split at byte {split}");
            assert!(stream.apc.len() <= MAX_APC_BYTES);
            assert!(stream.apc.capacity() <= MAX_APC_BYTES);
        }
        let mut stream = Stream::default();
        let events = input.iter().flat_map(|byte| stream.feed(&[*byte]));
        assert_eq!(normalize(events), expected, "one-byte fragmentation");
    }

    #[test]
    fn mixed_text_cursor_movement_queries_and_graphics_keep_exact_order_at_every_split() {
        let before = "日本語 e\u{301}\x1b[2;7H".as_bytes();
        let query = b"\x1b_Gi=31,a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\";
        let between = b"\x1b[6n\x1b[14t\r\n";
        let image = b"\x1b_Ga=T,i=32,m=0;YWJj\x1b\\";
        let after = "끝\x1b[18t".as_bytes();
        let input = [before, query, between, image, after].concat();
        assert_all_splits(
            &input,
            vec![
                text(before),
                apc(query),
                text(between),
                apc(image),
                text(after),
            ],
        );
    }

    #[test]
    fn adjacent_graphics_and_query_actions_are_never_dropped_or_coalesced() {
        let one = b"\x1b_Ga=q,i=1;\x1b\\";
        let two = b"\x1b_Ga=q,i=2;\x1b\\";
        assert_all_splits(&[&one[..], &two[..]].concat(), vec![apc(one), apc(two)]);
    }

    #[test]
    fn mixed_control_streams_are_lossless_and_chunk_invariant() {
        let tokens: &[&[u8]] = &[
            b"ordinary",
            "Ü日本語".as_bytes(),
            b"\x1b",
            b"_",
            b"G",
            b"\x1b[3;4H\x1b[6n",
            b"\x1b_Ga=q,i=1;AAAA\x1b\\",
            b"\x1b_Gbroken\x1b[18t",
            b"\x1b]0;title_G\x07",
            b"\x1bPqopaque\x1b\\",
            b"\x1b_non-graphics\x07",
            b"\x1b(\x00_G",
            b"\x18",
            b"\x1a",
            b"\x00\x7f\xff\x9c",
        ];
        for case in 0..128u64 {
            let mut random = case + 1;
            let mut next = || {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                (random >> 32) as usize
            };
            let mut input = Vec::new();
            for _ in 0..64 {
                input.extend_from_slice(tokens[next() % tokens.len()]);
            }
            // Resolve any retained prefix or incomplete APC, without inventing
            // a special EOF behavior absent from the public streaming API.
            input.extend_from_slice(b"\x1b\\");
            let expected = normalize(Stream::default().feed(&input));
            let recovered: Vec<u8> = expected
                .iter()
                .flat_map(|event| match event {
                    Event::Text(bytes) | Event::Apc(bytes) => bytes.iter().copied(),
                    Event::Oversized => panic!("short fixture cannot overflow"),
                })
                .collect();
            assert_eq!(recovered, input, "byte preservation for case {case}");

            let mut stream = Stream::default();
            let mut events = Vec::new();
            let mut offset = 0;
            while offset < input.len() {
                let end = (offset + next() % 17 + 1).min(input.len());
                events.extend(stream.feed(&input[offset..end]));
                offset = end;
            }
            assert_eq!(normalize(events), expected, "chunking for case {case}");
        }
    }

    #[test]
    fn ordinary_utf8_csi_and_opaque_strings_are_byte_exact() {
        let input = concat!(
            "é日本語끝\x1b[?2004h\x1b[3;4H\x1b[6n",
            "\x1b]0;title_Ga=q,i=1;AAAA\x07",
            "\x1bP1;2qopaque_Ga=q\x07still-dcs\x1b\\",
            "\x1b_not-graphics_Ga=q\x07still-apc\x1b\\",
            "\x1b^private_Ga=q\x1b\\\x1bXsos_Ga=q\x1b\\",
            "\x1b(_Ga=q\x1b\\\x1b\x00_Ga=q\x1b\\END"
        );
        assert_all_splits(input.as_bytes(), vec![text(input.as_bytes())]);
    }

    #[test]
    fn non_graphics_apc_streams_immediately_without_retaining_its_payload() {
        let mut stream = Stream::default();
        assert_eq!(stream.feed(b"\x1b_hello"), vec![text(b"\x1b_hello")]);
        let block = vec![b'x'; 128 * 1024];
        for _ in 0..8 {
            assert_eq!(stream.feed(&block), vec![text(&block)]);
            assert!(stream.apc.is_empty());
            assert_eq!(stream.apc.capacity(), 0);
        }
        assert_eq!(stream.feed(b"\x1b\\done"), vec![text(b"\x1b\\done")]);
    }

    #[test]
    fn only_potential_prefix_and_graphics_are_retained_between_calls() {
        let mut stream = Stream::default();
        assert_eq!(stream.feed(b"before\x1b"), vec![text(b"before")]);
        assert!(stream.feed(b"_").is_empty());
        assert!(stream.feed(b"G").is_empty());
        assert!(stream.feed(b"a=q,i=1;").is_empty());
        assert!(stream.feed(b"\x1b").is_empty());
        assert_eq!(
            stream.feed(b"\\after"),
            vec![apc(b"\x1b_Ga=q,i=1;\x1b\\"), text(b"after")]
        );
        assert_eq!(stream.state, State::Ground);
        assert!(stream.apc.is_empty());
    }

    #[test]
    fn false_prefixes_and_repeated_escapes_are_preserved() {
        let input = b"a\x1b[4;8H\x1b_X\x1b\\b\x1b\x1b_Ga=q;\x1b\\c";
        assert_all_splits(
            input,
            vec![
                text(b"a\x1b[4;8H\x1b_X\x1b\\b\x1b"),
                apc(b"\x1b_Ga=q;\x1b\\"),
                text(b"c"),
            ],
        );
    }

    #[test]
    fn escape_cancels_opaque_strings_and_can_start_a_real_graphics_command() {
        let packet = b"\x1b_Ga=q,i=7;\x1b\\";
        for prefix in [
            &b"\x1b]0;title"[..],
            &b"\x1bPqdata"[..],
            &b"\x1b_non-graphics"[..],
            &b"\x1b^private"[..],
            &b"\x1bXsos"[..],
            &b"\x1b[12;"[..],
            &b"\x1b("[..],
        ] {
            let input = [prefix, packet, b"\x1b[6n"].concat();
            assert_all_splits(&input, vec![text(prefix), apc(packet), text(b"\x1b[6n")]);
        }
    }

    #[test]
    fn malformed_escape_rejects_packet_and_replays_the_new_escape_sequence() {
        for escape in [
            &b"\x1b[6n"[..],
            &b"\x1b]0;title\x07"[..],
            &b"\x1bPqdata\x1b\\"[..],
            &b"\x1b\x1b\\"[..],
        ] {
            let rejected = [&b"\x1b_Ga=q,i=7;BAD"[..], escape, b"tail"].concat();
            let good = b"\x1b_Ga=q,i=8;\x1b\\";
            assert_all_splits(
                &[&rejected[..], good].concat(),
                vec![text(&rejected), apc(good)],
            );
        }
    }

    #[test]
    fn malformed_escape_can_start_a_new_graphics_packet_without_acknowledging_old_one() {
        let rejected = b"\x1b_Ga=q,i=1;BAD";
        let good = b"\x1b_Ga=q,i=2;GOOD\x1b\\";
        assert_all_splits(
            &[&rejected[..], &good[..], b"tail"].concat(),
            vec![text(rejected), apc(good), text(b"tail")],
        );
    }

    #[test]
    fn can_and_sub_cancel_graphics_and_ordinary_strings_at_every_split() {
        let good = b"\x1b_Ga=q,i=8;\x1b\\";
        for cancel in [0x18, 0x1a] {
            for prefix in [
                &b"\x1b_Ga=q,i=7;BAD"[..],
                &b"\x1b_Ga=q,i=7;BAD\x1b"[..],
                &b"\x1b_"[..],
                &b"\x1b]title"[..],
                &b"\x1bPqdata"[..],
            ] {
                let rejected = [prefix, &[cancel], b"_Gnot-a-packet\x1b\\"].concat();
                assert_all_splits(
                    &[&rejected[..], good].concat(),
                    vec![text(&rejected), apc(good)],
                );
            }
        }
    }

    #[test]
    fn bel_and_utf8_payload_do_not_prematurely_end_graphics_apc() {
        let packet = "\x1b_Ga=q;é\x07日本語\x1b\\".as_bytes();
        // The protocol layer may reject this payload. The framing layer must
        // not treat BEL or a UTF-8 continuation byte as a graphics terminator.
        assert_all_splits(packet, vec![apc(packet)]);
    }

    #[test]
    fn exactly_64_kib_including_framing_is_accepted_with_bounded_capacity() {
        let mut packet = b"\x1b_G".to_vec();
        packet.resize(MAX_APC_BYTES - 2, b'A');
        packet.extend_from_slice(b"\x1b\\");
        let mut stream = Stream::default();
        let mut events = Vec::new();
        for chunk in packet.chunks(13) {
            events.extend(stream.feed(chunk));
            assert!(stream.apc.len() <= MAX_APC_BYTES);
            assert!(stream.apc.capacity() <= MAX_APC_BYTES);
        }
        assert_eq!(events, vec![apc(&packet)]);
        assert_eq!(stream.state, State::Ground);
    }

    #[test]
    fn one_byte_over_limit_rejects_even_when_overflow_is_final_st_byte() {
        let mut packet = b"\x1b_G".to_vec();
        packet.resize(MAX_APC_BYTES - 1, b'A');
        packet.extend_from_slice(b"\x1b\\tail");
        for split in [
            0,
            1,
            2,
            3,
            MAX_APC_BYTES - 1,
            MAX_APC_BYTES,
            MAX_APC_BYTES + 1,
        ] {
            let mut stream = Stream::default();
            let mut events = stream.feed(&packet[..split]);
            events.extend(stream.feed(&packet[split..]));
            assert_eq!(normalize(events), vec![Event::Oversized, text(b"tail")]);
            assert_eq!(stream.state, State::Ground);
            assert!(stream.apc.is_empty());
            assert!(stream.apc.capacity() <= MAX_APC_BYTES);
        }
    }

    #[test]
    fn oversized_payload_is_discarded_to_st_with_no_nested_query_or_ack() {
        let mut packet = b"before\x1b_G".to_vec();
        packet.resize(6 + MAX_APC_BYTES + 1, b'A');
        packet.extend_from_slice(b"\x1b[6n\x1b_Ga=q,i=999;\x1b\\tail");
        let good = b"\x1b_Ga=q,i=1;\x1b\\";
        packet.extend_from_slice(good);
        let expected = vec![text(b"before"), Event::Oversized, text(b"tail"), apc(good)];
        for chunk_size in [1, 2, 7, 4096, MAX_APC_BYTES] {
            let mut stream = Stream::default();
            let events = packet
                .chunks(chunk_size)
                .flat_map(|chunk| stream.feed(chunk));
            assert_eq!(normalize(events), expected, "chunks of {chunk_size}");
        }
    }

    #[test]
    fn unterminated_attack_cannot_grow_retained_memory_or_repeat_rejection_events() {
        let mut stream = Stream::default();
        assert!(stream.feed(b"\x1b_G").is_empty());
        let block = vec![b'A'; 4096];
        let mut events = Vec::new();
        for _ in 0..1024 {
            events.extend(stream.feed(&block));
            assert!(stream.apc.len() <= MAX_APC_BYTES);
            assert!(stream.apc.capacity() <= MAX_APC_BYTES);
        }
        assert_eq!(events, vec![Event::Oversized]);
        assert_eq!(stream.state, State::Discard);
        assert_eq!(stream.feed(b"\x1b"), vec![]);
        assert_eq!(stream.feed(b"\\text"), vec![text(b"text")]);
    }

    #[test]
    fn can_and_sub_recover_even_from_oversized_discard_mode() {
        for cancel in [0x18, 0x1a] {
            for pending_escape in [false, true] {
                let mut stream = Stream::default();
                let mut oversized = b"\x1b_G".to_vec();
                oversized.resize(MAX_APC_BYTES + 1, b'A');
                assert_eq!(stream.feed(&oversized), vec![Event::Oversized]);
                if pending_escape {
                    assert!(stream.feed(b"\x1b").is_empty());
                }
                let packet = b"\x1b_Ga=q;\x1b\\";
                let remainder = [&[cancel][..], b"tail", packet].concat();
                assert_eq!(
                    stream.feed(&remainder),
                    vec![text(&[&[cancel][..], b"tail"].concat()), apc(packet)]
                );
            }
        }
    }
}
