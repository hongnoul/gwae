//! How this gwae would upgrade.
//!
//! Worth a line even when everything is fine: "how do I update this" is the
//! question every user of a curl-to-bash install asks eventually, and the
//! honest answer depends on facts (install path, receipt) only the binary
//! itself can see.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `updates` doctor line.
pub struct UpdatesStage;

impl SetupStage for UpdatesStage {
    fn id(&self) -> &'static str {
        "updates"
    }
    fn kind(&self) -> StageKind {
        StageKind::Info
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        if let Some(bad) = ctx.cfg.update.bad_source() {
            return format!(
                "INVALID update.source {bad:?}, so it is ignored; valid: {}",
                crate::update::Source::NAMES.join(", ")
            );
        }
        crate::update::doctor_line(ctx.cfg.update.source(), ctx.cfg.update.check)
    }
    fn check(&self, ctx: &Ctx) -> bool {
        ctx.cfg.update.bad_source().is_none()
    }
}
