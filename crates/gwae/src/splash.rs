//! The animated title card onboarding opens with.
//!
//! Why this exists: `gwae init` used to drop the user straight onto
//! question 1/6 with no indication of *what* they had just started
//! configuring. A one-second title card costs nothing, names the tool, and -
//! because it is painted in the palette the very next question asks about -
//! doubles as the first honest preview of the theme system.
//!
//! Shape of the code, matching [`crate::onboard`]:
//!
//! * The animation is **one pure function** ([`frame`]): step index in, a
//!   screen's worth of text out. Every frame is therefore testable without a
//!   terminal, and `gwae init --print-splash` shows the real thing.
//! * The **player is thin** ([`play`]): sleep, draw, and abort the instant a
//!   key is pressed, so the card can never stand between a user and the
//!   questions.
//!
//! Rendering rules the animation deliberately respects, because these frames
//! also have to survive being run *inside* a gwae pane (a hosted vt100
//! grid) rather than only on a bare terminal:
//!
//! * every line ends `\r\n` (the flow runs in raw mode),
//! * each frame repaints from a cleared screen with absolute cursor homing,
//!   so a dropped or interleaved frame self-corrects on the next one,
//! * nothing is ever drawn past `cols`, so no line wraps and scrolls the art,
//! * the art is pure ASCII plus one block glyph, all width-1 in the grid,
//! * colors are SGR only (no cursor save/restore, no alt screen, no DECSET),
//!   which is exactly the subset a hosted pane replays faithfully.

use crate::theme::Palette;
use crossterm::event;
use gwae_term::CColor;
use std::io::Write;
use std::time::Duration;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
/// Clear and home, so each frame is absolute rather than differential.
const CLEAR: &str = "\x1b[2J\x1b[H";

/// The cell the art is drawn with. One column wide in every emulator (unlike
/// the half-blocks and braille that tempt ASCII art, `█` is unambiguous).
const INK: char = '\u{2588}';

/// 3x5 block-font glyphs for the letters in the wordmark, MSB-left per row.
/// Deliberately a private table of just these four letters: a general font
/// would be a much bigger promise than one word needs.
fn glyph(ch: char) -> [u8; 5] {
    match ch {
        'g' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'w' => [0b101, 0b101, 0b101, 0b111, 0b101],
        'a' => [0b111, 0b101, 0b111, 0b101, 0b101],
        'e' => [0b111, 0b100, 0b111, 0b100, 0b111],
        _ => [0; 5],
    }
}

/// The wordmark, in the order it is revealed.
const WORD: &str = "gwae";
/// Rows in the block font.
const ART_ROWS: usize = 5;
/// Columns per glyph plus the one-column gap after it.
const GLYPH_W: usize = 4;

/// Width of the rendered wordmark in cells (no trailing gap).
pub fn art_width() -> usize {
    WORD.chars().count() * GLYPH_W - 1
}

/// How many frames after the wipe completes the shimmer runs for.
const SHIMMER: usize = 10;

/// Total frames in the animation. The wipe reveals one column per frame, then
/// a highlight sweeps back across the finished word.
pub fn frames() -> usize {
    art_width() + SHIMMER
}

/// Delay between frames. `frames() * TICK` is about a second: long enough to
/// read the word, too short to be in anyone's way.
pub const TICK: Duration = Duration::from_millis(28);

/// The lit columns of the wordmark as a bitmap: `on[row][col]`.
fn bitmap() -> Vec<Vec<bool>> {
    let w = art_width();
    let mut rows = vec![vec![false; w]; ART_ROWS];
    for (i, ch) in WORD.chars().enumerate() {
        let g = glyph(ch);
        for (r, bits) in g.iter().enumerate() {
            for c in 0..3 {
                if bits & (1 << (2 - c)) != 0 {
                    rows[r][i * GLYPH_W + c] = true;
                }
            }
        }
    }
    rows
}

/// SGR foreground for a palette color, passing indexed/default through so a
/// `terminal` theme previews as the user's own ANSI rather than as guesses.
fn fg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[38;5;{i}m"),
        CColor::Default => "\x1b[39m".to_string(),
    }
}

