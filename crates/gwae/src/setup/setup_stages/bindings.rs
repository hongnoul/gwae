//! Per-terminal keybinding snippets (e.g. Cmd+hjkl on iTerm2/kitty).
//!
//! V1 reports which terminal is in use and what snippet applies. Snippet
//! templates under `assets/terminal/` land with the bindings work.

use super::super::{Ctx, StageKind, SetupStage};
use crate::setup::setup_support::terminal;

/// Stage for the `bindings` snippets.
pub struct BindingsStage;

impl SetupStage for BindingsStage {
    fn id(&self) -> &'static str {
        "bindings"
    }
    fn kind(&self) -> StageKind {
        StageKind::Machine
    }
    fn doctor_line(&self, _ctx: &Ctx) -> String {
        match terminal::terminal_name() {
            "unknown" => "unknown terminal; no snippet available".to_string(),
            name => format!("{name}; no per-terminal snippet installed yet"),
        }
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        // Nothing installed anywhere yet; informational until templates land.
        true
    }
}
