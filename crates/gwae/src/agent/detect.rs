//! Harness discovery: what is installed and what looks like an agent.

use std::path::{Path, PathBuf};

/// Agent harnesses we can name, in the order they are shown. This list is
/// *labeling*, not the limit of what is detectable: anything matching
/// [`looks_like_agent`] on `PATH` is offered too, config can name more, and
/// the picker always lets you type a command. New harnesses appear constantly,
/// so an allowlist alone would be wrong the week after it shipped.
pub const KNOWN_AGENTS: &[(&str, &str)] = &[
    ("jcode", "jcode"),
    ("claude", "Claude Code"),
    ("codex", "OpenAI Codex"),
    ("gemini", "Gemini CLI"),
    ("muse", "Muse Code"),
    ("hermes", "Hermes Agent"),
    ("opencode", "opencode"),
    ("crush", "Crush"),
    ("aider", "aider"),
    ("cursor-agent", "Cursor Agent"),
    ("amp", "Amp"),
    ("goose", "goose"),
    ("copilot", "GitHub Copilot CLI"),
    ("q", "Amazon Q"),
    ("cline", "Cline"),
    ("continue", "Continue"),
    ("droid", "Factory Droid"),
    ("codebuff", "Codebuff"),
    ("forge", "Forge"),
    ("kode", "Kode"),
    ("octofriend", "Octofriend"),
];

/// Word-ish fragments that mark a command as *probably* an agent harness,
/// used to find ones we have never heard of. Deliberately narrow: a false
/// positive puts a junk entry in the picker, which is far more annoying than
/// a miss the user can still fix by typing the command.
const AGENT_HINTS: &[&str] = &["agent", "code", "coder", "llm", "gpt", "ai"];

/// Hints that also count when they merely *end* a one-word name, so
/// `musecode` is found without `codesign` being dragged in. Kept separate and
/// short: a leading match is almost never an agent.
const SUFFIX_HINTS: &[&str] = &["code", "coder", "agent"];

/// System directories that are never where an agent harness installs itself.
///
/// This is the single highest-value filter: without it, a stock macOS `PATH`
/// contributes `ssh-agent`, `KernelEventAgent`, `b64encode`, `uudecode` and
/// the disk tool `gpt`, which buries the two or three real entries. Harnesses
/// ship via npm/cargo/homebrew/pipx, i.e. under `$HOME` or a package prefix,
/// so scanning only those loses nothing real. Explicitly-known names and
/// config entries are still resolved anywhere on `PATH`.
const SYSTEM_DIRS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/libexec",
    "/System",
    "/Library",
    "/var",
    "/etc",
    // Legacy: the agent scan once ran on Windows too. Kept so the constant
    // stays a complete denylist if the scan ever sees such a path.
    "C:\\Windows",
];

/// Whether the heuristic scan should look inside `dir` at all.
pub fn scannable_dir(dir: &Path) -> bool {
    let d = dir.to_string_lossy();
    // `/usr/local/*` and `/opt/*` are package prefixes, not the OS, so they
    // stay in scope even though `/usr/*` broadly does not.
    if d.starts_with("/usr/local") || d.starts_with("/opt") {
        return true;
    }
    !SYSTEM_DIRS
        .iter()
        .any(|s| d == *s || d.starts_with(&format!("{s}/")))
}

/// Suffixes that mark a file as not-a-command even when it is executable.
const NOISE_SUFFIXES: &[&str] = &[
    ".new", ".old", ".bak", ".orig", ".tmp", ".save", ".dSYM", ".dylib", ".so", ".1",
];

/// Prefixes/names that match the hints but are definitely not agents. Without
/// these, a normal developer machine offers `code` (VS Code), `codesign`, and
/// half of `pkgconf` under the "ai"/"code" hints.
const NOISE_NAMES: &[&str] = &[
    "code",
    "codesign",
    "codesign_allocate",
    "aiff",
    "aifccompiler",
    "ailment",
    "pagestuff",
    "encode",
    "decode",
    "geocode",
    "unicode",
    "gencode",
    "barcode",
    "opcode",
    "zipcodes",
    "aiverify",
];

/// Whether a bare command name looks like an agent harness we should offer.
///
/// Split out and pure so the heuristic's exact edges are pinned by tests
/// rather than discovered by a user staring at a polluted picker.
pub fn looks_like_agent(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if NOISE_NAMES.contains(&lower.as_str()) {
        return false;
    }
    if NOISE_SUFFIXES.iter().any(|sfx| lower.ends_with(sfx)) {
        return false;
    }
    // Version-suffixed duplicates of a real command (`muse-bin-0.2.1-R1215.1`)
    // are the same tool twice; keep the clean name only.
    if lower.chars().any(|c| c.is_ascii_digit()) && lower.contains('-') {
        return false;
    }
    // A hint has to appear as a *word*, so `cursor-agent` and `my_code` match
    // but `codesign` does not.
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.iter().any(|w| AGENT_HINTS.contains(w)) {
        return true;
    }
    // Harnesses are also routinely named as one word ending in the hint
    // (`musecode`, `hermesagent`). Only trailing matches count, since a
    // leading one is nearly always a different kind of tool (`codesign`,
    // `aiff`), and the `NOISE_NAMES` list catches the common `-code` verbs
    // like `decode` and `unicode`.
    SUFFIX_HINTS
        .iter()
        .any(|h| lower.ends_with(h) && lower.len() > h.len() + 1)
}

