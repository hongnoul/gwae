//! Shell word splitting + the agent gateway command (verbatim move from `tui/mod.rs`).

/// The interactive shell a fresh pane runs when no command was given.
///
/// Unix reads `$SHELL` (else `sh`). Windows has neither: `$SHELL` is
/// normally unset and `sh` is not on PATH, so fall back to PowerShell,
/// which ConPTY hosts natively.
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        return std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "powershell.exe".into());
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
    }
}

/// The command an agent pane runs: this very binary's `agent` subcommand.
///
/// `current_exe` rather than a bare `gwae`, so a binary that is not on
/// `PATH` (a `cargo run` build, or an install into a directory the shell does
/// not know about) still spawns *itself* rather than some other gwae, or
/// nothing at all.
pub(crate) fn agent_gateway_cmd() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "gwae".to_string());
    // Quoted so an install path containing spaces survives `shell_split`.
    format!("\"{exe}\" agent")
}

/// Naive shell splitter: split on whitespace, keeping simple quoting (\"..\").
pub fn shell_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in cmd.chars() {
        match c {
            '\'' | '"' => {
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
