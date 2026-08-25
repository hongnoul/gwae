//! Installing the optional companion tools onboarding offers.
//!
//! Today that is exactly one: [`bottom`](https://github.com/ClementTsang/bottom)
//! (`btm`), the system monitor that makes a good permanent neighbour to an
//! agent pane. It is *offered* rather than bundled, because installing software
//! on someone's machine is not something a terminal multiplexer should do
//! behind their back.
//!
//! What is deliberately silent is the *how*: on macOS the install needs
//! Homebrew, so a "yes" to `btm` implies a "yes" to whatever Homebrew needs to
//! exist, and asking a second question about a package manager the user may
//! never have heard of would be passing our implementation detail to them.
//!
//! Shape of the code:
//!
//! * [`plan`] is pure and decides *what would happen* from the facts (is it
//!   installed, is brew there, what OS is this), so every branch is tested
//!   without touching the machine.
//! * [`run`] is the only function that executes anything, and it is a thin
//!   walk over the plan.
//!
//! Nothing here is ever fatal: a failed install costs the user a monitor they
//! did not have a minute ago, so it is reported and stepped over, never
//! allowed to block the flow into their harness.

use std::process::{Command, Stdio};

/// The package as the user knows it, and as each tool spells it.
pub const TOOL: &str = "btm";
/// Homebrew's name for it (the formula is `bottom`, the binary is `btm`).
pub const FORMULA: &str = "bottom";
/// Cargo's name for it, used on platforms with no supported package manager
/// but a working Rust toolchain.
pub const CRATE: &str = "bottom";

/// The facts [`plan`] decides from. Passed in rather than probed inside, so
/// tests describe a machine instead of needing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// `btm` already resolves on `PATH`.
    pub installed: bool,
    /// `brew` already resolves on `PATH`.
    pub brew: bool,
    /// `cargo` already resolves on `PATH`.
    pub cargo: bool,
    /// This is macOS, where Homebrew is the expected route.
    pub macos: bool,
}

/// The escape hatch that turns the offer off entirely.
///
/// Set by the test suite and by CI, and available to anyone scripting a
/// strimux setup: onboarding must never be able to install software on a
/// machine that is running unattended. It reports as "already installed"
/// because the effect is the same - there is nothing for us to do.
pub const SKIP_ENV: &str = "STRIMUX_NO_INSTALL";

impl Facts {
    /// Probe the real machine.
    pub fn probe() -> Facts {
        if std::env::var_os(SKIP_ENV).is_some() {
            return Facts {
                installed: true,
                brew: false,
                cargo: false,
                macos: cfg!(target_os = "macos"),
            };
        }
        Facts {
            installed: crate::agent::which(TOOL).is_some(),
            brew: crate::agent::which("brew").is_some(),
            cargo: crate::agent::which("cargo").is_some(),
            macos: cfg!(target_os = "macos"),
        }
    }
}

/// What installing `btm` would take on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// It is already there; do nothing and do not ask.
    AlreadyInstalled,
    /// `brew install bottom`.
    Brew,
    /// Install Homebrew first (silently, because it is our implementation
    /// detail), then `brew install bottom`.
    BrewThenInstall,
    /// `cargo install bottom`, for a machine with Rust but no brew.
    Cargo,
    /// Nothing we are willing to drive; tell the user where to get it.
    Manual,
}

/// Decide what to do from the facts. Pure.
///
/// Homebrew is only ever *installed* on macOS: on Linux a user with cargo has
/// a perfectly good route, and dropping a second package manager onto a distro
/// that already has one would be presumptuous.
pub fn plan(f: Facts) -> Plan {
    if f.installed {
        return Plan::AlreadyInstalled;
    }
    if f.brew {
        return Plan::Brew;
    }
    if f.macos {
        return Plan::BrewThenInstall;
    }
    if f.cargo {
        return Plan::Cargo;
    }
    Plan::Manual
}

/// Whether onboarding should ask about `btm` at all.
///
/// Not asking when it is already installed is the point: a question whose
/// only honest answer is "it is already done" teaches users that setup asks
/// things it already knows.
#[allow(dead_code)]
pub fn worth_asking(f: Facts) -> bool {
    !matches!(plan(f), Plan::AlreadyInstalled)
}

/// The one-line outcome shown on the summary screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It is on `PATH` now, whether we put it there or it already was.
    Installed,
    /// The user said no.
    Declined,
    /// We tried and it did not work; the string explains what to do by hand.
    Failed(String),
}

impl Outcome {
    /// The summary line for this outcome.
    pub fn line(&self) -> String {
        match self {
            Outcome::Installed => "installed".to_string(),
            Outcome::Declined => "skipped".to_string(),
            Outcome::Failed(why) => format!("not installed ({why})"),
        }
    }
}

