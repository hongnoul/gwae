//! strimux entry point.
//!
//! Milestone M0 goal is a single process owning PTYs and composing one 2D cell
//! buffer. This scaffold wires the CLI, config, and a `Layout` smoke demo; the
//! renderer/PTY loop lands in the M0 spike.

mod binds;
mod cli;
mod config;
mod cowsay;
mod keys;
mod select;
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
            println!(
                "  {} is the universal $mod and needs no config.",
                keys::mod_key()
            );
            Ok(())
        }
        Command::Doctor => {
            println!("strimux doctor:");
            let path = Config::default_path();
            println!("  config: {}", path.display());
            println!("  config file: {}", config_file_status(&path));
            let (_, bad_theme) = cfg.palette_checked();
            match bad_theme {
                Some(name) => {
                    println!("  theme: UNKNOWN {name:?} -> falling back to catppuccin-mocha");
                    println!("    available: {}", theme::Palette::NAMES.join(", "));
                }
                None => println!("  theme: {} [ok]", cfg.theme_name()),
            }
            println!("  layout smoke: {}", layout_smoke());
            Ok(())
        }
    }
}

/// Whether the config file exists and parses, for `doctor`.
///
/// A malformed file is silently ignored at startup (strimux falls back to
/// defaults rather than refusing to launch), so `doctor` is the only place a
/// user can find out their config is not being applied.
fn config_file_status(path: &std::path::Path) -> String {
    match std::fs::read_to_string(path) {
        Err(_) => "not present (using defaults) [ok]".to_string(),
        Ok(text) => match toml::from_str::<toml::Value>(&text) {
            Ok(_) => "parses [ok]".to_string(),
            Err(e) => format!("INVALID, so it is being ignored entirely: {e}"),
        },
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
