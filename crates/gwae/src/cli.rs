//! Command-line interface for the `gwae` binary.
//!
//! Single binary, subcommands: `run` (default), `agent`, `init`,
//! `setup`, `upgrade`, `doctor`.
//! There is deliberately no `server`/`ctl`/`ls`/`kill-server`: gwae is
//! daemon-free (ADR-003 reversed, ADR-011).
//! Latency tuning lives in `setup` (the `latency` stage), not its own
//! subcommand: one setup flow, not two.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "gwae",
    version,
    about = "gwae: a scrolling terminal multiplexer for macOS. Panes never shrink.",
    long_about = "A terminal-native, daemon-free multiplexer for macOS. Panes live on an infinite 2D \
                  grid of strips; Option+hjkl moves focus. Install with Homebrew: brew install hongnoul/tap/gwae."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Directory new panes start in, overriding `agent_dir` in the config for
    /// this session. Accepted before the subcommand so `gwae --dir ~/git`
    /// works as the shell alias people actually write.
    #[arg(long, global = true, value_name = "PATH")]
    pub dir: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the multiplexer (the default command).
    Run {
        /// Optional command to launch in the first pane instead of $SHELL.
        command: Option<String>,
    },
    /// The agent gateway that `⌥+;` runs: resolve `default_agent`, offer the
    /// harnesses found on PATH when it is unset or missing, save the choice,
    /// and exec it. Not usually run by hand.
    Agent {
        /// Print what would happen and exit, without prompting or exec'ing.
        #[arg(long)]
        print: bool,
    },
    /// Guided first-run setup (alias for `setup`).
    /// Safe to re-run.
    Init {
        /// Print every stage's planned steps instead of running anything.
        #[arg(long)]
        print: bool,
    },
    /// Run the unified setup flow: every stage in order, with confirmation
    /// before anything outside gwae's own config is touched.
    Setup {
        /// Audit only: report what would change and exit nonzero when any
        /// stage is unhealthy. No writes.
        #[arg(long)]
        check: bool,
        /// Apply without prompting (for scripts and dotfile bootstraps).
        /// Machine-owned files still print their diff first.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Run one stage by id (see `gwae setup --print` for ids).
        #[arg(long, value_name = "STAGE")]
        only: Option<String>,
        /// Print every stage's planned steps instead of running anything.
        #[arg(long)]
        print: bool,
    },
    /// Report the version, the detected install source, and the command
    /// that would upgrade this gwae. Never executes anything: run the
    /// printed command yourself.
    #[command(alias = "update")]
    Upgrade,
    /// Remove what the installer put down: the binary plus gwae's own
    /// config and state dirs. Brew/cargo/nix/system installs print the
    /// owner's command instead of fighting it. PATH lines the installer
    /// added are listed, never silently removed.
    Uninstall {
        /// Remove without prompting (for scripts). Without it, a tty
        /// confirm is required; piped sessions only print the plan.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Print diagnostics about the current terminal and $mod decoding.
    Doctor,
}
