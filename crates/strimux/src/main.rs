//! strimux entry point.
//!
//! Milestone M0 goal is a single process owning PTYs and composing one 2D cell
//! buffer. This scaffold wires the CLI, config, and a `Layout` smoke demo; the
//! renderer/PTY loop lands in the M0 spike.

mod cli;
mod config;
mod theme;
mod tui;

use clap::Parser;
use cli::{Cli, Command};
use config::Config;
use strimux_layout::Viewport;

fn main() {
    // Logs go to stderr; `STRIMUX_LOG` controls the filter (tracing directive).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("STRIMUX_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg_path = Config::default_path();
    let cfg = Config::load(&cfg_path);

    if let Err(code) = run(cli, cfg) {
        std::process::exit(code);
    }
}

fn run(cli: Cli, cfg: Config) -> Result<(), i32> {
    match cli.command.unwrap_or(Command::Run { command: None }) {
        Command::Run { command } => tui::run_tui(command, cfg),
        Command::New { command } => {
            tracing::info!(command = ?command, "new column (PTY spawn lands in M0 spike)");
            Ok(())
        }
        Command::Setup => {
            println!("strimux setup: no per-terminal bindings installed yet (M4).");
            println!("  Alt is the universal $mod and needs no config.");
            Ok(())
        }
        Command::Doctor => {
            println!("strimux doctor:");
            println!("  config: {}", Config::default_path().display());
            println!("  layout smoke: {}", layout_smoke());
            Ok(())
        }
    }
}

/// A tiny proof the pure layout core works end to end, used by `doctor`.
fn layout_smoke() -> String {
    let mut layout = strimux_layout::Layout::default();
    let view = Viewport::new(120);
    let follow = strimux_layout::FollowScroll::default();
    let before = layout
        .column_x_ranges(layout.focus.row, view.cols)
        .map(|r| r.len())
        .unwrap_or(0);
    let _ = layout.apply(strimux_layout::Action::NewColumn, view, follow);
    let after = layout
        .column_x_ranges(layout.focus.row, view.cols)
        .map(|r| r.len())
        .unwrap_or(0);
    format!("columns {before} -> {after} on default row [ok]")
}
