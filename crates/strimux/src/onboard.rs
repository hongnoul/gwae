//! First-run onboarding: configure *everything* worth configuring, once.
//!
//! The agent gateway used to ask exactly one question (which harness `⌥+;`
//! runs). Everything else - theme, layout, chrome - was discoverable only by
//! reading `docs/CONFIG.md` and hand-writing TOML, which in practice meant
//! almost nobody changed it. This module turns that into a short guided pass:
//! one full-screen question at a time, moved through with the arrow keys (or
//! `j`/`k`), confirmed with Enter, and finished with a summary screen showing
//! exactly what was written.
//!
//! Shape of the code, deliberately:
//!
//! * The **questions are data** ([`questions`]), so `strimux init --print`
//!   can show the whole flow and tests can assert every option is valid TOML
//!   that the real [`crate::config::Config`] accepts.
//! * The **input handling is pure** ([`step`] over [`Key`]), so every
//!   keystroke a user actually presses (arrows, `j`/`k`, a digit, Enter, esc)
//!   is tested without a PTY.
//! * The **write is one function** ([`save_answers`]) over the existing
//!   comment-preserving [`crate::agent::set_scalar_text`], so onboarding can
//!   never clobber a config a user already hand-edited.
//!
//! Machine-dependent tuning (`input_poll_ms`) is *not* a question: it has one
//! right answer, so [`crate::latency::apply_silently`] applies it before the
//! first question is drawn and the summary screen reports anything left that
//! only the user can change.
//!
//! Onboarding is only ever *offered*: it runs on the interactive gateway path
//! when the config has not been through it before, and on demand via
//! `strimux init`. A configured user is never interrupted.

use crate::theme::Palette;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;
use strimux_term::CColor;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
/// Clear the screen and park the cursor at the top-left. Each question owns
/// the whole screen, instead of scrolling past as one long transcript.
const CLEAR: &str = "\x1b[2J\x1b[H";

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

/// Every question, in the order they are asked.
///
/// Ordering is "biggest visible effect first": a user who bails after two
/// questions has still picked their harness and their colors. Defaults are
/// exactly [`crate::config::Config::default`], so accepting every default is a
/// no-op on behavior.
///
/// Deliberately *not* asked: anything with one right answer
/// (`input_poll_ms`, applied silently), and anything that is a niche taste
/// best left to a hand edit (`skeleton`'s inset frames, `[minimap]` geometry,
/// `scroll_margin`).
pub fn questions() -> Vec<Question> {
    vec![
        Question {
            key: "theme",
            prompt: "Color theme",
            help: "Chrome colors: background, focus frame, HUD, minimap, pane status tints.",
            options: vec![
                Opt {
                    label: "catppuccin-mocha",
                    value: "\"catppuccin-mocha\"",
                    blurb: "dark, muted purple-blue (default)",
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
            ],
            default: 0,
            swatch: true,
            keep_existing: false,
        },
        Question {
            key: "startup_panes",
            prompt: "Panes on screen at launch",
            help: "How many equal-width columns exist the moment strimux opens.",
            options: vec![
                Opt {
                    label: "1",
                    value: "1",
                    blurb: "one pane, the rest of the grid empty (default)",
                },
                Opt {
                    label: "2",
                    value: "2",
                    blurb: "side by side, e.g. agent + shell",
                },
                Opt {
                    label: "3",
                    value: "3",
                    blurb: "three up",
                },
                Opt {
                    label: "4",
                    value: "4",
                    blurb: "dense; best on a wide display",
                },
            ],
            default: 0,
            swatch: false,
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
            key: "content_width",
            prompt: "Logical pane width",
            help: "Wider than the column means long lines don't wrap; ⌥+←/→ pans.",
            options: vec![
                Opt {
                    label: "follow column",
                    value: "0",
                    blurb: "lines wrap to the visible width (default)",
                },
                Opt {
                    label: "100",
                    value: "100",
                    blurb: "keep 100-col output intact in a narrow column",
                },
                Opt {
                    label: "120",
                    value: "120",
                    blurb: "for 120-col logs and diffs",
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
                    label: "off",
                    value: "false",
                    blurb: "empty boxes stay bare (default)",
                },
                Opt {
                    label: "on",
                    value: "true",
                    blurb: "show which cell each empty box is",
                },
            ],
            default: 0,
            swatch: false,
            keep_existing: false,
        },
    ]
}

/// Re-point every question's default at what the config file already says, so
/// re-running `strimux init` starts from the user's current setup rather than
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
                label: "off",
                value: "false",
                blurb: "empty boxes stay quiet (default)",
            },
            Opt {
                label: "on",
                value: "true",
                blurb: "learn the bindings while the grid is still empty",
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

/// The `btm` question. Default *yes*: it is the one companion tool strimux
/// actively recommends, and the pane next to an agent is exactly where a
/// system monitor earns its place.
pub fn install_question() -> Question {
    Question {
        key: INSTALL_KEY,
        prompt: "Install btm, the system monitor",
        help: "A live CPU/memory/network pane to sit next to your agent. \
               strimux installs whatever this needs.",
        options: vec![
            Opt {
                label: "yes",
                value: "true",
                blurb: "install it now (default)",
            },
            Opt {
                label: "no",
                value: "false",
                blurb: "leave this machine alone",
            },
        ],
        default: 0,
        swatch: false,
        keep_existing: false,
    }
}

/// Every question of the flow, in order.
///
/// The `btm` offer comes last and only when it would do something: asking
/// someone who already has it would be setup pretending not to know.
pub fn all_questions() -> Vec<Question> {
    all_questions_for(crate::install::Facts::probe())
}

/// [`all_questions`] against a described machine, so tests never depend on
/// what happens to be installed on the one running them.
pub fn all_questions_for(f: crate::install::Facts) -> Vec<Question> {
    let mut qs = questions();
    qs.push(cowsay_question());
    if crate::install::worth_asking(f) {
        qs.push(install_question());
    }
    qs
}

/// One keystroke, already decoded from whatever the terminal sent.
///
/// Naming the *intent* rather than the byte sequence is what lets [`step`] be
/// tested directly: `↓` and `j` are the same `Key`, as are `→`, `l` and Enter,
/// so the several ways of driving the flow can never diverge.
///
/// The axes are deliberately separate: **up/down picks an option**, **left/
/// right moves between questions**. That is why `h`/`←`/backspace all mean the
/// same thing (go back a question) and `l`/`→`/Enter all mean the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Move the highlight up one option (`↑`, `k`).
    Up,
    /// Move the highlight down one option (`↓`, `j`).
    Down,
    /// Accept the highlighted option and go to the next question
    /// (`⏎`, `→`, `l`, space).
    Next,
    /// Go back to the previous question (`⌫`, `←`, `h`).
    Prev,
    /// A digit: select that option immediately, no Enter needed.
    Digit(usize),
    /// Leave this key out of the file entirely (`s`).
    Skip,
    /// Accept the highlighted option here and the default for everything left
    /// (`d`, `q`, `esc`).
    Rest,
    /// Abandon the flow (`ctrl-c`).
    Abort,
    /// Anything else: ignored, never a mis-set key.
    Other,
}

