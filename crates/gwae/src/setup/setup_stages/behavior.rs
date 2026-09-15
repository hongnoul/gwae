//! Whether the Mac stays awake while gwae runs.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `keep-awake` doctor line.
pub struct KeepAwakeStage;

impl SetupStage for KeepAwakeStage {
    fn id(&self) -> &'static str {
        "keep-awake"
    }
    fn kind(&self) -> StageKind {
        StageKind::Config
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        crate::keepawake::doctor_line(ctx.cfg.keep_awake)
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        // Informational: on and off are both valid choices.
        true
    }
}
