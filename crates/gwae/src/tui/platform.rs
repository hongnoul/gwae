//! Host platform detection + macOS Option-key poll (verbatim move from `tui/mod.rs`).
//!
//! The only file in `tui/` allowed `cfg(target_os = "macos")`.

/// Whether the terminal gwae itself runs in understands Kitty graphics.
///
/// Env-based, mirroring how jcode and ratatui-image decide: Kitty exports
/// `KITTY_WINDOW_ID`, Kitty-protocol terminals (Ghostty, WezTerm's kitty mode)
/// advertise via TERM/TERM_PROGRAM. `GWAE_KITTY_GRAPHICS=1/0` overrides
/// detection either way (e.g. gwae inside ssh where env vars were dropped).
pub(crate) fn host_supports_kitty_graphics() -> bool {
    if let Ok(v) = std::env::var("GWAE_KITTY_GRAPHICS") {
        return matches!(v.trim(), "1" | "true" | "yes" | "on");
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if term.contains("kitty") || term.contains("ghostty") {
        return true;
    }
    let prog = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    prog.contains("kitty") || prog.contains("ghostty") || prog.contains("wezterm")
}

pub(crate) fn is_ghostty() -> bool {
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if term.contains("ghostty") {
        return true;
    }
    std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase()
        .contains("ghostty")
}

pub(crate) fn native_modifier_poll_enabled(disable: Option<&str>) -> bool {
    disable != Some("1")
}

/// Whether the macOS Option key is physically held, via a native CoreGraphics poll.
/// (Full doc comment stays on the macOS impl in the original; see git history.)
#[cfg(target_os = "macos")]
pub(crate) fn macos_option_held() -> bool {
    // `kCGEventSourceStateCombinedSessionState = 0`
    // `kCGEventFlagMaskAlternate = NX_ALTERNATEMASK = 0x00080000`
    const K_COMBINED_SESSION: i32 = 0;
    const K_ALTERNATE: u64 = 0x0008_0000;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    // CGEventSourceFlagsState has been present since macOS 10.4.
    let flags = unsafe { CGEventSourceFlagsState(K_COMBINED_SESSION) };
    (flags & K_ALTERNATE) != 0
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn macos_option_held() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_modifier_poll_requires_an_explicit_opt_out() {
        assert!(native_modifier_poll_enabled(None));
        assert!(native_modifier_poll_enabled(Some("0")));
        assert!(!native_modifier_poll_enabled(Some("1")));
    }
}
