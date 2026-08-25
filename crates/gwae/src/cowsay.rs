//! A tiny, dependency-free cowsay used to fill empty placeholder boxes.
//!
//! The real `cowsay(1)` is deliberately *not* used: it isn't guaranteed to be
//! installed, shelling out once per empty box per repaint would be absurd, and
//! its fixed 40-column bubble would have to be re-wrapped to the box width
//! anyway. The width-aware wrapping is the only real work here, and no external
//! tool knows gwae's pane geometry.
//!
//! Everything in this module is pure `&str` -> `Vec<String>`, so it is unit
//! testable without a terminal, and the caller ([`crate::tui`]) only has to
//! decide where to paint the result.

/// The cow itself, below the speech bubble. The leading `\` characters are the
/// tail connecting up to the bubble; the caller pads these lines like any
/// other, so the whole block stays left-aligned as one unit.
const COW: [&str; 5] = [
    r"   \   ^__^",
    r"    \  (oo)\_______",
    r"       (__)\       )\/\",
    r"           ||----w |",
    r"           ||     ||",
];

/// Width of the widest cow line, in cells. The art is pure ASCII, so
/// `chars().count()` is the display width. Only used to hold [`MIN_WIDTH`]
/// honest in tests; the renderer sizes itself from the returned block.
#[cfg(test)]
pub fn cow_width() -> u16 {
    COW.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16
}

/// The narrowest useful total width. The cow art is a fixed 23 cells wide and
/// cannot be wrapped, so anything narrower would clip its rump off; the bubble
/// above it shrinks to fit but the cow sets the floor. Checked against the art
/// by `min_width_actually_fits_the_cow`.
pub const MIN_WIDTH: u16 = 23;

/// Render `text` as a cowsay block that fits within `max_w` columns.
///
/// Returns the lines of the block, left-aligned and *not* padded to equal
/// length (the caller centers each line, or pads as it prefers). Returns an
/// empty vec when `max_w` is too narrow to draw anything legible, so callers
/// can treat "no room" and "nothing to say" identically.
pub fn cow_frame(text: &str, max_w: u16) -> Vec<String> {
    if max_w < MIN_WIDTH {
        return Vec::new();
    }
    // The bubble costs 4 cells of chrome: "< " and " >".
    let inner_max = max_w.saturating_sub(4).max(1) as usize;
    let lines = wrap(text, inner_max);
    if lines.is_empty() {
        return Vec::new();
    }
    // The bubble is as wide as its widest line, never wider than needed, so
    // short messages get a snug bubble instead of one padded out to the box.
    let inner = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(1);

    let mut out = Vec::with_capacity(lines.len() + 2 + COW.len());
    out.push(format!(" {}", "_".repeat(inner + 2)));
    if lines.len() == 1 {
        out.push(format!("< {:w$} >", lines[0], w = inner));
    } else {
        // Multi-line bubbles use the classic `/ \` `| |` `\ /` side glyphs so
        // it still reads as one balloon rather than several one-line ones.
        for (i, l) in lines.iter().enumerate() {
            let (lb, rb) = if i == 0 {
                ('/', '\\')
            } else if i == lines.len() - 1 {
                ('\\', '/')
            } else {
                ('|', '|')
            };
            out.push(format!("{lb} {:w$} {rb}", l, w = inner));
        }
    }
    out.push(format!(" {}", "-".repeat(inner + 2)));
    for l in COW {
        out.push(l.to_string());
    }
    out
}

/// Greedy word wrap to `width` columns.
///
/// A word longer than `width` is hard-split rather than allowed to overflow,
/// because an overflowing line would be clipped by the box and break the
/// bubble's right edge. Input is treated as ASCII-ish: the art is only ever
/// drawn for short built-in hints, and any wide glyphs a user configures are
/// clipped by the caller instead of corrupting the layout.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                if chunk.chars().count() == width {
                    lines.push(std::mem::take(&mut chunk));
                }
                chunk.push(ch);
            }
            if !chunk.is_empty() {
                cur = chunk;
            }
            continue;
        }
        let clen = cur.chars().count();
        if cur.is_empty() {
            cur.push_str(word);
        } else if clen + 1 + wlen <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Pick one message for an empty placeholder box.
