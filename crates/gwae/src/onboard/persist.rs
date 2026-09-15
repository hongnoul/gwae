//! Onboarding persistence and driver: answers, save, and run loop.

use super::questions::{all_questions_with_extra, with_existing, Answer, Question, BOLD, DIM, INSTALL_KEY, MARKER, RESET, YELLOW};
use super::screen::{draw, render_summary, banner_lines, banner_palette, current_palette, draw_with_banner, extra_agents_from_text, key_from_event, palette_from_pairs, question_rows, render_sized, step, term_cols, term_size, Key, Step, summary_key};
use crossterm::event::{Event, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

pub fn apply_answers(text: &str, answers: &[(String, String)]) -> String {
    let mut out = if text.trim().is_empty() {
        "# gwae configuration\n# Written by `gwae init`; edit freely, \
         gwae only ever rewrites the keys it owns.\n"
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
#[allow(dead_code)]
pub fn run(cfg_path: &Path, input_poll_ms: u64) -> Vec<(String, String)> {
    if !std::io::stdin().is_terminal() {
        return Vec::new();
    }
    // Machine tuning first, and without asking: it has exactly one right
    // answer, so making it a question would be theater.
    let manual = crate::latency::apply_silently(input_poll_ms, cfg_path);

    let existing = std::fs::read_to_string(cfg_path).unwrap_or_default();
    let extra = extra_agents_from_text(&existing);
    let qs = with_existing(all_questions_with_extra(&extra), &existing);
    let total = qs.len();
    let mut chosen: Vec<Option<Answer>> = vec![None; total];
    let mut cursors: Vec<usize> = qs.iter().map(|q| q.default).collect();
    let raw = enable_raw_mode().is_ok();
    let base_palette = current_palette(&existing);
    let splash_completed = crate::splash::play(&base_palette, term_cols());

    // `at == total` is the summary screen: one state machine, so "back" out of
    // the summary is the same code path as "back" between questions.
    let mut at = 0usize;
    let mut banner_step: usize = if splash_completed {
        crate::splash::frames().saturating_sub(1)
    } else {
        0
    };
    // Cache the summary that was saved, so animation ticks don't re-save.
    let mut summary_cache: Option<(String, Option<crate::install::Outcome>)> = None;
    loop {
        if at == total {
            // Compute and save once per entry to the summary; animation ticks
            // only repaint.
            if summary_cache.is_none() {
                // Drop keys typed during the flow so a stray Enter cannot
                // dismiss the summary before it has been read.
                while matches!(event::poll(Duration::from_millis(0)), Ok(true)) {
                    let _ = event::read();
                }
                let install = run_install(&qs, &chosen);
                // `summary_screen` saves; keep its body and the install outcome
                // so ticks don't re-run the installer or rewrite the file.
                let body = summary_screen(&qs, &chosen, cfg_path, &manual, install.as_ref());
                summary_cache = Some((body, install));
            }
            let body = summary_cache.as_ref().unwrap().0.clone();
            let shown = answered(&qs, &chosen);
            let pal = palette_from_pairs(&shown, &base_palette);
            let (cols, rows) = term_size();
            draw_with_banner(banner_step, &pal, cols, rows, &body);
            // Animated wait: repaint banner every TICK even without input.
            let mut go_back = false;
            let mut done = false;
            match event::poll(crate::splash::TICK) {
                Ok(true) => {
                    let key = match event::read() {
                        Ok(Event::Key(ke)) if ke.kind != KeyEventKind::Release => {
                            summary_key(ke.code, ke.modifiers)
                        }
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                    match key {
                        Key::Prev if total > 0 => go_back = true,
                        Key::Next | Key::Abort => done = true,
                        _ => {}
                    }
                }
                Ok(false) => {
                    if banner_step + 1 < crate::splash::frames() {
                        banner_step += 1;
                    }
                }
                Err(_) => break,
            }
            if go_back {
                at = total - 1;
                if let Some(a) = chosen[at].clone() {
                    park_cursor(&qs[at], &mut cursors[at], &a);
                }
                chosen[at] = None;
                summary_cache = None;
            } else if done {
                break;
            }
            continue;
        }
        // Leaving/entering summary invalidates the cached save.
        summary_cache = None;

        let q = &qs[at];
        // Only answers *before* this question feed the preview; the current
        // one is supplied by the highlight, so moving the cursor repaints.
        let so_far = answered(&qs[..at], &chosen[..at]);
        let (cols, rows) = term_size();
        let bl = banner_lines(cols);
        let chrome = question_rows(q) + bl;
        let h = crate::preview::fits(cols, rows, chrome);
        let body = render_sized(q, at, total, cursors[at], &so_far, h);
        let pal = banner_palette(&so_far, q, cursors[at], &base_palette);
        draw_with_banner(banner_step, &pal, cols, rows, &body);
        match event::poll(crate::splash::TICK) {
            Ok(true) => {
                let raw_key = match event::read() {
                    Ok(Event::Key(ke)) if ke.kind != KeyEventKind::Release => {
                        key_from_event(ke.code, ke.modifiers)
                    }
                    Ok(_) => continue,
                    Err(_) => {
                        fill_defaults(&qs, &mut chosen, at);
                        at = total;
                        continue;
                    }
                };
                match step(q, cursors[at], raw_key) {
                    Step::Move(c) => cursors[at] = c,
                    Step::Ignore => {}
                    Step::Abort => {
                        if raw {
                            let _ = disable_raw_mode();
                        }
                        draw("");
                        return Vec::new();
                    }
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
            Ok(false) => {
                if banner_step + 1 < crate::splash::frames() {
                    banner_step += 1;
                }
            }
            Err(_) => {
                fill_defaults(&qs, &mut chosen, at);
                at = total;
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
                 gwae is handling everything it needs.{RESET}\r\n",
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
#[allow(dead_code)]
pub fn maybe_run(cfg_path: &Path, input_poll_ms: u64) {
    let text = std::fs::read_to_string(cfg_path).unwrap_or_default();
    if already_onboarded(&text) {
        return;
    }
    run(cfg_path, input_poll_ms);
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::questions::{all_questions_for, questions, with_existing, Answer, Question};
    use super::super::screen::{render_screen, render_sized, Key, Step};
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
    fn going_back_shows_the_answer_that_was_given_even_via_a_digit() {
        // The regression this guards, caught by driving the real binary: a
        // digit answers *without* moving the highlight, so `5` then backspace
        // re-opened the question with option 1 selected and silently discarded
        // the choice. Both the answer path and the summary's "back" use this
        // helper, so they cannot drift apart again.
        let qs = questions();
        let q = qs.iter().find(|q| q.key == "theme").unwrap();
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
        // factory settings, or `gwae init` becomes `gwae reset`.
        let text = "center_focus = true\ntheme = \"nord\"\n";
        let qs = with_existing(questions(), text);
        let theme = qs.iter().find(|q| q.key == "theme").unwrap();
        assert_eq!(theme.default_value(), "\"nord\"");
        let focus = qs.iter().find(|q| q.key == "center_focus").unwrap();
        assert_eq!(focus.default_value(), "true");
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
    fn marker_makes_onboarding_once_only() {
        assert!(!already_onboarded(""));
        assert!(!already_onboarded("default_agent = \"jcode\"\n"));
        assert!(!already_onboarded("# onboarded = true\n"));
        let text = apply_answers("", &[(MARKER.into(), "true".into())]);
        assert!(already_onboarded(&text));
        // And the marker itself must be a key the config parser tolerates.
        toml::from_str::<Config>(&text).expect("marker parses");
    }
}
