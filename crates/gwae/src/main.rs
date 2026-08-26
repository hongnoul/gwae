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
mod install;
mod keys;
mod latency;
mod onboard;
mod preview;
mod reap;
mod select;
mod spawndir;
mod splash;
mod theme;
mod tui;
mod update;

use clap::Parser;
use cli::{Cli, Command};
use config::Config;
use gwae_layout::Viewport;

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
        Command::Setup => {
            println!("gwae setup: no per-terminal bindings installed yet (M4).");
            println!(
                "  {} is the universal $mod and needs no config.",
                keys::mod_key()
            );
            Ok(())
        }
        Command::Doctor => {
            println!("gwae doctor:");
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
            println!("  agent: {}", agent_status(&cfg));
            println!("  updates: {}", update_status(&cfg));
            println!("  spawn dir: {}", spawn_dir_status(&cfg, dir.as_deref()));
            println!("  onboarding: {}", onboarding_status(&path));
            println!(
                "  latency: {}",
                latency::summary(&latency::audit(cfg.input_poll_ms))
            );
            println!("  layout smoke: {}", layout_smoke());
            Ok(())
        }
    }
}

/// The config file the agent gateway writes its saved choice to.
fn cfg_path_for_agent() -> std::path::PathBuf {
    Config::default_path()
}

/// Whether the config file exists and parses, for `doctor`.
///
/// A malformed file is silently ignored at startup (gwae falls back to
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

/// How `⌥+;` will resolve right now, for `doctor`. This is the same decision
/// the gateway makes, so doctor can never disagree with the live behavior.
fn agent_status(cfg: &Config) -> String {
    match agent::plan(&cfg.default_agent, agent::detect_with(&cfg.agents)) {
        agent::Plan::Configured(cmd) => format!("{cmd} [ok]"),
        agent::Plan::Choose(found) => format!(
            "unset; ⌥+; will offer {} [ok]",
            found
                .iter()
                .map(|f| f.cmd.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        agent::Plan::Missing { want, found } => format!(
            "MISSING {want:?}; ⌥+; will offer {}",
            found
                .iter()
                .map(|f| f.cmd.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        agent::Plan::NoneInstalled { .. } => {
            "none installed; ⌥+; opens a shell and says so".to_string()
        }
    }
}

/// Where `⌥+;` will open a pane right now, for `doctor`. Reports the same
/// decision `run_tui` makes, including the fallback, so a typo'd `agent_dir`
/// is findable instead of silently ignored.
fn spawn_dir_status(cfg: &Config, cli_dir: Option<&str>) -> String {
    let resolved = spawndir::resolve(cli_dir, &cfg.agent_dir);
    let unset = cfg.agent_dir.trim().is_empty() && cli_dir.is_none();
    match resolved {
        Some(p) if unset => format!("{} (gwae's cwd; unset, ⌥+d picks one) [ok]", p.display()),
        Some(p) if Some(&p) != spawndir::inherited().as_ref() => format!("{} [ok]", p.display()),
        _ => {
            let raw = cli_dir
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&cfg.agent_dir);
            match spawndir::check(raw) {
                Ok(p) => format!("{} [ok]", p.display()),
                Err(e) => format!("INVALID {raw:?}: {e}; panes inherit gwae's cwd"),
            }
        }
    }
}

/// How this gwae would upgrade, for `doctor`.
///
/// Worth a line even when everything is fine: "how do I update this" is the
/// question every user of a curl-to-bash install asks eventually, and the
/// honest answer depends on facts (install path, receipt) only the binary
/// itself can see.
fn update_status(cfg: &Config) -> String {
    if let Some(bad) = cfg.update.bad_source() {
        return format!(
            "INVALID update.source {bad:?}, so it is ignored; valid: {}",
            update::Source::NAMES.join(", ")
        );
    }
    update::doctor_line(cfg.update.source(), cfg.update.check)
}

/// Whether this config has been through `gwae init`, for `doctor`.
fn onboarding_status(path: &std::path::Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if onboard::already_onboarded(&text) {
        "done [ok]".to_string()
    } else {
        "not run; `gwae init` configures theme, layout, chrome, latency".to_string()
    }
}

/// A tiny proof the pure layout core works end to end, used by `doctor`.
fn layout_smoke() -> String {
    let mut layout = gwae_layout::Layout::default();
    let view = Viewport::new(120);
    let follow = gwae_layout::FollowScroll::default();
    let before = layout
        .column_x_ranges(layout.focus.row, view.cols)
        .map(|r| r.len())
        .unwrap_or(0);
    let _ = layout.apply(gwae_layout::Action::NewColumn, view, follow);
    let after = layout
        .column_x_ranges(layout.focus.row, view.cols)
        .map(|r| r.len())
        .unwrap_or(0);
    format!("columns {before} -> {after} on default row [ok]")
}
