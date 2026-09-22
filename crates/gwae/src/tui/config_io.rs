//! Config file writes + hot-reload exec (verbatim move from `tui/mod.rs`).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gwae_layout::{Layout, PaneId};
use gwae_term::TermGrid;

use super::pty::PtyPane;

/// Persist the picked spawn directory as `agent_dir` in the config file.
///
/// Reuses the agent gateway's comment-preserving rewrite, so saving a
/// directory from the picker cannot reformat a hand-written config or drop
/// its comments the way a parse/serialize round trip would.
pub(crate) fn write_agent_dir(path: &std::path::Path, dir: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let out =
        crate::agent::set_scalar_text(&text, "agent_dir", &crate::agent::toml_string_pub(dir));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// Persist the picked spawn directory as `agent_dir`.
///
/// The `harness` argument is accepted and ignored: per-harness spawn dirs
/// were removed, so every pick lands in the one `agent_dir` key.
pub(crate) fn write_harness_dir(
    path: &std::path::Path,
    _harness: &str,
    dir: &str,
) -> Result<(), String> {
    write_agent_dir(path, dir)
}

/// Collect the state the next image of gwae needs, then replace this process
/// with it. Returns only on failure, in which case this image carries on.
#[cfg(unix)]
pub(crate) fn perform_reload(
    layout: &Layout,
    panes: &HashMap<PaneId, PtyPane>,
    agent_panes: &HashSet<PaneId>,
    spawn_dir: Option<&std::path::Path>,
    boot: Instant,
) -> Result<std::convert::Infallible, String> {
    let exe = crate::reload::own_path()?;
    let mut handover_panes = Vec::new();
    for (pid, pane) in panes {
        let Some(fd) = pane.master.raw_fd() else {
            return Err(format!("pane {pid} has no fd to hand over"));
        };
        let (cols, rows) = {
            let sz = pane.grid.size();
            (sz.cols, sz.rows)
        };
        handover_panes.push(crate::reload::PaneHandover {
            id: *pid,
            fd,
            pid: pane.child.process_id(),
            cols,
            rows,
            is_agent: agent_panes.contains(pid),
        });
    }
    // Stable order so the new image rebuilds panes deterministically.
    handover_panes.sort_by_key(|p| p.id);
    let handover = crate::reload::Handover {
        layout: layout.clone(),
        panes: handover_panes,
        spawn_dir: spawn_dir.map(|p| p.to_path_buf()),
        from: exe.clone(),
        boot_ago_ms: boot.elapsed().as_millis().min(u64::MAX as u128) as u64,
    };
    crate::reload::exec_into(&exe, &handover)
}
