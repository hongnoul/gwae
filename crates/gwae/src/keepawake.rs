//! Keep the Mac awake while gwae runs.
//!
//! gwae is a single process with no daemon: when macOS sleeps, every pane
//! (and every agent in it) freezes until wake. With `keep_awake = true` gwae
//! holds a `caffeinate` assertion for its own lifetime, so idle sleep never
//! pauses a session you walked away from. The *display* is deliberately left
//! free to sleep: agents do not need a lit panel, and a static pane held on
//! screen for hours causes image retention (the "burn-in" look). The system
//! stays awake underneath the dark display.
//!
//! Why `caffeinate` alone is not enough on a laptop: its `-s` (no system
//! sleep) is honored **only on AC power**, and `-i` (no idle sleep) does
//! nothing about the lid. So unplugged, closing the lid put the Mac into
//! `Clamshell Sleep` with the assertion still held (seen in `pmset -g log`
//! as `Entering Sleep state due to 'Clamshell Sleep' ... Using Batt`), and
//! every agent froze until the lid opened again.
//!
//! The lid half is covered by [`lid`]: the kernel's clamshell-sleep
//! override (`IOPMrootDomain` user-client selector
//! `kPMSetClamshellSleepState`, the same switch powerd flips for clamshell
//! mode). It needs no root and no entitlement. The kernel still forces sleep
//! on low battery and thermal emergencies, so a hot bag is not left
//! running. Opt out with [`NO_LID_ENV`] to keep the old "lid close sleeps"
//! behavior.
//!
//! Shape: [`Guard`] spawns `caffeinate` with an assertion bound to our own
//! pid and kills it on drop. The `-w` flag is the backstop: even if gwae is
//! SIGKILLed and the drop never runs, the assertion dies with the pid it was
//! waiting on, so no orphan can hold the machine awake forever.
//!
//! The lid override is a kernel flag, not an assertion, so nothing clears it
//! automatically when its setter dies. It is therefore held by a separate
//! helper process (`gwae __lid-awake <pid>`, see [`run_lid_helper`]) that
//! watches gwae the way `caffeinate -w` does: when gwae exits, however it
//! exits, the helper clears the override and leaves. gwae clears it too on
//! every orderly release, in case the helper itself was killed.

use std::process::{Child, Command, Stdio};

/// Setting this in the environment disables the guard outright, for scripted
/// setups and for tests that must never spawn `caffeinate`.
pub const SKIP_ENV: &str = "GWAE_NO_KEEP_AWAKE";

/// Setting this keeps the idle/system assertion but leaves lid-close sleep
/// alone: closing the lid sleeps the Mac exactly as it would without gwae.
pub const NO_LID_ENV: &str = "GWAE_KEEP_AWAKE_NO_LID";

/// The hidden subcommand gwae re-executes itself as to hold the lid override.
pub const LID_HELPER_ARG: &str = "__lid-awake";

/// Owns the `caffeinate` child (and the lid-override helper) while gwae runs.
pub struct Guard {
    child: Option<Child>,
    lid: Option<Child>,
}

impl Guard {
    /// Start holding the assertion when `enabled`, else do nothing. Never
    /// panics and never blocks: a missing `caffeinate` is a warning, not an
    /// error, because the session is perfectly usable while it sleeps.
    pub fn acquire(enabled: bool) -> Guard {
        let child = spawn(enabled);
        // The lid override rides on the assertion: never hold one without
        // the other, so the HUD badge (which reads `active`) stays honest.
        let lid = if child.is_some() { spawn_lid() } else { None };
        Guard { child, lid }
    }

    /// Reconcile with a (possibly live-reloaded) config. Returns whether
    /// anything changed, so the caller can say so.
    pub fn refresh(&mut self, enabled: bool) -> bool {
        let want = enabled && matches!(availability(), Availability::Ready);
        if want == self.active() {
            return false;
        }
        // Release the old guard *before* acquiring: its lid cleanup clears
        // the shared kernel flag, and must not land after a new helper set it.
        drop(std::mem::replace(
            self,
            Guard {
                child: None,
                lid: None,
            },
        ));
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
    #[cfg(unix)]
    pub fn release(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
        }
        stop_lid(self.lid.take());
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
        stop_lid(self.lid.take());
    }
}

