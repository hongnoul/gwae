//! Keep the Mac awake while gwae runs.
//!
//! gwae is a single process with no daemon: when macOS sleeps, every pane
//! (and every agent in it) freezes until wake. With `keep_awake = true` gwae
//! holds a `caffeinate` assertion for its own lifetime, so idle and display
//! sleep never pause a session you walked away from.
//!
//! Honest limits, stated here because the `caffeinate(8)` man page buries
//! them: this does **not** defeat lid-close sleep. A closed lid still sleeps
//! the machine unless it is in clamshell mode (power + external display +
//! external input) or sleep is disabled outright (`sudo pmset disablesleep
//! 1`). What this buys is the common case: lid open, display asleep, agents
//! still running in the morning.
//!
//! Shape: [`Guard`] spawns `caffeinate` with an assertion bound to our own
//! pid and kills it on drop. The `-w` flag is the backstop: even if gwae is
//! SIGKILLed and the drop never runs, the assertion dies with the pid it was
//! waiting on, so no orphan can hold the machine awake forever.

use std::process::{Child, Command, Stdio};

/// Setting this in the environment disables the guard outright, for scripted
/// setups and for tests that must never spawn `caffeinate`.
pub const SKIP_ENV: &str = "GWAE_NO_KEEP_AWAKE";

/// Owns the `caffeinate` child while gwae runs.
pub struct Guard {
    child: Option<Child>,
}

impl Guard {
    /// Start holding the assertion when `enabled`, else do nothing. Never
    /// panics and never blocks: a missing `caffeinate` is a warning, not an
    /// error, because the session is perfectly usable while it sleeps.
    pub fn acquire(enabled: bool) -> Guard {
        Guard {
            child: spawn(enabled),
        }
    }

    /// Reconcile with a (possibly live-reloaded) config. Returns whether
    /// anything changed, so the caller can say so.
    pub fn refresh(&mut self, enabled: bool) -> bool {
        let want = enabled && matches!(availability(), Availability::Ready);
        if want == self.active() {
            return false;
        }
        *self = Guard::acquire(enabled);
        true
    }

    /// Whether an assertion is currently held.
    pub fn active(&self) -> bool {
        self.child.is_some()
    }

    /// Release the assertion without waiting, for paths that `exec` a new
    /// image (hot reload): the child is killed, not waited on, because the
    /// exec replaces this process and the new image acquires its own guard
    /// from the handed-over config. `-w` ends the old `caffeinate` once this
    /// pid is gone, so no orphan survives even if the kill races the exec.
    pub fn release(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            // `-w` would end it anyway when we exit; this is just prompt.
            // Failure means it already exited, which is the goal.
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Why gwae can or cannot hold a sleep assertion on this machine. Probed
/// rather than assumed so `doctor` and onboarding report facts, and split
/// from [`spawn`] so reporting never starts anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// macOS with `caffeinate` on PATH and no opt-out: asking is honest.
    Ready,
    /// Not macOS: there is no `caffeinate` to spawn.
    WrongOs,
    /// [`SKIP_ENV`] is set: the user (or their script) said no.
    OptedOut,
    /// macOS, but `caffeinate` is not on PATH (should not happen; it ships
    /// in `/usr/bin`).
    Missing,
}

/// Probe the machine. Pure query, no side effects.
pub fn availability() -> Availability {
    if !cfg!(target_os = "macos") {
        return Availability::WrongOs;
    }
    if std::env::var_os(SKIP_ENV).is_some() {
        return Availability::OptedOut;
    }
    if crate::agent::which("caffeinate").is_some() {
        Availability::Ready
    } else {
        Availability::Missing
    }
}

fn spawn(enabled: bool) -> Option<Child> {
    if !enabled || !matches!(availability(), Availability::Ready) {
        return None;
    }
    let pid = std::process::id();
    let mut cmd = Command::new("caffeinate");
    cmd.args(argv(pid));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(c) => {
            tracing::info!("keep-awake: holding idle/display sleep via caffeinate (pid {pid})");
            Some(c)
        }
        Err(e) => {
            tracing::warn!("keep-awake: could not start caffeinate: {e}");
            None
        }
    }
}

/// The assertion flags for `pid`: no idle sleep, no display sleep, no disk
/// sleep, no system sleep on AC, plus a user-activity declaration; `-w`
/// binds the assertion's life to ours.
fn argv(pid: u32) -> Vec<String> {
    vec![
        "-d".to_string(),
        "-i".to_string(),
        "-m".to_string(),
        "-s".to_string(),
        "-u".to_string(),
        "-w".to_string(),
        pid.to_string(),
    ]
}

/// The `gwae doctor` line for `keep_awake`.
pub fn doctor_line(enabled: bool) -> String {
    line_for(enabled, &availability())
}

