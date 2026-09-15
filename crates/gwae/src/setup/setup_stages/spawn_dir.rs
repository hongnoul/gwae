//! Where `⌥+;` will open a pane right now.
//!
//! Reports the same decision `run_tui` makes, including the fallback, so a
//! typo'd `agent_dir` is findable instead of silently ignored.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `spawn dir` doctor line.
pub struct SpawnDirStage;

impl SetupStage for SpawnDirStage {
    fn id(&self) -> &'static str {
        "spawn dir"
    }
    fn kind(&self) -> StageKind {
        StageKind::Config
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        let harness_dir = ctx.cfg.dir_for_harness(&ctx.cfg.default_agent);
        let resolved =
            crate::spawndir::resolve_for_harness(ctx.dir, harness_dir, &ctx.cfg.agent_dir);
        let h_label = if ctx.cfg.default_agent.trim().is_empty() {
            String::new()
        } else {
            crate::tui::shell_split(&ctx.cfg.default_agent)
                .first()
                .cloned()
                .unwrap_or_default()
        };
        let unset = harness_dir.trim().is_empty()
            && ctx.cfg.agent_dir.trim().is_empty()
            && ctx.dir.is_none();
        match resolved {
            Some(p) if unset => {
                format!("{} (gwae's cwd; unset, ⌥+d picks one) [ok]", p.display())
            }
            Some(p) if Some(&p) != crate::spawndir::inherited().as_ref() => {
                if !h_label.is_empty() && !harness_dir.trim().is_empty() {
                    format!("{} [{}] [ok]", p.display(), h_label)
                } else {
                    format!("{} [ok]", p.display())
                }
            }
            _ => {
                let raw = ctx
                    .dir
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(harness_dir);
                let raw = if raw.trim().is_empty() {
                    &ctx.cfg.agent_dir
                } else {
                    raw
                };
                match crate::spawndir::check(raw) {
                    Ok(p) => format!("{} [ok]", p.display()),
                    Err(e) => format!("INVALID {raw:?}: {e}; panes inherit gwae's cwd"),
                }
            }
        }
    }
    fn check(&self, ctx: &Ctx) -> bool {
        let line = self.doctor_line(ctx);
        !line.contains("INVALID")
    }
}
