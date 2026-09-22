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

use crate::agent::{parse_choice, render, Choice, HarnessState};
use crate::config::Config;

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

    println!("gwae first run — one-time setup in this terminal.\n");

    agent_step(cfg_path);
    latency_step(cfg_path);

    println!("\nready. entering gwae…\n");
    Config::load(cfg_path)
}

/// Pick the `⌥+;` harness at full terminal width and remember it in the
/// state file. Fast paths (override, memory, lone install) print one line
/// and never prompt.
fn agent_step(cfg_path: &Path) {
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
            println!("agent: {cmd} [ok] — ⌥+; goes straight there");
        }
        Plan::Auto(cmd) => {
            remember(&mut state, state_path.as_deref(), &cmd, true);
            println!("agent: {cmd} [ok] (only one installed)");
        }
        ref chooser @ (Plan::Choose(_) | Plan::Missing { .. }) => {
            let (text, choices) = render(chooser);
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt_loop(choices.len()) {
                Choice::Listed(i) => {
                    let cmd = choices.get(i).map(|f| f.cmd.clone()).unwrap_or_default();
                    if !cmd.is_empty() {
                        remember(&mut state, state_path.as_deref(), &cmd, true);
                        println!("agent: {cmd} — ⌥+; goes straight there");
                    }
                }
                Choice::Typed(cmd) => {
                    remember(&mut state, state_path.as_deref(), &cmd, false);
                    println!("agent: {cmd} — ⌥+; goes straight there");
                }
                Choice::Shell => println!("agent: skipped — ⌥+; will offer a shell"),
            }
        }
        Plan::NoneInstalled { .. } => {
            let (text, _) = render(&Plan::NoneInstalled { want: None });
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt_loop(0) {
                Choice::Typed(cmd) => {
                    remember(&mut state, state_path.as_deref(), &cmd, false);
                    println!("agent: {cmd} — ⌥+; goes straight there");
                }
                _ => println!("agent: no harness found — panes open a shell"),
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

/// Our own latency knob is ours to write: set it silently, report the rest
/// with the exact fix command instead of touching anything.
fn latency_step(cfg_path: &Path) {
    let cfg = Config::load(cfg_path);
    let audit = crate::latency::audit(cfg.input_poll_ms);
    let pending = crate::latency::pending(&audit);
    let (ours, theirs) = crate::latency::ours_and_theirs(&pending);
    if !ours.is_empty() {
        match crate::latency::save_input_poll(cfg_path, 1) {
            Ok(()) => println!("latency: input_poll_ms = 1"),
            Err(e) => println!("latency: could not write {}: {e}", cfg_path.display()),
        }
    } else {
        println!("latency: all layers tuned [ok]");
    }
    for s in theirs {
        if let Some(fix) = &s.fix {
            println!("latency: {}: run `{fix}` ({})", s.key, s.why);
        }
    }
}

/// Read one picker choice, re-prompting until valid. EOF returns Shell so
/// a closed stdin can never wedge first run.
fn prompt_loop(n: usize) -> Choice {
    use crate::agent::{DIM, RESET};
    let stdin = std::io::stdin();
    loop {
        print!("\n\x1b[36m>\x1b[0m ");
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
