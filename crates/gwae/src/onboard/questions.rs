//! Onboarding questions: data-driven flow over config keys.

use crate::theme::Palette;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use gwae_term::CColor;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

pub(super) const RESET: &str = "\x1b[0m";
pub(super) const DIM: &str = "\x1b[2m";
pub(super) const BOLD: &str = "\x1b[1m";
pub(super) const CYAN: &str = "\x1b[36m";
pub(super) const GREEN: &str = "\x1b[32m";
pub(super) const YELLOW: &str = "\x1b[33m";
/// Clear the screen and park the cursor at the top-left. Each question owns
/// the whole screen, instead of scrolling past as one long transcript.
pub(super) const CLEAR: &str = "\x1b[2J\x1b[H";

/// The marker key written at the end of a completed pass.
///
/// Presence of this key (not merely "the file exists") is what makes
/// onboarding a once-only event: a config created by the old one-question
/// gateway, or by hand, still gets offered the full flow exactly once.
pub const MARKER: &str = "onboarded";

/// One selectable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opt {
    /// What the user sees on the line.
    pub label: &'static str,
    /// The value as it will be written, already valid TOML (quoted for
    /// strings, bare for numbers/bools).
    pub value: &'static str,
    /// One-line consequence, so the choice is informed rather than guessed.
    pub blurb: &'static str,
}

/// One onboarding question: a top-level config key and its offered values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Top-level config key written when answered.
    pub key: &'static str,
    /// The headline question.
    pub prompt: &'static str,
    /// Why it matters, shown dimmed under the prompt.
    pub help: &'static str,
    /// Offered values, in presentation order.
    pub options: Vec<Opt>,
    /// Index into `options` the cursor starts on, and that Enter takes.
    pub default: usize,
    /// Draw a color swatch beside each option (theme presets).
    pub swatch: bool,
    /// The config file already sets this key to something not offered above
    /// (a hand-written `{ cells = 93 }`, a custom `[theme]` table). Enter then
    /// means *keep it*, because re-running setup must never quietly undo a
    /// deliberate hand edit.
    pub keep_existing: bool,
}

impl Question {
    /// The value the cursor's start position selects.
    pub fn default_value(&self) -> &'static str {
        self.options[self.default].value
    }

    /// What accepting the highlighted default does: keep an unlisted existing
    /// value, else write the marked default.
    pub fn enter(&self) -> Answer {
        if self.keep_existing {
            Answer::Skip
        } else {
            Answer::Set(self.default_value().to_string())
        }
    }

    /// The option whose value is `value`, for rendering a summary line.
    fn label_for(&self, value: &str) -> &'static str {
        self.options
            .iter()
            .find(|o| o.value == value)
            .map(|o| o.label)
            .unwrap_or("custom")
    }
}

/// What the user did with one question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Write this TOML value for the question's key.
    Set(String),
    /// Leave the key out of the file entirely (keep whatever is there, or the
    /// built-in default).
    Skip,
    /// Take the default here and for every remaining question.
    RestDefaults,
}

/// The harness question, built from what this machine actually has.
///
/// Lists the harnesses found on `PATH` (known names like `jcode`/`claude` plus
/// anything that looks like an agent), capped at nine so the instant-digit
/// shortcut stays unambiguous. When nothing is installed the question offers the
/// known names as intended choices so a fresh machine can still pick.
pub fn harness_question_with(extra: &[String]) -> Question {
    let found = crate::agent::detect_with(extra);
    let mut options: Vec<Opt> = Vec::new();
    if found.is_empty() {
        for (cmd, label) in crate::agent::KNOWN_AGENTS.iter().take(9) {
            let value = crate::agent::toml_string_pub(cmd);
            options.push(Opt {
                label: Box::leak(label.to_string().into_boxed_str()),
                value: Box::leak(value.into_boxed_str()),
                blurb: Box::leak("not yet installed".to_string().into_boxed_str()),
            });
        }
    } else {
        for f in found.iter().take(9) {
            let blurb = f.path.display().to_string();
            let blurb = if blurb.is_empty() {
                "installed".to_string()
            } else {
                blurb
            };
            options.push(Opt {
                label: Box::leak(f.label.clone().into_boxed_str()),
                value: Box::leak(crate::agent::toml_string_pub(&f.cmd).into_boxed_str()),
                blurb: Box::leak(blurb.into_boxed_str()),
            });
        }
    }
    Question {
        key: "default_agent",
        prompt: "Agent harness",
        help: "Which command ⌥+; runs in a new pane. Detected from your PATH.",
        options,
        default: 0,
        swatch: false,
        keep_existing: false,
    }
}

