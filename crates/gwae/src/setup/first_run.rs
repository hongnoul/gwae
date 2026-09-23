//! First-run configuration in the native terminal, before the TUI opens.
//!
//! The agent pick used to happen inside a small TUI pane (`gwae agent`
//! gateway): narrow, cramped picker form, and the latency step never ran at
//! all unless the user found `gwae init`. Instead the very first `gwae` run
//! (no config file yet) configures on stdin/stdout at full terminal width,
//! then enters the TUI. `gwae init` runs the same flow on demand. Non-tty
//! sessions skip every prompt so scripts and CI never block.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use crate::agent::{parse_choice, render, Choice, Found, HarnessState};
use crate::config::Config;

/// Styling for the first-run lines, gated the way clig.dev prescribes: no
/// escapes when stdout is not a tty, when `NO_COLOR` is set (any value), or
/// when `TERM=dumb`. First run only prompts on a tty, but `gwae init` can be
/// scripted or redirected, and the transcript must stay grep-clean there.
struct Style {
    on: bool,
}

impl Style {
    fn detect() -> Self {
        let on = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
            && std::env::var_os("TERM").is_none_or(|t| t != "dumb");
        Style { on }
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    /// A completed step: `✓ agent: jcode — ⌥+; goes straight there`.
    fn done(&self, s: &str) {
        println!("{} {s}", self.green("✓"));
    }
    /// A step header: `[1/1] agent`.
    fn step(&self, i: usize, n: usize, name: &str) {
        println!("{} {}", self.dim(&format!("[{i}/{n}]")), self.bold(name));
    }
    /// Pass text through, or strip SGR escapes when styling is off. The
    /// shared picker renders with colors baked in (it also runs inside TUI
    /// panes); first run owns the plain-output contract here, so `NO_COLOR`
    /// and `TERM=dumb` strip that text too instead of leaking escapes.
    fn filter(&self, s: &str) -> String {
        if self.on {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find('\x1b') {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            match tail.find('m') {
                Some(j) => rest = &tail[j + 1..],
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// True when this looks like a fresh machine: no config file yet.
pub fn is_first_run(cfg_path: &Path) -> bool {
    std::fs::metadata(cfg_path).is_err()
}

/// Run the native first-run flow when needed (or forced via `gwae init`),
/// then return the config the TUI should start with.
///
/// Never prompts without a tty: piped sessions get the silent path (apply
/// our own latency default, leave the rest for `gwae setup`).
pub fn ensure(cfg_path: &Path, force: bool) -> Config {
    if !force && !is_first_run(cfg_path) {
        return Config::load(cfg_path);
    }
    if !std::io::stdin().is_terminal() {
        let _ = crate::latency::save_input_poll(cfg_path, 1);
        return Config::load(cfg_path);
    }

    let sty = Style::detect();
    println!(
        "{} {}\n",
        sty.bold("gwae"),
        sty.dim("· one-time setup in this terminal")
    );

    sty.step(1, 1, "agent");
    agent_step(cfg_path, &sty);

    tune_latency_silently(cfg_path);

    println!(
        "\n{} {}\n",
        sty.green("✓"),
        sty.bold("ready — entering gwae…")
    );
    Config::load(cfg_path)
}

/// Pick the `⌥+;` harness at full terminal width and remember it in the
/// state file. Fast paths (override, memory, lone install) print one line
/// and never prompt.
fn agent_step(cfg_path: &Path, sty: &Style) {
    use crate::agent::{detect, load_harness_state, plan, Plan};
    let cfg = Config::load(cfg_path);
    let state_path = crate::agent::harness_state_path();
    let mut state: HarnessState = state_path
        .as_deref()
        .map(load_harness_state)
        .unwrap_or_default();
    let ordered = state.clone().order(detect());
    match plan(&cfg.default_agent, &state, ordered) {
        Plan::Configured(cmd) => {
            sty.done(&format!("agent: {cmd} — ⌥+; goes straight there"));
        }
        Plan::Auto(cmd) => {
            remember(&mut state, state_path.as_deref(), &cmd, true);
            sty.done(&format!("agent: {cmd} {}", sty.dim("(only one installed)")));
        }
        ref chooser @ (Plan::Choose(_) | Plan::Missing { .. }) => {
            let choices = match chooser {
                Plan::Choose(f) => f.clone(),
                Plan::Missing { found, .. } => found.clone(),
                _ => unreachable!(),
            };
            let choice = interactive_pick(chooser, &choices, sty).unwrap_or_else(|| {
                // No tty control (or raw mode failed): the numbered prompt
                // still works everywhere a line can be read.
                let (text, cs) = render(chooser);
                print!("{}", sty.filter(&text));
                let _ = std::io::stdout().flush();
                prompt_loop(cs.len(), sty)
            });
            match choice {
                Choice::Listed(i) => {
                    let cmd = choices.get(i).map(|f| f.cmd.clone()).unwrap_or_default();
                    if !cmd.is_empty() {
                        remember(&mut state, state_path.as_deref(), &cmd, true);
                        sty.done(&format!("agent: {cmd} — ⌥+; goes straight there"));
                    }
                }
                Choice::Typed(cmd) => {
                    remember(&mut state, state_path.as_deref(), &cmd, false);
                    sty.done(&format!("agent: {cmd} — ⌥+; goes straight there"));
                }
                Choice::Shell => sty.done(&format!(
                    "agent: skipped {}",
                    sty.dim("— ⌥+; will offer a shell")
                )),
            }
        }
        Plan::NoneInstalled { .. } => {
            let (text, _) = render(&Plan::NoneInstalled { want: None });
            print!("{}", sty.filter(&text));
            let _ = std::io::stdout().flush();
            match prompt_loop(0, sty) {
                Choice::Typed(cmd) => {
                    remember(&mut state, state_path.as_deref(), &cmd, false);
                    sty.done(&format!("agent: {cmd} — ⌥+; goes straight there"));
                }
                _ => sty.done(&format!("agent: none {}", sty.dim("— panes open a shell"))),
            }
        }
    }
}

fn remember(state: &mut HarnessState, path: Option<&Path>, cmd: &str, known: bool) {
    state.record_pick(cmd, known);
    if let Some(p) = path {
        let _ = crate::agent::save_harness_state(p, state);
    }
}

/// Arrow-key selector for the harness list: ↑/↓ or j/k move a highlight,
/// Enter picks, `s`/Esc take a shell, `t` drops to the typed-command prompt,
/// and a digit still jumps straight to that entry. Returns `None` when the
/// terminal cannot do raw input (styling off, no tty, raw mode refused), so
/// the caller can fall back to the line-based prompt.
fn interactive_pick(
    plan: &crate::agent::Plan,
    choices: &[Found],
    sty: &Style,
) -> Option<Choice> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::{cursor, execute, terminal};
    if !sty.on || !std::io::stdin().is_terminal() || choices.is_empty() {
        return None;
    }
    // Paths would wrap in a narrow terminal, which breaks the redraw's
    // MoveUp arithmetic, so they only paint when there is room.
    let cols = terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
    let show_paths = cols >= 60;
    terminal::enable_raw_mode().ok()?;
    let mut out = std::io::stdout();
    let header = match plan {
        crate::agent::Plan::Missing { want, .. } => format!(
            "{}\r\n{}\r\n",
            sty.paint("33;1", &format!("`{want}` is not installed.")),
            sty.dim("Your config asks for it, but it is not on PATH. Pick another:")
        ),
        _ => format!(
            "{}\r\n{}\r\n",
            sty.bold("Which agent should ⌥+; launch?"),
            sty.dim("↑/↓ or j/k move · Enter picks · s shell · t type a command")
        ),
    };
    let _ = write!(out, "{header}");
    // The shell opt-out is a real row, so every outcome is reachable with
    // just the arrows and Enter.
    let total = choices.len() + 1;
    let draw = |out: &mut std::io::Stdout, sel: usize| {
        let row = |out: &mut std::io::Stdout, i: usize, label: &str, note: &str| {
            let (marker, name) = if i == sel {
                (sty.paint("36;1", "❯"), sty.paint("36;1", label))
            } else {
                (" ".to_string(), sty.bold(label))
            };
            let _ = write!(out, "  {marker} {name}  {}\r\n", sty.dim(note));
        };
        for (i, f) in choices.iter().enumerate() {
            let note = if show_paths {
                f.path.display().to_string()
            } else {
                String::new()
            };
            row(out, i, &f.label, &note);
        }
        row(out, choices.len(), "just a shell", "skip, don't save");
        let _ = out.flush();
    };
    let mut sel = 0usize;
    draw(&mut out, sel);
    let choice = loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                match (k.code, k.modifiers) {
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        let _ = terminal::disable_raw_mode();
                        let _ = write!(out, "\r\n");
                        std::process::exit(130);
                    }
                    (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                        sel = if sel == 0 { total - 1 } else { sel - 1 };
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), _) => sel = (sel + 1) % total,
                    (KeyCode::Enter, _) => {
                        break if sel < choices.len() {
                            Choice::Listed(sel)
                        } else {
                            Choice::Shell
                        };
                    }
                    (KeyCode::Esc, _) | (KeyCode::Char('q'), _) | (KeyCode::Char('s'), _) => {
                        break Choice::Shell;
                    }
                    (KeyCode::Char('t'), _) | (KeyCode::Char('/'), _) => {
                        // The escape hatch for a harness we did not detect:
                        // back to cooked input for one typed command line.
                        let _ = terminal::disable_raw_mode();
                        println!(
                            "{}",
                            sty.dim("Type a command (e.g. hermes --resume), a number, or s for a shell.")
                        );
                        return Some(prompt_loop(choices.len(), sty));
                    }
                    (KeyCode::Char(c), _) if c.is_ascii_digit() => {
                        let i = (c as u8 - b'0') as usize;
                        if (1..=choices.len()).contains(&i) {
                            break Choice::Listed(i - 1);
                        }
                    }
                    _ => {}
                }
                let _ = execute!(out, cursor::MoveUp(total as u16));
                draw(&mut out, sel);
            }
            Ok(_) => {}
            Err(_) => break Choice::Shell,
        }
    };
    let _ = terminal::disable_raw_mode();
    Some(choice)
}

