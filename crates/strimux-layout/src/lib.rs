//! strimux-layout: the pure, no-I/O core of strimux.
//!
//! Implements the infinite 2D grid of strips (ADR-013): rows that are
//! niri-style horizontal strips of no-shrink columns, stacked infinitely
//! downward. This crate deliberately depends on nothing but `std` (+ `serde`)
//! so the whole layout model can be property-tested exhaustively and reused
//! (e.g. by a future GUI client, or by `jcode-desktop`-style embeddings).
//!
//! There is no I/O, no async, no PTY, no terminal emulation here. Those live
//! in the `strimux` binary and `strimux-term` crate.

pub mod minimap;
pub mod model;
pub mod model_serde;
pub mod verbs;
pub mod viewport;
pub mod width;

pub use model::{Column, Focus, Layout, Pane, PaneId, PaneStatus, Row, RowId};
pub use verbs::Action;
pub use viewport::{FollowScroll, Viewport};
pub use width::{Preset, Width};

/// A result type shared across the pure core.
pub type LayoutResult<T> = Result<T, LayoutError>;

/// Errors produced by layout operations on malformed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    UnknownRow(RowId),
    UnknownColumn(usize),
    UnknownPane(usize),
    NothingToKill,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::UnknownRow(r) => write!(f, "row {r} does not exist"),
            LayoutError::UnknownColumn(c) => write!(f, "column index {c} does not exist"),
            LayoutError::UnknownPane(p) => write!(f, "pane index {p} does not exist"),
            LayoutError::NothingToKill => write!(f, "no panes left to kill"),
        }
    }
}

impl std::error::Error for LayoutError {}
