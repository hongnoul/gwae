//! Where a new pane starts: the spawn directory.
//!
//! Every pane used to inherit gwae's own working directory, which is
//! whatever the terminal opened at (`$HOME`, usually). That made `⌥+;` a
//! two-step verb in practice: spawn the harness, then `cd` it to the repo you
//! actually meant. This module is the one place that answers "which
//! directory does a pane start in", for three inputs at three lifetimes:
//!
//! * `agent_dir` in the config file (persistent),
//! * `gwae run --dir` (this session),
//! * the `⌥+d` picker (this session, and optionally written back).
//!
//! The value is a *spawn-time* input only. Once a pane exists, its cwd
//! belongs to the child process: gwae cannot follow a `cd` without shell
//! integration it deliberately does not require, so there is nothing here to
//! keep in sync.


mod path;
mod picker;
mod scan;

pub use path::{check, expand, inherited, resolve, resolve_for_harness};
pub use picker::{candidates, candidates_for_harness, filter, tilde, Candidate};
pub use scan::{is_project, scan, search_roots, zoxide_dirs};
