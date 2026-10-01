//! One file per setup concern. Each stage wraps an existing module's probe
//! and report logic without changing its behavior: the trait is a new
//! projection over code that already works.
//!
//! A new feature adds one struct here (or one new file re-exported here)
//! plus one line in [`crate::setup::stages`]. Nothing else changes.

mod config_file;
mod focus;
mod harness;
mod latency;
mod spawn_dir;
mod updates;

pub use config_file::ConfigFileStage;
pub use focus::FocusStage;
pub use focus::{daemon_binary_path, daemon_bootout, daemon_plist_path};
pub use harness::HarnessStage;
pub use latency::LatencyStage;
pub use spawn_dir::SpawnDirStage;
pub use updates::UpdatesStage;
