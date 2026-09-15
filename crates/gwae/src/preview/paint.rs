//! Preview painter: grid mockup cells and boxes.

use super::encode::{bg, fg, run_length};
use super::prefs::{Prefs, RESET, H, H_MIN, W};
use crate::theme::Palette;
use gwae_layout::width::{Preset, Width};
use gwae_term::CColor;

fn paint(p: &Prefs, pal: &Palette, h: usize) -> Vec<Vec<Cell>> {
    let blank = Cell {
        ch: ' ',
        fg: pal.text,
        bg: pal.base,
    };
    let mut g = vec![vec![blank; W]; h];
    let cw = p.col_cells();
    // Where column 0 starts. `center_focus` is *the position of the focused
    // column*, so showing it is showing a different scroll offset: minimal
    // parks the focused column at the left edge, centered puts it mid-screen
    // with the previous column peeking in. Nothing else about the two modes
    // differs, and pretending otherwise would be inventing behavior.
    let start: isize = if p.centered {
        (W as isize - cw as isize) / 2
    } else {
        0
    };
    // Draw enough columns to cover the viewport in both directions, so a
    // centered focus has real neighbors instead of empty space.
    let first = -((start / cw as isize) + 1);
    let last = ((W as isize - start) / cw as isize) + 1;
    for i in first..=last {
        let x = start + i * cw as isize;
        // Column 0 is the focused one; panes 0..p.panes hold real shells.
        let live = i >= 0 && (i as usize) < p.panes;
        draw_box(&mut g, x, cw, i, live, i == 0, p, pal, h);
    }
    g
}

/// Draw one column box at `x` (which may hang off either edge).
#[allow(clippy::too_many_arguments)]
fn draw_box(
    g: &mut [Vec<Cell>],
    x: isize,
    cw: usize,
    idx: isize,
    live: bool,
    focused: bool,
    p: &Prefs,
    pal: &Palette,
    h: usize,
) {
    let bg_c = if live { pal.surface } else { pal.base };
    // The focused column wears the accent frame; everything else wears the
    // dim skeleton frame. That contrast is the single most useful thing a
    // gwae screenshot conveys, so the mockup leads with it.
    let frame_c = if focused { pal.accent } else { pal.overlay };
    let put = |g: &mut [Vec<Cell>], row: usize, col: isize, ch: char, fg: CColor, bg: CColor| {
        if row < h && col >= 0 && (col as usize) < W {
            g[row][col as usize] = Cell { ch, fg, bg };
        }
    };
    for r in 0..h {
        for c in 0..cw {
            let col = x + c as isize;
            let edge_t = r == 0;
            let edge_b = r == h - 1;
            let edge_l = c == 0;
            let edge_r = c == cw - 1;
            let ch = match (edge_t || edge_b, edge_l || edge_r) {
                (true, true) => match (edge_t, edge_l) {
                    (true, true) => '\u{250c}',
                    (true, false) => '\u{2510}',
                    (false, true) => '\u{2514}',
                    (false, false) => '\u{2518}',
                },
                (true, false) => '\u{2500}',
                (false, true) => '\u{2502}',
                (false, false) => ' ',
            };
            let is_edge = edge_t || edge_b || edge_l || edge_r;
            put(
                g,
                r,
                col,
                ch,
                if is_edge { frame_c } else { pal.text },
                if is_edge { pal.base } else { bg_c },
            );
        }
    }
    let inner = cw.saturating_sub(4);
    if inner < 3 {
        return;
    }
    let text_x = x + 2;
    if live {
        // A real pane: a status dot in the theme's own status colors, then a
        // couple of lines of plausible shell. Fake content, real palette.
        let (dot, dot_c) = if focused {
            ('\u{25cf}', pal.running)
        } else {
            ('\u{25cf}', pal.done)
        };
        put(g, 1, text_x, dot, dot_c, bg_c);
        let title = if idx == 0 { "agent" } else { "shell" };
        write_at(
            g,
            1,
            text_x + 2,
            title,
            pal.text,
            bg_c,
            inner.saturating_sub(2),
        );
        // Two sets of fake lines, because a clipped "$ cargo tes" reads as a
        // rendering bug in gwae rather than as a narrow column. The short
        // set fits the narrowest column the flow can produce.
        let wide = p.content as usize > cw;
        // Different panes run different things, because a mockup where every
        // column shows the same command undersells the one feature the whole
        // grid exists for: several things at once, side by side.
        let lines: &[&str] = match (wide, inner >= 16, idx == 0) {
            // A logical pane wider than its column: the long line keeps going
            // instead of wrapping, and the clip is the point being shown.
            (true, _, _) => &[
                "$ cargo test --workspace --all-features",
                "   41 passed in 3.2s, 0 failed",
                "$ \u{2588}",
            ],
            (false, true, true) => &["$ cargo test", "   41 passed", "$ \u{2588}"],
            (false, true, false) => &["$ git status", "   clean", "$ \u{2588}"],
            (false, false, true) => &["$ test", "   41 ok", "$ \u{2588}"],
            (false, false, false) => &["$ git st", "   clean", "$ \u{2588}"],
        };
        // Fewer rows means fewer fake lines. Dropped from the *middle*: the
        // command and the trailing prompt are what make the box read as a
        // live shell, while the output line in between is only texture.
        let room = h.saturating_sub(4).max(1);
        let short: Vec<&str> = if room >= lines.len() {
            lines.to_vec()
        } else if room == 1 {
            vec![lines[0]]
        } else {
            vec![lines[0], lines[lines.len() - 1]]
        };
        for (n, line) in short.iter().enumerate() {
            let clipped = line.chars().count() > inner;
            write_at(g, 3 + n, text_x, line, pal.text, bg_c, inner);
            // A marker in the frame color on the right edge: "there is more
            // over there, and \u{2325}+\u{2192} pans to it".
            if wide && clipped {
                put(
                    g,
                    3 + n,
                    x + cw as isize - 1,
                    '\u{203a}',
                    pal.accent,
                    pal.base,
                );
            }
        }
    } else {
        // An empty box: exactly the two things onboarding offers to put in
        // one, so answering "on" shows the thing appearing where it appears.
        let mid = (h / 2).max(1);
        if p.labels {
            let addr = format!("0.{}", idx.max(0));
            write_at(
                g,
                mid.saturating_sub(1),
                text_x,
                &addr,
                pal.label,
                bg_c,
                inner,
            );
        }
        if p.cowsay {
            // Clamped off the bottom frame: a hint that overwrites the box it
            // is inside would be showing the wrong thing about the setting.
            let row = if p.labels { mid + 1 } else { mid }.min(h.saturating_sub(2));
            // A hint clipped mid-word teaches nothing; the short form is a
            // real binding too, so a narrow column loses detail, not truth.
            let hint = if inner >= 12 {
                "\u{2325}+n new pane"
            } else {
                "\u{2325}+n"
            };
            write_at(g, row, text_x, hint, pal.overlay, bg_c, inner);
        }
    }
}

