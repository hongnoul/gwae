//! Embedded install payloads: files that ship with the repo but must be
//! available to an installed binary (brew, cargo) that has no `scripts/` or
//! `assets/` on disk.
//!
//! `include_str!` makes version skew impossible: the installed artifact is
//! always the one reviewed in the tree.

/// The kitty Space focus-repair daemon source
/// (`scripts/macos/gwae-focus-fix.swift`).
pub const FOCUS_FIX_SWIFT: &str =
    include_str!("../../../../../scripts/macos/gwae-focus-fix.swift");

/// The launchd plist template (`scripts/macos/com.gwae.focus-fix.plist`).
///
/// `$HOME` in `ProgramArguments` is a template hole: launchd does not
/// expand it, so the installer renders the absolute binary path.
pub const FOCUS_FIX_PLIST: &str =
    include_str!("../../../../../scripts/macos/com.gwae.focus-fix.plist");

/// Render the plist for this machine: absolute binary path plus socket.
///
/// Takes the binary path explicitly rather than reading `$HOME` so tests
/// can render into a temp dir.
pub fn render_focus_plist(binary: &std::path::Path, socket: &str) -> String {
    let mut out = FOCUS_FIX_PLIST.replace(
        "$HOME/.local/bin/gwae-focus-fix",
        &binary.display().to_string(),
    );
    out = out.replace("unix:/tmp/mykitty", socket);
    out
}

/// The socket the daemon and kitty agree on, unless `GWAE_KITTY_SOCKET` says
/// otherwise. One constant so the plist, the probe, and the docs agree.
pub fn kitty_socket() -> String {
    std::env::var("GWAE_KITTY_SOCKET").unwrap_or_else(|_| "unix:/tmp/mykitty".to_string())
}

/// Where the compiled daemon lives.
pub fn focus_binary_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join(".local/bin/gwae-focus-fix"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_renders_absolute_paths() {
        let bin = std::path::Path::new("/tmp/sandbox/bin/gwae-focus-fix");
        let out = render_focus_plist(bin, "unix:/tmp/testkitty");
        assert!(out.contains("/tmp/sandbox/bin/gwae-focus-fix"), "{out}");
        assert!(!out.contains("$HOME"), "launchd does not expand $HOME: {out}");
        assert!(out.contains("unix:/tmp/testkitty"), "{out}");
    }

    #[test]
    fn embedded_payloads_are_nonempty() {
        assert!(FOCUS_FIX_SWIFT.contains("focus-window"), "swift source changed shape");
        assert!(FOCUS_FIX_PLIST.contains("com.gwae.focus-fix"), "plist changed shape");
    }
}
