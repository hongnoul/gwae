//! The agent gateway: what `⌥+;` (spawn-agent) actually runs.
//!
//! `⌥+;` does not exec the user's harness directly. It opens a pane running
//! `strimux agent`, and *this* module decides what that pane becomes. The pane
//! is a real PTY, so the gateway is an ordinary interactive program: it can
//! print, read keys, and then `exec` the chosen harness so the pane's process
//! *is* the harness (no wrapper left in the process tree, and the harness's
//! own OSC title reaches the host untouched).
//!
//! Three outcomes, in order:
//!
//! 1. `default_agent` is set and resolves on `PATH` -> exec it immediately.
//!    The gateway paints nothing and costs one `execvp`.
//! 2. It is unset (or missing) and harnesses *are* installed -> show a picker,
//!    save the choice to the config file, exec it.
//! 3. Nothing is installed -> explain that, and exec `$SHELL` so the pane is
//!    still a usable terminal rather than a dead box.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Agent harnesses the picker offers when `default_agent` is unset, in the
/// order they are shown. Presence is probed on `PATH`; this list only decides
/// what we *look* for and how each is labeled.
pub const KNOWN_AGENTS: &[(&str, &str)] = &[
    ("jcode", "jcode"),
    ("claude", "Claude Code"),
    ("codex", "OpenAI Codex"),
    ("gemini", "Gemini CLI"),
    ("opencode", "opencode"),
    ("crush", "Crush"),
    ("aider", "aider"),
    ("cursor-agent", "Cursor Agent"),
    ("amp", "Amp"),
    ("goose", "goose"),
];

/// True when `p` is a file we could actually execute.
fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Resolve `exe` the way a shell would: an explicit path is taken as-is, a
/// bare name is searched across `PATH`. Returns the full path when found.
pub fn which(exe: &str) -> Option<PathBuf> {
    if exe.is_empty() {
        return None;
    }
    if exe.contains('/') || exe.contains('\\') {
        let p = PathBuf::from(exe);
        return executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|c| executable(c))
}

/// True when the first word of `cmd` resolves to something spawnable. `cmd`
/// may carry arguments (`"jcode --resume"`); only the executable is probed.
pub fn command_available(cmd: &str) -> bool {
    match crate::tui::shell_split(cmd).first() {
        Some(exe) => which(exe).is_some(),
        None => false,
    }
}

/// A harness found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The command to run (the bare name, as the user would type it).
    pub cmd: String,
    /// Human label for the picker.
    pub label: String,
    /// Where it was found, shown so the user can tell two installs apart.
    pub path: PathBuf,
}

/// Every known harness currently installed, in `KNOWN_AGENTS` order.
pub fn detect() -> Vec<Found> {
    KNOWN_AGENTS
        .iter()
        .filter_map(|(cmd, label)| {
            which(cmd).map(|path| Found {
                cmd: (*cmd).to_string(),
                label: (*label).to_string(),
                path,
            })
        })
        .collect()
}

/// What the gateway decided to do, before any of it is carried out. Split from
/// the doing so it can be tested without a PTY or an `exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `default_agent` resolved; exec it with no UI at all.
    Configured(String),
    /// `default_agent` is set but missing; offer these instead (never empty).
    Missing { want: String, found: Vec<Found> },
    /// Nothing configured, but harnesses exist; let the user pick.
    Choose(Vec<Found>),
    /// Nothing configured and nothing installed; fall back to a shell.
    NoneInstalled { want: Option<String> },
}

/// Decide what `strimux agent` should do for this `default_agent` setting.
pub fn plan(default_agent: &str, found: Vec<Found>) -> Plan {
    let want = default_agent.trim();
    if !want.is_empty() && command_available(want) {
        return Plan::Configured(want.to_string());
    }
    if found.is_empty() {
        return Plan::NoneInstalled {
            want: (!want.is_empty()).then(|| want.to_string()),
        };
    }
    if want.is_empty() {
        Plan::Choose(found)
    } else {
        Plan::Missing {
            want: want.to_string(),
            found,
        }
    }
}

/// The shell to fall back to when there is no harness to run.
pub fn fallback_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}

