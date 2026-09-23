//! Gateway entry: decide, paint, and exec the chosen harness.

use super::config::{fallback_shell, plan, Plan};
use super::detect::detect;
use super::detect::which;
use super::picker::{prompt, render, Choice};
use super::state::{save, HarnessState};
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
fn exec(cmd: &str) -> ! {
    use std::os::unix::process::CommandExt;
    let argv = crate::tui::shell_split(cmd);
    if argv.is_empty() {
        std::process::exit(1);
    }
    let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
    eprintln!("gwae agent: cannot run {}: {err}", argv[0]);
    std::process::exit(127);
}

/// Windows has no `exec`, so run the child and forward its status.
#[cfg(not(unix))]
fn exec(cmd: &str) -> ! {
    let argv = crate::tui::shell_split(cmd);
    if argv.is_empty() {
        std::process::exit(1);
    }
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("gwae agent: cannot run {}: {e}", argv[0]);
            std::process::exit(127);
        }
    }
}

// ANSI used by the picker. The gateway paints plain text into its own pane, so
// it only needs colors and a couple of attributes, not a renderer.
pub fn run(default_agent: &str, state: &HarnessState, state_path: &Path, print_only: bool) -> ! {
    let found = state.order(detect());
    let p = plan(default_agent, state, found);

    if print_only {
        match &p {
            // Name the actual source: an override, a remembered pick, or a
            // lone install are three different reasons to go straight there,
            // and `--print` is how scripts tell them apart.
            Plan::Configured(cmd) => {
                let src = if !default_agent.trim().is_empty() {
                    "default_agent"
                } else {
                    "remembered"
                };
                println!(
                    "{src}: {cmd} [ok] -> {}",
                    which(&crate::tui::shell_split(cmd)[0])
                        .unwrap_or_default()
                        .display()
                )
            }
            Plan::Auto(cmd) => println!(
                "auto: {cmd} [ok] (only one installed) -> {}",
                which(&crate::tui::shell_split(cmd)[0])
                    .unwrap_or_default()
                    .display()
            ),
            _ => {
                let (text, _) = render(&p);
                print!("{text}");
            }
        }
        std::process::exit(0);
    }

    // A remembered pick or a lone install execs silently and never interrupts
    // the user. Only the picker paths paint.

    let cmd = match p {
        Plan::Configured(cmd) | Plan::Auto(cmd) => cmd,
        Plan::NoneInstalled { .. } => {
            // Still offer the typed escape hatch: "we found nothing" is a
            // statement about our search, not about the user's machine.
            let (text, _) = render(&p);
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt(0) {
                Choice::Typed(cmd) => {
                    let mut s = state.clone();
                    s.record_pick(&cmd, false);
                    let _ = save(state_path, &s);
                    cmd
                }
                _ => fallback_shell(),
            }
        }
        ref chooser => {
            let (text, choices) = render(chooser);
            print!("{text}");
            let _ = std::io::stdout().flush();
            let pick = match prompt(choices.len()) {
                Choice::Listed(i) => choices.get(i).map(|c| (c.cmd.clone(), true)),
                Choice::Typed(cmd) => Some((cmd, false)),
                Choice::Shell => None,
            };
            match pick {
                Some((pick, known)) => {
                    let mut s = state.clone();
                    s.record_pick(&pick, known);
                    let _ = save(state_path, &s);
                    pick
                }
                None => fallback_shell(),
            }
        }
    };
    // The harness owns the pane from here: same pid, same PTY, no wrapper.
    exec(&cmd)
}