/// One frame of the title card, `cols` wide. Pure.
///
/// `step` runs `0..frames()`; anything at or past the end renders the settled
/// final frame, so a caller that overruns cannot land on a blank screen.
///
/// The card degrades rather than breaks on a narrow terminal: below the art
/// width it falls back to the plain word, and nothing ever exceeds `cols`.
pub fn frame(step: usize, p: &Palette, cols: u16) -> String {
    let w = art_width();
    let cols = cols as usize;
    let tag = "scrolling panes for your agents";
    if cols < w + 2 {
        // No room for the art: still name the tool, still never wrap.
        let word: String = WORD.chars().take(cols).collect();
        return format!("{BOLD}{}{}{RESET}\r\n", fg(p.accent), word);
    }
    let pad = " ".repeat((cols - w) / 2);
    // The wipe head, and the shimmer that sweeps back once it lands.
    // Clamping here is what makes an overrun settle on the finished word
    // rather than on whatever the arithmetic would have produced.
    let step = step.min(frames() - 1);
    let head = step.min(w);
    // The wipe's leading edge only exists while the wipe is moving.
    let edge = step < w;
    // The shimmer sweeps across the finished word, then stops: the last frame
    // is the plain settled wordmark, which is also what is left on screen.
    let shine = step
        .checked_sub(w)
        .filter(|t| t + 1 < SHIMMER)
        .map(|t| (t * w) / SHIMMER.max(1));
    let on = bitmap();
    let mut s = String::from("\r\n");
    for row in on.iter() {
        s.push_str(&pad);
        let mut cur: Option<CColor> = None;
        for (c, lit) in row.iter().enumerate() {
            if !lit || c >= head {
                // Unrevealed columns are blank, not dim ink: the wipe should
                // look like the word arriving, not like it fading in.
                if cur.is_some() {
                    s.push_str(RESET);
                    cur = None;
                }
                s.push(' ');
                continue;
            }
            let color = if edge && c + 1 == head {
                // The leading edge of the wipe, brightest.
                p.done
            } else if shine.map(|x| c + 1 == x || c == x).unwrap_or(false) {
                p.running
            } else {
                p.accent
            };
            if cur != Some(color) {
                s.push_str(&fg(color));
                cur = Some(color);
            }
            s.push(INK);
        }
        s.push_str(RESET);
        s.push_str("\r\n");
    }
    // The tagline fades in only once the word is whole, so the eye reads one
    // thing at a time.
    s.push_str("\r\n");
    if step >= w && tag.chars().count() <= cols {
        let tpad = " ".repeat((cols - tag.chars().count()) / 2);
        s.push_str(&format!("{tpad}{DIM}{tag}{RESET}\r\n"));
    } else {
        s.push_str("\r\n");
    }
    s
}

/// Every frame, concatenated, for `gwae init --print-splash` and docs.
pub fn render_all(p: &Palette, cols: u16) -> String {
    (0..frames()).map(|i| frame(i, p, cols)).collect()
}

