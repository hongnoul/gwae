//! Pixel-art sprites for empty placeholder boxes.
//!
//! Placeholder boxes used to carry cowsay hints, then key hints, then nothing
//! at all. Hints drifted and cluttered; blank frames are calm but read as
//! broken on a big screen. These sprites are the middle ground: tiny
//! hand-authored monochrome creatures ("placeholder pals") painted with
//! half-block cells, so each cell holds two pixels and a 12x12 sprite costs
//! only 12x6 cells.
//!
//! The rules that kept hints out still apply, so the art obeys them:
//! no text (nothing to translate, nothing that drifts from the keybinds),
//! stable between swaps (the frame differ sees a still image, never a
//! repaint) with a fresh random lineup each launch and on every pane swap,
//! palette-only color
//! (`overlay` on `base`, so only the focus ring ever signals focus), and
//! tiny boxes degrade to blank.

use gwae_term::{CColor, Cell};

use super::Rect;

pub(crate) const SPRITE_W: usize = 12;
pub(crate) const SPRITE_H: usize = 12;
/// Cell rows one sprite occupies: two sprite pixels per half-block cell.
pub(crate) const SPRITE_CELL_H: u16 = (SPRITE_H / 2) as u16;

/// The pals, in box order. The lineup is shuffled on launch and redealt on
/// every pane swap (see [`reshuffle`]), so the empty boxes always open with
/// a surprise cast, but a box always shows the same pal whatever the
/// occupancy: filling a neighbour never changes the art you are already
/// looking at.
const SPRITES: [&[&str; SPRITE_H]; 8] = [&BOT, &INVADER, &GHOST, &CAT, &FROG, &CRAB, &SLIME, &DINO];

const GHOST: [&str; SPRITE_H] = [
    "....XXXX....",
    "..XXXXXXXX..",
    ".XXXXXXXXXX.",
    "XXXXXXXXXXXX",
    "XXXXXXXXXXXX",
    "XXX..XX..XXX",
    "XXX..XX..XXX",
    "XXXXXXXXXXXX",
    "XXXXX..XXXXX",
    "XXXXXXXXXXXX",
    "XXXXXXXXXXXX",
    ".XXX.XX.XXX.",
];

const INVADER: [&str; SPRITE_H] = [
    "..X......X..",
    "...X....X...",
    "..XXXXXXXX..",
    ".XX.XXXX.XX.",
    "XXXXXXXXXXXX",
    "XX.XXXXXX.XX",
    "XX.XXXXXX.XX",
    "XXXXXXXXXXXX",
    "...XX..XX...",
    "..XXX..XXX..",
    ".XXX....XXX.",
    "............",
];

const BOT: [&str; SPRITE_H] = [
    ".....XX.....",
    ".....XX.....",
    "....XXXX....",
    "..XXXXXXXX..",
    ".XXXXXXXXXX.",
    ".XX..XX..XX.",
    ".XXXXXXXXXX.",
    "..XXXXXXXX..",
    "..XXXXXXXX..",
    "...XXXXXX...",
    "..XXX..XXX..",
    "............",
];

const CAT: [&str; SPRITE_H] = [
    ".XX......XX.",
    ".XXX....XXX.",
    ".XXXXXXXXXX.",
    "XXXXXXXXXXXX",
    "XXXXXXXXXXXX",
    "XX..XXXX..XX",
    "XX..XXXX..XX",
    "XXXXX..XXXXX",
    ".XXXXXXXXXX.",
    "..XXXXXXXX..",
    "...X.XX.X...",
    "............",
];

const FROG: [&str; SPRITE_H] = [
    "..XX....XX..",
    ".XXXX..XXXX.",
    ".X..X..X..X.",
    "XXXXXXXXXXXX",
    "XXXXXXXXXXXX",
    "XX........XX",
    "XXXXXXXXXXXX",
    ".XXXXXXXXXX.",
    ".XXXXXXXXXX.",
    "..XXXXXXXX..",
    ".XX..XX..XX.",
    "............",
];