/// Our own latency knob is ours to write: set it silently. macOS and kitty
/// settings stay visible in `gwae doctor` and `gwae setup`, never here.
fn tune_latency_silently(cfg_path: &Path) {
    let cfg = Config::load(cfg_path);
    let audit = crate::latency::audit(cfg.input_poll_ms);
    let pending = crate::latency::pending(&audit);
    let (ours, _) = crate::latency::ours_and_theirs(&pending);
    if !ours.is_empty() {
        let _ = crate::latency::save_input_poll(cfg_path, 1);
    }
}

/// Read one picker choice, re-prompting until valid. EOF returns Shell so
/// a closed stdin can never wedge first run.
fn prompt_loop(n: usize, sty: &Style) -> Choice {
    let stdin = std::io::stdin();
    loop {
        print!("\n{} ", sty.paint("36", ">"));
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Choice::Shell,
            Ok(_) => {}
        }
        match parse_choice(&line, n) {
            Ok(c) => return c,
            Err(msg) => println!("{}", sty.dim(&msg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_a_first_run() {
        assert!(is_first_run(Path::new("/definitely/not/here/gwae.toml")));
    }

    #[test]
    fn an_existing_file_is_not_a_first_run() {
        let dir = std::env::temp_dir().join(format!("gwae-firstrun-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("gwae.toml");
        std::fs::write(&p, "startup_panes = 1\n").unwrap();
        assert!(!is_first_run(&p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_tty_never_prompts_and_still_tunes_our_knob() {
        // Piped stdin: ensure() must return without blocking. This test
        // itself runs with a non-tty stdin under cargo, so it exercises the
        // silent path directly.
        if std::io::stdin().is_terminal() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "gwae-firstrun-silent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("gwae.toml");
        let cfg = ensure(&p, true);
        assert_eq!(cfg.input_poll_ms, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
