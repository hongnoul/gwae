//! Shared helpers for setup stages. No stage imports another stage; common
//! code lives here instead.
//!
//! * [`terminal`]: OS and terminal identity every kitty-gated stage needs.
//! * [`kitty_conf`]: parse one `key value` setting out of kitty's config.
//! * [`ghostty_conf`]: parse one `key = value` setting out of ghostty's config.

pub mod ghostty_conf;
pub mod kitty_conf;
pub mod terminal;