const CRAB: [&str; SPRITE_H] = [
    "X..........X",
    ".X.XX..XX.X.",
    "..XXXXXXXX..",
    ".XXXXXXXXXX.",
    "XXX..XX..XXX",
    "XXX..XX..XXX",
    "XXXXXXXXXXXX",
    ".XXXXXXXXXX.",
    "..XXXXXXXX..",
    ".X.XXXXXX.X.",
    "X..X.XX.X..X",
    "............",
];

const SLIME: [&str; SPRITE_H] = [
    "............",
    "............",
    "....XXXX....",
    "..XXXXXXXX..",
    ".XXXXXXXXXX.",
    ".XX..XX..XX.",
    "XXX..XX..XXX",
    "XXXXXXXXXXXX",
    "XXXX.XX.XXXX",
    "XXXXXXXXXXXX",
    ".XXXXXXXXXX.",
    "............",
];

const DINO: [&str; SPRITE_H] = [
    "......XXXXX.",
    "......XX.XX.",
    "......XXXXX.",
    "......XXX...",
    "X....XXXX...",
    "XX..XXXXXXX.",
    ".XXXXXXXX...",
    ".XXXXXXXX...",
    "..XXXXXX....",
    "...XXXXX....",
    "...XX..XX...",
    "............",
];

fn pixel(art: &[&str; SPRITE_H], x: usize, y: usize) -> bool {
    art.get(y)
        .and_then(|row| row.as_bytes().get(x))
        .is_some_and(|b| *b == b'X')
}

/// One shuffled lineup of the pals, drawn lazily on first paint. The seed
/// mixes the wall clock, the process id, and a monotonic salt, so every
/// launch opens with a surprise cast and every pane swap deals a fresh one,
/// while the frame differ still sees a still image between swaps. No rng
/// crate needed: a tiny xorshift shuffles the eight indices in place.
static ORDER: std::sync::OnceLock<std::sync::RwLock<[usize; 8]>> = std::sync::OnceLock::new();
static RESHUFFLE_SALT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fresh_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() << 32))
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let salt = RESHUFFLE_SALT.fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::SeqCst);
    let seed = nanos ^ ((std::process::id() as u64).wrapping_mul(0x9E37_79B9)) ^ salt;
    if seed == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        seed
    }
}