/// Convenience: [`harness_question_with`] with no configured extras.
#[allow(dead_code)]
pub fn harness_question() -> Question {
    harness_question_with(&[])
}

/// Every question, in the order they are asked.
///
/// Ordering is "biggest visible effect first": a user who bails after two
/// questions has still picked their harness and their colors. Appearance and
/// layout defaults match [`crate::config::Config::default`], except the theme:
/// fresh onboarding recommends white phosphor, while configs without a theme
/// retain the existing Catppuccin Mocha fallback.
///
/// Deliberately *not* asked: anything with one right answer
/// (`input_poll_ms`, applied silently), and anything that is a niche taste
/// best left to a hand edit (`skeleton`'s inset frames, `[minimap]` geometry,
/// `scroll_margin`).
#[allow(dead_code)]
pub fn questions() -> Vec<Question> {
    questions_with(&[])
}

pub fn questions_with(extra: &[String]) -> Vec<Question> {
    let mut qs = vec![harness_question_with(extra)];
    qs.extend(vec![
        Question {
            key: "theme",
            prompt: "Color theme",
            help: "Chrome colors: background, focus frame, HUD, minimap, pane status tints.",
            options: vec![
                Opt {
                    label: "catppuccin-mocha",
                    value: "\"catppuccin-mocha\"",
                    blurb: "dark, muted purple-blue",
                },
                Opt {
                    label: "catppuccin-latte",
                    value: "\"catppuccin-latte\"",
                    blurb: "the light one",
                },
                Opt {
                    label: "tokyo-night",
                    value: "\"tokyo-night\"",
                    blurb: "dark, high-contrast blue",
                },
                Opt {
                    label: "gruvbox",
                    value: "\"gruvbox\"",
                    blurb: "warm retro dark",
                },
                Opt {
                    label: "nord",
                    value: "\"nord\"",
                    blurb: "cool desaturated blue",
                },
                Opt {
                    label: "rose-pine",
                    value: "\"rose-pine\"",
                    blurb: "soft rose on ink",
                },
                Opt {
                    label: "dracula",
                    value: "\"dracula\"",
                    blurb: "vivid dark",
                },
                Opt {
                    label: "terminal",
                    value: "\"terminal\"",
                    blurb: "inherit your terminal's own ANSI palette",
                },
                Opt {
                    label: "white-phosphor",
                    value: "\"white-phosphor\"",
                    blurb: "monochrome CRT on true black (default)",
                },
            ],
            default: 8,
            swatch: true,
            keep_existing: false,
        },
        Question {
            key: "default_column_width",
            prompt: "Width of a new column",
            help: "The share of the screen each new column takes; \u{2325}+r cycles it later.",
            options: vec![
                Opt {
                    label: "quarter",
                    value: "\"quarter\"",
                    blurb: "four across; the scrolling default",
                },
                Opt {
                    label: "third",
                    value: "\"third\"",
                    blurb: "three across",
                },
                Opt {
                    label: "half",
                    value: "\"half\"",
                    blurb: "two side by side",
                },
                Opt {
                    label: "two-thirds",
                    value: "\"two-thirds\"",
                    blurb: "one big column plus a sliver",
                },
                Opt {
                    label: "full",
                    value: "\"full\"",
                    blurb: "one column at a time, scrolling",
                },
                Opt {
                    label: "80 cells",
                    value: "80",
                    blurb: "fixed 80 columns, whatever the terminal size",
                },
            ],
            default: 0,
            swatch: false,
            keep_existing: false,
        },
        Question {
            key: "center_focus",
            prompt: "Scrolling style",
            help: "Where the focused column lands when you move focus off screen.",
            options: vec![
                Opt {
                    label: "minimal",
                    value: "false",
                    blurb: "scroll just enough to reveal it (default)",
                },
                Opt {
                    label: "centered",
                    value: "true",
                    blurb: "always re-center the focused column",
                },
            ],
            default: 0,
            swatch: false,
            keep_existing: false,
        },
        Question {
            key: "cell_labels",
            prompt: "Address labels in empty boxes",
            help: "The big `strip.pane` identifier drawn in placeholder boxes.",
            options: vec![
                Opt {
                    label: "on",
                    value: "true",
                    blurb: "show which cell each empty box is (default)",
                },
                Opt {
                    label: "off",
                    value: "false",
                    blurb: "empty boxes stay bare",
                },
            ],
            default: 0,
            swatch: false,
            keep_existing: false,
        },
    ]);
    qs
}

