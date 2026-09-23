//! Config text editing: comment-preserving TOML key writes.

use super::detect::{command_available, Found};
use super::state::HarnessState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// An override (`default_agent`) or the remembered pick resolved; exec it
    /// with no UI at all.
    Configured(String),
    /// Exactly one harness is installed and nothing points elsewhere: go
    /// straight there. The common fresh-machine case, zero UI.
    Auto(String),
    /// `default_agent` is set but missing; offer these instead (never empty).
    Missing { want: String, found: Vec<Found> },
    /// Nothing configured, but harnesses exist; let the user pick.
    Choose(Vec<Found>),
    /// Nothing configured and nothing installed; fall back to a shell.
    NoneInstalled { want: Option<String> },
}

/// Decide what `gwae agent` should do.
///
/// Authority order: the explicit `default_agent` override first, then the
/// remembered `state.last` pick, then a lone installed harness (no point
/// asking when there is one answer), then the picker, then a shell.
///
/// `found` must already be state-ordered (see [`HarnessState::order`]), so the
/// first entry is the default everywhere a default matters.
pub fn plan(default_agent: &str, state: &HarnessState, found: Vec<Found>) -> Plan {
    let want = default_agent.trim();
    if !want.is_empty() && command_available(want) {
        return Plan::Configured(want.to_string());
    }
    if !state.last.trim().is_empty() && found.iter().any(|f| f.cmd == state.last) {
        return Plan::Configured(state.last.clone());
    }
    if found.is_empty() {
        return Plan::NoneInstalled {
            want: (!want.is_empty()).then(|| want.to_string()),
        };
    }
    if !want.is_empty() {
        return Plan::Missing {
            want: want.to_string(),
            found,
        };
    }
    if found.len() == 1 && state.last.trim().is_empty() {
        return Plan::Auto(found[0].cmd.clone());
    }
    Plan::Choose(found)
}

/// The shell to fall back to when there is no harness to run.
pub fn fallback_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}