/// Decode a terminal key event into a [`Key`].
pub fn key_from_event(code: KeyCode, mods: KeyModifiers) -> Key {
    match code {
        KeyCode::Char('c') | KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            Key::Abort
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => Key::Up,
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => Key::Down,
        KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => Key::Next,
        KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => Key::Prev,
        KeyCode::Char('s') => Key::Skip,
        KeyCode::Char('d') | KeyCode::Char('q') | KeyCode::Esc => Key::Rest,
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            Key::Digit(c.to_digit(10).unwrap() as usize)
        }
        _ => Key::Other,
    }
}

/// What one keystroke does to a question that is currently on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Redraw with the highlight on this option.
    Move(usize),
    /// The question is answered; go to the next one.
    Done(Answer),
    /// Go back to the previous question, leaving this one unanswered for now.
    Back,
    /// Stop the flow without writing anything further.
    Abort,
    /// Nothing happened (an unknown key, or a digit with no such option).
    Ignore,
}

/// Interpret one keystroke against `q` with the highlight at `cursor`. Pure.
///
/// A digit answers immediately rather than waiting for Enter, because the
/// number is unambiguous the moment it is typed; every other selection is
/// confirmed with Enter, so moving the highlight never commits by accident.
pub fn step(q: &Question, cursor: usize, key: Key) -> Step {
    let n = q.options.len();
    match key {
        Key::Up => Step::Move((cursor + n - 1) % n),
        Key::Down => Step::Move((cursor + 1) % n),
        Key::Prev => Step::Back,
        Key::Next => Step::Done(if q.keep_existing && cursor == q.default {
            // Enter on an unlisted hand-written value means "leave it alone",
            // not "overwrite it with the thing I happened to be sitting on".
            Answer::Skip
        } else {
            Answer::Set(q.options[cursor].value.to_string())
        }),
        Key::Digit(d) if d <= n => Step::Done(Answer::Set(q.options[d - 1].value.to_string())),
        Key::Digit(_) => Step::Ignore,
        Key::Skip => Step::Done(Answer::Skip),
        Key::Rest => Step::Done(Answer::RestDefaults),
        Key::Abort => Step::Abort,
        Key::Other => Step::Ignore,
    }
}

/// A one-line color swatch for a preset name, drawn from the real palette so
/// the preview can never disagree with what the theme actually paints.
pub fn swatch(preset: &str) -> String {
    let Some(p) = Palette::preset(preset) else {
        return String::new();
    };
    let mut s = String::new();
    for c in [
        p.base, p.surface, p.overlay, p.accent, p.running, p.done, p.failed,
    ] {
        s.push_str(&bg(c));
        s.push(' ');
        s.push(' ');
    }
    s.push_str(RESET);
    s
}

/// SGR background for a palette color, with indexed/default colors passed
/// through untouched (a `terminal` preset must preview as the *user's* ANSI).
fn bg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[48;5;{i}m"),
        CColor::Default => "\x1b[49m".to_string(),
    }
}

/// Render one question as its own screen, with `cursor` highlighted. Pure, so
/// `strimux init --print` and the tests see the real thing.
///
/// Lines end with `\r\n`: the flow runs in raw mode (to read arrow keys
/// without Enter), where a bare `\n` would stair-step down the screen.
pub fn render_question(q: &Question, idx: usize, total: usize, cursor: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{DIM}[{}/{}]{RESET} {BOLD}{}{RESET}\r\n{DIM}{}{RESET}\r\n\r\n",
        idx + 1,
        total,
        q.prompt,
        q.help
    ));
    for (i, o) in q.options.iter().enumerate() {
        let here = i == cursor;
        let (arrow, name) = if here {
            ("\u{276f}", format!("{CYAN}{BOLD}{}{RESET}", o.label))
        } else {
            (" ", o.label.to_string())
        };
        let pad = " ".repeat(18usize.saturating_sub(o.label.chars().count()));
        let sw = if q.swatch {
            format!(" {}", swatch(o.label))
        } else {
            String::new()
        };
        let star = if i == q.default { "*" } else { " " };
        s.push_str(&format!(
            " {arrow}{star}{DIM}{}{RESET} {name}{pad}{sw} {DIM}{}{RESET}\r\n",
            i + 1,
            o.blurb
        ));
    }
    s.push_str("\r\n");
    s.push_str(&if q.keep_existing {
        format!(
            "{DIM}\u{2191}\u{2193}/jk pick \u{00b7} \u{2192}/l/\u{23ce} keep your current setting \
             \u{00b7} \u{2190}/h/\u{232b} back \u{00b7} 1-{} jump \u{00b7} esc defaults for the \
             rest{RESET}\r\n",
            q.options.len()
        )
    } else {
        format!(
            "{DIM}\u{2191}\u{2193}/jk pick \u{00b7} \u{2192}/l/\u{23ce} next \u{00b7} \
             \u{2190}/h/\u{232b} back \u{00b7} 1-{} jump \u{00b7} s skip \u{00b7} esc defaults \
             for the rest{RESET}\r\n",
            q.options.len()
        )
    });
    s
}