/// True when `p` is a file we could actually execute.
fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Resolve `exe` the way a shell would: an explicit path is taken as-is, a
/// bare name is searched across `PATH`. Returns the full path when found.
pub fn which(exe: &str) -> Option<PathBuf> {
    if exe.is_empty() {
        return None;
    }
    if exe.contains('/') || exe.contains('\\') {
        let p = PathBuf::from(exe);
        return executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|c| executable(c))
}

/// True when the first word of `cmd` resolves to something spawnable. `cmd`
/// may carry arguments (`"jcode --resume"`); only the executable is probed.
pub fn command_available(cmd: &str) -> bool {
    match crate::tui::shell_split(cmd).first() {
        Some(exe) => which(exe).is_some(),
        None => false,
    }
}

/// A harness found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The command to run (the bare name, as the user would type it).
    pub cmd: String,
    /// Human label for the picker.
    pub label: String,
    /// Where it was found, shown so the user can tell two installs apart.
    pub path: PathBuf,
}

/// Every harness we can find, best-known first.
///
/// Three sources, merged and de-duplicated by command name:
/// 1. `extra` — names from the user's config, which always win the labeling
///    and come first, since the user told us about them explicitly.
/// 2. [`KNOWN_AGENTS`] — the ones we can name nicely.
/// 3. A scan of every `PATH` directory for anything [`looks_like_agent`].
///
/// The scan is what makes a brand-new harness (or a personal wrapper script)
/// show up without a gwae release, which an allowlist alone can never do.
pub fn detect_with(extra: &[String]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn push(
        seen: &mut std::collections::HashSet<String>,
        out: &mut Vec<Found>,
        cmd: &str,
        label: &str,
        path: PathBuf,
    ) {
        if seen.insert(cmd.to_string()) {
            out.push(Found {
                cmd: cmd.to_string(),
                label: label.to_string(),
                path,
            });
        }
    }

    // 1. Explicitly configured names, in the user's own order.
    for cmd in extra {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        if let Some(path) = which(shell_exe(cmd)) {
            push(&mut seen, &mut out, cmd, cmd, path);
        }
    }
    // 2. Names we can label.
    for (cmd, label) in KNOWN_AGENTS {
        if let Some(path) = which(cmd) {
            push(&mut seen, &mut out, cmd, label, path);
        }
    }
    // 3. Anything else on PATH that looks the part.
    let mut discovered: Vec<Found> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if !scannable_dir(&dir) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if seen.contains(&name) || !looks_like_agent(&name) {
                    continue;
                }
                let p = e.path();
                if executable(&p) {
                    discovered.push(Found {
                        cmd: name.clone(),
                        label: name,
                        path: p,
                    });
                }
            }
        }
    }
    // Stable output: PATH order is not, and directory order certainly is not.
    discovered.sort_by(|a, b| a.cmd.cmp(&b.cmd));
    for f in discovered {
        push(
            &mut seen,
            &mut out,
            &f.cmd.clone(),
            &f.label.clone(),
            f.path,
        );
    }
    out
}

/// The executable word of a command line (`"jcode --resume"` -> `"jcode"`).
pub(super) fn shell_exe(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or("")
}