///
/// `ordinal` is the box's position in reading order across the whole skeleton
/// (`0` is the first empty box on screen). `pinned` marks that box: it always
/// gets `messages[0]`, which callers set to the cheat-sheet hint.
///
/// Which cell is `0` is a *dynamic* choice made by the caller, not a fixed
/// address. Two facts about the layout make every fixed address wrong:
///
/// * `1.1` is where the first real pane lives, so its placeholder is never
///   drawn and a hint pinned there would be seen by nobody.
/// * Placeholders are consumed left-to-right as panes appear, so `1.2` is the
///   very next box to disappear. In practice users look at `1.3`, `1.4` and
///   then whole strips of `n.1..4`.
///
/// Pinning "the first empty box, wherever it currently is" therefore keeps the
/// hint in the one place the eye actually lands - immediately right of the live
/// panes - for every layout, instead of only in the pristine startup grid.
///
/// It also handles a full strip for free. Only the focused strip is rendered,
/// so when `n.1..4` are all live panes there is no placeholder and no hint;
/// moving down to strip `n+1` (which niri-style semantics create empty past
/// the end) makes its first cell the first empty box, and the pin lands on
/// `n+1.1`. The ordinal is per-render and per-strip, so this needs no
/// special case.
///
/// The remaining boxes deal out the rest of the pool without repeats. A plain
/// hash would collide and print the same hint two boxes apart, which reads as a
/// bug on a screen showing eight boxes at once; instead the ordinal walks the
/// pool by a stride coprime with its length, so a screenful of boxes is a
/// permutation of distinct hints and only wraps once the pool is exhausted.
///
/// Deliberately deterministic, never a random draw: [`crate::tui::paint`] diffs
/// each frame against the last, so a message that changed between repaints
/// would redraw the box every frame and would make the golden end-to-end frame
/// tests non-deterministic.
pub fn message_for(messages: &[String], ordinal: usize, pinned: bool) -> Option<&str> {
    if messages.is_empty() {
        return None;
    }
    let msg = if pinned || messages.len() == 1 {
        &messages[0]
    } else {
        // The pinned hint owns index 0; every other box deals from the tail so
        // the cheat-sheet line is never duplicated elsewhere on screen.
        let pool = &messages[1..];
        let n = pool.len();
        // Largest stride below `n` that is coprime with it, so repeatedly
        // adding it visits every entry before repeating (a full cycle mod n):
        // adjacent boxes differ and the whole screen stays varied.
        let stride = (1..=n).rev().find(|k| gcd(*k, n) == 1).unwrap_or(1);
        // `ordinal - 1` because ordinal 0 is the pinned box.
        &pool[ordinal.saturating_sub(1).wrapping_mul(stride) % n]
    };
    let t = msg.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Greatest common divisor, for picking a stride coprime with the pool size.
fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_width_actually_fits_the_cow() {
        // The cow is fixed-width art and cannot wrap, so MIN_WIDTH must never
        // drop below it. An earlier value of 20 clipped the cow's rump.
        assert!(
            MIN_WIDTH >= cow_width(),
            "MIN_WIDTH {MIN_WIDTH} is narrower than the cow ({})",
            cow_width()
        );
    }

    #[test]
    fn narrow_boxes_get_nothing() {
        // Below the minimum the cow would be clipped into nonsense, so the
        // caller gets an empty block and paints just the big cell label.
        assert!(cow_frame("hello", MIN_WIDTH - 1).is_empty());
        assert!(!cow_frame("hello", MIN_WIDTH).is_empty());
    }

    #[test]
    fn empty_message_says_nothing() {
        assert!(cow_frame("", 40).is_empty());
        assert!(cow_frame("   ", 40).is_empty());
    }

    #[test]
    fn single_line_bubble_is_snug_and_aligned() {
        let f = cow_frame("hi", 40);
        assert_eq!(f[0], " ____");
        assert_eq!(f[1], "< hi >");
        assert_eq!(f[2], " ----");
        // The cow follows the bubble unchanged.
        assert_eq!(&f[3..], &COW[..]);
    }

    #[test]
    fn every_line_fits_the_requested_width() {
        // The whole point of wrapping: nothing may exceed the box, or the
        // bubble's right edge would be clipped off.
        for w in MIN_WIDTH..60 {
            let f = cow_frame("Alt-Enter opens a new column right here", w);
            for l in &f {
                assert!(
                    l.chars().count() <= w as usize,
                    "line {l:?} exceeds width {w}"
                );
            }
        }
    }

    #[test]
    fn multi_line_bubble_uses_balloon_sides_and_pads_flush() {
        let f = cow_frame("one two three four five six", 24);
        let top = f[0].chars().count();
        assert!(f.len() > 3 + COW.len(), "expected a wrapped bubble: {f:?}");
        // Bubble body lines are all the same width as the top/bottom bars, so
        // the balloon has straight edges.
        for l in &f[1..f.len() - 1 - COW.len()] {
            assert_eq!(l.chars().count(), top + 1, "ragged bubble edge: {l:?}");
        }
        assert!(f[1].starts_with('/') && f[1].ends_with('\\'));
        let last = f.len() - 1 - COW.len() - 1;
        assert!(f[last].starts_with('\\') && f[last].ends_with('/'));
    }

    #[test]
    fn overlong_word_is_split_not_overflowed() {
        let f = cow_frame("supercalifragilisticexpialidocious", MIN_WIDTH);
        for l in &f {
            assert!(l.chars().count() <= MIN_WIDTH as usize, "overflow: {l:?}");
        }
    }

    #[test]
    fn the_pinned_box_always_gets_the_cheat_sheet_hint() {
        let msgs: Vec<String> = (0..8).map(|i| format!("m{i}")).collect();
        assert_eq!(message_for(&msgs, 0, true), Some("m0"));
        // And no other box ever repeats it, however many are on screen.
        for o in 1..64 {
            assert_ne!(
                message_for(&msgs, o, false),
                Some("m0"),
                "box {o} duplicated the pinned hint"
            );
        }
    }

    #[test]
    fn message_choice_is_stable_and_varies_by_cell() {
        let msgs: Vec<String> = (0..8).map(|i| format!("m{i}")).collect();
        // Stable across calls: this is what keeps repaints diff-free.
        for o in 0..8 {
            assert_eq!(message_for(&msgs, o, o == 0), message_for(&msgs, o, o == 0));
        }
        // A screenful of boxes says a different thing in every box.
        let seen: std::collections::HashSet<_> =
            (0..8).map(|o| message_for(&msgs, o, o == 0)).collect();
        assert_eq!(seen.len(), 8, "boxes repeated a hint: {seen:?}");
    }

    #[test]
    fn every_hint_is_reachable_before_the_pool_wraps() {
        // The stride must be a full cycle, not merely collision-free for the
        // first few boxes: with 27 hints a tall layout shows a lot of them.
        for pool in 2..30usize {
            let msgs: Vec<String> = (0..=pool).map(|i| format!("m{i}")).collect();
            let seen: std::collections::HashSet<_> =
                (1..=pool).map(|o| message_for(&msgs, o, false)).collect();
            assert_eq!(seen.len(), pool, "pool of {pool} did not cycle fully");
        }
    }

    #[test]
    fn no_messages_means_no_cow() {
        assert_eq!(message_for(&[], 1, false), None);
        assert_eq!(message_for(&["  ".to_string()], 1, true), None);
    }
}
