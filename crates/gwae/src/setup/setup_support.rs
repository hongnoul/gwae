//! Shared helpers for setup stages. No stage imports another stage; common
//! code lives here instead.
//!
//! * [`terminal`]: OS and terminal identity every kitty-gated stage needs.
//! * [`kitty_conf`]: parse and comment-preserving edit of kitty's
//!   `key value` config format.
//! * [`toml_write`]: the single writer for gwae's own TOML. All stages
//!   return `(key, value)` pairs; one commit writes them once.

pub mod embed;
pub mod kitty_conf;
pub mod terminal;
pub mod toml_write;
