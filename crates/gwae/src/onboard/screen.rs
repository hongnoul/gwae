//! Onboarding input and rendering: keys, steps, and screens.

use super::questions::{all_questions_for, Answer, Opt, Question, INSTALL_KEY, BOLD, CLEAR, CYAN, DIM, GREEN, RESET, YELLOW};
use crate::theme::Palette;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use gwae_term::CColor;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

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
pub fn bg(c: CColor) -> String {
    match c {
        CColor::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        CColor::Idx(i) => format!("\x1b[48;5;{i}m"),
        CColor::Default => "\x1b[49m".to_string(),
    }
}

/// Render one question as its own screen, with `cursor` highlighted. Pure, so
/// `gwae init --print` and the tests see the real thing.
///
/// Lines end with `\r\n`: the flow runs in raw mode (to read arrow keys
/// without Enter), where a bare `\n` would stair-step down the screen.
pub fn render_question(q: &Question, _idx: usize, _total: usize, cursor: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{BOLD}{}{RESET}\r\n{DIM}{}{RESET}\r\n\r\n",
        q.prompt, q.help
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
            " {arrow}{star} {name}{pad}{sw} {DIM}{}{RESET}\r\n",
            o.blurb
        ));
    }
    s.push_str("\r\n");
    s.push_str(&if q.keep_existing {
        format!(
            "{DIM}\u{2191}\u{2193}/jk pick \u{00b7} \u{2192}/l/\u{23ce} keep your current setting \
             \u{00b7} \u{2190}/h/\u{232b} back \u{00b7} esc defaults for the rest{RESET}\r\n"
        )
    } else {
        format!(
            "{DIM}\u{2191}\u{2193}/jk pick \u{00b7} \u{2192}/l/\u{23ce} next \u{00b7} \
             \u{2190}/h/\u{232b} back \u{00b7} s skip \u{00b7} esc defaults for the \
             rest{RESET}\r\n"
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
    prefs.apply(q.key, q.options[cursor].value);
    s.push_str("\r\n");
    s.push_str(&crate::preview::render_h(&prefs, h, true));
    s
}

/// The preview state for a question, or `None` when a picture would be a lie.
///
/// A question earns a mockup only if the mockup can *answer* it. `install.btm`
/// changes the machine, not the screen, and drawing an
/// unchanged grid under it would quietly teach the user that the preview
/// is decorative.
pub fn previewable(
    q: &Question,
    answered_so_far: &[(String, String)],
) -> Option<crate::preview::Prefs> {
    if q.key == INSTALL_KEY || q.key == "default_agent" {
        return None;
    }
    Some(crate::preview::Prefs::from_pairs(answered_so_far))
}

/// The whole flow as text, for `gwae init --print` and for docs.
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
    let mut s = format!("{BOLD}gwae is configured.{RESET}\r\n\r\n");
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
        "{DIM}Run {RESET}{CYAN}gwae init{RESET}{DIM} any time to change it, and see \
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

pub fn read_key() -> Option<Key> {
    read_with(key_from_event)
}

/// Read one keystroke, decoded by `f`. Lets the summary screen use a stricter
/// decoder than the questions without duplicating the event loop.
#[allow(dead_code)]
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
pub fn term_cols() -> u16 {
    crossterm::terminal::size().map(|(c, _)| c).unwrap_or(80)
}

/// The rows a question itself occupies: prompt, help, blank, one per option,
/// blank, key hints.
pub fn question_rows(q: &Question) -> usize {
    q.options.len() + 5
}

/// Rows the banner occupies: one line fallback on narrow terminals, else the full art.
pub fn banner_lines(cols: u16) -> usize {
    if (cols as usize) < crate::splash::art_width() + 2 {
        1
    } else {
        crate::splash::BANNER_LINES
    }
}

/// Palette the banner should be painted in right now.
///
/// The banner is the wordmark, so it should wear the theme the user is *looking
/// at*: the highlight when the current question is the theme picker, else the
/// already-answered theme, else the palette the config already had.
pub fn banner_palette(
    so_far: &[(String, String)],
    q: &Question,
    cursor: usize,
    fallback: &Palette,
) -> Palette {
    if q.key == "theme" {
        if let Some(p) = Palette::preset(q.options[cursor].label) {
            return p;
        }
    }
    for (k, v) in so_far.iter().rev() {
        if k == "theme" {
            let name = v.trim_matches('"');
            if let Some(p) = Palette::preset(name) {
                return p;
            }
        }
    }
    *fallback
}

/// Resolve the palette from already-answered pairs (for the summary screen).
pub fn palette_from_pairs(pairs: &[(String, String)], fallback: &Palette) -> Palette {
    for (k, v) in pairs.iter().rev() {
        if k == "theme" {
            let name = v.trim_matches('"');
            if let Some(p) = Palette::preset(name) {
                return p;
            }
        }
    }
    *fallback
}

/// The terminal size, defaulting to a conservative 80x24.
pub fn term_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// The palette the flow paints itself in: whatever the config already says,
/// so a re-run of `gwae init` opens in the theme the user picked last time.
pub fn current_palette(text: &str) -> Palette {
    toml::from_str::<crate::config::Config>(text)
        .map(|c| c.palette())
        .unwrap_or_default()
}

pub fn extra_agents_from_text(text: &str) -> Vec<String> {
    toml::from_str::<crate::config::Config>(text)
        .map(|c| c.agents)
        .unwrap_or_default()
}

/// Draw one screen and flush it, so a question is never half-painted while we
/// are already blocking on a keystroke.
pub fn draw(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(CLEAR.as_bytes());
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// Draw the animated banner plus `body` as one screen. On short terminals,
/// omit the decoration rather than scrolling the question off the screen.
pub fn draw_with_banner(banner_step: usize, pal: &Palette, cols: u16, rows: u16, body: &str) {
    let banner = crate::splash::banner(banner_step, pal, cols);
    if banner.lines().count() + body.lines().count() > rows as usize {
        draw(body);
        return;
    }
    let mut s = String::with_capacity(banner.len() + body.len());
    s.push_str(&banner);
    s.push_str(body);
    draw(&s);
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
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::persist::apply_answers;
    use super::super::questions::questions;
    use crate::theme::Palette;

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
        let i = qs.iter().position(|q| q.key == "center_focus").unwrap();
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
        let i = qs
            .iter()
            .position(|q| q.key == "default_column_width")
            .unwrap();
        let one = render_screen(&qs[i], i, qs.len(), 0, &[], false);
        let four = render_screen(&qs[i], i, qs.len(), 3, &[], false);
        assert_ne!(one, four);
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
    fn arrows_and_jk_move_the_same_highlight_and_wrap() {
        let qs = questions();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
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
        let qs = questions();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
        assert_eq!(
            step(q, q.default, Key::Next),
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
        // The ninth choice is white phosphor; an out-of-range choice is ignored.
        assert_eq!(
            step(q, 0, Key::Digit(9)),
            Step::Done(Answer::Set("\"white-phosphor\"".into()))
        );
        assert_eq!(step(q, 0, Key::Digit(10)), Step::Ignore);
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
        let qs = questions();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
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
    fn each_question_is_its_own_screen_with_a_visible_highlight() {
        let qs = all();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
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
            ("center_focus".to_string(), "true".to_string()),
        ];
        let path = Path::new("/tmp/gwae.toml");
        let out = render_summary(&qs, &answers, path, Some("do the thing\n".into()), None);
        assert!(out.contains("nord"), "answered value missing: {out}");
        assert!(out.contains("Scrolling style"), "{out}");
        assert!(out.contains("/tmp/gwae.toml"), "{out}");
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
