//! macOS fullscreen keyboard-focus repair, per terminal.
//!
//! The bug: a terminal in *native* fullscreen owns its own macOS Space.
//! Returning to that Space re-activates the app and restores the window as
//! `AXMain`, but macOS does not always send `windowDidBecomeKey:`, so the
//! terminal's windowing layer believes nothing is focused and drops every
//! keystroke until the user clicks. gwae never sees the keys; they die
//! between macOS and the terminal.
//!
//! The durable fix is the same idea everywhere: use the terminal's
//! *non-native* fullscreen so the window never moves to its own Space and
//! the buggy return path never runs. This stage detects the terminal,
//! checks its config, and reports the exact one-line setting. It never
//! writes another program's config file.

use super::super::{Ctx, SetupStage, StageKind};
use crate::setup::setup_support::{ghostty_conf, kitty_conf, terminal};

/// Stage for the `focus` doctor line.
pub struct FocusStage;

/// What the probe learned: which terminal, and whether its config already
/// carries the fullscreen fix.
enum Probe {
    /// Not macOS: the main-vs-key Space bug does not exist.
    NotMacos,
    /// Ghostty, with the setting's current effective value (None = unset).
    Ghostty(Option<String>),
    /// kitty, with the setting's current effective value (None = unset).
    Kitty(Option<String>),
    /// Some other terminal: nothing actionable to say.
    Unknown,
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
            return Probe::Ghostty(val);
        }
        if terminal::in_kitty() {
            let val = terminal::kitty_conf_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|text| kitty_conf::get(&text, "macos_traditional_fullscreen"));
            return Probe::Kitty(val);
        }
        Probe::Unknown
    }

    /// True when the terminal will keep keyboard focus across fullscreen
    /// Space switches. `Unknown` is healthy: no advice beats wrong advice.
    fn healthy(p: &Probe) -> bool {
        match p {
            Probe::NotMacos | Probe::Unknown => true,
            Probe::Ghostty(v) => matches!(v.as_deref(), Some("true") | Some("visible-menu")),
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
            Probe::Ghostty(v) if Self::healthy(&p) => format!(
                "ghostty macos-non-native-fullscreen = {} [ok]",
                v.as_deref().unwrap_or("?")
            ),
            Probe::Ghostty(_) => {
                "ghostty native fullscreen drops keystrokes after a Space switch".into()
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
            Probe::Ghostty(_) => vec![
                "add `macos-non-native-fullscreen = true` to ~/.config/ghostty/config".into(),
                "reload ghostty config (cmd+shift+,), then re-enter fullscreen".into(),
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

    /// The advice is inverted per terminal (ghostty wants non-native = true,
    /// kitty wants traditional = yes); pin both so a refactor cannot swap
    /// them and tell a user to set the wrong key.
    #[test]
    fn healthy_reads_each_terminals_own_vocabulary() {
        assert!(FocusStage::healthy(&Probe::Ghostty(Some("true".into()))));
        assert!(FocusStage::healthy(&Probe::Ghostty(Some(
            "visible-menu".into()
        ))));
        assert!(!FocusStage::healthy(&Probe::Ghostty(Some("false".into()))));
        assert!(!FocusStage::healthy(&Probe::Ghostty(None)));
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
