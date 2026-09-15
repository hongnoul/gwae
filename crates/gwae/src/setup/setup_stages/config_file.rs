//! Whether the config file exists and parses.
//!
//! A malformed file is silently ignored at startup (gwae falls back to
//! defaults rather than refusing to launch), so `doctor` is the only place a
//! user can find out their config is not being applied.

use super::super::{Ctx, SetupStage, StageKind};

/// Stage for the `config file` doctor line.
pub struct ConfigFileStage;

impl SetupStage for ConfigFileStage {
    fn id(&self) -> &'static str {
        "config file"
    }
    fn kind(&self) -> StageKind {
        StageKind::Info
    }
    fn doctor_line(&self, ctx: &Ctx) -> String {
        match std::fs::read_to_string(ctx.cfg_path) {
            Err(_) => "not present (using defaults) [ok]".to_string(),
            Ok(text) => match toml::from_str::<toml::Value>(&text) {
                Ok(_) => "parses [ok]".to_string(),
                Err(e) => format!("INVALID, so it is being ignored entirely: {e}"),
            },
        }
    }
    fn check(&self, ctx: &Ctx) -> bool {
        let text = std::fs::read_to_string(ctx.cfg_path).unwrap_or_default();
        // Absent is fine (defaults); present must parse.
        std::fs::read(ctx.cfg_path).is_err() || toml::from_str::<toml::Value>(&text).is_ok()
    }
}