/// One question *plus* the live mockup of what its highlighted option would
/// do. This is the screen a user actually sees.
///
/// Split from [`render_question`] rather than folded into it so the pure
/// question text stays independently testable, and so a question that changes
/// nothing visible (the `btm` install offer) can simply be rendered without a
/// picture instead of being given a misleading one.
///
/// The preview reflects **every answer so far**, not just this question: by
/// the time the flow asks about cell labels, the mockup is already wearing the
/// theme and the column width the user picked, so each answer is judged in the
/// setup it will actually live in.
pub fn render_screen(
    q: &Question,
    idx: usize,
    total: usize,
    cursor: usize,
    answered_so_far: &[(String, String)],
    crlf: bool,
) -> String {
    let mut s = render_question(q, idx, total, cursor);
    let Some(mut prefs) = previewable(q, answered_so_far) else {
        return s;
    };
    // The highlighted option, applied on top: the picture shows what pressing
    // Enter right now would produce, which is the entire point of a preview.
    prefs.apply(q.key, q.options[cursor].value);
    let art = crate::preview::render(&prefs, crlf);
    let nl = if crlf { "\r\n" } else { "\n" };
    s.push_str(nl);
    s.push_str(&art);
    s
}

/// [`render_screen`] with an explicit mockup height, or none at all.
///
/// `height` is what [`crate::preview::fits`] decided this terminal can spare.
pub fn render_sized(
    q: &Question,
    idx: usize,
    total: usize,
    cursor: usize,
    answered_so_far: &[(String, String)],
    height: Option<usize>,
) -> String {
    let mut s = render_question(q, idx, total, cursor);
    let (Some(h), Some(mut prefs)) = (height, previewable(q, answered_so_far)) else {
        return s;
    };
    if true { return s; }
    prefs.apply(q.key, q.options[cursor].value);
    s.push_str("\r\n");
    s.push_str(&crate::preview::render_h(&prefs, h, true));
    s
}

/// The preview state for a question, or `None` when a picture would be a lie.
///
/// A question earns a mockup only if the mockup can *answer* it. `install.btm`
/// changes the machine, not the screen, and drawing an unchanged grid under it
/// would quietly teach the user that the preview is decorative.
fn previewable(
    q: &Question,
    answered_so_far: &[(String, String)],
) -> Option<crate::preview::Prefs> {
    if q.key == INSTALL_KEY {
        return None;
    }
    Some(crate::preview::Prefs::from_pairs(answered_so_far))
}

/// The whole flow as text, for `strimux init --print` and for docs.
///
/// Every question, including the `btm` offer that a machine which already has
/// it would not be shown: this is documentation of the flow, not a prediction
/// of one run of it.
pub fn render_all() -> String {
    let qs = all_questions_for(crate::install::Facts {
        installed: false,
        ..crate::install::Facts::probe()
    });
    let n = qs.len();
    let mut s = String::new();
    // Answers accumulate exactly as they would in a real run that accepted
    // every default, so `--print` shows the flow a new user is walked through
    // rather than n unrelated screens.
    let mut so_far: Vec<(String, String)> = Vec::new();
    for (i, q) in qs.iter().enumerate() {
        s.push_str(&render_screen(q, i, n, q.default, &so_far, true));
        s.push_str("\r\n");
        so_far.push((q.key.to_string(), q.default_value().to_string()));
    }
    s
}

/// The closing screen: every setting as it now stands, where it lives, and
/// anything about the machine that only the user can fix. Pure.
///
/// `install` is the result of the `btm` step, reported on its own question's
/// line: what actually happened on the machine, not merely what was answered,
/// because "yes" and "installed" are not the same claim.
pub fn render_summary(
    qs: &[Question],
    answers: &[(String, String)],
    cfg_path: &Path,
    manual: Option<String>,
    install: Option<&crate::install::Outcome>,
) -> String {
    let mut s = format!("{BOLD}strimux is configured.{RESET}\r\n\r\n");
    for q in qs {
        let (mark, shown) = match (q.key, install) {
            // The install line reports the machine, not the answer.
            (INSTALL_KEY, Some(o)) => (
                match o {
                    crate::install::Outcome::Installed => GREEN,
                    crate::install::Outcome::Declined => DIM,
                    crate::install::Outcome::Failed(_) => YELLOW,
                },
                o.line(),
            ),
            _ => match answers.iter().find(|(k, _)| k == q.key) {
                Some((_, v)) => (GREEN, q.label_for(v).to_string()),
                None => (DIM, "kept as it was".to_string()),
            },
        };
        // At least one space, so the longest prompt does not butt straight
        // into its value.
        let pad = " ".repeat(31usize.saturating_sub(q.prompt.chars().count()).max(1));
        s.push_str(&format!(
            "  {mark}\u{2713}{RESET} {}{pad}{BOLD}{shown}{RESET}\r\n",
            q.prompt
        ));
    }
    s.push_str(&format!(
        "\r\n{DIM}Written to {RESET}{}{DIM}; it live-reloads, so edits apply \
         without a restart.{RESET}\r\n",
        cfg_path.display()
    ));
    s.push_str(&format!(
        "{DIM}Run {RESET}{CYAN}strimux init{RESET}{DIM} any time to change it, and see \
         {RESET}{CYAN}docs/CONFIG.md{RESET}{DIM} for the keys nobody should have to be asked \
         about.{RESET}\r\n"
    ));
    if let Some(m) = manual {
        s.push_str("\r\n");
        s.push_str(&m.replace('\n', "\r\n"));
    }
    s
}

/// Apply answered top-level keys to config `text`, preserving comments and
/// any keys onboarding never asked about. Pure.
pub fn apply_answers(text: &str, answers: &[(String, String)]) -> String {
    let mut out = if text.trim().is_empty() {
        "# strimux configuration\n# Written by `strimux init`; edit freely, \
         strimux only ever rewrites the keys it owns.\n"
            .to_string()
    } else {
        text.to_string()
    };
    for (k, v) in answers {
        // Not a config setting: it is an action taken on the machine, and
        // writing it would invent a key the parser knows nothing about.
        if k == INSTALL_KEY {
            continue;
        }
        out = match k.strip_prefix("cowsay.") {
            Some(sub) => set_table_scalar_text(&out, "cowsay", sub, v),
            None => crate::agent::set_scalar_text(&out, k, v),
        };
    }
    out
}