/// [`detect_with`] with no configured extras.
#[cfg(test)]
pub fn detect() -> Vec<Found> {
    detect_with(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_resolves_path_names_and_rejects_missing_or_non_executable() {
        assert!(which("sh").is_some());
        assert!(which("gwae-no-such-agent-xyz").is_none());
        assert!(which("/bin/sh").is_some());
        assert!(which("/bin/definitely-not-here").is_none());
        // A directory exists but is not spawnable.
        assert!(which("/bin").is_none());
        assert!(which("").is_none());
    }

    #[test]
    fn command_available_probes_only_the_executable_word() {
        assert!(command_available("sh -c 'echo hi'"));
        assert!(!command_available("gwae-nope --resume"));
        assert!(!command_available("   "));
    }

    #[test]
    fn detect_lists_known_agents_before_discovered_ones() {
        // Known names are labeled and ordered; discovered ones follow. The
        // picker's numbering depends on that being stable across runs.
        let got = detect();
        let known: Vec<&str> = KNOWN_AGENTS.iter().map(|(c, _)| *c).collect();
        let split = got
            .iter()
            .position(|f| !known.contains(&f.cmd.as_str()))
            .unwrap_or(got.len());
        let mut last = 0;
        for f in &got[..split] {
            let at = known.iter().position(|n| *n == f.cmd).unwrap();
            assert!(at >= last, "known agents out of order: {:?}", f.cmd);
            last = at;
        }
        // Discovered ones are sorted, so two runs agree.
        let tail: Vec<&str> = got[split..].iter().map(|f| f.cmd.as_str()).collect();
        let mut sorted = tail.clone();
        sorted.sort_unstable();
        assert_eq!(tail, sorted, "discovered agents must be sorted");
        // Nothing is listed twice, and everything listed really exists.
        let mut names: Vec<&str> = got.iter().map(|f| f.cmd.as_str()).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "duplicate entries in {got:?}");
        for f in &got {
            assert!(f.path.exists(), "listed a missing path: {:?}", f.path);
        }
    }

    #[test]
    fn configured_extras_are_offered_first_and_are_never_duplicated() {
        // `sh` stands in for a harness gwae has never heard of.
        let got = detect_with(&["sh".to_string()]);
        assert_eq!(got[0].cmd, "sh", "configured names come first: {got:?}");
        assert_eq!(got.iter().filter(|f| f.cmd == "sh").count(), 1);
        // Ones that are not installed are simply not shown, not errors.
        let got = detect_with(&["gwae-not-real-xyz".to_string()]);
        assert!(!got.iter().any(|f| f.cmd == "gwae-not-real-xyz"));
        // Blank entries in the config are ignored rather than listed.
        let got = detect_with(&["".to_string(), "   ".to_string()]);
        assert!(got.iter().all(|f| !f.cmd.trim().is_empty()));
    }

    #[test]
    fn the_heuristic_catches_unknown_harnesses_without_dragging_in_junk() {
        // The point of the scan: names we have never shipped support for.
        assert!(looks_like_agent("hermes-agent"));
        assert!(looks_like_agent("musecode"));
        assert!(looks_like_agent("my_code"));
        assert!(looks_like_agent("someone-ai"));
        assert!(looks_like_agent("llm"));
        assert!(looks_like_agent("zed-agent"));

        // ...without turning the picker into a listing of /usr/bin.
        assert!(!looks_like_agent("codesign"), "VS Code's neighbors");
        assert!(!looks_like_agent("code"), "an editor, not an agent");
        assert!(!looks_like_agent("decode"));
        assert!(!looks_like_agent("encode"));
        assert!(!looks_like_agent("unicode"));
        assert!(!looks_like_agent("git"));
        assert!(!looks_like_agent("python3"));
        assert!(!looks_like_agent("ls"));
        assert!(!looks_like_agent("aiff"));

        // Version-suffixed duplicates and editor backups are noise.
        assert!(!looks_like_agent("muse-bin-0.2.1-R1215.1"));
        assert!(!looks_like_agent("jcode.new"));
        assert!(!looks_like_agent("jcode.bak"));
    }

    #[test]
    fn the_scan_skips_system_directories_that_are_full_of_false_positives() {
        // Without this, a stock macOS PATH offers ssh-agent, KernelEventAgent,
        // b64encode, uudecode and the disk tool `gpt` above the real ones.
        assert!(!scannable_dir(Path::new("/usr/bin")));
        assert!(!scannable_dir(Path::new("/usr/sbin")));
        assert!(!scannable_dir(Path::new("/bin")));
        assert!(!scannable_dir(Path::new("/sbin")));
        assert!(!scannable_dir(Path::new("/usr/libexec")));
        assert!(!scannable_dir(Path::new("/System/Cryptexes/App/usr/bin")));

        // Where harnesses actually install.
        assert!(scannable_dir(Path::new("/Users/me/.local/bin")));
        assert!(scannable_dir(Path::new("/Users/me/.cargo/bin")));
        assert!(scannable_dir(Path::new("/opt/homebrew/bin")));
        assert!(scannable_dir(Path::new("/usr/local/bin")));
        assert!(scannable_dir(Path::new("/home/me/.npm-global/bin")));
    }

    #[test]
    fn a_real_path_scan_stays_free_of_system_noise() {
        // The end result users judge this on: the list must be short and real.
        let got = detect();
        for junk in [
            "ssh-agent",
            "KernelEventAgent",
            "BTLEServerAgent",
            "b64encode",
            "b64decode",
            "uuencode",
            "uudecode",
            "gpt",
            "codesign",
        ] {
            assert!(
                !got.iter().any(|f| f.cmd == junk),
                "{junk:?} must never be offered as an agent; got {:?}",
                got.iter().map(|f| &f.cmd).collect::<Vec<_>>()
            );
        }
    }
}