/// Whether the lid override is wanted at all on this machine and session.
fn lid_wanted() -> bool {
    cfg!(target_os = "macos") && std::env::var_os(NO_LID_ENV).is_none()
}

/// Start the lid-override helper: this same binary, re-executed as
/// [`LID_HELPER_ARG`] and bound to our pid. Best effort: a failure is a log
/// line, and the idle/system assertion still holds on its own.
fn spawn_lid() -> Option<Child> {
    if !lid_wanted() {
        return None;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("keep-awake: lid override unavailable (no own path: {e})");
            return None;
        }
    };
    let mut cmd = Command::new(exe);
    cmd.arg(LID_HELPER_ARG).arg(std::process::id().to_string());
    // Own process group: a group-wide SIGKILL aimed at gwae (`kill -9 -<pgid>`,
    // a terminal tearing down its job) must not take the helper with it,
    // because a SIGKILLed helper cannot clear the flag. The helper instead
    // outlives gwae for up to one tick, sees it is gone, clears, and exits.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(c) => {
            tracing::info!(
                "keep-awake: lid-close sleep overridden (helper pid {})",
                c.id()
            );
            Some(c)
        }
        Err(e) => {
            tracing::warn!("keep-awake: could not start lid helper: {e}");
            None
        }
    }
}

/// Stop the helper (SIGTERM, not SIGKILL: a killed helper cannot clean up)
/// and clear the override here too, in case the helper was already gone.
///
/// Never blocks the render loop: the helper's last act is always a clear,
/// even if it was mid-reassert when the signal landed, so the final state
/// is "off" regardless of ordering. The child is reaped on a side thread.
fn stop_lid(child: Option<Child>) {
    let Some(mut c) = child else {
        return;
    };
    #[cfg(unix)]
    unsafe {
        libc::kill(c.id() as libc::pid_t, libc::SIGTERM);
    }
    let _ = lid::set_override(false);
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match c.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = c.kill();
                    let _ = c.wait();
                    let _ = lid::set_override(false);
                    return;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            }
        }
    });
}

/// Set by the helper's signal handler; polled by its loop.
#[cfg(unix)]
static LID_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_lid_signal(_: libc::c_int) {
    LID_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Body of `gwae __lid-awake <pid>`: hold the lid override while `parent`
/// lives, then clear it. Returns the process exit code.
///
/// Re-asserted every second because powerd shares the same kernel bit and
/// clears it after some power events (it re-evaluates clamshell state after
/// a wake), and so does another gwae instance releasing its own guard; a
/// stale clear would otherwise let the next lid close sleep.
pub fn run_lid_helper(parent: u32) -> i32 {
    #[cfg(unix)]
    unsafe {
        let h = on_lid_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, h);
        libc::signal(libc::SIGINT, h);
        libc::signal(libc::SIGHUP, h);
    }
    if let Err(e) = lid::set_override(true) {
        eprintln!("gwae: lid override failed: {e}");
        return 1;
    }
    const TICK: std::time::Duration = std::time::Duration::from_millis(100);
    const REASSERT: std::time::Duration = std::time::Duration::from_secs(1);
    let mut last = std::time::Instant::now();
    while parent_alive(parent) && !lid_stop_requested() {
        if last.elapsed() >= REASSERT {
            let _ = lid::set_override(true);
            last = std::time::Instant::now();
        }
        std::thread::sleep(TICK);
    }
    let _ = lid::set_override(false);
    0
}

