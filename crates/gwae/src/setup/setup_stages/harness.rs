//! How `⌥+;` will resolve right now.
//!
//! This is the same decision the gateway makes, so doctor can never disagree
//! with the live behavior.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `agent` doctor line.
pub struct HarnessStage;

impl SetupStage for HarnessStage {
    fn id(&self) -> &'static str {
        "agent"
    }
    fn kind(&self) -> StageKind {
        StageKind::Config
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        match crate::agent::plan(&ctx.cfg.default_agent, crate::agent::detect_with(&ctx.cfg.agents)) {
            crate::agent::Plan::Configured(cmd) => format!("{cmd} [ok]"),
            crate::agent::Plan::Choose(found) => format!(
                "unset; ⌥+; will offer {} [ok]",
                found
                    .iter()
                    .map(|f| f.cmd.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            crate::agent::Plan::Missing { want, found } => format!(
                "MISSING {want:?}; ⌥+; will offer {}",
                found
                    .iter()
                    .map(|f| f.cmd.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            crate::agent::Plan::NoneInstalled { .. } => {
                "none installed; ⌥+; opens a shell and says so".to_string()
            }
        }
    }
    fn check(&self, ctx: &Ctx) -> bool {
        !matches!(
            crate::agent::plan(&ctx.cfg.default_agent, crate::agent::detect_with(&ctx.cfg.agents)),
            crate::agent::Plan::Missing { .. }
        )
    }
}
