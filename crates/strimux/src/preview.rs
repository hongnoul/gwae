//! A live mockup of the grid, drawn beside every onboarding question.
//!
//! Onboarding used to describe its layout answers in words: "four across",
//! "always re-center the focused column", "show which cell each empty box
//! is". Words are the wrong medium for a tool whose entire subject is *what
//! the screen looks like*. Every mature terminal configurator that survives
//! contact with real users solves this the same way - Zellij's theme editor
//! renders a whole fake workspace, Claude Code's `/theme` repaints its own
//! chrome as the highlight moves - because a preview turns a question about
//! vocabulary ("is a `two-thirds` column what I want?") into a question about
//! a picture.
//!
//! So this module renders a small, honest picture of the grid from the
//! answers *so far plus the option currently under the cursor*, and
//! [`crate::onboard`] repaints it on every keystroke.
//!
//! Two properties make it trustworthy rather than decorative:
//!
//! * It is **pure** ([`render`] over [`Prefs`]), so `strimux init --print`
//!   and the tests see exactly the bytes a user sees.
//! * It reads its colors from the **real** [`Palette`] presets and its widths
//!   from the **real** [`strimux_layout::width::Width`], so a preview can never
//!   drift from what the multiplexer actually paints. A mockup that lies is
//!   worse than no mockup, because it is believed.

use crate::theme::Palette;
use strimux_layout::width::{Preset, Width};
use strimux_term::CColor;

const RESET: &str = "\x1b[0m";

/// Interior width of the mocked viewport, in cells.
///
/// Narrow enough to sit under a question on an 80-column terminal without
/// wrapping (the frame and the left indent cost 4 more), wide enough that a
/// `quarter` column is still a recognizable box rather than a sliver.
pub const W: usize = 60;
/// Interior height of the mocked viewport at full size, in rows.
pub const H: usize = 9;
/// The shortest mockup still worth drawing: a frame, one line of pane content
/// and a bottom frame.
///
/// Below this the picture stops being a picture, so [`fits`] declines rather
/// than shipping a two-row smear that a user has to squint at.
pub const H_MIN: usize = 5;

/// The tallest mockup that fits in `rows` alongside a question needing
/// `chrome` rows, or `None` when even the shortest one would push the options
/// off the screen.
///
/// Adaptive rather than all-or-nothing because 80x24 is still the default
/// size of a great many terminals, and "no preview on the most common terminal
/// in the world" would make the feature a rumor.
pub fn fits(cols: u16, rows: u16, chrome: usize) -> Option<usize> {
    if (cols as usize) < W + 6 {
        return None;
    }
    // +2 for the mockup's own frame, +1 for the blank line above it.
    let spare = (rows as usize).checked_sub(chrome + 3)?;
    (spare >= H_MIN).then(|| spare.min(H))
}

/// The settings a preview can show: the subset of config that is *visible*.
///
/// Deliberately not the whole [`crate::config::Config`]. A preview that
/// accepted every key would imply it could show every key, and the honest
/// answer for `input_poll_ms` or `scroll_margin` is that a static picture
/// shows nothing at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Theme preset name, resolved through [`Palette::preset`].
    pub theme: String,
    /// Columns occupied by a real pane at launch; the rest are placeholders.
    pub panes: usize,
    /// Width of each column.
    pub width: Width,
    /// Whether focus re-centers, which is *where* the focused column sits.
    pub centered: bool,
    /// Whether empty boxes carry their `strip.pane` address.
    pub labels: bool,
    /// Whether empty boxes carry a keybinding hint.
    pub cowsay: bool,
    /// Logical pane width in cells; 0 means "follow the column".
    ///
    /// A pane wider than its column is the one setting whose *consequence* is
    /// off-screen, so the preview shows the consequence: content that runs
    /// past the right edge, marked, instead of wrapping.
    pub content: u16,
}

impl Default for Prefs {
    /// Exactly [`crate::config::Config::default`], so an untouched preview
    /// shows what an untouched strimux looks like.
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".to_string(),
            panes: 1,
            width: Width::Preset(Preset::Quarter),
            centered: false,
            labels: false,
            cowsay: false,
            content: 0,
        }
    }
}

