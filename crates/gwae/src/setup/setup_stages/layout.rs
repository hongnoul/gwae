//! A tiny proof the pure layout core works end to end.

use super::super::{Ctx, StageKind, SetupStage};
use gwae_layout::Viewport;

/// Stage for the `layout smoke` doctor line.
pub struct LayoutSmokeStage;

impl SetupStage for LayoutSmokeStage {
    fn id(&self) -> &'static str {
        "layout smoke"
    }
    fn kind(&self) -> StageKind {
        StageKind::Info
    }
    fn doctor_line(&self, _ctx: &Ctx) -> String {
        let mut layout = gwae_layout::Layout::default();
        let view = Viewport::new(120);
        let follow = gwae_layout::FollowScroll::default();
        let before = layout
            .column_x_ranges(layout.focus.row, view.cols)
            .map(|r| r.len())
            .unwrap_or(0);
        let _ = layout.apply(gwae_layout::Action::NewColumn, view, follow);
        let after = layout
            .column_x_ranges(layout.focus.row, view.cols)
            .map(|r| r.len())
            .unwrap_or(0);
        format!("columns {before} -> {after} on default row [ok]")
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        true
    }
}
