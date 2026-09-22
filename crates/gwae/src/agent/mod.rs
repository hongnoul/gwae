//! The agent gateway: what a `⌥+;` pane runs when the TUI has nothing to
//! resolve it to directly.
//!
//! The fast paths (an explicit `default_agent`, a remembered pick, a lone
//! install) never reach this module: the TUI spawns the harness directly.
//! The gateway is the cold-start bootstrap and the `gwae agent` CLI path. The
//! pane is a real PTY, so the gateway is an ordinary interactive program: it
//! can print, read keys, and then `exec` the chosen harness so the pane's
//! process *is* the harness (no wrapper left in the process tree, and the
//! harness's own OSC title reaches the host untouched).
//!
//! Three outcomes, in order:
//!
//! 1. `default_agent` is set and resolves on `PATH` -> exec it immediately.
//!    The gateway paints nothing and costs one `execvp`. A remembered pick
//!    that still resolves behaves the same way.
//! 2. It is unset (or missing) and harnesses *are* installed -> show a picker
//!    and exec the pick. The choice is remembered in the harness state file,
//!    never in the config file.
//! 3. Nothing is installed -> explain that, and exec `$SHELL` so the pane is
//!    still a usable terminal rather than a dead box.

mod config;
mod detect;
mod picker;
mod run;
mod state;

pub use config::Plan;
pub use config::{plan, set_scalar_text, toml_string_pub};
pub use detect::{command_available, detect, which, Found};
pub(crate) use picker::{parse_choice, render, Choice, DIM, RESET};
pub use run::run;
pub use state::{
    default_path as harness_state_path, load as load_harness_state,
    load_seeded as load_seeded_harness_state, save as save_harness_state, HarnessState,
};