impl Prefs {
    /// Fold one answered `(key, toml_value)` pair in, ignoring keys a picture
    /// cannot show and values that do not parse.
    ///
    /// Lenient on purpose: this runs on every keystroke, and an option the
    /// preview does not understand should cost the user a *less specific*
    /// picture, never a crash mid-setup.
    pub fn apply(&mut self, key: &str, value: &str) {
        let parsed = toml::from_str::<toml::Value>(&format!("x = {value}\n"))
            .ok()
            .and_then(|v| v.get("x").cloned());
        let Some(v) = parsed else { return };
        match key {
            "theme" => {
                if let Some(s) = v.as_str() {
                    self.theme = s.to_string();
                }
            }
            "startup_panes" => {
                if let Some(n) = v.as_integer() {
                    self.panes = n.clamp(0, 16) as usize;
                }
            }
            "default_column_width" => {
                if let Ok(w) = v.clone().try_into::<Width>() {
                    self.width = w;
                }
            }
            "center_focus" => self.centered = v.as_bool().unwrap_or(self.centered),
            "content_width" => {
                if let Some(n) = v.as_integer() {
                    self.content = n.clamp(0, 1000) as u16;
                }
            }
            "cell_labels" => self.labels = v.as_bool().unwrap_or(self.labels),
            "cowsay.enabled" => self.cowsay = v.as_bool().unwrap_or(self.cowsay),
            _ => {}
        }
    }

    /// Build from every answer so far, over the defaults.
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut p = Self::default();
        for (k, v) in pairs {
            p.apply(k, v);
        }
        p
    }

    /// The width of one column in the mocked viewport, in cells.
    ///
    /// Clamped to at least 6 so that a fixed `80 cells` answer on a 60-cell
    /// mockup still reads as "wider than the screen, so it scrolls" rather
    /// than collapsing into a line.
    fn col_cells(&self) -> usize {
        self.width.cells(W as u16).max(6) as usize
    }
}

/// One rendered cell: a character with its colors.
#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: CColor,
    bg: CColor,
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
    // strimux screenshot conveys, so the mockup leads with it.
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
        // rendering bug in strimux rather than as a narrow column. The short
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
fn run_length(row: &[Cell]) -> String {
    let mut s = String::new();
    let mut cur: Option<(CColor, CColor)> = None;
    for c in row {
        if cur != Some((c.fg, c.bg)) {
            s.push_str(&fg(c.fg));
            s.push_str(&bg(c.bg));
            cur = Some((c.fg, c.bg));
        }
        s.push(c.ch);
    }
    s.push_str(RESET);
    s
}

/// SGR foreground, passing indexed/default colors through untouched so the
/// `terminal` preset previews as the user's own ANSI palette.
fn fg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[38;5;{i}m"),
        CColor::Default => "\x1b[39m".to_string(),
    }
}

/// SGR background, with the same pass-through rule as [`fg`].
fn bg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[48;5;{i}m"),
        CColor::Default => "\x1b[49m".to_string(),
    }
}

#[cfg(test)]
mod tests {
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
    fn the_size_gate_matches_real_terminals() {
        // 80x24 with a 6-option question: adaptive, so still previewed.
        assert_eq!(fits(80, 24, 13), Some(8));
        // A tall window gets the full-size mockup.
        assert_eq!(fits(120, 50, 13), Some(H));
        // Too narrow at any height.
        assert_eq!(fits(50, 60, 13), None);
        // Too short once the question is accounted for.
        assert_eq!(fits(100, 14, 13), None);
    }

    /// Every line is the same width. A mockup that is ragged reads as a bug in
    /// strimux itself, which is the opposite of what a first run should teach.
    #[test]
    fn every_row_is_exactly_the_frame_width() {
        for centered in [false, true] {
            for w in [
                Width::Preset(Preset::Quarter),
                Width::Preset(Preset::Third),
                Width::Preset(Preset::Half),
                Width::Preset(Preset::TwoThirds),
                Width::Preset(Preset::Full),
                Width::Cells(80),
            ] {
                let p = Prefs {
                    width: w,
                    centered,
                    panes: 2,
                    ..Default::default()
                };
                let text = plain(&render(&p, false));
                let widths: Vec<usize> = text.lines().map(|l| l.chars().count()).collect();
                assert!(
                    widths.windows(2).all(|w| w[0] == w[1]),
                    "ragged preview for {w:?} centered={centered}: {widths:?}"
                );
                assert_eq!(widths[0], W + 4, "frame width for {w:?}");
                assert_eq!(text.lines().count(), H + 2);
            }
        }
    }

