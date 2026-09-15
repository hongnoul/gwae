//! Input-latency settings across macOS, the terminal, and gwae.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `latency` doctor line.
pub struct LatencyStage;

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
        crate::latency::pending(&crate::latency::audit(ctx.cfg.input_poll_ms)).is_empty()
    }
}
