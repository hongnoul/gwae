//! Retro stipple dither for empty placeholder boxes.
//!
//! Placeholder boxes used to carry cowsay hints, then key hints, then pixel
//! pals, then nothing at all. Hints drifted and cluttered; blank frames are
//! calm but read as broken on a big screen; pals read playful against the
//! retro functional chrome and need a big hole to land. This is the middle
//! ground: a sparse uniform `·` dot grid, like a CRT dither or a vacant
//! architectural lot.
//!
//! The rules that kept hints out still apply, so the dither obeys them:
//! no text (nothing to translate, nothing that drifts from the keybinds),
//! deterministic per absolute screen position (the frame differ sees a still
//! image, never a repaint; the texture even runs continuous across boxes,
//! like one wallpaper behind the frames), palette-only color (`overlay` on
//! `base`, so only the focus ring ever signals focus), and tiny boxes
//! degrade to blank.

use gwae_term::{CColor, Cell};

use super::Rect;

/// Minimum box size that carries the dither (frame included). Smaller boxes
/// stay blank, exactly like before.
const MIN_W: u16 = 12;
const MIN_H: u16 = 8;
/// Breathing room between the ring and the dots, in cells.
const MARGIN: u16 = 2;
/// Dot grid pitch: every 4th column, every 2nd row. About one cell in eight
/// carries ink: visible texture, calm at a glance.
const PITCH_X: u16 = 4;
const PHASE_X: u16 = 2;

/// Paint the stipple inside `boxr` (frame included: the dots are inset from
/// the ring like pane content is).
///
/// `ordinal` is accepted for call-site stability (the box's strip-column
/// index) but ignored: the pattern is a function of absolute screen position
/// so it is stable across fills and continuous across adjacent boxes.
/// `fg` is the art ink (the skeleton `overlay`), `bg` the interior fill it
/// blends into. No-op when the box is too small to hold dots with breathing
/// room.
pub(crate) fn paint(
    out: &mut [Cell],
    cols: u16,
    boxr: Rect,
    _ordinal: usize,
    fg: CColor,
    bg: CColor,
) {
    if boxr.w < MIN_W || boxr.h < MIN_H {
        return;
    }
    let x0 = boxr.x.saturating_add(MARGIN);
    let y0 = boxr.y.saturating_add(MARGIN);
    let x1 = boxr
        .x
        .saturating_add(boxr.w)
        .saturating_sub(MARGIN)
        .min(cols);
    let y1 = boxr.y.saturating_add(boxr.h).saturating_sub(MARGIN);
    for y in y0..y1 {
        if y % 2 != 0 {
            continue;
        }
        for x in x0..x1 {
            if x % PITCH_X != PHASE_X {
                continue;
            }
            let idx = y as usize * cols as usize + x as usize;
            if let Some(c) = out.get_mut(idx) {
                *c = Cell {
                    ch: '·',
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

    fn cell_at(out: &[Cell], cols: u16, x: u16, y: u16) -> &Cell {
        &out[y as usize * cols as usize + x as usize]
    }

    #[test]
    fn dots_land_on_the_grid_phase() {
        // Box at the origin: absolute phase is directly visible.
        let cols: u16 = 40;
        let boxr = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 12,
        };
        let mut out = vec![Cell::default(); cols as usize * 12];
        let fg = CColor::Rgb(0xff, 0xff, 0xff);
        let bg = CColor::Idx(235);
        paint(&mut out, cols, boxr, 0, fg, bg);
        // (2,2): x % 4 == 2, y % 2 == 0, inside the margin: ink.
        assert_eq!(cell_at(&out, cols, 2, 2).ch, '·');
        assert_eq!(cell_at(&out, cols, 2, 2).style.fg, fg);
        assert_eq!(cell_at(&out, cols, 2, 2).style.bg, bg);
        // Off-phase neighbours stay untouched.
        assert_eq!(cell_at(&out, cols, 3, 2).ch, Cell::default().ch);
        assert_eq!(cell_at(&out, cols, 2, 3).ch, Cell::default().ch);
        // The margin stays clear: (0,0) would be on-phase but is ring room.
        assert_eq!(cell_at(&out, cols, 0, 0).ch, Cell::default().ch);
    }

    #[test]
    fn tiny_boxes_stay_blank() {
        let cols: u16 = 40;
        let fg = CColor::Rgb(0xff, 0xff, 0xff);
        let bg = CColor::Idx(235);
        for (w, h) in [(MIN_W - 1, 24), (24, MIN_H - 1), (10, 6)] {
            let mut out = vec![Cell::default(); cols as usize * 24];
            paint(&mut out, cols, Rect { x: 5, y: 5, w, h }, 0, fg, bg);
            assert!(
                out.iter().all(|c| c.ch != '·'),
                "box {w}x{h} must stay blank"
            );
        }
    }

    #[test]
    fn paint_is_deterministic_and_ordinal_stable() {
        // Same box, different ordinals and separate buffers: identical art.
        // Filling a neighbour (which only changes the ordinal) never shifts
        // the dots you are already looking at.
        let cols: u16 = 40;
        let boxr = Rect {
            x: 4,
            y: 2,
            w: 24,
            h: 14,
        };
        let fg = CColor::Rgb(0xff, 0xff, 0xff);
        let bg = CColor::Idx(235);
        let mut a = vec![Cell::default(); cols as usize * 24];
        let mut b = vec![Cell::default(); cols as usize * 24];
        paint(&mut a, cols, boxr, 2, fg, bg);
        paint(&mut b, cols, boxr, 5, fg, bg);
        assert!(
            !a.iter().any(|c| c.ch == '·') == false,
            "box must carry dots"
        );
        assert_eq!(
            a.iter().map(|c| c.ch).collect::<Vec<_>>(),
            b.iter().map(|c| c.ch).collect::<Vec<_>>(),
            "stipple changed with ordinal"
        );
    }
}
