//! Harness memory: which agent `⌥+;` reaches for without being asked.
//!
//! This is *state*, not config: nothing in here is hand-edited, and losing it
//! costs one extra pick. It lives next to the update cache under
//! `$XDG_STATE_HOME/gwae/harness.json` (or `~/.local/state/gwae`), never in
//! `gwae.toml`, so picking a harness never rewrites the user's config file.
//!
//! Shape: `last` is the command spawned next with no UI, `mru` ranks the rest
//! for the picker, and `custom` remembers typed commands that detection could
//! never guess (the old `agents` config key, without a config key).

use std::path::{Path, PathBuf};

/// How many recent picks are remembered for picker ranking.
const MRU_CAP: usize = 8;
/// How many typed custom commands are remembered.
const CUSTOM_CAP: usize = 16;

/// What `⌥+;` remembers about past picks.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HarnessState {
    /// The last picked command, spawned directly next time it still resolves.
    #[serde(default)]
    pub last: String,
    /// Recent picks, most recent first, for picker ranking.
    #[serde(default)]
    pub mru: Vec<String>,
    /// Typed commands detection could not have found, offered like the rest.
    #[serde(default)]
    pub custom: Vec<String>,
}

impl HarnessState {
    /// Record a pick. `known` is false for a typed command the detector did
    /// not list, which is then kept in `custom` so it is offered next time.
    pub fn record_pick(&mut self, cmd: &str, known: bool) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        self.last = cmd.to_string();
        self.mru.retain(|c| c != cmd);
        self.mru.insert(0, cmd.to_string());
        self.mru.truncate(MRU_CAP);
        if !known && !self.custom.iter().any(|c| c == cmd) {
            self.custom.insert(0, cmd.to_string());
            self.custom.truncate(CUSTOM_CAP);
        }
    }

    /// Order detected harnesses the way the picker shows them: the last pick
    /// first (when still installed), then recent picks, then the rest in
    /// detection order. Custom typed commands that still resolve are appended,
    /// since detection never lists them.
    pub fn order(&self, found: Vec<super::detect::Found>) -> Vec<super::detect::Found> {
        use super::detect::{command_available, Found};
        let mut out: Vec<Found> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Last pick first, but only when it still resolves: a stale entry
        // must never outrank something installed.
        if !self.last.trim().is_empty() {
            if let Some(f) = found.iter().find(|f| f.cmd == self.last) {
                if seen.insert(f.cmd.clone()) {
                    out.push(f.clone());
                }
            } else if command_available(&self.last) {
                if let Some(path) = super::detect::which_first(&self.last) {
                    if seen.insert(self.last.clone()) {
                        out.push(Found {
                            cmd: self.last.clone(),
                            label: self.last.clone(),
                            path,
                        });
                    }
                }
            }
        }
        for m in &self.mru {
            if let Some(f) = found.iter().find(|f| &f.cmd == m) {
                if seen.insert(f.cmd.clone()) {
                    out.push(f.clone());
                }
            }
        }
        for f in &found {
            if seen.insert(f.cmd.clone()) {
                out.push(f.clone());
            }
        }
        for c in &self.custom {
            if seen.contains(c) || !command_available(c) {
                continue;
            }
            if let Some(path) = super::detect::which_first(c) {
                if seen.insert(c.clone()) {
                    out.push(Found {
                        cmd: c.clone(),
                        label: c.clone(),
                        path,
                    });
                }
            }
        }
        out
    }
}

/// Where the state file lives. Mirrors the update cache's directory logic:
/// `$XDG_STATE_HOME/gwae`, else `~/.local/state/gwae`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(x).join("gwae/harness.json"));
    }
    Some(crate::config::home_dir()?.join(".local/state/gwae/harness.json"))
}

/// Load the state file, or the default when it is missing or corrupt. A
/// corrupt file must never wedge a spawn: forgetting the last pick costs one
/// extra picker, while refusing to start costs the session.
pub fn load(path: &Path) -> HarnessState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HarnessState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Load the state, seeding `last` from the explicit `default_agent` override
/// on a fresh state file. Both the `gwae agent` entry and the TUI do this, so
/// an existing override keeps working silently after the redesign without the
/// state file ever owning what the config pins. After that the state file
/// owns the memory, not the config.
pub fn load_seeded(path: &Path, default_agent: &str) -> HarnessState {
    let mut state = load(path);
    if state.last.trim().is_empty() && !default_agent.trim().is_empty() {
        state.last = default_agent.trim().to_string();
    }
    state
}

/// Persist the state, creating the parent directory when needed.
pub fn save(path: &Path, state: &HarnessState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string(state).unwrap_or_default();
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::super::detect::Found;
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gwae-harness-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn a_missing_or_corrupt_file_is_a_fresh_state_not_an_error() {
        let dir = tmp();
        let path = dir.join("harness.json");
        assert_eq!(load(&path), HarnessState::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load(&path), HarnessState::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pick_round_trips_through_the_file() {
        let dir = tmp();
        let path = dir.join("nested/harness.json");
        let mut s = HarnessState::default();
        s.record_pick("claude", true);
        save(&path, &s).unwrap();
        let back = load(&path);
        assert_eq!(back.last, "claude");
        assert_eq!(back.mru, vec!["claude"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_rank_most_recent_first_without_duplicates() {
        let mut s = HarnessState::default();
        s.record_pick("aider", true);
        s.record_pick("claude", true);
        s.record_pick("aider", true);
        assert_eq!(s.last, "aider");
        assert_eq!(s.mru, vec!["aider", "claude"]);
    }

    #[test]
    fn blank_picks_are_ignored() {
        let mut s = HarnessState::default();
        s.record_pick("  ", true);
        assert_eq!(s, HarnessState::default());
    }

    #[test]
    fn seeding_adopts_the_override_only_on_a_fresh_state() {
        let dir = tmp();
        let path = dir.join("harness.json");
        // Fresh file: the override becomes memory so existing configs keep
        // working silently after the redesign.
        let s = super::load_seeded(&path, "claude");
        assert_eq!(s.last, "claude");
        // ...but an existing memory is never clobbered by the config.
        let mut mine = HarnessState::default();
        mine.record_pick("aider", true);
        super::save(&path, &mine).unwrap();
        let s = super::load_seeded(&path, "claude");
        assert_eq!(s.last, "aider");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_commands_are_kept_as_custom_but_listed_ones_are_not() {
        let mut s = HarnessState::default();
        s.record_pick("zz --resume", false);
        s.record_pick("claude", true);
        assert_eq!(s.custom, vec!["zz --resume"]);
    }

    fn mk(cmd: &str) -> Found {
        Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: std::path::PathBuf::from("/bin").join(cmd),
        }
    }

    #[test]
    fn ordering_puts_the_last_pick_first_then_mru_then_rest() {
        let mut s = HarnessState::default();
        s.record_pick("aider", true);
        s.record_pick("claude", true);
        let found = vec![mk("aider"), mk("claude"), mk("codex")];
        let cmds: Vec<String> = s.order(found).iter().map(|f| f.cmd.clone()).collect();
        assert_eq!(cmds, vec!["claude", "aider", "codex"]);
    }

    #[test]
    fn a_stale_last_pick_never_outranks_what_is_installed() {
        let s = HarnessState {
            last: "gone-xyz".into(),
            ..HarnessState::default()
        };
        let found = vec![mk("claude")];
        let cmds: Vec<String> = s.order(found).iter().map(|f| f.cmd.clone()).collect();
        assert_eq!(cmds, vec!["claude"]);
    }
}