/// The focus-ring color while the assertion is held: unmissable red.
///
/// A fixed color rather than a theme key, because it is a *state* signal,
/// not a taste: red means "this machine is deliberately not sleeping", and
/// that must read the same on every preset.
pub const ACTIVE_ACCENT: gwae_term::CColor = gwae_term::CColor::Rgb(0xff, 0x40, 0x40);

/// The palette this frame should paint with: the configured one, with the
/// accent swapped for [`ACTIVE_ACCENT`] while the guard holds an assertion.
///
/// Layered at render time rather than stored, so toggling off restores the
/// theme exactly and a config reload can never bake the red in.
pub fn effective_palette(base: &crate::theme::Palette, guard: &Guard) -> crate::theme::Palette {
    let mut pal = *base;
    if guard.active() {
        pal.accent = ACTIVE_ACCENT;
    }
    pal
}

/// [`effective_palette`] against a described state, so tests can cover the
/// active branch without spawning `caffeinate`.
#[cfg(test)]
pub fn effective_palette_for_test(
    base: &crate::theme::Palette,
    active: bool,
) -> crate::theme::Palette {
    let mut pal = *base;
    if active {
        pal.accent = ACTIVE_ACCENT;
    }
    pal
}

/// [`doctor_line`] against a described machine, so tests never depend on
/// what happens to be installed on the one running them.
fn line_for(enabled: bool, avail: &Availability) -> String {
    if !enabled {
        return match avail {
            Availability::WrongOs => "off (macOS only)".to_string(),
            _ => "off".to_string(),
        };
    }
    match avail {
        Availability::Ready => "on; caffeinate holds idle/display sleep while gwae runs \
             (a closed lid still sleeps outside clamshell mode)"
            .to_string(),
        Availability::WrongOs => "on, but this is macOS-only so it does nothing here".to_string(),
        Availability::OptedOut => {
            format!("on, but {SKIP_ENV} is set so it stays off")
        }
        Availability::Missing => "on, but `caffeinate` was not found on PATH".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_never_spawns() {
        let g = Guard::acquire(false);
        assert!(!g.active(), "a disabled guard must hold nothing");
    }

    #[test]
    fn refresh_toward_disabled_is_a_no_op_without_a_child() {
        let mut g = Guard::acquire(false);
        assert!(
            !g.refresh(false),
            "nothing held and nothing wanted must report no change"
        );
        assert!(!g.active());
    }

    #[test]
    fn argv_binds_the_assertion_to_our_pid() {
        // These execute on a user's machine, so the flags are pinned: drop
        // one and the machine sleeps in a way the user explicitly asked it
        // not to. `-w` is the load-bearing one (no orphaned assertions).
        let a = argv(1234);
        for flag in ["-d", "-i", "-m", "-s", "-u", "-w"] {
            assert!(a.contains(&flag.to_string()), "missing {flag}: {a:?}");
        }
        assert_eq!(a.last().map(|s| s.as_str()), Some("1234"));
    }

    #[test]
    fn every_state_says_something_a_user_can_act_on() {
        // Off is quiet everywhere except where the key cannot work at all.
        assert_eq!(
            line_for(false, &Availability::Ready),
            "off",
            "no noise when the feature simply is not wanted"
        );
        assert!(
            line_for(false, &Availability::WrongOs).contains("macOS"),
            "a Linux user should learn the key is not for them"
        );
        // On always names what happens next, including the lid caveat.
        let ready = line_for(true, &Availability::Ready);
        assert!(ready.contains("caffeinate"), "{ready}");
        assert!(ready.contains("lid"), "{ready}");
        assert!(
            line_for(true, &Availability::WrongOs).contains("macOS-only"),
            "an on-that-does-nothing must not read as working"
        );
        assert!(
            line_for(true, &Availability::OptedOut).contains(SKIP_ENV),
            "the opt-out must be named so it can be unset"
        );
        assert!(
            line_for(true, &Availability::Missing).contains("caffeinate"),
            "a missing binary must be named so it can be found"
        );
    }

    #[test]
    fn probing_the_real_machine_never_panics() {
        let _ = availability();
        let _ = doctor_line(false);
        let _ = doctor_line(true);
    }

    #[test]
    fn inactive_guard_leaves_every_color_alone() {
        // Toggling off must restore the theme exactly: the red is layered
        // per frame, never written into the palette a reload would keep.
        let g = Guard::acquire(false);
        for base in [
            crate::theme::Palette::CATPPUCCIN_MOCHA,
            crate::theme::Palette::NORD,
        ] {
            assert_eq!(effective_palette(&base, &g), base);
            assert_eq!(effective_palette_for_test(&base, false), base);
        }
    }

    #[test]
    fn active_state_swaps_only_the_accent() {
        let base = crate::theme::Palette::NORD;
        let on = effective_palette_for_test(&base, true);
        assert_eq!(on.accent, ACTIVE_ACCENT, "the ring must read as red");
        let mut rest = on;
        rest.accent = base.accent;
        assert_eq!(rest, base, "nothing else may change");
    }
}
