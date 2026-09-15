//! First-run onboarding: configure *everything* worth configuring, once.
//!
//! The agent gateway used to ask exactly one question (which harness `⌥+;`
//! runs). Everything else - theme, layout, chrome - was discoverable only by
//! reading `docs/CONFIG.md` and hand-writing TOML, which in practice meant
//! almost nobody changed it. This module turns that into a short guided pass:
//! one full-screen question at a time, moved through with the arrow keys (or
//! `j`/`k`), confirmed with Enter, and finished with a summary screen showing
//! exactly what was written.
//!
//! Shape of the code, deliberately:
//!
//! * The **questions are data** ([`questions`]), so `gwae init --print`
//!   can show the whole flow and tests can assert every option is valid TOML
//!   that the real [`crate::config::Config`] accepts.
//! * The **input handling is pure** ([`step`] over [`Key`]), so every
//!   keystroke a user actually presses (arrows, `j`/`k`, a digit, Enter, esc)
//!   is tested without a PTY.
//! * The **write is one function** ([`save_answers`]) over the existing
//!   comment-preserving [`crate::agent::set_scalar_text`], so onboarding can
//!   never clobber a config a user already hand-edited.
//!
//! Machine-dependent tuning (`input_poll_ms`) is *not* a question: it has one
//! right answer, so [`crate::latency::apply_silently`] applies it before the
//! first question is drawn and the summary screen reports anything left that
//! only the user can change.
//!
//! Onboarding is only ever *offered*: it runs on the interactive gateway path
//! when the config has not been through it before, and on demand via
//! `gwae init`. A configured user is never interrupted.

mod persist;
mod questions;
mod screen;

pub use persist::{
    already_onboarded, apply_answers, maybe_run, run, save_answers, set_table_scalar_text,
};
pub use questions::{
    all_questions, all_questions_for, all_questions_for_with_extra, all_questions_with_extra,
    cowsay_question, harness_question, harness_question_with, questions, questions_with,
    with_existing, Answer, Opt, Question, INSTALL_KEY, MARKER,
};
pub use screen::{
    key_from_event, render_all, render_question, render_screen, render_sized, render_summary,
    step, summary_key, swatch, Key, Step,
};
