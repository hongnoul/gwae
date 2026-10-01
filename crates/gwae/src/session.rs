//! Session persistence: tmux-attach feel without a daemon (dev-gated).
//!
//! gwae is daemon-free (ADR-011), so "attach" cannot mean reconnecting to
//! live PTYs. It means *resurrection*: the layout tree, each pane's working
//! directory, and each jcode pane's session id are continuously saved to a
//! tiny state file, and a fresh launch rebuilds the session from it. Agent
//! panes respawn `jcode --resume <id>`, so the agent's own transcript
//! substitutes for the lost scrollback; shells respawn in their old cwd.
//!
//! ## What is deliberately not stored
//!
//! Grid contents, scrollback, OSC 133 statuses, parser state: all either
//! recoverable (the jcode transcript), reprintable (the shell prompt), or
//! stale the moment the process dies. The file stays a few KB and the schema
//! stays versioning-safe, mirroring the reload handover's reasoning (see
//! `PaneHandover`).
//!
//! ## Save model
//!
//! Always-on while gated: the event loop snapshots on a slow poll and on
//! teardown, writing atomically (temp + rename) only when the serialized
//! state actually changed. Quitting is therefore always a "saved quit" and
//! needs no keybind of its own; `gwae --new` is the discard path.
//!
//! ## Gate
//!
//! `GWAE_SESSION_RESTORE=1` only. Optimized for jcode (the session-id
//! mapping rides the existing `harness_status` daemon poll); other harnesses
//! respawn fresh in their cwd. Not shipped as default behavior.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Env var that opts a session into save/restore at all.
pub const ENABLE_VAR: &str = "GWAE_SESSION_RESTORE";

/// How often the event loop snapshots the session. Slow: the state is
/// layout + cwds, not grid bytes, and a crash loses at most this window.
pub const SAVE_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether this session saves and restores state.
pub fn enabled() -> bool {
    std::env::var(ENABLE_VAR).is_ok_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    })
}

/// One pane, as it must be described to a future gwae.
///
/// Deliberately tiny, like `PaneHandover`: everything else about a pane is
/// recoverable or cheap to lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRecord {
    /// The layout's id for this pane, so the tree and the records rejoin.
    pub id: u64,
    /// The child's working directory at save time (libproc), so the
    /// respawned shell starts where the old one lived, not in `~`.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Whether this pane was created by `⌥+;` (or was pane 1.1's harness).
    #[serde(default)]
    pub is_agent: bool,
    /// The exact harness command the pane ran, e.g. `jcode`. Restored into
    /// `agent_cmds` so mid-session respawn semantics survive too.
    #[serde(default)]
    pub agent_cmd: Option<String>,
    /// The jcode session id this pane was running, captured from the
    /// daemon's `clients:map` via the pane title match. The restore spawns
    /// `<agent_cmd> --resume <id>`: exact conversation, no "most recent"
    /// guessing, unambiguous with many panes in one directory.
    #[serde(default)]
    pub resume: Option<String>,
}

/// Everything a future gwae needs to resurrect this session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// The serialized pane tree (widths, focus, strip scroll, row memory).
    pub layout: gwae_layout::Layout,
    /// The panes, by layout id.
    pub panes: Vec<PaneRecord>,
    /// The `⌥+d` spawn directory in force, so the choice survives.
    #[serde(default)]
    pub spawn_dir: Option<PathBuf>,
    /// Whether `⌥+w` keep-awake was held, re-asserted on restore.
    #[serde(default)]
    pub keep_awake: bool,
}

/// Where the session file lives. Mirrors `agent::harness_state_path`:
/// `$XDG_STATE_HOME/gwae`, else `~/.local/state/gwae`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(x).join("gwae/session.json"));
    }
    Some(crate::config::home_dir()?.join(".local/state/gwae/session.json"))
}

/// Serialize for the change check: save skips the write when nothing moved.
pub fn serialize(state: &SessionState) -> Option<String> {
    serde_json::to_string(state).ok()
}

/// Persist atomically: temp file + rename, so a crash mid-write leaves the
/// previous good state, never a torn file.
pub fn save_serialized(path: &Path, json: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// Load the saved session, or `None` when missing or unreadable. A corrupt
/// file must never wedge startup: losing a layout costs a fresh session,
/// refusing to start costs everything.
pub fn load(path: &Path) -> Option<SessionState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Discard the saved session (`gwae --new`): archive to `.bak` rather than
/// delete, so one fat-fingered flag is still recoverable by hand.
pub fn discard(path: &Path) {
    let bak = path.with_extension("json.bak");
    match std::fs::rename(path, &bak) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("discard session {}: {e}", path.display()),
    }
}

