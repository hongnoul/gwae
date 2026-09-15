//! A live mockup of the grid, drawn beside every onboarding question.
//!
//! Onboarding used to describe its layout answers in words: "four across",
//! "always re-center the focused column", "show which cell each empty box
//! is". Words are the wrong medium for a tool whose entire subject is *what
//! the screen looks like*. Every mature terminal configurator that survives
//! contact with real users solves this the same way - Zellij's theme editor
//! renders a whole fake workspace, Claude Code's `/theme` repaints its own
//! chrome as the highlight moves - because a preview turns a question about
//! vocabulary ("is a `two-thirds` column what I want?") into a question about
//! a picture.
//!
//! So this module renders a small, honest picture of the grid from the
//! answers *so far plus the option currently under the cursor*, and
//! [`crate::onboard`] repaints it on every keystroke.
//!
//! Two properties make it trustworthy rather than decorative:
//!
//! * It is **pure** ([`render`] over [`Prefs`]), so `gwae init --print`
//!   and the tests see exactly the bytes a user sees.
//! * It reads its colors from the **real** [`Palette`] presets and its widths
//!   from the **real** [`gwae_layout::width::Width`], so a preview can never
//!   drift from what the multiplexer actually paints. A mockup that lies is
//!   worse than no mockup, because it is believed.

mod encode;
mod paint;
mod prefs;

pub use paint::{render, render_h};
pub use prefs::{fits, Prefs, H, H_MIN, W};
