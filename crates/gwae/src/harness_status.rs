//! Authoritative harness status from the jcode daemon (`clients:map`).
//!
//! The activity heuristic mistakes an idle agent TUI's maintenance output
//! (spinner redraws, notification toasts from other sessions) for work, so a
//! finished client paints `Running` forever. The daemon already knows the
//! truth: each connected client's swarm member carries `status`
//! (`running` while generating, `ready` when done). This module polls that
//! mapping on a background thread and hands the event loop a slot to read,
//! following the same detached-thread-plus-slot pattern as the update check.
//!
//! Mapping panes to sessions goes through the pane's terminal title: a jcode
//! client sets its window title (OSC 0/2) to `icon + display title`, and the
//! fallback label always embeds the session short name (`terminal_session_label`
//! returns either the bare name or `Title (name)`). So a title containing the
//! short name (case-insensitive) identifies the session. A custom display
//! title never hides the name, and two panes on the same session simply share
//! its status, which is the correct answer for both.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the background thread re-queries the daemon. Slow enough that a
/// ~20ms `jcode` subprocess is noise, fast enough that a finished agent's
/// tile settles within a few seconds.
pub const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Env kill switch (tests, minimal installs): skip the daemon poll entirely.
pub const NO_POLL_ENV: &str = "GWAE_NO_HARNESS_STATUS";

/// One connected client's authoritative status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientStatus {
    /// Session short name, e.g. `iwazaru` (matches `friendly_name`).
    pub name: String,
    /// Full session id, e.g. `session_iwazaru_...`.
    pub session_id: String,
    /// True while the daemon reports this session as actively generating.
    pub busy: bool,
}

/// The latest polled snapshot, shared with the event loop.
pub type StatusSlot = Arc<Mutex<HashMap<String, ClientStatus>>>;

/// Whether a daemon status string means "actively generating".
///
/// Mirrors jcode's `is_active_status` (`running`, `streaming`, `thinking`).
/// Everything else (`ready`, `done`, `failed`, `stopped`, ...) counts as
/// settled: the agent is not producing output the user is waiting on.
pub fn status_is_busy(status: &str) -> bool {
    matches!(status, "running" | "streaming" | "thinking")
}

/// Parse the JSON body of `jcode debug clients:map` into statuses keyed by
/// lowercased short name. Returns an empty map on any shape mismatch: a
/// version-skewed daemon must degrade to the heuristic, never to a panic.
pub fn parse_clients_map(text: &str) -> HashMap<String, ClientStatus> {
    let mut out = HashMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return out;
    };
    let clients = v
        .get("clients")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    for c in clients {
        let name = c
            .get("friendly_name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let session_id = c
            .get("session_id")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        let status = c.get("status").and_then(|s| s.as_str()).unwrap_or("");
        out.insert(
            name.to_lowercase(),
            ClientStatus {
                name,
                session_id,
                busy: status_is_busy(status),
            },
        );
    }
    out
}

/// Query the daemon once. `jcode` is resolved on `PATH`; any failure
/// (missing binary, no daemon, bad JSON) yields an empty map so the caller
/// keeps the last good snapshot.
pub fn query_once() -> HashMap<String, ClientStatus> {
    let exe = crate::agent::which("jcode");
    let Some(path) = exe else {
        return HashMap::new();
    };
    let Ok(out) = std::process::Command::new(path)
        .args(["debug", "clients:map"])
        .output()
    else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    parse_clients_map(&String::from_utf8_lossy(&out.stdout))
}

/// Poll the daemon in the background, refreshing `slot` every
/// [`POLL_INTERVAL`]. Non-blocking by construction: the thread is detached
/// and the event loop only ever locks the slot briefly to clone it.
///
/// An empty query result keeps the previous snapshot rather than clearing it:
/// a daemon restart mid-session must not flap every tile to unknown.
pub fn spawn_poll() -> StatusSlot {
    let slot: StatusSlot = Arc::new(Mutex::new(HashMap::new()));
    if std::env::var_os(NO_POLL_ENV).is_some() {
        return slot;
    }
    // No daemon without a `jcode` binary: skip the thread, not just the query.
    if crate::agent::which("jcode").is_none() {
        return slot;
    }
    let out = Arc::clone(&slot);
    std::thread::spawn(move || loop {
        let fresh = query_once();
        if !fresh.is_empty() {
            if let Ok(mut g) = out.lock() {
                *g = fresh;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    });
    slot
}

/// Find the polled status for the session named in a pane title.
///
/// The title embeds the short name either bare (`Iwazaru`, the capitalized
/// fallback) or parenthesized (`Release planning (fox)`). A plain substring
/// search over-asserts on short names (`bo` inside `about`), so require a
/// word boundary on both sides: ASCII alphanumerics extend a name, anything
/// else (space, paren, emoji, start/end) terminates it.
pub fn status_for_title<'a>(
    title: &str,
    snapshot: &'a HashMap<String, ClientStatus>,
) -> Option<&'a ClientStatus> {
    let lower = title.to_lowercase();
    let bytes = lower.as_bytes();
    snapshot.values().find(|cs| {
        let needle = cs.name.to_lowercase();
        let n = needle.as_bytes();
        if n.is_empty() || n.len() > bytes.len() {
            return false;
        }
        bytes
            .windows(n.len())
            .position(|w| w == n)
            .is_some_and(|pos| {
                let before_ok = pos == 0 || !bytes[pos - 1].is_ascii_alphanumeric();
                let end = pos + n.len();
                let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
                before_ok && after_ok
            })
    })
}

