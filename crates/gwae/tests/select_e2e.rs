//! Clipboard removed: select_e2e disabled.
//!
//! gwae no longer writes the system clipboard (drag-to-copy clipboard
//! write and ⌥+c copy mode removed). Selection highlight remains for
//! visual feedback only. This file is kept as a tombstone. The retained
//! highlight-only behavior and clipboard preservation are exercised by the real
//! PTY drag regression in `paste_e2e.rs`, sharing its isolated clipboard harness.