/// Rewrite a top-level key in a config file *without* disturbing anything
/// else: an existing top-level assignment is replaced in place, otherwise the
/// line is appended. Comments, key order, and formatting all survive, which a
/// parse/re-serialize round trip would destroy.
///
/// Only a top-level key is touched. Lines inside a `[table]` are skipped, so a
/// same-named key under some future section can never be clobbered.
pub fn set_scalar_text(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
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
        // Append before the first table header if there is one, since a bare
        // key after `[theme]` would silently become `theme.default_agent`.
        let at = out
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .unwrap_or(out.len());
        let at = if at > 0 && !out[at.saturating_sub(1)].trim().is_empty() {
            out.insert(at, String::new());
            out.insert(at + 1, line);
            at + 1
        } else {
            out.insert(at, line);
            at
        };
        // Keep a blank line between the key and a following table header, so
        // repeated writes cannot glue `key = v` onto `[table]` and make the
        // file read as if the key were inside it.
        if out.get(at + 1).map(|l| l.trim_start().starts_with('[')) == Some(true) {
            out.insert(at + 1, String::new());
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// [`toml_string`] for callers outside this module (the `⌥+d` picker writes
/// `agent_dir` through the same comment-preserving path).
pub fn toml_string_pub(s: &str) -> String {
    toml_string(s)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rewrite must leave a file that still parses and holds the new
    /// value: the config is the user's, and a corrupted one is silently
    /// ignored at startup, which would look like gwae losing settings.
    fn check(before: &str, key: &str, value: &str) -> toml::Value {
        let after = set_scalar_text(before, key, value);
        assert!(
            after.ends_with('\n'),
            "must stay newline-terminated: {after:?}"
        );
        toml::from_str(&after).unwrap_or_else(|e| panic!("broke the file: {e}\n{after:?}"))
    }

    #[test]
    fn saving_replaces_an_existing_key_and_preserves_comments_and_order() {
        let before = "# my config\nstartup_panes = 1\nagent_dir = \"~/a\"\n";
        let after = set_scalar_text(before, "agent_dir", "\"~/b\"");
        assert_eq!(
            after,
            "# my config\nstartup_panes = 1\nagent_dir = \"~/b\"\n"
        );
    }

    #[test]
    fn saving_appends_when_absent_and_stays_above_any_table_header() {
        let before = "startup_panes = 1\n\n[theme]\npreset = \"nord\"\n";
        let after = set_scalar_text(before, "agent_dir", "\"~/b\"");
        // Must land before `[theme]`, or it would become theme.agent_dir.
        let agent_at = after.find("agent_dir").unwrap();
        let table_at = after.find("[theme]").unwrap();
        assert!(agent_at < table_at, "{after}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["agent_dir"].as_str(), Some("~/b"));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn a_key_inside_a_table_is_never_clobbered() {
        let before = "[theme]\nagent_dir = \"decoy\"\n";
        let after = set_scalar_text(before, "agent_dir", "\"~/b\"");
        assert!(after.contains("agent_dir = \"decoy\""));
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["agent_dir"].as_str(), Some("~/b"));
        assert_eq!(v["theme"]["agent_dir"].as_str(), Some("decoy"));
    }

    #[test]
    fn a_key_whose_name_merely_starts_the_same_is_left_alone() {
        let after = set_scalar_text("agent_dirs = [\"x\"]\n", "agent_dir", "\"~/b\"");
        assert!(after.contains("agent_dirs = [\"x\"]"), "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["agent_dir"].as_str(), Some("~/b"));
    }

    #[test]
    fn repeated_saves_never_accumulate_duplicate_keys() {
        let mut text = "startup_panes = 1\n".to_string();
        for d in ["~/a", "~/b", "~/c"] {
            text = set_scalar_text(&text, "agent_dir", &format!("{d:?}"));
        }
        assert_eq!(text.matches("agent_dir").count(), 1, "{text:?}");
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["agent_dir"].as_str(), Some("~/c"));
    }

    #[test]
    fn an_empty_file_gains_the_key_cleanly() {
        assert_eq!(
            set_scalar_text("", "agent_dir", "\"~/b\""),
            "agent_dir = \"~/b\"\n"
        );
        // And a file without a trailing newline stays valid.
        let v = check("startup_panes = 1", "agent_dir", "\"~/b\"");
        assert_eq!(v["agent_dir"].as_str(), Some("~/b"));
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn a_remembered_pick_beats_everything_but_the_explicit_override() {
        let s = HarnessState {
            last: "sh".into(),
            ..HarnessState::default()
        };
        // An override that resolves always wins, even over memory.
        assert_eq!(plan("sh", &s, vec![]), Plan::Configured("sh".into()));
        // Memory wins over detection order.
        let found = vec![mk("aider"), mk("claude")];
        let s = HarnessState {
            last: "claude".into(),
            ..HarnessState::default()
        };
        assert_eq!(
            plan("", &s, found.clone()),
            Plan::Configured("claude".into())
        );
    }

    #[test]
    fn a_lone_install_with_no_memory_launches_itself() {
        let s = HarnessState::default();
        assert_eq!(
            plan("", &s, vec![mk("claude")]),
            Plan::Auto("claude".into())
        );
        // ...but memory of an uninstalled pick does not invent a harness.
        let s = HarnessState {
            last: "gone-xyz".into(),
            ..HarnessState::default()
        };
        assert_eq!(
            plan("", &s, vec![mk("claude")]),
            Plan::Choose(vec![mk("claude")])
        );
    }

    #[test]
    fn unset_agent_with_installs_offers_a_choice_and_without_them_falls_back() {
        let s = HarnessState::default();
        assert_eq!(
            plan("", &s, vec![mk("aider"), mk("claude")]),
            Plan::Choose(vec![mk("aider"), mk("claude")])
        );
        assert_eq!(plan("", &s, vec![]), Plan::NoneInstalled { want: None });
    }

    #[test]
    fn a_configured_but_missing_agent_reports_what_was_wanted() {
        let s = HarnessState::default();
        assert_eq!(
            plan("jcode-not-real", &s, vec![mk("claude")]),
            Plan::Missing {
                want: "jcode-not-real".into(),
                found: vec![mk("claude")],
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
    fn a_resolvable_configured_agent_short_circuits_every_prompt() {
        let s = HarnessState::default();
        // The common case must never paint: config wins, no detection UI.
        assert_eq!(
            plan("sh", &s, vec![mk("jcode")]),
            Plan::Configured("sh".into())
        );
        // Whitespace is not a configuration.
        assert!(matches!(
            plan("   ", &s, vec![]),
            Plan::NoneInstalled { .. }
        ));
    }

    fn mk(cmd: &str) -> Found {
        Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: std::path::PathBuf::from("/usr/bin").join(cmd),
        }
    }
}
