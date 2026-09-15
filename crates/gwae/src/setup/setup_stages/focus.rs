//! The kitty Space focus-repair daemon as a setup stage.
//!
//! kitty.conf edits are reported, never applied: the file belongs to the
//! user. Compile plus plist install run on confirmation.

use super::super::focus;
use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `focus` repair.
pub struct FocusStage;

impl SetupStage for FocusStage {
    fn id(&self) -> &'static str {
        "focus"
    }
    fn kind(&self) -> StageKind {
        StageKind::Machine
    }
    fn doctor_line(&self, _ctx: &Ctx) -> String {
        focus::doctor_line(&focus::Facts::probe())
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        matches!(
            focus::plan(&focus::Facts::probe()),
            focus::Plan::Ready
                | focus::Plan::NotApplicable(_)
                | focus::Plan::TraditionalFullscreen
        )
    }
    fn steps(&self, _ctx: &Ctx) -> super::super::Steps {
        match focus::plan(&focus::Facts::probe()) {
            focus::Plan::Install(steps) => steps
                .iter()
                .map(|s| match s {
                    focus::Step::KittyConf { missing } => {
                        format!("add to kitty.conf: {}", missing.join(", "))
                    }
                    focus::Step::Toolchain => {
                        "install Xcode command-line tools (`xcode-select --install`)".to_string()
                    }
                    focus::Step::Compile => {
                        "compile the focus daemon with swiftc".to_string()
                    }
                    focus::Step::Plist => {
                        "install and load the LaunchAgent".to_string()
                    }
                    focus::Step::RestartKitty => {
                        "restart kitty (listen_on is read at startup)".to_string()
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    fn apply(&self, _ctx: &Ctx, _yes: bool) -> Vec<String> {
        focus::run(&focus::plan(&focus::Facts::probe()))
    }
}