fn lid_stop_requested() -> bool {
    #[cfg(unix)]
    {
        LID_STOP.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The helper is gwae's direct child, so reparenting means gwae is gone,
/// even when the pid has already been reused by something else.
fn parent_alive(parent: u32) -> bool {
    #[cfg(unix)]
    {
        (unsafe { libc::getppid() }) as u32 == parent
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        false
    }
}

/// The kernel's clamshell-sleep override, via the `IOPMrootDomain` user
/// client. Off macOS every call is a no-op error.
pub mod lid {
    /// Ask the kernel to skip (`true`) or resume (`false`) lid-close sleep.
    #[cfg(target_os = "macos")]
    pub fn set_override(disable: bool) -> Result<(), String> {
        use std::ffi::{c_char, c_void};
        type MachPort = u32;
        type KernReturn = i32;
        // `IOPMLibDefs.h`: kPMSetClamshellSleepState.
        const K_PM_SET_CLAMSHELL_SLEEP_STATE: u32 = 12;
        #[link(name = "IOKit", kind = "framework")]
        extern "C" {
            static mach_task_self_: MachPort;
            fn IOServiceMatching(name: *const c_char) -> *mut c_void;
            fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> MachPort;
            fn IOServiceOpen(
                service: MachPort,
                owning_task: MachPort,
                kind: u32,
                connect: *mut MachPort,
            ) -> KernReturn;
            fn IOServiceClose(connect: MachPort) -> KernReturn;
            fn IOObjectRelease(object: MachPort) -> KernReturn;
            fn IOConnectCallScalarMethod(
                connection: MachPort,
                selector: u32,
                input: *const u64,
                input_cnt: u32,
                output: *mut u64,
                output_cnt: *mut u32,
            ) -> KernReturn;
        }
        unsafe {
            // `IOServiceGetMatchingService` consumes the matching dict.
            let matching = IOServiceMatching(c"IOPMrootDomain".as_ptr());
            if matching.is_null() {
                return Err("IOServiceMatching failed".into());
            }
            let service = IOServiceGetMatchingService(0, matching);
            if service == 0 {
                return Err("IOPMrootDomain not found".into());
            }
            let mut connect: MachPort = 0;
            let kr = IOServiceOpen(service, mach_task_self_, 0, &mut connect);
            IOObjectRelease(service);
            if kr != 0 {
                return Err(format!("IOServiceOpen: {kr:#x}"));
            }
            let input = [u64::from(disable)];
            let kr = IOConnectCallScalarMethod(
                connect,
                K_PM_SET_CLAMSHELL_SLEEP_STATE,
                input.as_ptr(),
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            IOServiceClose(connect);
            if kr != 0 {
                return Err(format!("kPMSetClamshellSleepState: {kr:#x}"));
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_override(_disable: bool) -> Result<(), String> {
        Err("macOS only".into())
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

/// Short human reason the assertion is not held, for toasts.
pub fn availability_note() -> &'static str {
    match availability() {
        Availability::Ready => "caffeinate failed to start",
        Availability::WrongOs => "macOS only",
        Availability::OptedOut => "GWAE_NO_KEEP_AWAKE is set",
        Availability::Missing => "caffeinate not found",
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
            tracing::info!(
                "keep-awake: holding idle/system sleep via caffeinate (pid {pid}), display may sleep"
            );
            Some(c)
        }
        Err(e) => {
            tracing::warn!("keep-awake: could not start caffeinate: {e}");
            None
        }
    }
}

/// The assertion flags for `pid`: no idle sleep (`-i`), no disk sleep
/// (`-m`), no system sleep on AC (`-s`); `-w` binds the assertion's life to
/// ours.
///
/// Deliberately absent: `-d` (prevent display sleep) and `-u` (declare user
/// activity, which also lights the display). Agents run fine behind a dark
/// panel, and holding a static TUI on screen for hours causes image
/// retention. Letting the display sleep on the user's normal schedule is
/// strictly better: same uptime for the agents, no lit static image, less
/// battery.
fn argv(pid: u32) -> Vec<String> {
    vec![
        "-i".to_string(),
        "-m".to_string(),
        "-s".to_string(),
        "-w".to_string(),
        pid.to_string(),
    ]
}

/// Small badge stamped onto the Option HUD chrome while the
/// assertion is held.
///
/// A fixed annotation rather than a palette key, because it is a *state*
/// signal, not a taste: it reads the same whatever the terminal colors
/// are. It lives only on the HUD, so the palette itself — focus ring
/// included — is never touched and toggling off restores every color
/// exactly. The coffee emoji names caffeinate without words and is narrow
/// enough (4 columns with padding, ☕ itself is double-width) to fit even
/// a 1-pane dashboard frame.
pub const KEEP_AWAKE_BADGE: &str = " ☕ ";

/// The `gwae doctor` line against a described machine, so tests never
/// depend on what happens to be installed on the one running them.
#[cfg(test)]
fn line_for(enabled: bool, avail: &Availability) -> String {
    if !enabled {
        return match avail {
            Availability::WrongOs => "off (macOS only)".to_string(),
            _ => "off".to_string(),
        };
    }
    match avail {
        Availability::Ready => "on; caffeinate holds idle/system sleep while gwae runs, \
             and lid-close sleep is overridden so a closed lid on battery keeps running; \
             the display sleeps as normal"
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
        assert!(g.lid.is_none(), "no lid helper without the assertion");
    }

    #[test]
    fn ready_doctor_line_promises_the_lid_on_battery() {
        // The bug this guards: `-s` is AC-only and `-i` ignores the lid, so
        // unplugged + lid closed slept with keep-awake on. The doctor line
        // must now say the lid is covered, not repeat the old caveat.
        let ready = line_for(true, &Availability::Ready);
        assert!(ready.contains("lid"), "{ready}");
        assert!(ready.contains("battery"), "{ready}");
        assert!(!ready.contains("still sleeps"), "{ready}");
    }

    #[test]
    fn parent_check_rejects_a_pid_that_is_not_ours() {
        // pid 1 is never the parent of a test process.
        assert!(!parent_alive(1));
        #[cfg(unix)]
        assert!(parent_alive(unsafe { libc::getppid() } as u32));
    }

    #[test]
    fn helper_arg_is_hidden_by_construction() {
        // Leading underscores keep it from colliding with a real subcommand
        // and mark it internal; clap never sees it (main dispatches first).
        assert!(LID_HELPER_ARG.starts_with("__"));
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
        for flag in ["-i", "-m", "-s", "-w"] {
            assert!(a.contains(&flag.to_string()), "missing {flag}: {a:?}");
        }
        // And pinned in the other direction: `-d`/`-u` keep the display lit
        // on a static image for hours, which causes image retention. They
        // must never come back by accident.
        for flag in ["-d", "-u"] {
            assert!(
                !a.contains(&flag.to_string()),
                "{flag} forces the display awake and risks burn-in: {a:?}"
            );
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
            "a non-macOS checkout should learn the key is not for it"
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
        let _ = availability_note();
    }

    #[test]
    fn availability_note_names_the_blocker() {
        // The Opt+W toast stays honest when the guard cannot hold: the
        // note must name what to unset or install, never read as working.
        assert!(!availability_note().is_empty());
    }

    #[test]
    fn keep_awake_badge_is_small_plain_text() {
        // The badge is stamped onto box-drawing chrome cell by cell. It must
        // stay tiny enough for a 1-pane dashboard frame (~9 columns) and be
        // the coffee emoji: the symbol for caffeinate, readable without
        // words at any panel size.
        use unicode_width::UnicodeWidthChar;
        let cols: usize = KEEP_AWAKE_BADGE
            .chars()
            .map(|c| c.width().unwrap_or(1))
            .sum();
        assert!(
            cols <= 6,
            "badge must fit a 1-pane frame: {KEEP_AWAKE_BADGE:?} is {cols} columns"
        );
        assert!(
            KEEP_AWAKE_BADGE.contains('\u{2615}'),
            "badge must be the coffee emoji, got {KEEP_AWAKE_BADGE:?}"
        );
    }
}
