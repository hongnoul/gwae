//! Keyboard path: Meta versus glyph Option handling (NAV-REWRITE 3.6).
//!
//! V1 is read-only: it names the terminal and the expected `$mod` path.
//! The runtime detector and toast land with the keymap rewrite, without
//! changing this trait.

use super::super::{Ctx, StageKind, SetupStage};
use crate::setup::setup_support::terminal;

/// Stage for the `keyboard` path report.
pub struct KeyboardStage;

impl SetupStage for KeyboardStage {
    fn id(&self) -> &'static str {
        "keyboard"
    }
    fn kind(&self) -> StageKind {
        StageKind::Manual
    }
    fn doctor_line(&self, _ctx: &Ctx) -> String {
        let name = terminal::terminal_name();
        if terminal::in_kitty() {
            format!("{name}; Meta path expected [ok]")
        } else {
            format!("{name}; enable Option-as-Meta for $mod navigation")
        }
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        true
    }
    fn steps(&self, _ctx: &Ctx) -> super::super::Steps {
        if terminal::in_kitty() {
            Vec::new()
        } else {
            vec!["enable Option-as-Meta in your terminal's settings".to_string()]
        }
    }
}
