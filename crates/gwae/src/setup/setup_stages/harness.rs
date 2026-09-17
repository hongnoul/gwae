//! How `⌥+;` will resolve right now.
//!
//! This is the same decision the gateway makes, so doctor can never disagree
//! with the live behavior.

use super::super::{Ctx, SetupStage, StageKind};

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
        let state_path = crate::agent::harness_state_path();
        let state = state_path
            .as_deref()
            .map(crate::agent::load_harness_state)
            .unwrap_or_default();
        let ordered = state.clone().order(crate::agent::detect());
        // An override that no longer resolves is worth naming even when
        // memory covers it: otherwise the dead pin sits in the config
        // silently while every press takes the remembered path.
        let broken_override = {
            let want = ctx.cfg.default_agent.trim();
            if !want.is_empty() && !crate::agent::command_available(want) {
                Some(want.to_string())
            } else {
                None
            }
        };
        match crate::agent::plan(&ctx.cfg.default_agent, &state, ordered) {
            crate::agent::Plan::Configured(cmd) => {
                if ctx.cfg.default_agent.trim() == cmd.trim() && !cmd.trim().is_empty() {
                    format!("{cmd} [ok]")
                } else if let Some(want) = broken_override {
                    format!("{cmd} [remembered] (override `{want}` not installed)")
                } else {
                    format!("{cmd} [remembered] (⌥+; goes straight there)")
                }
            }
            crate::agent::Plan::Auto(cmd) => format!("{cmd} [ok] (only one installed)"),
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
        let state_path = crate::agent::harness_state_path();
        let state = state_path
            .as_deref()
            .map(crate::agent::load_harness_state)
            .unwrap_or_default();
        let ordered = state.clone().order(crate::agent::detect());
        !matches!(
            crate::agent::plan(&ctx.cfg.default_agent, &state, ordered),
            crate::agent::Plan::Missing { .. }
        )
    }
}
