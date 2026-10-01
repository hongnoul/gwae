//! macOS fullscreen keyboard-focus repair, per terminal.
//!
//! The bug: a terminal in *native* fullscreen owns its own macOS Space.
//! Returning to that Space re-activates the app and restores the window as
//! `AXMain`, but macOS does not always send `windowDidBecomeKey:`, so the
//! terminal's windowing layer believes nothing is focused and drops every
//! keystroke until the user clicks. gwae never sees the keys; they die
//! between macOS and the terminal.
//!
//! Two durable fixes, in preference order:
//!
//! 1. The `com.gwae.focus-fix` LaunchAgent (`scripts/macos/`): a small
//!    Swift daemon that watches for Ghostty activation / Space changes and
//!    re-asserts key focus via Ghostty's AppleScript `focus` verb. Native
//!    fullscreen keeps its own Space, swipe, and animations.
//! 2. The terminal's *non-native* fullscreen so the window never moves to
//!    its own Space and the buggy return path never runs. No daemon, but no
//!    swipe or Space isolation either.
//!
//! This stage detects the terminal, checks its config, checks whether the
//! daemon is loaded, and reports the exact next step. It never writes
//! another program's config file.

use super::super::{Ctx, SetupStage, StageKind};
use crate::setup::setup_support::{ghostty_conf, kitty_conf, terminal};

/// Launchd label for the rekey daemon.
pub const FOCUS_DAEMON_LABEL: &str = "com.gwae.focus-fix";

/// Stage for the `focus` doctor line.
pub struct FocusStage;

/// What the probe learned: which terminal, its fullscreen setting, and
/// whether the rekey daemon covers native fullscreen.
enum Probe {
    /// Not macOS: the main-vs-key Space bug does not exist.
    NotMacos,
    /// Ghostty: the setting's current effective value (None = unset =
    /// native) plus whether the rekey daemon is loaded and its binary
    /// exists.
    Ghostty {
        setting: Option<String>,
        daemon: bool,
    },
    /// kitty, with the setting's current effective value (None = unset).
    Kitty(Option<String>),
    /// Some other terminal: nothing actionable to say.
    Unknown,
}

/// Where the compiled daemon lives.
fn daemon_binary_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join(".local/bin/gwae-focus-fix"))
}

/// True when the LaunchAgent is loaded and its binary exists.
fn daemon_ready() -> bool {
    let loaded = std::process::Command::new("launchctl")
        .args(["list", FOCUS_DAEMON_LABEL])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let binary = daemon_binary_path().map(|p| p.is_file()).unwrap_or(false);
    loaded && binary
}

impl FocusStage {
    fn probe() -> Probe {
        if !cfg!(target_os = "macos") {
            return Probe::NotMacos;
        }
        if terminal::in_ghostty() {
            let mut val = None;
            for p in terminal::ghostty_conf_paths() {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    if let Some(v) = ghostty_conf::get(&text, "macos-non-native-fullscreen") {
                        val = Some(v);
                    }
                }
            }
            return Probe::Ghostty {
                setting: val,
                daemon: daemon_ready(),
            };
        }
        if terminal::in_kitty() {
            let val = terminal::kitty_conf_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|text| kitty_conf::get(&text, "macos_traditional_fullscreen"));
            return Probe::Kitty(val);
        }
        Probe::Unknown
    }

    /// True when the setting is one of ghostty's non-native spellings.
    fn ghostty_non_native(setting: &Option<String>) -> bool {
        matches!(setting.as_deref(), Some("true") | Some("visible-menu"))
    }

    /// True when the terminal will keep keyboard focus across fullscreen
    /// Space switches. `Unknown` is healthy: no advice beats wrong advice.
    fn healthy(p: &Probe) -> bool {
        match p {
            Probe::NotMacos | Probe::Unknown => true,
            Probe::Ghostty { setting, daemon } => {
                Self::ghostty_non_native(setting) || *daemon
            }
            Probe::Kitty(v) => matches!(v.as_deref(), Some("yes") | Some("true")),
        }
    }
}

