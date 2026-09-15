//! Drag-to-copy selection and text paste inside a PTY pane.
//!
//! gwae captures the mouse (for click-to-focus, and to forward events to a
//! child that asked for mouse reporting), which takes native selection away
//! from the host terminal. A left drag copies the selected text on release.
//! Native clipboard helpers are preferred, with OSC 52 for remote terminals.
//! The host terminal owns clipboard reads. Native paste (Cmd+V or the host's
//! equivalent) arrives as text, encoded here for the child's bracketed-paste
//! mode. There is no separate Option+V shortcut or second clipboard read.

mod clipboard;
mod paste;
mod selection;

pub use clipboard::copy_to_clipboard;
pub use paste::{paste_bytes, PASTE_CHUNK};
pub use selection::{selected_text, Point, Selection};
