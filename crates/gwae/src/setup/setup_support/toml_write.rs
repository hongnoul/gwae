//! The single writer for gwae's own TOML.
//!
//! All stages return `(key, toml_value)` pairs; one commit writes them once
//! through the existing comment-preserving rewrite in `onboard`. This is
//! what ends the four-writer race (`save_default_agent`, `save_input_poll`,
//! `save_answers`, `write_keep_awake`) on one file.

use std::path::Path;

/// Apply answered top-level keys to config `text`, preserving comments and
/// any keys setup never asked about. Pure.
pub fn apply_answers(text: &str, answers: &[(String, String)]) -> String {
    crate::onboard::apply_answers(text, answers)
}

/// Write the answers to `path`, creating the file and its parent as needed.
pub fn save_answers(path: &Path, answers: &[(String, String)]) -> std::io::Result<()> {
    crate::onboard::save_answers(path, answers)
}