/// The command line for a step of the plan, as `(program, args)`.
///
/// Split out so tests can pin the exact commands we would run on a user's
/// machine without running them.
pub fn commands(p: &Plan) -> Vec<(&'static str, Vec<String>)> {
    match p {
        Plan::AlreadyInstalled | Plan::Manual => vec![],
        Plan::Brew => vec![("brew", vec!["install".into(), FORMULA.into()])],
        Plan::BrewThenInstall => vec![
            // The official non-interactive install. `NONINTERACTIVE=1` is what
            // makes it safe to run from here: without it the script stops to
            // ask for a keypress, which would wedge a setup flow that has
            // already told the user it is handling this.
            ("/bin/bash", vec!["-c".into(), BREW_INSTALL.into()]),
            ("brew", vec!["install".into(), FORMULA.into()]),
        ],
        Plan::Cargo => vec![("cargo", vec!["install".into(), CRATE.into()])],
    }
}

/// The Homebrew installer, run non-interactively.
const BREW_INSTALL: &str = "NONINTERACTIVE=1 /bin/bash -c \"$(curl -fsSL \
     https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"";

/// Where to send someone we cannot help.
pub const MANUAL_HINT: &str = "see https://github.com/ClementTsang/bottom";

/// The prefixes a package manager installs binaries into, in the order they
/// should be preferred (Apple Silicon, Intel, Linuxbrew).
///
/// Needed because a *freshly installed* Homebrew is not on this process's
/// `PATH`: the environment was inherited when strimux started, long before
/// `/opt/homebrew/bin` existed. Without this, the second step of
/// [`Plan::BrewThenInstall`] would always fail with "brew not found" moments
/// after successfully installing brew.
const PREFIXES: [&str; 3] = ["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"];

/// `PATH` with every known package-manager prefix **appended**, so a command
/// run straight after an install can find what was just installed.
///
/// Appended, not prepended: the user's own `PATH` is their explicit statement
/// about which tools to use, and a package-manager prefix that jumped the
/// queue would silently shadow it. These directories are a fallback for the
/// one case the inherited `PATH` cannot cover - a prefix that did not exist
/// when this process started.
fn augmented_path() -> std::ffi::OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    for p in PREFIXES {
        let bin = std::path::Path::new(p).join("bin");
        if bin.is_dir() && !dirs.contains(&bin) {
            dirs.push(bin);
        }
    }
    std::env::join_paths(dirs).unwrap_or(current)
}

/// Resolve a program against [`augmented_path`], so a just-installed `brew` is
/// found even though this process started before it existed.
fn resolve(prog: &str) -> std::path::PathBuf {
    if prog.starts_with('/') {
        return std::path::PathBuf::from(prog);
    }
    for dir in std::env::split_paths(&augmented_path()) {
        let c = dir.join(prog);
        if c.is_file() {
            return c;
        }
    }
    std::path::PathBuf::from(prog)
}

/// Run the plan, returning what to tell the user. The only function here that
/// touches the machine.
///
/// Output is swallowed: a package manager's progress bars would scribble over
/// the setup screen, and the user did not ask to watch a build. Failures are
/// reported as text, never as a panic or a non-zero exit from setup.
pub fn run(p: &Plan) -> Outcome {
    match p {
        Plan::AlreadyInstalled => Outcome::Installed,
        Plan::Manual => Outcome::Failed(MANUAL_HINT.to_string()),
        _ => {
            for (prog, args) in commands(p) {
                // Resolved (and run) against an augmented PATH: step two of
                // `BrewThenInstall` runs seconds after step one created
                // `/opt/homebrew/bin/brew`, which our inherited PATH knows
                // nothing about.
                let ok = Command::new(resolve(prog))
                    .args(&args)
                    .env("PATH", augmented_path())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ok {
                    return Outcome::Failed(format!("`{prog} {}` failed", args.join(" ")));
                }
            }
            // Trust the result, not the exit code: a package manager can
            // succeed and still leave nothing on this shell's PATH (a fresh
            // Homebrew prefix is the common case), and claiming an install
            // the user cannot then run would be a lie on the summary screen.
            if crate::agent::which(TOOL).is_some() || brew_prefix_has_it() {
                Outcome::Installed
            } else {
                Outcome::Failed(format!("installed, but `{TOOL}` is not on PATH yet"))
            }
        }
    }
}

