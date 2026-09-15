//! gwae-term: the emulator facade.
//!
//! Isolates the terminal-emulation crate behind a single `TermGrid` trait so
//! swapping the backend touches exactly one crate. The Alacritty core reflows
//! primary-screen text and scrollback when pane widths change.

mod compat;

mod apc;
mod backend;
mod cell;
mod grid;
mod null;

pub use apc::KittyApcExtractor;
pub use backend::{TerminalGrid, Vt100Grid};
pub use cell::{CColor, Cell, Style, MAX_COMBINING, NO_COMBINING};
pub use grid::{Damage, Size, TermGrid};
pub use null::NullGrid;