/// Reconcile one agent pane's tile against the authoritative snapshot.
///
/// Returns the status the tile should claim, or `None` when the daemon knows
/// nothing about this pane and the heuristic stands:
/// - daemon says busy -> `Running` regardless of quiet time (a quiet stretch
///   mid-generation, e.g. a long tool call with no redraw, is still work).
/// - daemon says settled -> `Idle` ("wants attention": the agent is done and
///   waiting on the user), never `Running`.
/// - only applies to panes currently claiming `Running`: a real `Done`,
///   `Failed`, or OSC-`Idle` is a sharper fact than the daemon's coarse
///   lifecycle and must not be clobbered.
pub fn reconcile_running(
    current: gwae_layout::PaneStatus,
    snapshot: Option<&ClientStatus>,
) -> Option<gwae_layout::PaneStatus> {
    use gwae_layout::PaneStatus;
    if current != PaneStatus::Running {
        return None;
    }
    let cs = snapshot?;
    if cs.busy {
        None
    } else {
        Some(PaneStatus::Idle)
    }
}

/// When the snapshot is old enough that it may describe a previous turn,
/// stop trusting it: fall back to the heuristic rather than pinning a stale
/// verdict. The poller refreshes every [`POLL_INTERVAL`]; twice that plus
/// headroom means at least one refresh was missed.
pub fn snapshot_stale(last_refresh: Instant, now: Instant) -> bool {
    now.duration_since(last_refresh) >= POLL_INTERVAL * 2 + Duration::from_secs(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "count": 2,
        "clients": [
            {"session_id": "session_iwazaru_1_abc", "friendly_name": "iwazaru",
             "status": "ready", "working_dir": "/g/gwae"},
            {"session_id": "session_piglet_2_def", "friendly_name": "piglet",
             "status": "running", "working_dir": "/g/gwae"}
        ]
    }"#;

    #[test]
    fn busy_means_generating_and_nothing_else() {
        for s in ["running", "streaming", "thinking"] {
            assert!(status_is_busy(s), "{s} must count as busy");
        }
        for s in [
            "ready",
            "done",
            "completed",
            "failed",
            "stopped",
            "spawned",
            "",
        ] {
            assert!(!status_is_busy(s), "{s} must count as settled");
        }
    }

    #[test]
    fn parse_reads_busy_flags_by_short_name() {
        let m = parse_clients_map(SAMPLE);
        assert_eq!(m.len(), 2);
        assert!(!m["iwazaru"].busy);
        assert!(m["piglet"].busy);
        assert_eq!(m["iwazaru"].session_id, "session_iwazaru_1_abc");
    }

    #[test]
    fn parse_tolerates_garbage_and_shape_drift() {
        assert!(parse_clients_map("not json").is_empty());
        assert!(parse_clients_map("{}").is_empty());
        assert!(parse_clients_map(r#"{"clients": "nope"}"#).is_empty());
        // A client without a name cannot be title-matched; skip it.
        let m = parse_clients_map(r#"{"clients": [{"status": "running"}]}"#);
        assert!(m.is_empty());
    }

    #[test]
    fn title_match_needs_word_boundaries() {
        let m = parse_clients_map(SAMPLE);
        // Bare fallback label, capitalized the way jcode capitalizes it.
        assert_eq!(
            status_for_title("🐀 jcode Iwazaru", &m).unwrap().name,
            "iwazaru"
        );
        // Display title keeps the short name in parens.
        assert_eq!(
            status_for_title("🐀 Release planning (iwazaru)", &m)
                .unwrap()
                .name,
            "iwazaru"
        );
        // Short names must not match inside other words.
        let mut bo = HashMap::new();
        bo.insert(
            "bo".to_string(),
            ClientStatus {
                name: "bo".to_string(),
                session_id: "s".to_string(),
                busy: false,
            },
        );
        assert!(status_for_title("about to run", &bo).is_none());
        assert!(status_for_title("hi bo!", &bo).is_some());
        // Unknown titles match nothing.
        assert!(status_for_title("🐚 fish shell", &m).is_none());
    }

    #[test]
    fn reconcile_only_demotes_stale_running() {
        use gwae_layout::PaneStatus;
        let m = parse_clients_map(SAMPLE);
        let done = &m["iwazaru"];
        let working = &m["piglet"];
        // Settled daemon + Running tile -> Idle (done, wants attention).
        assert_eq!(
            reconcile_running(PaneStatus::Running, Some(done)),
            Some(PaneStatus::Idle)
        );
        // Busy daemon + Running tile -> keep the heuristic's claim.
        assert_eq!(reconcile_running(PaneStatus::Running, Some(working)), None);
        // Sharper facts are never clobbered, even when the daemon is settled.
        for st in [
            PaneStatus::Done,
            PaneStatus::Failed,
            PaneStatus::Idle,
            PaneStatus::Plain,
        ] {
            assert_eq!(reconcile_running(st, Some(done)), None, "{st:?}");
            assert_eq!(reconcile_running(st, Some(working)), None, "{st:?}");
        }
        // No daemon knowledge -> heuristic stands.
        assert_eq!(reconcile_running(PaneStatus::Running, None), None);
    }

    #[test]
    fn staleness_needs_a_missed_refresh_plus_headroom() {
        let t0 = Instant::now();
        assert!(!snapshot_stale(t0, t0 + POLL_INTERVAL));
        assert!(!snapshot_stale(t0, t0 + POLL_INTERVAL * 2));
        assert!(snapshot_stale(
            t0,
            t0 + POLL_INTERVAL * 2 + Duration::from_secs(3)
        ));
    }
}