impl SetupStage for FocusStage {
    fn id(&self) -> &'static str {
        "focus"
    }
    fn kind(&self) -> StageKind {
        StageKind::Info
    }
    fn doctor_line(&self, _ctx: &Ctx) -> String {
        let p = Self::probe();
        match &p {
            Probe::NotMacos => "not macOS; fullscreen focus bug does not apply [ok]".into(),
            Probe::Unknown => "terminal not recognized; no fullscreen advice [ok]".into(),
            Probe::Ghostty { setting, daemon } if Self::healthy(&p) => {
                if Self::ghostty_non_native(setting) {
                    format!(
                        "ghostty macos-non-native-fullscreen = {} [ok]",
                        setting.as_deref().unwrap_or("?")
                    )
                } else if *daemon {
                    "ghostty native fullscreen + focus daemon loaded [ok]".into()
                } else {
                    // Unreachable: healthy with native setting means daemon.
                    "ghostty native fullscreen [ok]".into()
                }
            }
            Probe::Ghostty { .. } => {
                "ghostty native fullscreen drops keystrokes after a Space switch (focus daemon not loaded)".into()
            }
            Probe::Kitty(v) if Self::healthy(&p) => format!(
                "kitty macos_traditional_fullscreen = {} [ok]",
                v.as_deref().unwrap_or("?")
            ),
            Probe::Kitty(_) => {
                "kitty native fullscreen drops keystrokes after a Space switch".into()
            }
        }
    }
    fn check(&self, _ctx: &Ctx) -> bool {
        Self::healthy(&Self::probe())
    }
    fn steps(&self, _ctx: &Ctx) -> super::super::Steps {
        let p = Self::probe();
        if Self::healthy(&p) {
            return Vec::new();
        }
        match p {
            Probe::Ghostty { .. } => vec![
                "compile the rekey daemon: swiftc -O scripts/macos/gwae-focus-fix.swift -o ~/.local/bin/gwae-focus-fix (from the gwae repo)".into(),
                "render scripts/macos/com.gwae.focus-fix.plist with your absolute binary path into ~/Library/LaunchAgents/com.gwae.focus-fix.plist, then `launchctl load -w` it".into(),
                "allow the one-time Automation prompt (control Ghostty), then re-enter fullscreen".into(),
                "fallback without a daemon: add `macos-non-native-fullscreen = true` to ~/.config/ghostty/config".into(),
            ],
            Probe::Kitty(_) => vec![
                "add `macos_traditional_fullscreen yes` to ~/.config/kitty/kitty.conf".into(),
                "restart kitty, then re-enter fullscreen".into(),
            ],
            Probe::NotMacos | Probe::Unknown => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ghostty(setting: Option<&str>, daemon: bool) -> Probe {
        Probe::Ghostty {
            setting: setting.map(str::to_string),
            daemon,
        }
    }

    /// The advice is inverted per terminal (ghostty wants non-native = true,
    /// kitty wants traditional = yes); pin both so a refactor cannot swap
    /// them and tell a user to set the wrong key.
    #[test]
    fn healthy_reads_each_terminals_own_vocabulary() {
        assert!(FocusStage::healthy(&ghostty(Some("true"), false)));
        assert!(FocusStage::healthy(&ghostty(Some("visible-menu"), false)));
        // Native fullscreen is healthy only with the rekey daemon loaded.
        assert!(FocusStage::healthy(&ghostty(Some("false"), true)));
        assert!(FocusStage::healthy(&ghostty(None, true)));
        assert!(!FocusStage::healthy(&ghostty(Some("false"), false)));
        assert!(!FocusStage::healthy(&ghostty(None, false)));
        assert!(FocusStage::healthy(&Probe::Kitty(Some("yes".into()))));
        assert!(!FocusStage::healthy(&Probe::Kitty(None)));
    }

    /// A machine gwae cannot advise must read healthy: a red doctor line
    /// with no actionable step is noise that trains users to ignore doctor.
    #[test]
    fn unknown_and_non_macos_are_healthy() {
        assert!(FocusStage::healthy(&Probe::Unknown));
        assert!(FocusStage::healthy(&Probe::NotMacos));
    }
}
