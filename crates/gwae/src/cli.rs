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
    about = "gwae: niri's scrolling tiling for your CLI agents, in any terminal",
    long_about = "A terminal-native, daemon-free multiplexer. Panes live on an infinite 2D \
                  grid of strips; Alt+hjkl moves focus. macOS, Windows, Linux."
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
    /// Move this gwae to the latest release, using the same route it was
    /// installed by (installer script, Homebrew, cargo), or print the command
    /// for the package manager that owns it (Nix, AUR, distro).
    ///
    /// Never runs anything without printing it first, and never touches a
    /// binary another package manager owns.
    #[command(alias = "update")]
    Upgrade {
        /// Report the version, the detected install source, and the command
        /// that would run, then stop.
        #[arg(long)]
        check: bool,
        /// Skip the confirmation prompt (for scripts and dotfile bootstraps).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Print diagnostics about the current terminal and $mod decoding.
    Doctor,
}
