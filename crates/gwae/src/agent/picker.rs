//! Picker rendering and choice parsing for the agent gateway.

use super::config::Plan;
use super::detect::{command_available, Found, KNOWN_AGENTS};
use std::io::{IsTerminal, Write};

pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const CYAN: &str = "\x1b[36m";
pub(crate) const YELLOW: &str = "\x1b[33m";
pub(crate) const RESET: &str = "\x1b[0m";

/// The pane's width in columns, for laying the picker out. Panes are often a
/// quarter of the screen, so the full-width form does not fit and its most
/// important part (the list) would scroll away above the prompt.
fn term_cols() -> usize {
    // 80 when there is no tty (a pipe, `--print` into a file): the wide form
    // is the better default for something a human will read later.
    crossterm::terminal::size()
        .ok()
        .map(|(c, _)| c as usize)
        .filter(|c| *c > 0)
        .unwrap_or(80)
}

/// Render the plan as the text the user sees, and return the choices that the
/// on-screen numbers map to. Pure in `cols`, so the exact layout at any pane
/// width is testable.
pub fn render_at(plan: &Plan, cols: usize) -> (String, Vec<Found>) {
    let mut header = String::new();
    if !matches!(plan, Plan::Configured(_) | Plan::Auto(_)) && cols >= 17 {
        header.push_str("gwae\n");
    }
    // Under ~50 columns (a quarter-width pane on a typical screen) the paths
    // and the explanatory footer push the list off the top of the pane, which
    // leaves the user staring at a prompt with no visible options. The narrow
    // form drops both and keeps the choices adjacent to the prompt.
    let narrow = cols < 50;
    let mut s = String::new();
    let choices = match plan {
        Plan::Configured(_) | Plan::Auto(_) => Vec::new(),
        Plan::Missing { want, found } => {
            if narrow {
                s.push_str(&format!(
                    "{YELLOW}{BOLD}`{want}` is not installed.{RESET}\n"
                ));
            } else {
                s.push_str(&format!(
                    "{YELLOW}{BOLD}`{want}` is not installed.{RESET}\n{DIM}Your config asks for it, but it is not on PATH. Pick another:{RESET}\n\n"
                ));
            }
            found.clone()
        }
        Plan::Choose(found) => {
            if narrow {
                s.push_str(&format!("{BOLD}Pick an agent:{RESET}\n"));
            } else {
                s.push_str(&format!(
                    "{BOLD}Which agent should {CYAN}⌥+;{RESET}{BOLD} launch?{RESET}\n{DIM}Found on your PATH:{RESET}\n\n"
                ));
            }
            found.clone()
        }
        Plan::NoneInstalled { want } => {
            match want {
                Some(w) if narrow => {
                    s.push_str(&format!("{YELLOW}{BOLD}`{w}` is not installed.{RESET}\n"))
                }
                Some(w) => s.push_str(&format!(
                    "{YELLOW}{BOLD}`{w}` is not installed{RESET}, and no other agent harness was found on your PATH.\n"
                )),
                None => s.push_str(&format!(
                    "{YELLOW}{BOLD}No agent harness found on your PATH.{RESET}\n"
                )),
            }
            if narrow {
                s.push_str(&format!(
                    "{DIM}Type its command, or Enter for a shell.{RESET}\n"
                ));
            } else {
                s.push_str(&format!(
                    "{DIM}Looked for: {}, plus anything on PATH that looks like an agent.{RESET}\n\nInstall one and press {CYAN}⌥+;{RESET} again, or type its command now\nif it lives somewhere we did not look. {DIM}Enter alone opens a shell.{RESET}\n",
                    KNOWN_AGENTS
                        .iter()
                        .map(|(c, _)| *c)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Vec::new()
        }
    };
    for (i, f) in choices.iter().enumerate() {
        // Enter takes the first entry, so it has to be labeled as such: an
        // unmarked default is one the user only discovers by triggering it.
        let dflt = if i == 0 {
            format!("  {DIM}(default){RESET}")
        } else {
            String::new()
        };
        if narrow {
            s.push_str(&format!(
                "  {CYAN}{}{RESET} {BOLD}{}{RESET}{dflt}\n",
                i + 1,
                f.label
            ));
        } else {
            s.push_str(&format!(
                "  {CYAN}{}{RESET}  {BOLD}{}{RESET}  {DIM}{}{RESET}{dflt}\n",
                i + 1,
                f.label,
                f.path.display()
            ));
        }
    }
    if !choices.is_empty() {
        if narrow {
            s.push_str(&format!(
                "  {CYAN}s{RESET} {BOLD}shell{RESET}\n{DIM}...or type a command.{RESET}\n"
            ));
        } else {
            s.push_str(&format!(
                "  {CYAN}s{RESET}  {BOLD}just a shell{RESET}  {DIM}skip, don't save{RESET}\n\n{DIM}Not listed? Type the command itself (e.g. {RESET}{CYAN}hermes --resume{RESET}{DIM}).\nYour choice is remembered, so ⌥+; goes straight there next time.{RESET}\n",
            ));
        }
    }
    if !header.is_empty() {
        s = header + &s;
    }
    (s, choices)
}

/// [`render_at`] using the live terminal width.
pub fn render(plan: &Plan) -> (String, Vec<Found>) {
    render_at(plan, term_cols())
}

/// What the user typed at the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// One of the listed harnesses, by index.
    Listed(usize),
    /// A command they typed themselves, which resolved on `PATH`. This is the
    /// escape hatch that makes a harness gwae has never heard of usable
    /// today rather than after a release.
    Typed(String),
    /// Just a shell; save nothing.
    Shell,
}

/// The executable word of a command line (`"jcode --resume"` -> `"jcode"`),
/// for naming the missing piece in an error.
fn shell_exe(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or("")
}

/// Interpret one line of picker input. Pure, so every branch (including the
/// typo cases a user actually hits) is testable without a terminal.
pub fn parse_choice(line: &str, n: usize) -> Result<Choice, String> {
    let t = line.trim();
    if t.is_empty() {
        // Enter takes the first (most preferred) harness, or a shell when
        // there is nothing to take.
        return Ok(if n > 0 {
            Choice::Listed(0)
        } else {
            Choice::Shell
        });
    }
    if t.eq_ignore_ascii_case("s") || t.eq_ignore_ascii_case("shell") {
        return Ok(Choice::Shell);
    }
    // A bare number only means an index when it *is* one; otherwise fall
    // through, since a command could plausibly be named oddly.
    if let Ok(i) = t.parse::<usize>() {
        return if i >= 1 && i <= n {
            Ok(Choice::Listed(i - 1))
        } else {
            Err(format!("There is no {i}. Enter 1-{n}, a command, or s."))
        };
    }
    if command_available(t) {
        return Ok(Choice::Typed(t.to_string()));
    }
    Err(format!(
        "`{}` is not on your PATH. Enter 1-{n}, another command, or s for a shell.",
        shell_exe(t)
    ))
}

/// Read a choice, re-prompting until it is valid. Returns [`Choice::Shell`]
/// on EOF or a non-tty, so the gateway can never wedge a pane waiting for
/// input that will not come.
pub(super) fn prompt(n: usize) -> Choice {
    use std::io::BufRead;
    if !std::io::stdin().is_terminal() {
        return Choice::Shell;
    }
    let stdin = std::io::stdin();
    loop {
        print!("\n{CYAN}>{RESET} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Choice::Shell,
            Ok(_) => {}
        }
        match parse_choice(&line, n) {
            Ok(c) => return c,
            Err(msg) => println!("{DIM}{msg}{RESET}"),
        }
    }
}

/// `gwae agent`: resolve, maybe ask, save, and exec. Never returns.
#[cfg(test)]
mod tests {
    use super::super::config::{fallback_shell, plan, Plan};
    use super::super::detect::Found;
    use super::super::state::HarnessState;
    #[cfg(unix)]
    use super::Choice;
    use super::*;
    use std::path::PathBuf;

    /// Drop SGR escapes so assertions read the text a user sees.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn found(cmd: &str) -> Found {
        Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: PathBuf::from("/usr/bin").join(cmd),
        }
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn a_resolvable_configured_agent_short_circuits_every_prompt() {
        let s = HarnessState::default();
        // The common case must never paint: config wins, no detection UI.
        assert_eq!(
            plan("sh", &s, vec![found("jcode")]),
            Plan::Configured("sh".into())
        );
        // Whitespace is not a configuration.
        assert!(matches!(
            plan("   ", &s, vec![]),
            Plan::NoneInstalled { .. }
        ));
    }

    #[test]
    fn unset_agent_with_installs_offers_a_choice_and_without_them_falls_back() {
        let s = HarnessState::default();
        assert_eq!(
            plan("", &s, vec![found("jcode"), found("claude")]),
            Plan::Choose(vec![found("jcode"), found("claude")])
        );
        assert_eq!(plan("", &s, vec![]), Plan::NoneInstalled { want: None });
    }

    #[test]
    fn a_configured_but_missing_agent_reports_what_was_wanted() {
        let s = HarnessState::default();
        assert_eq!(
            plan("jcode-not-real", &s, vec![found("claude")]),
            Plan::Missing {
                want: "jcode-not-real".into(),
                found: vec![found("claude")],
            }
        );
        assert_eq!(
            plan("jcode-not-real", &s, vec![]),
            Plan::NoneInstalled {
                want: Some("jcode-not-real".into())
            }
        );
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn rendering_lists_every_choice_and_names_the_missing_agent() {
        let (text, choices) = render(&Plan::Missing {
            want: "jcode".into(),
            found: vec![found("claude"), found("codex")],
        });
        assert!(text.contains("`jcode` is not installed"));
        assert!(text.contains("/usr/bin/claude"));
        assert!(text.contains("/usr/bin/codex"));
        // Numbering is 1-based and matches the returned choice order.
        let plain = strip_ansi(&text);
        assert!(plain.contains("1  claude"), "{plain}");
        assert!(plain.contains("2  codex"), "{plain}");
        assert!(plain.find("1  claude") < plain.find("2  codex"));
        // "just a shell" is always an out, so the pane is never a dead end.
        assert!(plain.contains("just a shell"));
        // Enter picks the first entry, so the list must say which that is.
        assert!(plain.contains("1  claude"));
        let dflt = plain.find("(default)").expect("default marked");
        assert!(dflt > plain.find("1  claude").unwrap() && dflt < plain.find("2  codex").unwrap());
        assert_eq!(choices.len(), 2);
        // A resolved config renders nothing at all.
        let (text, choices) = render(&Plan::Configured("jcode".into()));
        assert!(text.is_empty());
        assert!(choices.is_empty());
    }

    #[test]
    fn the_none_installed_screen_lists_what_was_searched_for() {
        let (text, choices) = render(&Plan::NoneInstalled { want: None });
        assert!(text.contains("No agent harness found"));
        assert!(text.contains("jcode"));
        assert!(text.contains("aider"));
        assert!(choices.is_empty());
    }

    #[test]
    fn a_narrow_pane_keeps_the_choices_next_to_the_prompt() {
        // A quarter-width pane is ~24 columns. The wide form's paths and
        // footer scroll the list off the top, leaving a prompt with no
        // visible options, which is the one thing the picker must never do.
        let plan = Plan::Choose(vec![found("claude"), found("aider")]);
        let (wide, _) = render_at(&plan, 100);
        let (narrow, _) = render_at(&plan, 24);
        assert!(
            narrow.lines().count() < wide.lines().count(),
            "narrow must be shorter:\n{narrow}"
        );
        let plain = strip_ansi(&narrow);
        // Everything needed to choose is still there.
        assert!(plain.contains("1 claude"), "{plain}");
        assert!(plain.contains("2 aider"), "{plain}");
        assert!(plain.contains("(default)"), "{plain}");
        assert!(plain.contains("s shell"), "{plain}");
        assert!(plain.contains("type a command"), "{plain}");
        // The long path and footer, which are what overflowed, are gone.
        assert!(!plain.contains("/usr/bin/claude"), "{plain}");
        assert!(!plain.contains("goes straight there"), "{plain}");
        // And it fits: every line inside the pane width.
        for l in plain.lines() {
            assert!(l.chars().count() <= 24, "line too wide: {l:?}");
        }
    }

    #[test]
    fn the_narrow_none_installed_screen_still_says_what_to_do() {
        let (narrow, _) = render_at(&Plan::NoneInstalled { want: None }, 24);
        let plain = strip_ansi(&narrow);
        assert!(plain.contains("No agent harness found"), "{plain}");
        assert!(plain.contains("Type its command"), "{plain}");
        assert!(plain.contains("Enter for a shell"), "{plain}");
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn parse_choice_accepts_indexes_typed_commands_and_shell() {
        assert_eq!(parse_choice("2", 3), Ok(Choice::Listed(1)));
        assert_eq!(parse_choice("  1  ", 3), Ok(Choice::Listed(0)));
        assert_eq!(parse_choice("", 3), Ok(Choice::Listed(0)), "Enter = first");
        assert_eq!(parse_choice("s", 3), Ok(Choice::Shell));
        assert_eq!(parse_choice("SHELL", 3), Ok(Choice::Shell));
        // With nothing listed, Enter can only mean a shell.
        assert_eq!(parse_choice("", 0), Ok(Choice::Shell));
        // The escape hatch: any real command, with or without args.
        assert_eq!(parse_choice("sh", 3), Ok(Choice::Typed("sh".into())));
        assert_eq!(
            parse_choice("sh --resume", 3),
            Ok(Choice::Typed("sh --resume".into()))
        );
    }

    #[test]
    fn parse_choice_explains_rejections_instead_of_silently_failing() {
        // An out-of-range number is a typo, not a command.
        let e = parse_choice("9", 3).unwrap_err();
        assert!(e.contains("no 9"), "{e}");
        assert!(e.contains("1-3"), "{e}");
        // A command that does not exist says so by name, so the user can see
        // the typo rather than wondering why nothing happened.
        let e = parse_choice("hermes-not-installed", 3).unwrap_err();
        assert!(e.contains("hermes-not-installed"), "{e}");
        assert!(e.contains("not on your PATH"), "{e}");
        // Args are stripped when naming the missing executable.
        let e = parse_choice("hermes-nope --resume", 3).unwrap_err();
        assert!(e.contains("`hermes-nope`"), "{e}");
    }

    #[test]
    fn fallback_shell_is_never_empty() {
        assert!(!fallback_shell().is_empty());
    }
}