/// Rewrite `default_agent` in a config file *without* disturbing anything
/// else: an existing top-level assignment is replaced in place, otherwise the
/// line is appended. Comments, key order, and formatting all survive, which a
/// parse/re-serialize round trip would destroy.
///
/// Only a top-level key is touched. Lines inside a `[table]` are skipped, so a
/// `default_agent` under some future section can never be clobbered.
pub fn set_default_agent_text(text: &str, agent: &str) -> String {
    let line = format!("default_agent = {}", toml_string(agent));
    let mut out: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut replaced = false;
    for raw in text.lines() {
        let t = raw.trim_start();
        if t.starts_with('[') {
            in_table = true;
        }
        let is_key = !in_table
            && !replaced
            && t.strip_prefix("default_agent")
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
        // Append before the first table header if there is one, since a bare
        // key after `[theme]` would silently become `theme.default_agent`.
        let at = out
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .unwrap_or(out.len());
        if at > 0 && !out[at.saturating_sub(1)].trim().is_empty() {
            out.insert(at, String::new());
            out.insert(at + 1, line);
        } else {
            out.insert(at, line);
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Quote a value as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Persist `default_agent` to `path`, creating the file (and its parent) when
/// it does not exist yet.
pub fn save_default_agent(path: &Path, agent: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = if existing.trim().is_empty() {
        format!(
            "# strimux configuration\n{}\n",
            format_args!("default_agent = {}", toml_string(agent))
        )
    } else {
        set_default_agent_text(&existing, agent)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

/// Replace this process with `cmd` (split shell-style). On unix this is a real
/// `execvp`, so the pane's PTY, pid, and signals all transfer to the harness.
#[cfg(unix)]
fn exec(cmd: &str) -> ! {
    use std::os::unix::process::CommandExt;
    let argv = crate::tui::shell_split(cmd);
    if argv.is_empty() {
        std::process::exit(1);
    }
    let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
    eprintln!("strimux agent: cannot run {}: {err}", argv[0]);
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
            eprintln!("strimux agent: cannot run {}: {e}", argv[0]);
            std::process::exit(127);
        }
    }
}

// ANSI used by the picker. The gateway paints plain text into its own pane, so
// it only needs colors and a couple of attributes, not a renderer.
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Render the plan as the text the user sees, and return the choices that the
/// on-screen numbers map to. Pure, so the exact UI is testable.
pub fn render(plan: &Plan) -> (String, Vec<Found>) {
    let mut s = String::new();
    let choices = match plan {
        Plan::Configured(_) => Vec::new(),
        Plan::Missing { want, found } => {
            s.push_str(&format!(
                "{YELLOW}{BOLD}`{want}` is not installed.{RESET}\n{DIM}Your config asks for it, but it is not on PATH. Pick another:{RESET}\n\n"
            ));
            found.clone()
        }
        Plan::Choose(found) => {
            s.push_str(&format!(
                "{BOLD}Which agent should {CYAN}⌥+;{RESET}{BOLD} launch?{RESET}\n{DIM}Found on your PATH:{RESET}\n\n"
            ));
            found.clone()
        }
        Plan::NoneInstalled { want } => {
            match want {
                Some(w) => s.push_str(&format!(
                    "{YELLOW}{BOLD}`{w}` is not installed{RESET}, and no other agent harness was found on your PATH.\n"
                )),
                None => s.push_str(&format!(
                    "{YELLOW}{BOLD}No agent harness found on your PATH.{RESET}\n"
                )),
            }
            s.push_str(&format!(
                "{DIM}Looked for: {}{RESET}\n\nInstall one, then press {CYAN}⌥+;{RESET} again.\n{DIM}Opening a shell instead.{RESET}\n",
                KNOWN_AGENTS
                    .iter()
                    .map(|(c, _)| *c)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
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
        s.push_str(&format!(
            "  {CYAN}{}{RESET}  {BOLD}{}{RESET}  {DIM}{}{RESET}{dflt}\n",
            i + 1,
            f.label,
            f.path.display()
        ));
    }
    if !choices.is_empty() {
        s.push_str(&format!(
            "  {CYAN}s{RESET}  {BOLD}just a shell{RESET}  {DIM}skip, don't save{RESET}\n\n{DIM}Your choice is saved to {} as `default_agent`, so ⌥+; goes straight there next time.{RESET}\n",
            crate::config::Config::default_path().display()
        ));
    }
    (s, choices)
}

/// Read a single choice. Returns `Some(index)` for a listed harness, `None`
/// for "just a shell" (including EOF or a non-tty, so the gateway can never
/// wedge a pane waiting for input that will not come).
fn prompt(n: usize) -> Option<usize> {
    use std::io::BufRead;
    if !std::io::stdin().is_terminal() {
        return None;
    }
    let stdin = std::io::stdin();
    loop {
        print!("\n{CYAN}>{RESET} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        let t = line.trim();
        if t.is_empty() {
            // Enter takes the first (most preferred) harness.
            return Some(0);
        }
        if t.eq_ignore_ascii_case("s") || t.eq_ignore_ascii_case("shell") {
            return None;
        }
        if let Ok(i) = t.parse::<usize>() {
            if i >= 1 && i <= n {
                return Some(i - 1);
            }
        }
        println!("{DIM}Enter 1-{n}, or s for a shell.{RESET}");
    }
}

/// `strimux agent`: resolve, maybe ask, save, and exec. Never returns.
pub fn run(default_agent: &str, cfg_path: &Path, print_only: bool) -> ! {
    let p = plan(default_agent, detect());

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

    let cmd = match p {
        Plan::Configured(cmd) => cmd,
        Plan::NoneInstalled { .. } => {
            let (text, _) = render(&p);
            print!("{text}");
            let _ = std::io::stdout().flush();
            fallback_shell()
        }
        ref chooser => {
            let (text, choices) = render(chooser);
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt(choices.len()) {
                Some(i) => {
                    let pick = choices[i].cmd.clone();
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn which_resolves_path_names_and_rejects_missing_or_non_executable() {
        assert!(which("sh").is_some());
        assert!(which("strimux-no-such-agent-xyz").is_none());
        assert!(which("/bin/sh").is_some());
        assert!(which("/bin/definitely-not-here").is_none());
        // A directory exists but is not spawnable.
        assert!(which("/bin").is_none());
        assert!(which("").is_none());
    }

    #[test]
    fn command_available_probes_only_the_executable_word() {
        assert!(command_available("sh -c 'echo hi'"));
        assert!(!command_available("strimux-nope --resume"));
        assert!(!command_available("   "));
    }

    #[test]
    fn a_resolvable_configured_agent_short_circuits_every_prompt() {
        // The common case must never paint: config wins, no detection UI.
        assert_eq!(
            plan("sh", vec![found("jcode")]),
            Plan::Configured("sh".into())
        );
        // Whitespace is not a configuration.
        assert!(matches!(plan("   ", vec![]), Plan::NoneInstalled { .. }));
    }

    #[test]
    fn unset_agent_with_installs_offers_a_choice_and_without_them_falls_back() {
        assert_eq!(
            plan("", vec![found("jcode"), found("claude")]),
            Plan::Choose(vec![found("jcode"), found("claude")])
        );
        assert_eq!(plan("", vec![]), Plan::NoneInstalled { want: None });
    }

    #[test]
    fn a_configured_but_missing_agent_reports_what_was_wanted() {
        assert_eq!(
            plan("jcode-not-real", vec![found("claude")]),
            Plan::Missing {
                want: "jcode-not-real".into(),
                found: vec![found("claude")],
            }
        );
        assert_eq!(
            plan("jcode-not-real", vec![]),
            Plan::NoneInstalled {
                want: Some("jcode-not-real".into())
            }
        );
    }

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
    fn saving_replaces_an_existing_key_and_preserves_comments_and_order() {
        let before = "# my config\nstartup_panes = 1\ndefault_agent = \"jcode\"\nmouse = true\n";
        let after = set_default_agent_text(before, "claude");
        assert_eq!(
            after,
            "# my config\nstartup_panes = 1\ndefault_agent = \"claude\"\nmouse = true\n"
        );
    }

    #[test]
    fn saving_appends_when_absent_and_stays_above_any_table_header() {
        let before = "startup_panes = 1\n\n[theme]\npreset = \"nord\"\n";
        let after = set_default_agent_text(before, "claude");
        // Must land before `[theme]`, or it would become theme.default_agent.
        let agent_at = after.find("default_agent").unwrap();
        let table_at = after.find("[theme]").unwrap();
        assert!(agent_at < table_at, "{after}");
        assert!(after.contains("preset = \"nord\""));
        // And it must still parse as the value we asked for.
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn saving_into_a_flat_file_appends_at_the_end() {
        let after = set_default_agent_text("startup_panes = 1\n", "codex");
        assert_eq!(after, "startup_panes = 1\n\ndefault_agent = \"codex\"\n");
    }

    #[test]
    fn a_default_agent_inside_a_table_is_never_clobbered() {
        let before = "[theme]\ndefault_agent = \"decoy\"\n";
        let after = set_default_agent_text(before, "claude");
        assert!(after.contains("default_agent = \"decoy\""));
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["default_agent"].as_str(), Some("decoy"));
    }

    #[test]
    fn saved_values_are_quoted_so_odd_commands_round_trip() {
        let after = set_default_agent_text("", "my agent");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("my agent"));
    }

    #[test]
    fn save_creates_the_file_and_its_parent_directory() {
        let dir = std::env::temp_dir().join(format!("strimux-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/strimux.toml");
        save_default_agent(&path, "jcode").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("jcode"));
        // Saving again over the fresh file replaces rather than duplicates.
        save_default_agent(&path, "claude").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("default_agent").count(), 1);
        assert!(text.contains("claude"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_returns_known_agents_in_listed_order() {
        // Whatever is installed here, the result must be a subsequence of the
        // known list (the picker's numbering depends on that stability).
        let got = detect();
        let names: Vec<&str> = KNOWN_AGENTS.iter().map(|(c, _)| *c).collect();
        let mut last = 0;
        for f in &got {
            let at = names.iter().position(|n| *n == f.cmd).unwrap();
            assert!(at >= last, "out of order: {:?}", f.cmd);
            last = at;
            assert!(f.path.is_absolute() || f.path.exists());
        }
    }

    #[test]
    fn fallback_shell_is_never_empty() {
        assert!(!fallback_shell().is_empty());
    }
}
