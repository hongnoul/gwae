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
//! deterministic per screen position (the frame differ sees a still image,
//! never a repaint), palette-only color (`overlay` on `base`, so only the
//! focus ring ever signals focus), and tiny boxes degrade to blank.

use gwae_term::{CColor, Cell};

use super::Rect;

pub(crate) const SPRITE_W: usize = 12;
pub(crate) const SPRITE_H: usize = 12;
/// Cell rows one sprite occupies: two sprite pixels per half-block cell.
pub(crate) const SPRITE_CELL_H: u16 = (SPRITE_H / 2) as u16;

/// The pals, in box order. The box's strip-column index picks the sprite, so
/// a box always shows the same pal whatever the occupancy: filling a neighbour
/// never changes the art you are already looking at.
const SPRITES: [&[&str; SPRITE_H]; 8] =
    [&BOT, &INVADER, &GHOST, &CAT, &FROG, &CRAB, &SLIME, &DINO];

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

/// Paint one centered sprite inside `boxr` (frame included: the sprite is
/// inset from the ring like pane content is).
///
/// `ordinal` is the box's strip-column index, so the pal is stable across
/// fills. `fg` is the art ink (the skeleton `overlay`), `bg` the interior
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
    let art = SPRITES[ordinal % SPRITES.len()];
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
}
