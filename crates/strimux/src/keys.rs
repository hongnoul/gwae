//! Platform-aware naming for the one modifier strimux uses.
//!
//! Every strimux binding is a chord on a single `$mod` key. That key is
//! physically the same key everywhere, but its *name* is not: macOS keyboards
//! label it `⌥` (Option) and users look for that glyph, while on Linux and
//! Windows the same key is `Alt` and the `⌥` glyph is meaningless (and often
//! not even present in the terminal font). Hard-coding either name makes the
//! HUD and the cowsay hints wrong on half the platforms, so all user-facing
//! strings go through here.
//!
//! Resolution is `cfg!(target_os = "macos")` at compile time: strimux runs on
//! the machine whose keyboard the user is typing on, so the build target is
//! the right answer, and it costs nothing at runtime.

/// The modifier's display name: `⌥` on macOS, `Alt` elsewhere.
pub fn mod_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    }
}

/// The Return key's display name: `↵` on macOS (matching how macOS itself
/// renders it in menus), spelled out as `Enter` elsewhere, where keycaps say
/// "Enter" and the glyph is unfamiliar.
pub fn enter_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "↵"
    } else {
        "Enter"
    }
}

/// The Shift key's display name: `⇧` on macOS, `Shift` elsewhere.
pub fn shift_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⇧"
    } else {
        "Shift"
    }
}

/// A chord, rendered as the platform's modifier plus `key` (e.g. `⌥+g` or
/// `Alt+g`). Used by both the cheat-sheet HUD and the cowsay hints so the two
/// can never disagree about how a binding is spelled.
pub fn chord(key: &str) -> String {
    format!("{}+{}", mod_key(), key)
}

/// A shifted chord, e.g. `⌥+⇧+q` on macOS or `Alt+Shift+q` elsewhere.
pub fn shift_chord(key: &str) -> String {
    format!("{}+{}+{}", mod_key(), shift_key(), key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_the_build_target() {
        if cfg!(target_os = "macos") {
            assert_eq!(mod_key(), "⌥");
            assert_eq!(chord("g"), "⌥+g");
        } else {
            assert_eq!(mod_key(), "Alt");
            assert_eq!(chord("g"), "Alt+g");
        }
    }

    #[test]
    fn non_macos_names_are_ascii_words() {
        // The point of the fallback: no glyphs that a Linux/Windows terminal
        // font may not have, and no macOS-only vocabulary.
        if !cfg!(target_os = "macos") {
            for s in [mod_key(), enter_key(), shift_key()] {
                assert!(s.is_ascii(), "{s:?} should be plain ASCII off macOS");
            }
        }
    }
}
