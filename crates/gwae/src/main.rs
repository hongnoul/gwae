//! gwae entry point.
//!
//! Milestone M0 goal is a single process owning PTYs and composing one 2D cell
//! buffer. This scaffold wires the CLI, config, and a `Layout` smoke demo; the
//! renderer/PTY loop lands in the M0 spike.

mod agent;
mod binds;
mod cli;
mod config;
mod cowsay;
mod geometry;
mod graphics;
mod graphics_diacritics;
mod graphics_host;
mod graphics_legacy;
mod graphics_stream;
mod install;
mod keepawake;
mod keys;
mod latency;
mod onboard;
mod preview;
mod reap;
mod reload;
mod select;
mod setup;
mod spawndir;
mod splash;
mod theme;
mod tui;
mod update;

use clap::Parser;
use cli::{Cli, Command};
use config::Config;

fn main() {
    // Logs go to stderr; `GWAE_LOG` controls the filter (tracing directive).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GWAE_LOG")
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
    let dir = cli.dir.clone();
    match cli.command.unwrap_or(Command::Run { command: None }) {
        Command::Run { command } => tui::run_tui(command, cfg, dir),
        Command::New { command } => {
            tracing::info!(command = ?command, "new column (PTY spawn lands in M0 spike)");
            Ok(())
        }
        Command::Agent { print } => agent::run(
            &cfg.default_agent,
            &cfg.agents,
            cfg.input_poll_ms,
            &cfg_path_for_agent(),
            print,
        ),
        Command::Tune { apply } => {
            let code = latency::run_tune(cfg.input_poll_ms, &cfg_path_for_agent(), apply);
            if code == 0 {
                Ok(())
            } else {
                Err(code)
            }
        }
        Command::Init {
            print,
            print_splash,
        } => {
            if print_splash {
                let cols = crossterm::terminal::size().map(|(c, _)| c).unwrap_or(80);
                print!("{}", splash::render_all(&cfg.palette(), cols));
            } else if print {
                print!("{}", onboard::render_all());
            } else {
                onboard::run(&cfg_path_for_agent(), cfg.input_poll_ms);
            }
            Ok(())
        }
        Command::Upgrade { check, yes } => {
            match update::run_upgrade(cfg.update.source(), check, yes) {
                0 => Ok(()),
                code => Err(code),
            }
        }
        Command::Setup { check, yes, only, print } => {
            let path = Config::default_path();
            let ctx = setup::Ctx {
                cfg: &cfg,
                cfg_path: &path,
                dir: dir.as_deref(),
            };
            match setup::run_setup(&ctx, check, yes, only.as_deref(), print) {
                0 => Ok(()),
                code => Err(code),
            }
        }
        Command::Doctor => {
            println!("gwae doctor:");
            let path = Config::default_path();
            println!("  config: {}", path.display());
            // Every line below is produced by its owning setup stage, so
            // doctor can never disagree with the flow that acts on it.
            let ctx = setup::Ctx {
                cfg: &cfg,
                cfg_path: &path,
                dir: dir.as_deref(),
            };
            for (id, line) in setup::doctor_body(&ctx) {
                for (i, part) in line.split('\n').enumerate() {
                    if i == 0 {
                        println!("  {id}: {part}");
                    } else {
                        println!("    {part}");
                    }
                }
            }
            Ok(())
        }
    }
}

/// The config file the agent gateway writes its saved choice to.
fn cfg_path_for_agent() -> std::path::PathBuf {
    Config::default_path()
}