/// Whether a freshly installed package manager has left the binary somewhere,
/// even though this process's `PATH` predates it.
fn brew_prefix_has_it() -> bool {
    PREFIXES
        .iter()
        .any(|p| std::path::Path::new(p).join("bin").join(TOOL).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(installed: bool, brew: bool, cargo: bool, macos: bool) -> Facts {
        Facts {
            installed,
            brew,
            cargo,
            macos,
        }
    }

    #[test]
    fn an_existing_install_is_never_touched_or_asked_about() {
        // The most important branch: a user who already runs btm must not be
        // offered an install that would reinstall it.
        let f = facts(true, true, true, true);
        assert_eq!(plan(f), Plan::AlreadyInstalled);
        assert!(!worth_asking(f));
        assert!(commands(&plan(f)).is_empty(), "would run something");
        // ...and that is true however the machine is otherwise equipped.
        assert_eq!(
            plan(facts(true, false, false, false)),
            Plan::AlreadyInstalled
        );
    }

    #[test]
    fn brew_is_used_when_present_and_installed_only_on_macos() {
        // Present: just use it, on any OS.
        assert_eq!(plan(facts(false, true, false, true)), Plan::Brew);
        assert_eq!(plan(facts(false, true, false, false)), Plan::Brew);
        // Missing on macOS: it is the expected route, so put it there.
        assert_eq!(plan(facts(false, false, true, true)), Plan::BrewThenInstall);
        // Missing on Linux: use the toolchain the user already has rather
        // than dropping a second package manager onto their distro.
        assert_eq!(plan(facts(false, false, true, false)), Plan::Cargo);
        // Nothing to work with: say where to get it instead of guessing.
        assert_eq!(plan(facts(false, false, false, false)), Plan::Manual);
        assert!(commands(&Plan::Manual).is_empty());
    }

    #[test]
    fn the_commands_are_exactly_what_we_claim_to_run() {
        // These execute on a user's machine, so they are pinned rather than
        // left to drift: the formula is `bottom` even though the binary is
        // `btm`, and the brew installer must be non-interactive or it would
        // wedge the flow waiting for a keypress nobody is watching for.
        assert_eq!(
            commands(&Plan::Brew),
            vec![("brew", vec!["install".to_string(), "bottom".to_string()])]
        );
        let steps = commands(&Plan::BrewThenInstall);
        assert_eq!(steps.len(), 2, "install brew, then the formula");
        assert!(steps[0].1[1].contains("NONINTERACTIVE=1"), "{:?}", steps[0]);
        assert!(steps[0].1[1].contains("Homebrew/install"), "{:?}", steps[0]);
        assert_eq!(steps[1].0, "brew");
        assert_eq!(
            commands(&Plan::Cargo),
            vec![("cargo", vec!["install".to_string(), "bottom".to_string()])]
        );
    }

    #[test]
    fn every_outcome_says_something_a_user_can_act_on() {
        assert_eq!(Outcome::Installed.line(), "installed");
        assert_eq!(Outcome::Declined.line(), "skipped");
        let failed = Outcome::Failed(MANUAL_HINT.to_string());
        assert!(failed.line().contains("github.com"), "{}", failed.line());
        // A failure never reads as a success.
        assert!(failed.line().contains("not installed"));
    }

    #[test]
    fn the_skip_env_var_turns_the_offer_off_entirely() {
        // Unattended runs (CI, scripted setup, this test suite) must not be
        // able to install software. This is the guard that makes the default
        // "yes" safe to ship.
        // SAFETY: single-threaded test; the var is read, never held.
        unsafe { std::env::set_var(SKIP_ENV, "1") };
        let f = Facts::probe();
        assert!(f.installed, "the guard must report nothing to do");
        assert!(!worth_asking(f), "the question must not be asked");
        assert_eq!(plan(f), Plan::AlreadyInstalled);
        assert!(commands(&plan(f)).is_empty());
        unsafe { std::env::remove_var(SKIP_ENV) };
    }

    #[test]
    fn a_just_installed_package_manager_is_still_findable() {
        // The bug this guards: step two of `BrewThenInstall` runs seconds
        // after step one created `/opt/homebrew/bin/brew`, but this process
        // inherited its PATH at launch and knows nothing about it. Resolving
        // against the augmented PATH is what stops "installed brew, then could
        // not find brew" from being the normal outcome on a fresh Mac.
        let path = augmented_path();
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        let inherited: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        for prefix in PREFIXES {
            let bin = std::path::Path::new(prefix).join("bin");
            if bin.is_dir() {
                assert!(
                    dirs.contains(&bin),
                    "{bin:?} exists but is not on the augmented PATH"
                );
            }
        }
        // The user's own PATH keeps priority: a prefix that jumped the queue
        // would shadow the tools they deliberately put in front. That is not
        // hypothetical - it made the e2e install case pick the developer's
        // real brew over the stub the test had staged.
        assert_eq!(
            dirs[..inherited.len()],
            inherited[..],
            "the inherited PATH must come first, untouched"
        );
        // An absolute program is passed through untouched (the brew installer
        // is spelled `/bin/bash`, which must not be re-resolved).
        assert_eq!(resolve("/bin/bash"), std::path::PathBuf::from("/bin/bash"));
        // A program that exists resolves to a real file...
        assert!(resolve("sh").is_file(), "sh should resolve");
        // ...and one that does not is returned as-is, so the spawn fails with
        // a normal "not found" rather than this function panicking.
        assert_eq!(
            resolve("strimux-no-such-program-xyz"),
            std::path::PathBuf::from("strimux-no-such-program-xyz")
        );
    }

    #[test]
    fn a_machine_we_cannot_help_is_told_where_to_look_not_left_silent() {
        assert_eq!(run(&Plan::Manual), Outcome::Failed(MANUAL_HINT.to_string()));
        // And probing the real machine never panics, whatever is installed.
        let _ = plan(Facts::probe());
    }
}
