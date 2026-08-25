//! Command-line interface for the `strimux` binary.
//!
//! Single binary, subcommands: `run` (default), `new`, `agent`, `init`, `tune`, `setup`, `doctor`.
//! There is deliberately no `server`/`ctl`/`ls`/`kill-server`: strimux is
//! daemon-free (ADR-003 reversed, ADR-011).

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "strimux",
    version,
    about = "strimux: niri's scrolling tiling for your CLI agents, in any terminal",
    long_about = "A terminal-native, daemon-free multiplexer. Panes live on an infinite 2D \
                  grid of strips; Alt+hjkl moves focus. macOS, Windows, Linux."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the multiplexer (the default command).
    Run {
        /// Optional command to launch in the first pane instead of $SHELL.
        command: Option<String>,
    },
    /// Start a new column running a command in a fresh session.
    New {
        /// The command (with args) to run in the new column.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// The agent gateway that `⌥+;` runs: resolve `default_agent`, offer the
    /// harnesses found on PATH when it is unset or missing, save the choice,
    /// and exec it. Not usually run by hand.
    Agent {
        /// Print what would happen and exit, without prompting or exec'ing.
        #[arg(long)]
        print: bool,
    },
    /// Report input-latency settings across macOS, your terminal, and
    /// strimux, and apply the ones strimux owns.
    Tune {
        /// Write strimux's own fix to the config file (never touches macOS
        /// settings or your terminal's config).
        #[arg(long)]
        apply: bool,
    },
    /// Guided first-run setup: theme, layout, chrome, and an offer to install
    /// btm. Safe to re-run; it only rewrites the keys you answer and keeps
    /// your comments. Input latency is tuned silently, without asking.
    Init {
        /// Print every question and option instead of asking anything.
        #[arg(long)]
        print: bool,
        /// Print every frame of the opening title card instead of playing it.
        #[arg(long)]
        print_splash: bool,
    },
    /// Install optional per-terminal bindings (e.g. Cmd+hjkl on iTerm2/kitty).
    Setup,
    /// Print diagnostics about the current terminal and $mod decoding.
    Doctor,
}