    /// Every theme the flow offers renders, including `terminal`, which has no
    /// RGB values at all.
    #[test]
    fn every_offered_theme_previews() {
        for q in crate::onboard::questions() {
            if q.key != "theme" {
                continue;
            }
            for o in &q.options {
                let p = Prefs {
                    theme: o.label.to_string(),
                    ..Default::default()
                };
                let text = plain(&render(&p, false));
                assert_eq!(text.lines().count(), H + 2, "{}", o.label);
            }
        }
    }

    /// The preview answers the question it is drawn under: turning a setting
    /// on has to change the picture, or it is decoration.
    #[test]
    fn each_visible_setting_changes_the_picture() {
        let base = plain(&render(&Prefs::default(), false));
        let mut cases: Vec<(&str, Prefs)> = Vec::new();
        cases.push((
            "panes",
            Prefs {
                panes: 3,
                ..Default::default()
            },
        ));
        cases.push((
            "width",
            Prefs {
                width: Width::Preset(Preset::Half),
                ..Default::default()
            },
        ));
        cases.push((
            "content",
            Prefs {
                content: 120,
                ..Default::default()
            },
        ));
        cases.push((
            "centered",
            Prefs {
                centered: true,
                ..Default::default()
            },
        ));
        cases.push((
            "labels",
            Prefs {
                labels: true,
                ..Default::default()
            },
        ));
        cases.push((
            "cowsay",
            Prefs {
                cowsay: true,
                ..Default::default()
            },
        ));
        for (name, p) in cases {
            assert_ne!(base, plain(&render(&p, false)), "{name} changed nothing");
        }
        // Theme changes colors, not characters, so it is checked on the raw
        // bytes rather than the stripped text.
        let themed = render(
            &Prefs {
                theme: "gruvbox".to_string(),
                ..Default::default()
            },
            false,
        );
        assert_ne!(render(&Prefs::default(), false), themed);
    }

    /// `apply` accepts exactly the TOML the onboarding options are written in,
    /// so the preview can never disagree with the value about to be saved.
    #[test]
    fn every_onboarding_option_value_is_understood() {
        for q in crate::onboard::all_questions_for(crate::install::Facts {
            installed: false,
            brew: true,
            cargo: true,
            macos: true,
        }) {
            for o in &q.options {
                let mut p = Prefs::default();
                p.apply(q.key, o.value);
                // Must not panic, and must render at full size.
                assert_eq!(plain(&render(&p, false)).lines().count(), H + 2);
            }
        }
    }

    /// A junk value leaves the preview alone rather than taking the flow down.
    #[test]
    fn unparseable_and_unknown_values_are_ignored() {
        let mut p = Prefs::default();
        p.apply("theme", "not valid toml [[");
        p.apply("input_poll_ms", "4");
        p.apply("startup_panes", "\"three\"");
        assert_eq!(p, Prefs::default());
    }

    /// Answers accumulate in order, last write winning.
    #[test]
    fn from_pairs_folds_answers_in_order() {
        let p = Prefs::from_pairs(&[
            ("theme".to_string(), "\"nord\"".to_string()),
            ("startup_panes".to_string(), "2".to_string()),
            ("startup_panes".to_string(), "4".to_string()),
            ("cowsay.enabled".to_string(), "true".to_string()),
        ]);
        assert_eq!(p.theme, "nord");
        assert_eq!(p.panes, 4);
        assert!(p.cowsay);
    }

    /// A logical pane wider than its column shows the overflow marker, which
    /// is the only visible evidence that `content_width` did anything.
    #[test]
    fn a_wide_logical_pane_shows_the_overflow_marker() {
        let p = Prefs {
            content: 120,
            ..Default::default()
        };
        assert!(plain(&render(&p, false)).contains('\u{203a}'));
        assert!(!plain(&render(&Prefs::default(), false)).contains('\u{203a}'));
    }

    /// Raw mode needs `\r\n` on every line or the mockup stair-steps.
    #[test]
    fn crlf_mode_terminates_every_line() {
        let s = render(&Prefs::default(), true);
        assert_eq!(s.matches("\r\n").count(), H + 2);
        assert!(!s.replace("\r\n", "").contains('\n'));
    }
}