/// Set `key = value` inside `[table]`, creating the table when missing.
///
/// Kept here rather than in `agent` because onboarding is the only writer that
/// needs a table today, and a general TOML-editing layer would be a much
/// bigger promise than one key in one section.
pub fn set_table_scalar_text(text: &str, table: &str, key: &str, value: &str) -> String {
    let header = format!("[{table}]");
    let line = format!("{key} = {value}");
    let mut out: Vec<String> = Vec::new();
    let mut in_ours = false;
    let mut replaced = false;
    let mut seen_table = false;
    for raw in text.lines() {
        let t = raw.trim();
        if t.starts_with('[') {
            in_ours = t == header;
            seen_table |= in_ours;
            if in_ours {
                out.push(raw.to_string());
                continue;
            }
        }
        let is_key = in_ours
            && !replaced
            && t.strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);
        if is_key {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        if seen_table {
            // Insert right after the header so the key lands in the table.
            let at = out.iter().position(|l| l.trim() == header).unwrap() + 1;
            out.insert(at, line);
        } else {
            // A blank line before a new table, so the file stays readable
            // next to the spaced-out top-level keys already written above.
            if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
                out.push(String::new());
            }
            out.push(header);
            out.push(line);
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Write the answers to `path`, creating the file and its parent as needed.
pub fn save_answers(path: &Path, answers: &[(String, String)]) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = apply_answers(&existing, answers);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

/// Whether a config file has already been through onboarding.
///
/// Text-level rather than parsed, so it works on a file that fails to parse
/// (where re-running the flow would only make the breakage worse).
pub fn already_onboarded(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('#')
            && t.strip_prefix(MARKER)
                .map(|r| r.trim_start().starts_with('='))
                .unwrap_or(false)
    })
}

/// Read one keystroke, blocking. `None` on EOF / read error, which the caller
/// treats as "stop asking" rather than as an answer.
fn read_key() -> Option<Key> {
    read_with(key_from_event)
}