/// A live process's current working directory, via libproc. `None` when the
/// pid is gone or the kernel refuses; the restore then falls back to the
/// spawn directory, which was the pane's birthplace anyway.
#[cfg(target_os = "macos")]
pub fn pid_cwd(pid: u32) -> Option<PathBuf> {
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`, which is
    // a plain-old-data struct zeroed beforehand; the vip_path field is a
    // NUL-terminated C string within the struct's own storage.
    unsafe {
        let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        let r = libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if r <= 0 {
            return None;
        }
        let c = std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char);
        let s = c.to_str().ok()?;
        if s.is_empty() {
            return None;
        }
        Some(PathBuf::from(s))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn pid_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(unix))]
pub fn pid_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

/// The command a restored pane spawns.
///
/// jcode panes with a captured session id resume that exact conversation.
/// Everything else (other harnesses, shells) starts fresh: resuming the
/// wrong conversation into a pane is worse than resuming nothing.
pub fn restore_cmd(rec: &PaneRecord) -> String {
    match (&rec.agent_cmd, &rec.resume) {
        (Some(cmd), Some(id)) if is_jcode_cmd(cmd) && !id.trim().is_empty() => {
            format!("{cmd} --resume {id}")
        }
        (Some(cmd), _) if rec.is_agent => cmd.clone(),
        _ => String::new(),
    }
}

/// Whether a harness command is jcode: first token's basename, exactly.
/// `jcode`, `/usr/local/bin/jcode`, `jcode --flag` all count; `jcoded` or
/// another harness wrapping the word does not.
pub fn is_jcode_cmd(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    let base = std::path::Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    base == "jcode"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(agent_cmd: Option<&str>, resume: Option<&str>, is_agent: bool) -> PaneRecord {
        PaneRecord {
            id: 1,
            cwd: None,
            is_agent,
            agent_cmd: agent_cmd.map(|s| s.to_string()),
            resume: resume.map(|s| s.to_string()),
        }
    }

    #[test]
    fn jcode_panes_resume_their_exact_session() {
        let r = rec(Some("jcode"), Some("session_monkey_123"), true);
        assert_eq!(restore_cmd(&r), "jcode --resume session_monkey_123");
    }

    #[test]
    fn full_path_jcode_still_counts() {
        let r = rec(Some("/opt/bin/jcode"), Some("sid"), true);
        assert_eq!(restore_cmd(&r), "/opt/bin/jcode --resume sid");
    }

    #[test]
    fn non_jcode_harnesses_respawn_fresh_never_guess() {
        // `--continue`-style most-recent guessing resumes the wrong
        // conversation when two panes share a cwd; fresh is the honest
        // degradation.
        let r = rec(Some("claude"), Some("sid"), true);
        assert_eq!(restore_cmd(&r), "claude");
        let shell = rec(None, None, false);
        assert_eq!(restore_cmd(&shell), "");
    }

    #[test]
    fn jcode_without_a_captured_id_respawns_fresh() {
        let r = rec(Some("jcode"), None, true);
        assert_eq!(restore_cmd(&r), "jcode");
    }

    #[test]
    fn lookalike_binaries_are_not_jcode() {
        assert!(is_jcode_cmd("jcode"));
        assert!(is_jcode_cmd("jcode --dangerously"));
        assert!(is_jcode_cmd("/usr/local/bin/jcode"));
        assert!(!is_jcode_cmd("jcoded"));
        assert!(!is_jcode_cmd("myjcode"));
        assert!(!is_jcode_cmd(""));
    }

    #[test]
    fn state_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("gwae-session-test-{}", std::process::id()));
        let path = dir.join("session.json");
        let state = SessionState {
            layout: gwae_layout::Layout::default(),
            panes: vec![rec(Some("jcode"), Some("sid"), true)],
            spawn_dir: Some(PathBuf::from("/tmp")),
            keep_awake: true,
        };
        let json = serialize(&state).unwrap();
        save_serialized(&path, &json).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(state, back);
        // Discard archives rather than deletes.
        discard(&path);
        assert!(!path.exists());
        assert!(path.with_extension("json.bak").exists());
        // Discarding again (nothing there) is silent, not an error.
        discard(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_files_load_as_none_not_panic() {
        let dir = std::env::temp_dir().join(format!("gwae-session-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn own_cwd_is_readable() {
        let cwd = pid_cwd(std::process::id()).expect("own cwd");
        assert!(cwd.is_absolute());
    }
}
