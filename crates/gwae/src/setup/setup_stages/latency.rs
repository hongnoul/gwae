//! Input-latency settings across macOS, the terminal, and gwae.
//!
//! `input_poll_ms` is ours to write; kitty and macOS settings are reported
//! with the exact command, never silently applied.

use super::super::{Ctx, SetupStage, StageKind};

/// Stage for the `latency` doctor line.
pub struct LatencyStage;

impl LatencyStage {
    fn pending(ctx: &Ctx) -> Vec<crate::latency::Setting> {
        crate::latency::pending(&crate::latency::audit(ctx.cfg.input_poll_ms))
            .into_iter()
            .cloned()
            .collect()
    }
}

impl SetupStage for LatencyStage {
    fn id(&self) -> &'static str {
        "latency"
    }
    fn kind(&self) -> StageKind {
        StageKind::Config
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        crate::latency::summary(&crate::latency::audit(ctx.cfg.input_poll_ms))
    }
    fn check(&self, ctx: &Ctx) -> bool {
        Self::pending(ctx).is_empty()
    }
    fn steps(&self, ctx: &Ctx) -> super::super::Steps {
        let pending = Self::pending(ctx);
        let refs: Vec<&crate::latency::Setting> = pending.iter().collect();
        let (ours, theirs) = crate::latency::ours_and_theirs(&refs);
        let mut out = Vec::new();
        for s in ours {
            out.push(format!(
                "set {} = {} in {}",
                s.key,
                s.want,
                ctx.cfg_path.display()
            ));
        }
        for s in theirs {
            if let Some(fix) = &s.fix {
                out.push(format!("{}: run `{fix}` ({})", s.key, s.why));
            }
        }
        out
    }
    fn apply(&self, ctx: &Ctx, _yes: bool) -> Vec<String> {
        // Our own knob only: macOS globals and kitty.conf are never written.
        match crate::latency::save_input_poll(ctx.cfg_path, 1) {
            Ok(()) => vec!["input_poll_ms = 1".to_string()],
            Err(e) => vec![format!("could not write {}: {e}", ctx.cfg_path.display())],
        }
    }
}