/// Re-point every question's default at what the config file already says, so
/// re-running `gwae init` starts from the user's current setup rather than
/// from the factory one.
///
/// This is what makes the flow safe to re-run and safe to offer to someone who
/// hand-wrote their config years ago: accepting every default is always a
/// no-op on the file's *meaning*, never a reset.
pub fn with_existing(qs: Vec<Question>, text: &str) -> Vec<Question> {
    let doc: toml::Value = toml::from_str(text).unwrap_or(toml::Value::Table(Default::default()));
    qs.into_iter()
        .map(|mut q| {
            let Some(cur) = lookup(&doc, q.key) else {
                return q;
            };
            match q.options.iter().position(|o| toml_eq(o.value, &cur)) {
                Some(i) => q.default = i,
                None => q.keep_existing = true,
            }
            q
        })
        .collect()
}

/// The current value of a (possibly dotted) config key.
fn lookup(doc: &toml::Value, key: &str) -> Option<toml::Value> {
    let mut cur = doc;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur.clone())
}

/// Whether an option's TOML text denotes the same value the file holds.
/// Compared as parsed values, not as strings, so `0.9` and `0.90` agree and a
/// `{ preset = "half" }` table is not mistaken for a different width.
fn toml_eq(option_value: &str, current: &toml::Value) -> bool {
    let wrapped = format!("x = {option_value}\n");
    match toml::from_str::<toml::Value>(&wrapped) {
        Ok(v) => v.get("x") == Some(current),
        Err(_) => false,
    }
}

/// The cowsay question, asked separately because it writes into a `[cowsay]`
/// table rather than a top-level key.
pub fn cowsay_question() -> Question {
    Question {
        key: "cowsay.enabled",
        prompt: "Keybinding hints in empty boxes",
        help:
            "A cow reciting one real binding per empty box: the cheat-sheet you read by accident.",
        options: vec![
            Opt {
                label: "on",
                value: "true",
                blurb: "learn the bindings while the grid is still empty (default)",
            },
            Opt {
                label: "off",
                value: "false",
                blurb: "empty boxes stay quiet",
            },
        ],
        default: 0,
        swatch: false,
        keep_existing: false,
    }
}

/// The key of the one question that is not a config setting.
///
/// Answering `true` installs [`crate::install::TOOL`] on the machine. It is
/// filtered out of the config write ([`apply_answers`]) rather than being a
/// separate flow, so it gets the same screen, the same keys and the same
/// summary line as everything else.
pub const INSTALL_KEY: &str = "install.btm";

