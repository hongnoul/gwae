//! Whether this config has been through `gwae init`.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `onboarding` doctor line.
pub struct OnboardingStage;

impl SetupStage for OnboardingStage {
    fn id(&self) -> &'static str {
        "onboarding"
    }
    fn kind(&self) -> StageKind {
        StageKind::Info
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        let text = std::fs::read_to_string(ctx.cfg_path).unwrap_or_default();
        if crate::onboard::already_onboarded(&text) {
            "done [ok]".to_string()
        } else {
            "not run; `gwae init` configures theme, layout, chrome, latency".to_string()
        }
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        // Informational: a fresh machine simply has not run setup yet.
        true
    }
}
