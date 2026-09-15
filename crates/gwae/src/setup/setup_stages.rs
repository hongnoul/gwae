//! One file per setup concern. Each stage wraps an existing module's probe
//! and report logic without changing its behavior: the trait is a new
//! projection over code that already works.
//!
//! A new feature adds one struct here (or one new file re-exported here)
//! plus one line in [`crate::setup::stages`]. Nothing else changes.

mod behavior;
mod config_file;
mod harness;
mod latency;
mod layout;
mod onboarding;
mod spawn_dir;
mod theme;
mod updates;

pub use behavior::KeepAwakeStage;
pub use config_file::ConfigFileStage;
pub use harness::HarnessStage;
pub use latency::LatencyStage;
pub use layout::LayoutSmokeStage;
pub use onboarding::OnboardingStage;
pub use spawn_dir::SpawnDirStage;
pub use theme::ThemeStage;
pub use updates::UpdatesStage;