/// Every question of the flow, in order.
///
/// The `btm` offer comes last and only when it would do something: asking
/// someone who already has it would be setup pretending not to know.
#[allow(dead_code)]
pub fn all_questions() -> Vec<Question> {
    all_questions_for(crate::install::Facts::probe())
}

/// Like [`all_questions`] but seeded with extra harness names from config.
pub fn all_questions_with_extra(extra: &[String]) -> Vec<Question> {
    all_questions_for_with_extra(crate::install::Facts::probe(), extra)
}

/// [`all_questions`] against a described machine, so tests never depend on
/// what happens to be installed on the one running them.
pub fn all_questions_for(f: crate::install::Facts) -> Vec<Question> {
    all_questions_for_with_extra(f, &[])
}

pub fn all_questions_for_with_extra(f: crate::install::Facts, extra: &[String]) -> Vec<Question> {
    let _ = f;
    let mut qs = questions_with(extra);
    qs.push(cowsay_question());
    // keep_awake is always on by default (config default true) and toggled
    // with ⌥+w mid-session, so it is not a question.
    qs
}

/// One keystroke, already decoded from whatever the terminal sent.
///
/// Naming the *intent* rather than the byte sequence is what lets [`step`] be
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::persist::{apply_answers, fill_defaults};
    use super::super::screen::{banner_palette, render_question, render_screen, step, Key, Step};
    use crate::theme::Palette;
    use crate::config::Config;

    /// The whole flow on a machine that does not have `btm`, so the offer is
    /// present. Describing the machine keeps these tests independent of
    /// whatever is installed on the one running them.
    fn all() -> Vec<Question> {
        all_questions_for(crate::install::Facts {
            installed: false,
            brew: true,
            cargo: true,
            macos: true,
        })
    }

    /// The preview under a question reflects the answers already given, so a
    /// theme picked on screen 1 is what screen 5 is judged in.
    
    #[test]
    fn every_offered_value_is_config_the_real_parser_accepts() {
        // The whole point of onboarding is that a user never hand-writes TOML.
        // If an option's value did not parse, we would be *creating* the
        // broken config that `doctor` then blames on the user.
        for q in all() {
            for o in &q.options {
                let toml = match q.key.strip_prefix("cowsay.") {
                    Some(sub) => format!("[cowsay]\n{sub} = {}\n", o.value),
                    None => format!("{} = {}\n", q.key, o.value),
                };
                let cfg: Result<Config, _> = toml::from_str(&toml);
                assert!(
                    cfg.is_ok(),
                    "{} = {} does not parse: {:?}",
                    q.key,
                    o.value,
                    cfg.err()
                );
            }
        }
    }


    #[test]
    fn accepting_every_default_uses_white_phosphor_and_the_usual_layout() {
        // Fresh setup recommends white phosphor without changing the runtime
        // fallback for existing configs that never named a theme.
        let answers: Vec<(String, String)> = all()
            .iter()
            .map(|q| (q.key.to_string(), q.default_value().to_string()))
            .collect();
        let text = apply_answers("", &answers);
        let cfg: Config = toml::from_str(&text).expect("generated config parses");
        let d = Config::default();
        assert_eq!(cfg.startup_panes, d.startup_panes);
        assert_eq!(cfg.center_focus, d.center_focus);
        assert_eq!(cfg.content_width, d.content_width);
        assert_eq!(cfg.cell_labels, d.cell_labels);
        assert_eq!(cfg.cowsay.enabled, d.cowsay.enabled);
        assert_eq!(cfg.theme_name(), "white-phosphor");
        assert_eq!(cfg.palette(), Palette::WHITE_PHOSPHOR);
        assert_eq!(d.palette(), Palette::CATPPUCCIN_MOCHA);
        assert_eq!(cfg.default_column_width, d.default_column_width);
    }


    #[test]
    fn fresh_theme_default_is_highlighted_previewed_and_saved() {
        let qs = with_existing(all(), "");
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
        assert_eq!(q.default_value(), "\"white-phosphor\"");
        assert_eq!(
            step(q, q.default, Key::Next),
            Step::Done(Answer::Set("\"white-phosphor\"".into()))
        );
        let shown = render_question(q, 1, qs.len(), q.default);
        assert!(shown.contains(&format!("❯* {CYAN}{BOLD}white-phosphor{RESET}")));
        assert!(!shown.contains("dark, muted purple-blue (default)"));
        assert_eq!(
            banner_palette(&[], q, q.default, &Palette::default()),
            Palette::WHITE_PHOSPHOR
        );

        // Esc from the first question must choose the same theme, even when
        // the user never visits the theme picker.
        let mut chosen = vec![None; qs.len()];
        fill_defaults(&qs, &mut chosen, 0);
        let text = apply_answers("", &answered(&qs, &chosen));
        let cfg: Config = toml::from_str(&text).expect("default answers parse");
        assert_eq!(cfg.palette(), Palette::WHITE_PHOSPHOR);
    }


    #[test]
    fn no_question_has_more_than_nine_options() {
        // A digit answers *immediately*, with no Enter to disambiguate it. That
        // is only safe while every list is single-digit: a tenth option would
        // make `1` ambiguous between "option 1" and the first half of "10",
        // and the flow would have to start waiting again.
        for q in all() {
            assert!(
                q.options.len() <= 9,
                "{} has {} options; instant digits need <= 9",
                q.key,
                q.options.len()
            );
        }
    }


    #[test]
    fn retired_questions_are_gone_for_good() {
        // Mouse capture is no longer a knob, and the inset skeleton frames are
        // a hand-edit-only taste; asking about either is what this rework
        // removed, so the list must not quietly grow them back.
        for q in all() {
            assert!(
                !matches!(q.key, "mouse" | "scroll_lines" | "skeleton"),
                "{} is not a question any more",
                q.key
            );
        }
    }


    #[test]
    fn keep_awake_is_not_a_question_and_defaults_on() {
        // keep_awake is always on unless the user opts out with
        // `keep_awake = false` or ⌥+w, so setup never asks about it.
        assert!(
            all().iter().all(|q| q.key != "keep_awake"),
            "keep_awake must not be asked"
        );
        assert!(Config::default().keep_awake);
        let cfg: Config = toml::from_str("").expect("empty parses");
        assert!(cfg.keep_awake, "absent key means on");
        let cfg: Config = toml::from_str("keep_awake = false\n").expect("parses");
        assert!(!cfg.keep_awake, "explicit opt-out still works");
    }


    #[test]
    fn keep_awake_survives_a_rerun_and_a_live_reload() {
        // A mid-session edit of the key applies live: the reload does
        // not pin the old value, it adopts the file's (the render loop then
        // reconciles the guard and says so).
        let mut cfg = Config::default();
        assert!(cfg.keep_awake);
        let new: Config = toml::from_str("keep_awake = false\n").unwrap();
        cfg.adopt_appearance(new);
        assert!(!cfg.keep_awake, "live reload must adopt keep-awake");
        // ...while keys the session consumed at launch stay pinned.
        let mut cfg = Config {
            startup_panes: 4,
            ..Default::default()
        };
        let new: Config = toml::from_str("theme = \"nord\"\nstartup_panes = 9\n").unwrap();
        cfg.adopt_appearance(new);
        assert_eq!(cfg.startup_panes, 4, "startup_panes stays pinned");
        assert_eq!(cfg.palette(), Palette::NORD, "but the theme rides along");
    }


    #[test]
    fn theme_options_are_the_real_presets() {
        let qs = questions();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
        let labels: Vec<&str> = q.options.iter().map(|o| o.label).collect();
        assert_eq!(
            labels,
            Palette::NAMES,
            "picker drifted from the preset list"
        );
        for o in &q.options {
            assert!(!swatch(o.label).is_empty(), "no swatch for {}", o.label);
        }
    }
}
