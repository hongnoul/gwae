//! The agent gateway: what `⌥+;` (spawn-agent) actually runs.
//!
//! `⌥+;` does not exec the user's harness directly. It opens a pane running
//! `gwae agent`, and *this* module decides what that pane becomes. The pane
//! is a real PTY, so the gateway is an ordinary interactive program: it can
//! print, read keys, and then `exec` the chosen harness so the pane's process
//! *is* the harness (no wrapper left in the process tree, and the harness's
//! own OSC title reaches the host untouched).
//!
//! Three outcomes, in order:
//!
//! 1. `default_agent` is set and resolves on `PATH` -> exec it immediately.
//!    The gateway paints nothing and costs one `execvp`.
//! 2. It is unset (or missing) and harnesses *are* installed -> show a picker,
//!    save the choice to the config file, exec it.
//! 3. Nothing is installed -> explain that, and exec `$SHELL` so the pane is
//!    still a usable terminal rather than a dead box.

mod config;
mod detect;
mod picker;
mod run;

pub use config::Plan;
pub use config::{plan, set_scalar_text, toml_string_pub};
pub use detect::{detect_with, which};
pub use run::run;