fn shuffle_into(order: &mut [usize; 8], mut seed: u64) {
    if seed == 0 {
        seed = 0x9E37_79B9_7F4A_7C15;
    }
    // Fisher-Yates with xorshift64.
    for i in (1..order.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        order.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

fn shuffled_order() -> [usize; 8] {
    *ORDER
        .get_or_init(|| {
            let mut order = [0usize, 1, 2, 3, 4, 5, 6, 7];
            shuffle_into(&mut order, fresh_seed());
            std::sync::RwLock::new(order)
        })
        .read()
        .unwrap_or_else(|e| e.into_inner())
}

/// Deal a fresh random lineup, so the next paint shows new pals. Called on
/// every pane swap (`Alt+Shift+hjkl`): the empty boxes reload to a surprise
/// cast instead of sitting frozen on the launch shuffle. Retries a few times
/// so a swap that redraws immediately never visibly keeps the old cast.
pub(crate) fn reshuffle() {
    let lock = ORDER.get_or_init(|| {
        let mut order = [0usize, 1, 2, 3, 4, 5, 6, 7];
        shuffle_into(&mut order, fresh_seed());
        std::sync::RwLock::new(order)
    });
    let mut guard = lock.write().unwrap_or_else(|e| e.into_inner());
    let prev = *guard;
    for _ in 0..4 {
        shuffle_into(&mut guard, fresh_seed());
        if *guard != prev {
            return;
        }
    }
}

/// Paint one centered sprite inside `boxr` (frame included: the sprite is
/// inset from the ring like pane content is).
///
/// `ordinal` is the box's strip-column index, so the pal is stable across
/// fills within a session; the lineup itself is shuffled once per launch.
/// `fg` is the art ink (the skeleton `overlay`), `bg` the interior
/// fill it blends into. No-op when the interior cannot hold the sprite with
/// a cell of breathing room: small boxes stay blank, exactly like before.
pub(crate) fn paint(
    out: &mut [Cell],
    cols: u16,
    boxr: Rect,
    ordinal: usize,
    fg: CColor,
    bg: CColor,
) {
    let cw = SPRITE_W as u16;
    if boxr.w < cw + 4 || boxr.h < SPRITE_CELL_H + 4 {
        return;
    }
    let order = shuffled_order();
    let art = SPRITES[order[ordinal % order.len()] % SPRITES.len()];
    let x0 = boxr.x + (boxr.w - cw) / 2;
    let y0 = boxr.y + (boxr.h - SPRITE_CELL_H) / 2;
    for cy in 0..SPRITE_CELL_H {
        for cx in 0..cw {
            let top = pixel(art, cx as usize, (cy * 2) as usize);
            let bot = pixel(art, cx as usize, (cy * 2 + 1) as usize);
            let glyph = match (top, bot) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => continue,
            };
            let x = x0 + cx;
            let y = y0 + cy;
            if x >= cols {
                continue;
            }
            let idx = y as usize * cols as usize + x as usize;
            if let Some(c) = out.get_mut(idx) {
                *c = Cell {
                    ch: glyph,
                    style: gwae_term::Style {
                        fg,
                        bg,
                        ..Default::default()
                    },
                    width: 1,
                    ..Default::default()
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sprite_row_is_twelve_pixels_wide() {
        for (i, art) in SPRITES.iter().enumerate() {
            assert_eq!(art.len(), SPRITE_H, "sprite {i} height");
            for (y, row) in art.iter().enumerate() {
                assert_eq!(row.len(), SPRITE_W, "sprite {i} row {y}: {row:?}");
            }
        }
    }

    #[test]
    fn every_pal_has_eyes_and_a_footprint() {
        // Eyes are holes (transparent pixels fully enclosed by ink); the
        // footprint is ink in the bottom half. This catches a sprite that
        // was edited into a blob or clipped to nothing.
        for (i, art) in SPRITES.iter().enumerate() {
            let ink = |x: usize, y: usize| pixel(art, x, y);
            // Eyes are holes with ink on both sides of the same row
            // (two-pixel-wide eyes count: each hole pixel sees ink left
            // and right, just not adjacent on both sides).
            let holes = (0..SPRITE_W).any(|x| {
                (0..SPRITE_H).any(|y| {
                    !ink(x, y)
                        && y > 0
                        && y + 1 < SPRITE_H
                        && (0..x).any(|lx| ink(lx, y))
                        && ((x + 1)..SPRITE_W).any(|rx| ink(rx, y))
                })
            });
            assert!(holes, "sprite {i} has no enclosed hole (eyes)");
            assert!(
                (0..SPRITE_W).any(|x| ink(x, SPRITE_H - 2) || ink(x, SPRITE_H - 3)),
                "sprite {i} has no footprint in the bottom rows"
            );
        }
    }

    #[test]
    fn shuffled_lineup_is_a_stable_permutation_between_swaps() {
        // The launch shuffle must cover every pal exactly once, and the
        // order must stay identical across paints until a swap redeals it
        // (the frame differ sees a still image between swaps).
        let a = shuffled_order();
        let b = shuffled_order();
        assert_eq!(a, b, "lineup changed between paints with no swap");
        let mut seen = [false; 8];
        for &i in a.iter() {
            assert!(i < SPRITES.len(), "lineup index {i} out of range");
            assert!(!seen[i], "lineup repeats pal {i}");
            seen[i] = true;
        }
        // A swap redeals the lineup: still a full permutation, but a fresh
        // cast rather than the launch one.
        reshuffle();
        let c = shuffled_order();
        let mut seen = [false; 8];
        for &i in c.iter() {
            assert!(i < SPRITES.len(), "lineup index {i} out of range");
            assert!(!seen[i], "lineup repeats pal {i}");
            seen[i] = true;
        }
        assert_ne!(c, a, "a swap must redeal the lineup");
        let d = shuffled_order();
        assert_eq!(c, d, "lineup changed with no swap after a reshuffle");
    }
}
