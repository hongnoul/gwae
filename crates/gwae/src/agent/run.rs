//! Gateway entry: decide, paint, and exec the chosen harness.

use super::config::{fallback_shell, plan, save_default_agent, set_default_agent_text, Plan};
use super::detect::{detect_with, which, Found};
use super::picker::{parse_choice, prompt, render, render_at, Choice, BOLD, CYAN, DIM, RESET, YELLOW};
use std::io::Write;
use std::path::{Path, PathBuf};

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
pub fn run(
    default_agent: &str,
    extra: &[String],
    _input_poll_ms: u64,
    cfg_path: &Path,
    print_only: bool,
) -> ! {
    let p = plan(default_agent, detect_with(extra));

    if print_only {
        match &p {
            Plan::Configured(cmd) => println!(
                "default_agent: {cmd} [ok] -> {}",
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

    // First run with no saved choice: the gateway's own numbered picker
    // (`render` + `prompt` below) asks and saves, with no other flow in
    // between. A `Configured` harness execs silently and never interrupts
    // an already-configured user.

    let cmd = match p {
        Plan::Configured(cmd) => cmd,
        Plan::NoneInstalled { .. } => {
            // Still offer the typed escape hatch: "we found nothing" is a
            // statement about our search, not about the user's machine.
            let (text, _) = render(&p);
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt(0) {
                Choice::Typed(cmd) => {
                    match save_default_agent(cfg_path, &cmd) {
                        Ok(()) => println!("{DIM}Saved default_agent = \"{cmd}\".{RESET}"),
                        Err(e) => println!(
                            "{YELLOW}Could not save to {}: {e}{RESET}",
                            cfg_path.display()
                        ),
                    }
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
                Choice::Listed(i) => Some(choices[i].cmd.clone()),
                Choice::Typed(cmd) => Some(cmd),
                Choice::Shell => None,
            };
            match pick {
                Some(pick) => {
                    match save_default_agent(cfg_path, &pick) {
                        Ok(()) => println!("{DIM}Saved default_agent = \"{pick}\".{RESET}"),
                        Err(e) => println!(
                            "{YELLOW}Could not save to {}: {e}{RESET}",
                            cfg_path.display()
                        ),
                    }
                    pick
                }
                None => fallback_shell(),
            }
        }
    };
    // The harness owns the pane from here: same pid, same PTY, no wrapper.
    exec(&cmd)
}

mod save_edge_cases {
    use super::*;

    /// Every rewrite must leave a file that still parses and holds the new
    /// value: the config is the user's, and a corrupted one is silently
    /// ignored at startup, which would look like gwae losing settings.
    fn check(before: &str, agent: &str) -> toml::Value {
        let after = set_default_agent_text(before, agent);
        assert!(
            after.ends_with('\n'),
            "must stay newline-terminated: {after:?}"
        );
        let v: toml::Value =
            toml::from_str(&after).unwrap_or_else(|e| panic!("broke the file: {e}\n{after:?}"));
        assert_eq!(v["default_agent"].as_str(), Some(agent), "{after:?}");
        v
    }

    #[test]
    fn a_file_without_a_trailing_newline_is_still_valid_after() {
        check("startup_panes = 1", "claude");
    }

    #[test]
    fn crlf_line_endings_survive() {
        let v = check(
            "startup_panes = 1\r\ndefault_agent = \"jcode\"\r\nmouse = true\r\n",
            "claude",
        );
        assert_eq!(v["mouse"].as_bool(), Some(true));
        assert_eq!(v["startup_panes"].as_integer(), Some(1));
    }

    #[test]
    fn an_indented_key_is_still_the_key_we_replace() {
        let after = set_default_agent_text("  default_agent = \"jcode\"\n", "claude");
        assert_eq!(after.matches("default_agent").count(), 1, "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
    }

    #[test]
    fn a_comment_only_file_gains_the_key_and_keeps_its_comments() {
        let after = set_default_agent_text("# my notes\n# more notes\n", "claude");
        assert!(after.contains("# my notes"), "{after:?}");
        assert!(after.contains("# more notes"), "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
    }

    #[test]
    fn a_file_that_is_only_a_table_gets_the_key_above_it() {
        let v = check("[theme]\npreset = \"nord\"\n", "claude");
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn an_empty_string_is_handled_without_producing_a_stray_blank_line() {
        let after = set_default_agent_text("", "claude");
        assert_eq!(after, "default_agent = \"claude\"\n");
    }

    #[test]
    fn repeated_saves_never_accumulate_duplicate_keys() {
        // The picker can run many times; each must replace, not append.
        let mut text = "startup_panes = 1\n".to_string();
        for a in ["claude", "codex", "aider", "jcode"] {
            text = set_default_agent_text(&text, a);
        }
        assert_eq!(text.matches("default_agent").count(), 1, "{text:?}");
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("jcode"));
    }

    #[test]
    fn a_key_whose_name_merely_starts_the_same_is_left_alone() {
        let after = set_default_agent_text("default_agent_args = \"x\"\n", "claude");
        assert!(after.contains("default_agent_args = \"x\""), "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["default_agent_args"].as_str(), Some("x"));
    }
}