/// Read one keystroke, decoded by `f`. Lets the summary screen use a stricter
/// decoder than the questions without duplicating the event loop.
fn read_with(f: fn(KeyCode, KeyModifiers) -> Key) -> Option<Key> {
    loop {
        match event::read() {
            Ok(Event::Key(ke)) if ke.kind != KeyEventKind::Release => {
                return Some(f(ke.code, ke.modifiers))
            }
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

/// Decode a keystroke *on the summary screen*, where only two keys mean
/// anything: Enter finishes and Backspace goes back to the last question.
///
/// Deliberately stricter than [`key_from_event`]. That decoder is written for
/// a question, where `→`/`l`/space are all reasonable ways to say "next"; on a
/// screen that is not asking anything, those same keys would dismiss the
/// summary out from under someone who was just pressing keys at it. The one
/// screen that reports what was written to disk should take a deliberate
/// keypress to leave.
pub fn summary_key(code: KeyCode, mods: KeyModifiers) -> Key {
    match code {
        KeyCode::Char('c') | KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
            Key::Abort
        }
        KeyCode::Enter => Key::Next,
        KeyCode::Backspace => Key::Prev,
        _ => Key::Other,
    }
}

/// The terminal width, defaulting to a conservative 80 when it cannot be
/// determined (a pipe, a terminal that won't answer): art that assumes too
/// much width wraps and scrolls, art that assumes too little merely sits
/// left of center.
fn term_cols() -> u16 {
    crossterm::terminal::size().map(|(c, _)| c).unwrap_or(80)
}

/// The rows a question itself occupies: prompt, help, blank, one per option,
/// blank, key hints.
fn question_rows(q: &Question) -> usize {
    q.options.len() + 5
}

/// The terminal size, defaulting to a conservative 80x24.
fn term_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// The palette the flow paints itself in: whatever the config already says,
/// so a re-run of `strimux init` opens in the theme the user picked last time.
fn current_palette(text: &str) -> Palette {
    toml::from_str::<crate::config::Config>(text)
        .map(|c| c.palette())
        .unwrap_or_default()
}

/// Draw one screen and flush it, so a question is never half-painted while we
/// are already blocking on a keystroke.
fn draw(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(CLEAR.as_bytes());
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// Run the guided flow and write the result. Returns the keys written.
///
/// `input_poll_ms` is threaded through so the latency pass (the one piece of
/// configuration that depends on the *machine* rather than on taste) can be
/// applied silently up front, before the first question is drawn.
///
/// Navigation is two-axis and fully reversible: up/down picks an option,
/// right/Enter moves forward, left/backspace moves *back*. The summary is
/// simply the screen after the last question, so backspacing out of it returns
/// to that question with the earlier answer still highlighted - which is why
/// answers are stored per question index rather than appended.
pub fn run(cfg_path: &Path, input_poll_ms: u64) -> Vec<(String, String)> {
    if !std::io::stdin().is_terminal() {
        return Vec::new();
    }
    // Machine tuning first, and without asking: it has exactly one right
    // answer, so making it a question would be theater.
    let manual = crate::latency::apply_silently(input_poll_ms, cfg_path);

    let existing = std::fs::read_to_string(cfg_path).unwrap_or_default();
    let qs = with_existing(all_questions(), &existing);
    let total = qs.len();
    let mut chosen: Vec<Option<Answer>> = vec![None; total];
    let mut cursors: Vec<usize> = qs.iter().map(|q| q.default).collect();
    let raw = enable_raw_mode().is_ok();
    // The title card, played only on a first run: it is a greeting, and
    // greeting someone who is here to *change* a setting is just a delay.
    if !already_onboarded(&existing) {
        crate::splash::play(&current_palette(&existing), term_cols());
    }

    // `at == total` is the summary screen: one state machine, so "back" out of
    // the summary is the same code path as "back" between questions.
    let mut at = 0usize;
    let mut drained = false;
    loop {
        if at == total {
            // The one answer that changes the *machine* rather than a file,
            // done on the way to the summary so its real result (not merely
            // the answer) is what the screen reports. Backing up and changing
            // the answer re-runs it, which is why the outcome is computed here
            // rather than remembered from the first pass.
            let install = run_install(&qs, &chosen);
            draw(&summary_screen(
                &qs,
                &chosen,
                cfg_path,
                &manual,
                install.as_ref(),
            ));
            if !drained {
                // Drop keys typed *during* the flow so a stray Enter cannot
                // dismiss the summary before it has been read.
                while matches!(event::poll(Duration::from_millis(0)), Ok(true)) {
                    let _ = event::read();
                }
                drained = true;
            }
            // Only Enter and backspace mean anything at a screen that is not
            // asking a question; `summary_key` is what enforces that.
            match read_with(summary_key) {
                Some(Key::Prev) if total > 0 => {
                    at = total - 1;
                    // Re-ask with the previous answer under the cursor.
                    if let Some(a) = chosen[at].clone() {
                        park_cursor(&qs[at], &mut cursors[at], &a);
                    }
                    chosen[at] = None;
                }
                Some(Key::Next) | Some(Key::Abort) | None => break,
                Some(_) => {}
            }
            continue;
        }

        let q = &qs[at];
        // Only answers *before* this question feed the preview; the current
        // one is supplied by the highlight, so moving the cursor repaints.
        let so_far = answered(&qs[..at], &chosen[..at]);
        let (cols, rows) = term_size();
        // Re-measured every frame, not once at startup: a user who resizes
        // mid-flow gets a preview sized for the window they are looking at.
        // The mockup shrinks before it disappears, and disappears before the
        // question it is illustrating scrolls off the top.
        draw(&render_sized(
            q,
            at,
            total,
            cursors[at],
            &so_far,
            crate::preview::fits(cols, rows, question_rows(q)),
        ));
        let Some(key) = read_key() else {
            // EOF (a closed pipe, a terminal going away): take the defaults
            // for everything still unanswered rather than half-writing.
            fill_defaults(&qs, &mut chosen, at);
            at = total;
            continue;
        };
        match step(q, cursors[at], key) {
            Step::Move(c) => cursors[at] = c,
            Step::Ignore => {}
            Step::Abort => {
                if raw {
                    let _ = disable_raw_mode();
                }
                draw("");
                return Vec::new();
            }
            // Backspace on the first question has nowhere to go, so it is a
            // no-op rather than an accidental exit from setup.
            Step::Back => at = at.saturating_sub(1),
            Step::Done(Answer::RestDefaults) => {
                fill_defaults(&qs, &mut chosen, at);
                at = total;
            }
            Step::Done(a) => {
                park_cursor(q, &mut cursors[at], &a);
                chosen[at] = Some(a);
                at += 1;
            }
        }
    }

    if raw {
        let _ = disable_raw_mode();
    }
    // Hand a clean screen to whatever runs next (usually the agent harness).
    draw("");
    let mut out = answered(&qs, &chosen);
    out.push((MARKER.to_string(), "true".to_string()));
    out
}

/// Carry out the `btm` answer, if it was asked and said yes.
///
/// The screen is repainted first because this blocks for as long as a package
/// manager takes: a flow that went silent for a minute with no explanation
/// would read as a hang, and the one thing worse than a slow install is a slow
/// install nobody was told about.
fn run_install(qs: &[Question], chosen: &[Option<Answer>]) -> Option<crate::install::Outcome> {
    let i = qs.iter().position(|q| q.key == INSTALL_KEY)?;
    match chosen.get(i) {
        Some(Some(Answer::Set(v))) if v == "true" => {
            let plan = crate::install::plan(crate::install::Facts::probe());
            draw(&format!(
                "{BOLD}Installing {}\u{2026}{RESET}\r\n\r\n{DIM}This can take a minute; \
                 strimux is handling everything it needs.{RESET}\r\n",
                crate::install::TOOL
            ));
            Some(crate::install::run(&plan))
        }
        // Skipped or answered no: both mean "we left the machine alone".
        Some(Some(Answer::Set(_))) => Some(crate::install::Outcome::Declined),
        _ => None,
    }
}

/// Put the cursor on the option `a` selected, so coming back to this question
/// shows the answer rather than wherever the highlight happened to be sitting.
///
/// This matters most for the digit shortcut, which answers *without* moving
/// the highlight: `5` then backspace would otherwise re-open the question with
/// option 1 selected, quietly discarding what the user picked.
fn park_cursor(q: &Question, cursor: &mut usize, a: &Answer) {
    if let Answer::Set(v) = a {
        if let Some(pos) = q.options.iter().position(|o| o.value == v) {
            *cursor = pos;
        }
    }
}

/// Take the default for every question from `from` on that is still unanswered.
fn fill_defaults(qs: &[Question], chosen: &mut [Option<Answer>], from: usize) {
    for (j, c) in chosen.iter_mut().enumerate().skip(from) {
        if c.is_none() {
            *c = Some(qs[j].enter());
        }
    }
}

/// The `(key, value)` pairs for the questions that were actually answered.
fn answered(qs: &[Question], chosen: &[Option<Answer>]) -> Vec<(String, String)> {
    qs.iter()
        .zip(chosen)
        .filter_map(|(q, a)| match a {
            Some(Answer::Set(v)) => Some((q.key.to_string(), v.clone())),
            _ => None,
        })
        .collect()
}

/// Save the answers and render the closing screen describing what landed.
///
/// Saving here, rather than once the summary is dismissed, is what lets the
/// screen be honest: it reports the write it just made (or the error), and a
/// user who backs up and changes an answer gets the file rewritten before the
/// new summary claims anything about it.
fn summary_screen(
    qs: &[Question],
    chosen: &[Option<Answer>],
    cfg_path: &Path,
    manual: &Option<String>,
    install: Option<&crate::install::Outcome>,
) -> String {
    let shown = answered(qs, chosen);
    let mut answers = shown.clone();
    answers.push((MARKER.to_string(), "true".to_string()));
    let err = save_answers(cfg_path, &answers).err();
    let mut screen = render_summary(
        qs,
        if err.is_some() { &[] } else { &shown },
        cfg_path,
        manual.clone(),
        install,
    );
    if let Some(e) = &err {
        screen.push_str(&format!(
            "\r\n{YELLOW}Could not write {}: {e}{RESET}\r\n",
            cfg_path.display()
        ));
    }
    screen.push_str(&format!(
        "\r\n{DIM}\u{23ce} done \u{00b7} \u{232b} back to the last question{RESET}"
    ));
    screen
}

/// Offer onboarding from the agent gateway: only when this config has never
/// been through it, and only on a real terminal.
pub fn maybe_run(cfg_path: &Path, input_poll_ms: u64) {
    let text = std::fs::read_to_string(cfg_path).unwrap_or_default();
    if already_onboarded(&text) {
        return;
    }
    run(cfg_path, input_poll_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn the_preview_carries_earlier_answers_forward() {
        let qs = all();
        let i = qs.iter().position(|q| q.key == "cell_labels").unwrap();
        let mocha = render_screen(&qs[i], i, qs.len(), 0, &[], false);
        let nord = render_screen(
            &qs[i],
            i,
            qs.len(),
            0,
            &[("theme".to_string(), "\"nord\"".to_string())],
            false,
        );
        assert_ne!(
            mocha, nord,
            "an earlier theme answer did not reach the preview"
        );
    }

    /// Moving the highlight repaints the preview: that is what makes it a
    /// preview rather than a picture of the default.
    #[test]
    fn moving_the_highlight_changes_the_preview() {
        let qs = all();
        let i = qs.iter().position(|q| q.key == "startup_panes").unwrap();
        let one = render_screen(&qs[i], i, qs.len(), 0, &[], false);
        let four = render_screen(&qs[i], i, qs.len(), 3, &[], false);
        assert_ne!(one, four);
    }

    /// The install question changes the machine, not the screen, so it gets no
    /// mockup: an unchanging picture would teach that previews are decorative.
    #[test]
    fn the_install_question_gets_no_mockup() {
        let qs = all();
        let i = qs.iter().position(|q| q.key == INSTALL_KEY).unwrap();
        assert_eq!(
            render_screen(&qs[i], i, qs.len(), 0, &[], false),
            render_question(&qs[i], i, qs.len(), 0),
        );
    }

    /// Whatever the terminal size, the whole screen fits in it: the question a
    /// user is answering must never be scrolled off by its own illustration.
    #[test]
    fn no_screen_ever_overflows_the_terminal_it_was_sized_for() {
        let qs = all();
        for rows in 10u16..=60 {
            for cols in [70u16, 80, 100, 140] {
                for (i, q) in qs.iter().enumerate() {
                    let h = crate::preview::fits(cols, rows, question_rows(q));
                    let screen = render_sized(q, i, qs.len(), 0, &[], h);
                    let lines = screen.matches("\r\n").count();
                    // A question with more options than the terminal has rows
                    // does not fit with or without a preview; what must hold
                    // is that the *preview never makes it worse*, i.e. it is
                    // only ever drawn when there was room to spare.
                    let bare = render_question(q, i, qs.len(), 0).matches("\r\n").count();
                    if bare <= rows as usize {
                        assert!(
                            lines <= rows as usize,
                            "{cols}x{rows} q={} drew {lines} lines (bare {bare})",
                            q.key
                        );
                    } else {
                        assert_eq!(
                            h, None,
                            "{cols}x{rows} q={} previewed despite not fitting bare",
                            q.key
                        );
                    }
                }
            }
        }
    }

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
    fn accepting_every_default_changes_nothing() {
        // Defaults must equal Config::default(), or "just hit Enter" would
        // silently reconfigure someone's terminal.
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
        assert_eq!(cfg.palette(), d.palette());
        assert_eq!(cfg.default_column_width, d.default_column_width);
    }

    #[test]
    fn the_btm_offer_defaults_to_yes_and_is_skipped_when_it_is_already_there() {
        let missing = crate::install::Facts {
            installed: false,
            brew: true,
            cargo: true,
            macos: true,
        };
        let qs = all_questions_for(missing);
        let q = qs.last().expect("questions");
        assert_eq!(q.key, INSTALL_KEY, "the offer comes last");
        assert_eq!(q.default_value(), "true", "btm is recommended, so yes");
        // Already installed: the question would have only one honest answer,
        // so it is not asked at all.
        let have = crate::install::Facts {
            installed: true,
            ..missing
        };
        assert!(
            !all_questions_for(have).iter().any(|q| q.key == INSTALL_KEY),
            "offered an install to someone who already has it"
        );
    }

    #[test]
    fn the_install_answer_never_reaches_the_config_file() {
        // It is an action on the machine, not a setting: writing it would
        // invent a key the parser knows nothing about.
        let text = apply_answers(
            "",
            &[
                (INSTALL_KEY.into(), "true".into()),
                ("theme".into(), "\"nord\"".into()),
            ],
        );
        assert!(!text.contains("install"), "leaked into the config: {text}");
        assert!(text.contains("theme = \"nord\""), "{text}");
        let cfg: Config = toml::from_str(&text).expect("still parses");
        assert_eq!(cfg.palette(), Palette::NORD);
    }

    #[test]
    fn the_summary_reports_what_the_install_did_not_what_was_answered() {
        // "yes" and "installed" are different claims: a failed install that
        // showed up as a tick would send the user looking for a binary that
        // is not there.
        let qs = all();
        let answers = vec![(INSTALL_KEY.to_string(), "true".to_string())];
        let path = Path::new("/tmp/strimux.toml");
        let failed = crate::install::Outcome::Failed("brew exploded".into());
        let out = render_summary(&qs, &answers, path, None, Some(&failed));
        assert!(out.contains("not installed"), "{out}");
        assert!(out.contains("brew exploded"), "{out}");
        let ok = crate::install::Outcome::Installed;
        let out = render_summary(&qs, &answers, path, None, Some(&ok));
        assert!(out.contains("installed"), "{out}");
        assert!(!out.contains("not installed"), "{out}");
        // Declining says so rather than going silent.
        let no = crate::install::Outcome::Declined;
        let out = render_summary(&qs, &answers, path, None, Some(&no));
        assert!(out.contains("skipped"), "{out}");
    }

    #[test]
    fn going_back_shows_the_answer_that_was_given_even_via_a_digit() {
        // The regression this guards, caught by driving the real binary: a
        // digit answers *without* moving the highlight, so `5` then backspace
        // re-opened the question with option 1 selected and silently discarded
        // the choice. Both the answer path and the summary's "back" use this
        // helper, so they cannot drift apart again.
        let q = &questions()[0];
        let mut cursor = q.default;
        // The digit path: cursor is still on the default when Done arrives.
        let Step::Done(a) = step(q, cursor, Key::Digit(5)) else {
            panic!("a digit should answer outright");
        };
        assert_eq!(a, Answer::Set("\"nord\"".into()));
        park_cursor(q, &mut cursor, &a);
        assert_eq!(cursor, 4, "cursor did not follow the digit's answer");

        // The arrow path already agrees, and must keep agreeing.
        let mut cursor = 2;
        let Step::Done(a) = step(q, cursor, Key::Next) else {
            panic!("Enter should answer");
        };
        park_cursor(q, &mut cursor, &a);
        assert_eq!(cursor, 2);

        // A Skip parks nothing: there is no answer to point at.
        let mut cursor = 3;
        park_cursor(q, &mut cursor, &Answer::Skip);
        assert_eq!(cursor, 3, "Skip must not move the highlight");
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
    fn theme_options_are_the_real_presets() {
        let q = &questions()[0];
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

    #[test]
    fn arrows_and_jk_move_the_same_highlight_and_wrap() {
        let q = &questions()[0];
        let n = q.options.len();
        assert_eq!(step(q, 0, Key::Down), Step::Move(1));
        assert_eq!(step(q, 0, Key::Up), Step::Move(n - 1), "wraps to the end");
        assert_eq!(step(q, n - 1, Key::Down), Step::Move(0), "wraps to the top");
        for (code, want) in [
            (KeyCode::Down, Key::Down),
            (KeyCode::Char('j'), Key::Down),
            (KeyCode::Up, Key::Up),
            (KeyCode::Char('k'), Key::Up),
        ] {
            assert_eq!(key_from_event(code, KeyModifiers::NONE), want, "{code:?}");
        }
    }

    #[test]
    fn enter_takes_the_highlight_and_a_digit_takes_effect_immediately() {
        let q = &questions()[0];
        assert_eq!(
            step(q, 0, Key::Next),
            Step::Done(Answer::Set(q.default_value().into()))
        );
        assert_eq!(
            step(q, 4, Key::Next),
            Step::Done(Answer::Set("\"nord\"".into())),
            "Enter selects where the cursor is, not the factory default"
        );
        // A digit needs no Enter: it is unambiguous the moment it is typed.
        assert_eq!(
            step(q, 0, Key::Digit(3)),
            Step::Done(Answer::Set("\"tokyo-night\"".into()))
        );
        // ...and a digit past the end does nothing at all.
        assert_eq!(step(q, 0, Key::Digit(9)), Step::Ignore);
        assert_eq!(step(q, 0, Key::Other), Step::Ignore);
        assert_eq!(step(q, 0, Key::Skip), Step::Done(Answer::Skip));
        assert_eq!(step(q, 0, Key::Rest), Step::Done(Answer::RestDefaults));
        assert_eq!(step(q, 0, Key::Abort), Step::Abort);
    }

    #[test]
    fn h_l_and_the_horizontal_arrows_move_between_questions() {
        // Two axes, and each spelled three ways: vim keys, arrows, and the
        // conventional Enter/backspace. If these ever disagreed, the footer
        // would be teaching keys that do something else.
        for (code, want) in [
            (KeyCode::Char('l'), Key::Next),
            (KeyCode::Right, Key::Next),
            (KeyCode::Enter, Key::Next),
            (KeyCode::Char(' '), Key::Next),
            (KeyCode::Char('h'), Key::Prev),
            (KeyCode::Left, Key::Prev),
            (KeyCode::Backspace, Key::Prev),
        ] {
            assert_eq!(key_from_event(code, KeyModifiers::NONE), want, "{code:?}");
        }
        let q = &questions()[0];
        // Going back is not an answer: it must not set the key.
        assert_eq!(step(q, 3, Key::Prev), Step::Back);
        // ...and going forward from the same place still is.
        assert_eq!(
            step(q, 3, Key::Next),
            Step::Done(Answer::Set(q.options[3].value.into()))
        );
    }

    #[test]
    fn ctrl_c_aborts_and_esc_only_finishes_with_defaults() {
        assert_eq!(
            key_from_event(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Key::Abort
        );
        assert_eq!(key_from_event(KeyCode::Esc, KeyModifiers::NONE), Key::Rest);
        // A plain `c` is just an unknown key, not an abort.
        assert_eq!(
            key_from_event(KeyCode::Char('c'), KeyModifiers::NONE),
            Key::Other
        );
    }

    #[test]
    fn skipped_questions_leave_the_file_alone() {
        let before = "# mine\nstartup_panes = 3\n";
        let after = apply_answers(before, &[("theme".into(), "\"nord\"".into())]);
        assert!(
            after.contains("startup_panes = 3"),
            "clobbered an untouched key"
        );
        assert!(after.contains("# mine"), "dropped a comment");
        assert!(after.contains("theme = \"nord\""));
    }

    #[test]
    fn answers_replace_rather_than_duplicate() {
        let before = "theme = \"nord\"\nstartup_panes = 1\n";
        let after = apply_answers(before, &[("theme".into(), "\"dracula\"".into())]);
        assert_eq!(
            after.matches("theme =").count(),
            1,
            "duplicate key: {after}"
        );
        assert!(after.contains("theme = \"dracula\""));
    }

    #[test]
    fn cowsay_lands_inside_its_table() {
        let text = apply_answers("", &[("cowsay.enabled".into(), "true".into())]);
        let cfg: Config = toml::from_str(&text).expect("parses");
        assert!(
            cfg.cowsay.enabled,
            "cowsay.enabled was not written into [cowsay]"
        );
        assert!(!cfg.cowsay.messages.is_empty(), "hint list must survive");
        // And a second pass edits in place rather than stacking tables.
        let again = apply_answers(&text, &[("cowsay.enabled".into(), "false".into())]);
        assert_eq!(again.matches("[cowsay]").count(), 1, "{again}");
        assert_eq!(again.matches("enabled =").count(), 1, "{again}");
    }

    #[test]
    fn top_level_keys_never_fall_into_a_table() {
        // The classic corruption: a bare key appended after `[cowsay]` becomes
        // `cowsay.theme` and silently does nothing.
        let text = apply_answers(
            "[cowsay]\nenabled = true\n",
            &[("theme".into(), "\"nord\"".into())],
        );
        let cfg: Config = toml::from_str(&text).expect("parses");
        assert_eq!(cfg.palette(), Palette::NORD, "{text}");
    }

    #[test]
    fn existing_settings_become_the_defaults() {
        // Re-running setup must start from what the user has, not from the
        // factory settings, or `strimux init` becomes `strimux reset`.
        let text = "startup_panes = 3\ntheme = \"nord\"\n";
        let qs = with_existing(questions(), text);
        let theme = qs.iter().find(|q| q.key == "theme").unwrap();
        assert_eq!(theme.default_value(), "\"nord\"");
        let panes = qs.iter().find(|q| q.key == "startup_panes").unwrap();
        assert_eq!(panes.default_value(), "3");
    }

    #[test]
    fn a_hand_written_value_we_never_offer_is_kept_not_overwritten() {
        // The dangerous case: a custom `[theme]` table or an odd column width
        // has no matching option, so Enter must mean "leave it alone".
        let text = "[theme]\npreset = \"nord\"\naccent = \"#ff0000\"\n";
        let qs = with_existing(questions(), text);
        let theme = qs.iter().find(|q| q.key == "theme").unwrap();
        assert!(theme.keep_existing, "custom theme table was not detected");
        assert_eq!(theme.enter(), Answer::Skip);
        assert_eq!(
            step(theme, theme.default, Key::Next),
            Step::Done(Answer::Skip)
        );
        // Moving off the default and pressing Enter is a deliberate change,
        // so that one does write.
        assert_eq!(
            step(theme, 2, Key::Next),
            Step::Done(Answer::Set("\"tokyo-night\"".into()))
        );
    }

    #[test]
    fn accepting_defaults_through_a_rerun_is_a_no_op_on_an_existing_config() {
        // End to end for the property that makes the flow safe to offer at
        // every gateway visit: same parsed config in, same parsed config out.
        let before = "# hand written\nstartup_panes = 3\ndefault_agent = \"claude\"\n\n[theme]\npreset = \"nord\"\n";
        let qs = with_existing(all(), before);
        let answers: Vec<(String, String)> = qs
            .iter()
            .filter_map(|q| match q.enter() {
                Answer::Set(v) => Some((q.key.to_string(), v)),
                _ => None,
            })
            .collect();
        let after = apply_answers(before, &answers);
        let a: Config = toml::from_str(before).expect("before parses");
        let b: Config = toml::from_str(&after).expect("after parses");
        assert_eq!(a.startup_panes, b.startup_panes);
        assert_eq!(a.palette(), b.palette(), "{after}");
        assert_eq!(a.default_agent, b.default_agent);
        assert_eq!(a.default_column_width, b.default_column_width);
        assert!(after.contains("# hand written"), "{after}");
    }

    #[test]
    fn column_width_options_all_resolve_to_distinct_real_widths() {
        // These are the values the friendly `Width` wire format exists for;
        // if two spellings collapsed, the picker would be lying.
        let q = questions()
            .into_iter()
            .find(|q| q.key == "default_column_width")
            .unwrap();
        let mut seen = Vec::new();
        for o in &q.options {
            let cfg: Config =
                toml::from_str(&format!("default_column_width = {}\n", o.value)).unwrap();
            assert!(
                !seen.contains(&cfg.default_column_width),
                "{} duplicates",
                o.label
            );
            seen.push(cfg.default_column_width);
        }
    }

    #[test]
    fn marker_makes_onboarding_once_only() {
        assert!(!already_onboarded(""));
        assert!(!already_onboarded("default_agent = \"jcode\"\n"));
        assert!(!already_onboarded("# onboarded = true\n"));
        let text = apply_answers("", &[(MARKER.into(), "true".into())]);
        assert!(already_onboarded(&text));
        // And the marker itself must be a key the config parser tolerates.
        toml::from_str::<Config>(&text).expect("marker parses");
    }

    #[test]
    fn each_question_is_its_own_screen_with_a_visible_highlight() {
        let qs = all();
        let q = &qs[0];
        let screen = render_question(q, 0, qs.len(), 2);
        // The highlight is on the cursor, not on the factory default.
        let line = screen
            .lines()
            .find(|l| l.contains(q.options[2].label))
            .unwrap();
        assert!(line.contains('\u{276f}'), "no cursor marker: {line:?}");
        let other = screen
            .lines()
            .find(|l| l.contains(q.options[3].label))
            .unwrap();
        assert!(!other.contains('\u{276f}'), "two cursors: {other:?}");
        // Raw mode needs CR before LF or the screen stair-steps.
        assert!(!screen.contains("\n") || screen.contains("\r\n"));
        for l in screen.split("\r\n") {
            assert!(!l.contains('\n'), "bare LF in raw-mode output: {l:?}");
        }
        // The footer teaches every key that actually works, on both axes.
        for taught in ["jk", "back", "next", "skip", "\u{2190}", "\u{2192}"] {
            assert!(screen.contains(taught), "footer omits {taught:?}: {screen}");
        }
    }

    #[test]
    fn rendering_names_every_option() {
        let out = render_all();
        for q in all() {
            assert!(out.contains(q.prompt), "missing question {}", q.prompt);
            for o in &q.options {
                assert!(out.contains(o.label), "missing option {}", o.label);
            }
        }
    }

    #[test]
    fn the_summary_only_answers_to_enter_and_backspace() {
        // It is not asking a question, so a user pressing keys at it must not
        // fall through into something that looks like an answer. Exactly two
        // keys do anything.
        assert_eq!(summary_key(KeyCode::Enter, KeyModifiers::NONE), Key::Next);
        assert_eq!(
            summary_key(KeyCode::Backspace, KeyModifiers::NONE),
            Key::Prev
        );
        // Everything else is inert - including the keys that *do* mean "next"
        // on a question. Space, `l` and `→` are fine answers to "which of
        // these", and a terrible way to dismiss the one screen that reports
        // what was written to disk. Driving the real binary caught space
        // doing exactly that.
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char('l'),
            KeyCode::Right,
            KeyCode::Char('h'),
            KeyCode::Left,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char('3'),
            KeyCode::Char('s'),
            KeyCode::Char('q'),
            KeyCode::Esc,
        ] {
            assert_eq!(
                summary_key(code, KeyModifiers::NONE),
                Key::Other,
                "{code:?} must be inert on the summary screen"
            );
        }
        // Ctrl-C still gets you out, as it does everywhere.
        assert_eq!(
            summary_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Key::Abort
        );
        // ...and the question decoder stays permissive, since the two screens
        // deliberately differ.
        assert_eq!(
            key_from_event(KeyCode::Char(' '), KeyModifiers::NONE),
            Key::Next
        );
    }

    #[test]
    fn the_summary_screen_shows_every_answer_and_the_file_it_landed_in() {
        let qs = all();
        let answers = vec![
            ("theme".to_string(), "\"nord\"".to_string()),
            ("startup_panes".to_string(), "2".to_string()),
        ];
        let path = Path::new("/tmp/strimux.toml");
        let out = render_summary(&qs, &answers, path, Some("do the thing\n".into()), None);
        assert!(out.contains("nord"), "answered value missing: {out}");
        assert!(out.contains("Panes on screen at launch"), "{out}");
        assert!(out.contains("/tmp/strimux.toml"), "{out}");
        // Unanswered questions are reported as untouched, not as a value the
        // user never chose.
        assert!(out.contains("kept as it was"), "{out}");
        // Anything only the user can fix rides along on the same screen.
        assert!(out.contains("do the thing"), "{out}");
        for l in out.split("\r\n") {
            assert!(!l.contains('\n'), "bare LF in raw-mode output: {l:?}");
        }
    }
}
