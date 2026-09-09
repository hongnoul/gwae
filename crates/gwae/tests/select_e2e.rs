//! Drag-copy end-to-end coverage lives in `paste_e2e.rs`, sharing its isolated
//! native clipboard helpers and real PTY harness. It covers copy-on-release,
//! copy/paste round trips, Unicode, mouse ownership, clamping, and helper failure.
//! Set GWAE_E2E_BIN to replay the same assertions against an earlier binary.