/// Write `s` at a position, clipped to `max` cells and to the viewport.
fn write_at(
    g: &mut [Vec<Cell>],
    row: usize,
    col: isize,
    s: &str,
    fg: CColor,
    bg: CColor,
    max: usize,
) {
    if row >= g.len() {
        return;
    }
    for (i, ch) in s.chars().take(max).enumerate() {
        let c = col + i as isize;
        if c >= 0 && (c as usize) < W {
            g[row][c as usize] = Cell { ch, fg, bg };
        }
    }
}

/// Emit a row, changing color only when it actually changes.
///
/// Run-length encoding the SGR codes is not micro-optimization: the preview is
/// repainted on *every keystroke*, and a naive per-cell reset would send ~10x
/// the bytes, which is visible as tearing over ssh.

#[derive(Clone, Copy)]
pub(super) struct Cell {
    pub(super) ch: char,
    pub(super) fg: CColor,
    pub(super) bg: CColor,
}

/// Render the mockup, framed and indented, ending in a newline.
///
/// `crlf` picks the line ending: onboarding runs in raw mode, where a bare
/// `\n` stair-steps down the screen, but `--print` and docs want plain text.
pub fn render(p: &Prefs, crlf: bool) -> String {
    render_h(p, H, crlf)
}

/// [`render`] at an explicit height, for terminals too short for the full one.
pub fn render_h(p: &Prefs, h: usize, crlf: bool) -> String {
    let h = h.clamp(3, 64);
    let pal = Palette::preset(&p.theme).unwrap_or_default();
    let grid = paint(p, &pal, h);
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut s = String::new();
    // A thin frame in the theme's own overlay color: it marks where the
    // terminal ends, which is the whole point of showing a column that is
    // wider than it.
    let edge = fg(pal.overlay);
    s.push_str(&format!(
        "  {edge}\u{256d}{}\u{256e}{RESET}{nl}",
        "\u{2500}".repeat(W)
    ));
    for row in &grid {
        s.push_str(&format!("  {edge}\u{2502}{RESET}"));
        s.push_str(&run_length(row));
        s.push_str(&format!("{edge}\u{2502}{RESET}{nl}"));
    }
    s.push_str(&format!(
        "  {edge}\u{2570}{}\u{256f}{RESET}{nl}",
        "\u{2500}".repeat(W)
    ));
    s
}

/// Paint the viewport into a `H` x `W` grid of cells. The heart of the module.

#[cfg(test)]
mod tests {
    use super::*;

    use super::*;

    /// The visible characters of a rendered preview, ANSI stripped.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut it = s.chars();
        while let Some(c) = it.next() {
            if c == '\x1b' {
                for c2 in it.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// The mockup stays rectangular and inside its frame at every height the
    /// adaptive path can pick, including the shortest.
    
    #[test]
    fn every_supported_height_renders_a_clean_box() {
        for h in H_MIN..=H {
            for p in [
                Prefs::default(),
                Prefs {
                    labels: true,
                    cowsay: true,
                    ..Default::default()
                },
                Prefs {
                    panes: 4,
                    width: Width::Preset(Preset::Full),
                    centered: true,
                    ..Default::default()
                },
            ] {
                let text = plain(&render_h(&p, h, false));
                let widths: Vec<usize> = text.lines().map(|l| l.chars().count()).collect();
                assert!(widths.iter().all(|w| *w == W + 4), "h={h}: {widths:?}");
                assert_eq!(text.lines().count(), h + 2, "h={h}");
                // The bottom frame of every box must still be intact.
                let last_box_row = text.lines().nth(h).unwrap();
                assert!(
                    last_box_row.contains('\u{2518}') || last_box_row.contains('\u{2500}'),
                    "h={h}: bottom frame overwritten: {last_box_row}"
                );
            }
        }
    }

    /// The size gate is what keeps the question on screen; it must be honest
    /// about the common terminal sizes.

    #[test]
    fn a_wide_logical_pane_shows_the_overflow_marker() {
        let p = Prefs {
            content: 120,
            ..Default::default()
        };
        assert!(plain(&render(&p, false)).contains('\u{203a}'));
        assert!(!plain(&render(&Prefs::default(), false)).contains('\u{203a}'));
    }
}

