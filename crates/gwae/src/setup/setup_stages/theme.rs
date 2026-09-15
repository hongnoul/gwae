//! The configured chrome theme: the named preset, or the fallback when the
//! configured theme was not recognized.

use super::super::{Ctx, StageKind, SetupStage};

/// Stage for the `theme` doctor line.
pub struct ThemeStage;

impl SetupStage for ThemeStage {
    fn id(&self) -> &'static str {
        "theme"
    }
    fn kind(&self) -> StageKind {
        StageKind::Config
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        match ctx.cfg.palette_checked().1 {
            Some(name) => {
                format!(
                    "UNKNOWN {name:?} -> falling back to catppuccin-mocha\n    available: {}",
                    crate::theme::Palette::NAMES.join(", ")
                )
            }
            None => format!("{} [ok]", ctx.cfg.theme_name()),
        }
    }
    fn check(&self, ctx: &Ctx) -> bool {
        ctx.cfg.palette_checked().1.is_none()
    }
}