/// Play the card on stdout, returning early the moment a key is pressed.
///
/// Returns `false` if the user interrupted it, which the caller uses only to
/// decide whether to pause; the questions run either way. Assumes the caller
/// has already put the terminal in raw mode (onboarding does), so a keypress
/// is visible without waiting for Enter.
pub fn play(p: &Palette, cols: u16) -> bool {
    let mut out = std::io::stdout();
    for i in 0..frames() {
        let _ = out.write_all(CLEAR.as_bytes());
        let _ = out.write_all(frame(i, p, cols).as_bytes());
        let _ = out.flush();
        if matches!(event::poll(TICK), Ok(true)) {
            // Drain the impatient keystroke so it cannot also answer the
            // first question.
            let _ = event::read();
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip CSI sequences, leaving the glyphs that actually land on screen.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c != '\u{1b}' {
                out.push(c);
                continue;
            }
            if it.next() == Some('[') {
                for c in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
        }
        out
    }

    fn widths(s: &str) -> Vec<usize> {
        plain(s).split("\r\n").map(|l| l.chars().count()).collect()
    }

    #[test]
    fn the_last_frame_spells_the_word() {
        let p = Palette::default();
        let last = plain(&frame(frames() - 1, &p, 80));
        // Every glyph column of the wordmark is lit by the end, so the art
        // rows must contain ink and the tagline must have arrived.
        let inked: usize = last.chars().filter(|c| *c == INK).count();
        let expected: usize = bitmap().iter().flatten().filter(|b| **b).count();
        assert_eq!(inked, expected, "final frame is missing lit cells");
        assert!(last.contains("scrolling panes"), "tagline never appeared");
    }

    #[test]
    fn the_wipe_reveals_left_to_right_and_never_goes_backwards() {
        let p = Palette::default();
        let mut prev = 0usize;
        for i in 0..frames() {
            let n = plain(&frame(i, &p, 80))
                .chars()
                .filter(|c| *c == INK)
                .count();
            assert!(n >= prev, "frame {i} un-revealed cells ({n} < {prev})");
            prev = n;
        }
        assert_eq!(
            plain(&frame(0, &p, 80))
                .chars()
                .filter(|c| *c == INK)
                .count(),
            0,
            "frame 0 should start empty"
        );
    }

    #[test]
    fn overrunning_the_animation_settles_rather_than_blanks() {
        // A caller that keeps stepping (a slow loop, a resize redraw) must not
        // fall off the end into an empty screen.
        let p = Palette::default();
        assert_eq!(frame(frames() * 3, &p, 80), frame(frames() - 1, &p, 80));
    }

    #[test]
    fn no_line_ever_exceeds_the_terminal_width() {
        // A wrapped line scrolls the card and shears the art; in a hosted
        // gwae pane it would also corrupt the pane below.
        let p = Palette::default();
        for cols in [20u16, 40, 60, 80, 200] {
            for i in 0..frames() {
                for (n, w) in widths(&frame(i, &p, cols)).iter().enumerate() {
                    assert!(
                        *w <= cols as usize,
                        "cols={cols} frame={i} line {n} is {w} wide"
                    );
                }
            }
        }
    }

    #[test]
    fn a_narrow_terminal_still_names_the_tool() {
        let p = Palette::default();
        let s = plain(&frame(frames() - 1, &p, 10));
        assert!(s.contains("gwae"), "fallback lost the word: {s:?}");
    }

    #[test]
    fn every_line_is_crlf_terminated_for_raw_mode() {
        let p = Palette::default();
        let s = frame(3, &p, 80);
        assert!(!s.contains('\n') || s.matches('\n').count() == s.matches("\r\n").count());
    }

    #[test]
    fn frames_only_use_sgr_so_a_hosted_pane_can_replay_them() {
        // The card is also drawn inside a gwae pane. Cursor moves, alt
        // screen and DECSET modes would fight the host renderer; plain color
        // is the subset that always survives.
        let p = Palette::default();
        for i in 0..frames() {
            let f = frame(i, &p, 80);
            let mut it = f.chars().peekable();
            while let Some(c) = it.next() {
                if c != '\u{1b}' {
                    continue;
                }
                assert_eq!(it.next(), Some('['), "non-CSI escape in frame {i}");
                let mut seq = String::new();
                for c in it.by_ref() {
                    seq.push(c);
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
                assert!(
                    seq.ends_with('m'),
                    "frame {i} emits a non-SGR sequence: {seq:?}"
                );
            }
        }
    }

    #[test]
    fn the_card_is_painted_in_the_chosen_palette() {
        // The card previews the theme the next question asks about, so a
        // different preset must actually look different.
        let a = frame(frames() - 1, &Palette::preset("nord").unwrap(), 80);
        let b = frame(frames() - 1, &Palette::preset("gruvbox").unwrap(), 80);
        assert_ne!(a, b);
        assert_eq!(plain(&a), plain(&b), "only the colors should differ");
    }

    #[test]
    fn the_whole_card_stays_under_a_second_and_a_half() {
        // Onboarding is the thing the user asked for; the card is not.
        assert!(
            TICK * frames() as u32 <= Duration::from_millis(1500),
            "splash is too long: {:?}",
            TICK * frames() as u32
        );
    }
}
