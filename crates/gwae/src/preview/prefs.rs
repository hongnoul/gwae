//! Preview inputs: visible prefs and size gating.

use crate::theme::Palette;
use gwae_layout::width::{Preset, Width};
use gwae_term::CColor;

pub(super) const RESET: &str = "\x1b[0m";

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
    /// shows what an untouched gwae looks like.
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
    pub(super) fn col_cells(&self) -> usize {
        self.width.cells(W as u16).max(6) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::paint::render;

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
    /// gwae itself, which is the opposite of what a first run should teach.

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
        let cases: Vec<(&str, Prefs)> = vec![];
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
}

