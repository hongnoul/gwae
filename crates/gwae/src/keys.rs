//! macOS key naming for the one modifier gwae uses.
//!
//! gwae is macOS-only, so every user-facing string uses the macOS glyphs:
//! `⌥` (Option), `↵` (Return), `⇧` (Shift), `⌃` (Control), `Cmd+V` for
//! paste. All bindings go through here so the cheat-sheet HUD spells every
//! chord the same way.

/// The modifier's display name: `⌥` (Option).
pub fn mod_key() -> &'static str {
    "⌥"
}

/// The Return key's display name: `↵` (matching how macOS itself renders it
/// in menus).
pub fn enter_key() -> &'static str {
    "↵"
}

/// The host terminal's normal paste shortcut: `Cmd+V`.
/// The terminal emits a paste event; gwae never needs a second clipboard key.
pub const fn paste_key() -> &'static str {
    "Cmd+V"
}

/// The Shift key's display name: `⇧`.
pub fn shift_key() -> &'static str {
    "⇧"
}

/// A chord, rendered as the modifier plus `key` (e.g. `⌥+g`). Used by the
/// cheat-sheet HUD so every binding is spelled the same way everywhere.
pub fn chord(key: &str) -> String {
    format!("{}+{}", mod_key(), key)
}

/// The Control key's display name: `⌃` (matching how macOS itself renders it
/// in menus).
pub fn ctrl_key() -> &'static str {
    "⌃"
}

/// A Control chord with Shift, e.g. `⌃+⇧+K`. Used for the jcode-style
/// transcript scroll bindings, which are Control chords rather than the
/// `$mod` (Option) chords the rest of gwae's bindings use.
pub fn ctrl_shift_chord(key: &str) -> String {
    format!("{}+{}+{}", ctrl_key(), shift_key(), key)
}

/// A shifted chord, e.g. `⌥+⇧+q`.
pub fn shift_chord(key: &str) -> String {
    format!("{}+{}+{}", mod_key(), shift_key(), key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_macos_glyphs() {
        assert_eq!(mod_key(), "⌥");
        assert_eq!(enter_key(), "↵");
        assert_eq!(shift_key(), "⇧");
        assert_eq!(ctrl_key(), "⌃");
        assert_eq!(chord("g"), "⌥+g");
        assert_eq!(ctrl_shift_chord("K"), "⌃+⇧+K");
        assert_eq!(paste_key(), "Cmd+V");
    }
}
