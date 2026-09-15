//! Staying current: how an *already installed* gwae learns about a new
//! version, and how it moves to one (ADR-016).
//!
//! Homebrew is the absolute source of truth: `brew install hongnoul/tap/gwae`
//! installs, `brew upgrade gwae` upgrades. Every other source below is legacy
//! detection so old installs get a truthful answer, not a supported route.
//!
//! The rule this module exists to enforce is one sentence: **gwae updates
//! itself the way it was installed, or not at all.** A binary that overwrites
//! itself in place would be wrong for most of the ways gwae is actually on a
//! machine — Homebrew tracks file ownership, `cargo install` owns
//! `~/.cargo/bin`, Nix store paths are read-only by design, and a distro
//! package manager would silently disown a file it did not write. So the
//! upgrade *route* is a decision, and it is made here.
//!
//! Three separable questions, deliberately kept apart:
//!
//! 1. **Where did this binary come from?** [`Source`], decided by
//!    [`detect`] from facts ([`Facts`]) rather than probed inline, so every
//!    branch is testable without owning five differently-installed machines.
//!    Order of authority: the user's config, then the legacy receipt the
//!    retired installer left behind, then the path the running binary
//!    sits at. A guess is always labelled as one ([`Source::Unknown`]).
//! 2. **What would upgrading take?** [`plan`], pure, yielding either a
//!    command we are willing to run ([`Plan::commands`]) or an explanation of
//!    the one command *you* should run when the answer belongs to a package
//!    manager we must not fight — or to a retired route that now means
//!    reinstalling with Homebrew.
//! 3. **Is there anything to upgrade to?** [`latest_version`] asks GitHub's
//!    `releases/latest` redirect for a tag. That request is a bare HTTP HEAD:
//!    it carries no version, no machine id, and nothing about the user, and
//!    it happens at most once a day ([`CHECK_INTERVAL`]) behind a config key
//!    that can turn it off entirely.
//!
//! What this module never does is install anything on its own. The check
//! notifies; `gwae upgrade` prints the exact command and stops.
//! Software that replaces itself without being asked is a class of surprise a
//! terminal multiplexer has not earned the right to hand anyone.

use std::time::Duration;

mod check;
mod plan;
mod source;

pub const REPO: &str = "hongnoul/gwae";

/// The version this binary was built as.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How long a "latest version" answer is trusted before we ask again.
///
/// A day. Long enough that a machine that opens forty gwae sessions a day
/// makes one request, short enough that a release is noticed the next
/// morning.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the network check may take before it is abandoned.
///
/// The check runs on a background thread and its result is *optional*, so the
/// only thing a slow answer can cost is the answer itself. Five seconds keeps
/// a stalled captive-portal DNS from leaving a thread alive for the whole
/// session.
const NET_TIMEOUT_SECS: u64 = 5;

/// Kill switch for the network check, for CI and for anyone scripting gwae.
///
/// Checked in addition to the config key so that a machine can be made quiet
/// without editing a config file it may not own.
pub const NO_CHECK_ENV: &str = "GWAE_NO_UPDATE_CHECK";

/// Override for the detected install source, e.g. `GWAE_UPDATE_SOURCE=brew`.
/// Same values the config key takes. Mostly for tests and for packagers who
/// vendor gwae somewhere the heuristics cannot see.
pub const SOURCE_ENV: &str = "GWAE_UPDATE_SOURCE";

pub use check::{doctor_line, run_upgrade, spawn_check};
pub use source::{detect, probe, Source};
