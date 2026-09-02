//! The agent gateway: what `⌥+;` (spawn-agent) actually runs.
//!
//! `⌥+;` does not exec the user's harness directly. It opens a pane running
//! `gwae agent`, and *this* module decides what that pane becomes. The pane
//! is a real PTY, so the gateway is an ordinary interactive program: it can
//! print, read keys, and then `exec` the chosen harness so the pane's process
//! *is* the harness (no wrapper left in the process tree, and the harness's
//! own OSC title reaches the host untouched).
//!
//! Three outcomes, in order:
//!
//! 1. `default_agent` is set and resolves on `PATH` -> exec it immediately.
//!    The gateway paints nothing and costs one `execvp`.
//! 2. It is unset (or missing) and harnesses *are* installed -> show a picker,
//!    save the choice to the config file, exec it.
//! 3. Nothing is installed -> explain that, and exec `$SHELL` so the pane is
//!    still a usable terminal rather than a dead box.

use std::io::{IsTerminal, Write};
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
fn shell_exe(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or("")
}

/// [`detect_with`] with no configured extras.
#[cfg(test)]
pub fn detect() -> Vec<Found> {
    detect_with(&[])
}

/// What the gateway decided to do, before any of it is carried out. Split from
/// the doing so it can be tested without a PTY or an `exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `default_agent` resolved; exec it with no UI at all.
    Configured(String),
    /// `default_agent` is set but missing; offer these instead (never empty).
    Missing { want: String, found: Vec<Found> },
    /// Nothing configured, but harnesses exist; let the user pick.
    Choose(Vec<Found>),
    /// Nothing configured and nothing installed; fall back to a shell.
    NoneInstalled { want: Option<String> },
}

/// Decide what `gwae agent` should do for this `default_agent` setting.
pub fn plan(default_agent: &str, found: Vec<Found>) -> Plan {
    let want = default_agent.trim();
    if !want.is_empty() && command_available(want) {
        return Plan::Configured(want.to_string());
    }
    if found.is_empty() {
        return Plan::NoneInstalled {
            want: (!want.is_empty()).then(|| want.to_string()),
        };
    }
    if want.is_empty() {
        Plan::Choose(found)
    } else {
        Plan::Missing {
            want: want.to_string(),
            found,
        }
    }
}

/// The shell to fall back to when there is no harness to run.
pub fn fallback_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "sh".into())
}

/// Rewrite `default_agent` in a config file *without* disturbing anything
/// else: an existing top-level assignment is replaced in place, otherwise the
/// line is appended. Comments, key order, and formatting all survive, which a
/// parse/re-serialize round trip would destroy.
///
/// Only a top-level key is touched. Lines inside a `[table]` are skipped, so a
/// `default_agent` under some future section can never be clobbered.
pub fn set_default_agent_text(text: &str, agent: &str) -> String {
    set_scalar_text(text, "default_agent", &toml_string(agent))
}

/// As [`set_default_agent_text`], for any top-level key. `value` must already
/// be valid TOML (quoted for strings, bare for numbers), so the same
/// comment-preserving rewrite serves both the agent gateway and the latency
/// tuner rather than each growing its own config writer.
pub fn set_scalar_text(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut out: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut replaced = false;
    for raw in text.lines() {
        let t = raw.trim_start();
        if t.starts_with('[') {
            in_table = true;
        }
        let is_key = !in_table
            && !replaced
            && t.strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);
        if is_key {
            out.push(line.clone());
            replaced = true;
        } else {
            out.push(raw.to_string());
        }
    }
    if !replaced {
        // Append before the first table header if there is one, since a bare
        // key after `[theme]` would silently become `theme.default_agent`.
        let at = out
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .unwrap_or(out.len());
        let at = if at > 0 && !out[at.saturating_sub(1)].trim().is_empty() {
            out.insert(at, String::new());
            out.insert(at + 1, line);
            at + 1
        } else {
            out.insert(at, line);
            at
        };
        // Keep a blank line between the key and a following table header, so
        // repeated writes cannot glue `key = v` onto `[table]` and make the
        // file read as if the key were inside it.
        if out.get(at + 1).map(|l| l.trim_start().starts_with('[')) == Some(true) {
            out.insert(at + 1, String::new());
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// [`toml_string`] for callers outside this module (the `⌥+d` picker writes
/// `agent_dir` through the same comment-preserving path).
pub fn toml_string_pub(s: &str) -> String {
    toml_string(s)
}

/// Comment-preserving rewrite for `harness_dirs.<key> = "dir"`.
///
/// TOML allows `harness_dirs = { jcode = "..." }` and the dotted form
/// `[harness_dirs]` + `jcode = "..."` and inline mutations. The cheap
/// approach that handles each without a full parser is to do three passes:
/// 1) replace a dotted `harness_dirs.<key> = ...` line anywhere,
/// 2) replace inside an existing inline table `harness_dirs = { … }`,
/// 3) otherwise append/insert a dotted assignment after any existing
///    `[harness_dirs]` block or at the file's top-level tail.
///
/// Preserves comments/order; the file stays valid TOML.
pub fn set_harness_dir_text(text: &str, key: &str, dir: &str) -> String {
    let dotted = format!("harness_dirs.{key}");
    // 1. dotted form anywhere.
    let as_dotted = set_scalar_text(text, &dotted, &toml_string(dir));
    if as_dotted != text && as_dotted.contains(&format!("{dotted} =")) {
        // We actually replaced a dotted line; done.
        // Detect via whether the new text differs and contains the key now.
        // If user had no dotted line, set_scalar_text just appended one —
        // we still want to prefer inline table editing when one exists, so
        // only early-return when the original had a dotted line.
        if text.lines().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#')
                && !t.starts_with('[')
                && t.starts_with(&dotted)
                && t[dotted.len()..].trim_start().starts_with('=')
        }) {
            return as_dotted;
        }
    }
    // 2. inline table `harness_dirs = { ... }`
    let lines: Vec<&str> = text.lines().collect();
    for (idx, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        if t.starts_with('[') {
            continue;
        }
        // Look for `harness_dirs = {` on this line.
        let Some(eq) = t.find('=') else { continue };
        let lhs = t[..eq].trim();
        if lhs != "harness_dirs" {
            continue;
        }
        let rhs = t[eq + 1..].trim_start();
        if !(rhs.starts_with('{') && rhs.contains('}')) {
            continue;
        }
        // Replace or insert `key = "dir"` inside the braces, preserving prior content.
        // Parse naively: extract inside `{ }`.
        let lbrace = t.find('{').unwrap();
        let rbrace = t.rfind('}').unwrap();
        let inside = &t[lbrace + 1..rbrace];
        let val = toml_string(dir);
        // If key already there, replace its value; else append.
        let mut replaced = false;
        let mut parts: Vec<String> = Vec::new();
        // Split on commas not inside quotes (values are strings, so crude split on ',' is fine).
        let mut cur = String::new();
        let mut in_q = false;
        let mut esc = false;
        for ch in inside.chars() {
            if esc {
                cur.push(ch);
                esc = false;
                continue;
            }
            if ch == '\\' && in_q {
                cur.push(ch);
                esc = true;
                continue;
            }
            if ch == '"' {
                in_q = !in_q;
                cur.push(ch);
                continue;
            }
            if ch == ',' && !in_q {
                parts.push(cur);
                cur = String::new();
                continue;
            }
            cur.push(ch);
        }
        if !cur.trim().is_empty() || inside.contains(',') {
            parts.push(cur);
        }
        for p in parts.iter_mut() {
            let trimmed = p.trim_start();
            // Extract key before `=`
            if let Some(eq2) = trimmed.find('=') {
                let k = trimmed[..eq2].trim().trim_matches('"').trim();
                // Unquote dotted key form key.
                if k == key {
                    *p = format!(" {key} = {val} ");
                    replaced = true;
                }
            }
        }
        let new_inside = if replaced {
            parts.join(",")
        } else {
            if inside.trim().is_empty() {
                format!(" {key} = {val} ")
            } else {
                let trimmed = inside.trim_end();
                let sep = if trimmed.ends_with(',') { " " } else { ", " };
                format!("{inside}{sep}{key} = {val} ")
            }
        };
        let prefix = &raw[..raw.find('{').unwrap() + 1];
        let suffix = &raw[raw.rfind('}').unwrap()..];
        // Rebuild line preserving leading indent and trailing comment outside braces? Keep simple: just the braces content.
        // Preserve any leading indent from original.
        let indent_len = raw.len() - raw.trim_start().len();
        let indent = &raw[..indent_len];
        let new_line = format!("{indent}harness_dirs = {{{new_inside}}}");
        let _ = (prefix, suffix); // not used but keep signature clear
        let mut out: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        out[idx] = new_line;
        let mut s = out.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // 3. No inline table; if a `[harness_dirs]` section exists, add/replace `key =` inside it.
    if text.lines().any(|l| l.trim() == "[harness_dirs]") {
        let mut out: Vec<String> = Vec::new();
        let mut in_harness = false;
        let mut replaced = false;
        for raw in text.lines() {
            let t = raw.trim();
            if t.starts_with('[') {
                in_harness = t == "[harness_dirs]";
                out.push(raw.to_string());
                continue;
            }
            if in_harness && !replaced {
                let trimmed = raw.trim_start();
                if !trimmed.starts_with('#') && !trimmed.is_empty() {
                    if let Some(eq) = trimmed.find('=') {
                        let k = trimmed[..eq].trim().trim_matches('"').trim();
                        if k == key {
                            let indent_len = raw.len() - raw.trim_start().len();
                            let indent = &raw[..indent_len];
                            out.push(format!("{indent}{key} = {}", toml_string(dir)));
                            replaced = true;
                            continue;
                        }
                    }
                }
            }
            out.push(raw.to_string());
        }
        if !replaced {
            // Append inside the section, before next table or EOF.
            let mut inserted = false;
            let mut out2: Vec<String> = Vec::new();
            let mut in_harness2 = false;
            for (i, raw) in out.iter().enumerate() {
                if raw.trim() == "[harness_dirs]" {
                    in_harness2 = true;
                } else if raw.trim_start().starts_with('[') {
                    if in_harness2 && !inserted {
                        out2.push(format!("{key} = {}", toml_string(dir)));
                        inserted = true;
                    }
                    in_harness2 = false;
                }
                out2.push(raw.clone());
                // If at EOF and still in harness section.
                if i == out.len() - 1 && in_harness2 && !inserted {
                    out2.push(format!("{key} = {}", toml_string(dir)));
                    inserted = true;
                }
            }
            let mut s = out2.join("\n");
            if !s.ends_with('\n') {
                s.push('\n');
            }
            return s;
        }
        let mut s = out.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // 4. No existing harness_dirs at all: insert a dotted assignment like default_agent does,
    //    before the first table header if any.
    let line = format!("harness_dirs.{key} = {}", toml_string(dir));
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    // Reuse dotted insertion path: the `as_dotted` above already handled appending a dotted line;
    // but we short-circuited only when original had a dotted line, so this path adds it now.
    // Prefer inline table for first write: `harness_dirs = { jcode = "..." }`
    let inline = format!("harness_dirs = {{ {key} = {} }}", toml_string(dir));
    for raw in text.lines() {
        let t = raw.trim_start();
        if t.starts_with('[') && !replaced {
            // Insert before first table.
            if out
                .last()
                .map(|l: &String| !l.trim().is_empty())
                .unwrap_or(false)
            {
                out.push(String::new());
            }
            out.push(inline.clone());
            out.push(String::new());
            replaced = true;
        }
        out.push(raw.to_string());
    }
    if !replaced {
        if !out.is_empty() && !out.last().unwrap().trim().is_empty() {
            out.push(String::new());
        }
        out.push(inline);
    }
    let _ = line;
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Quote a value as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Persist `default_agent` to `path`, creating the file (and its parent) when
/// it does not exist yet.
pub fn save_default_agent(path: &Path, agent: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = if existing.trim().is_empty() {
        format!(
            "# gwae configuration\n{}\n",
            format_args!("default_agent = {}", toml_string(agent))
        )
    } else {
        set_default_agent_text(&existing, agent)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

/// Replace this process with `cmd` (split shell-style). On unix this is a real
/// `execvp`, so the pane's PTY, pid, and signals all transfer to the harness.
#[cfg(unix)]
fn exec(cmd: &str) -> ! {
    use std::os::unix::process::CommandExt;
    let argv = crate::tui::shell_split(cmd);
    if argv.is_empty() {
        std::process::exit(1);
    }
    let err = std::process::Command::new(&argv[0]).args(&argv[1..]).exec();
    eprintln!("gwae agent: cannot run {}: {err}", argv[0]);
    std::process::exit(127);
}

/// Windows has no `exec`, so run the child and forward its status.
#[cfg(not(unix))]
fn exec(cmd: &str) -> ! {
    let argv = crate::tui::shell_split(cmd);
    if argv.is_empty() {
        std::process::exit(1);
    }
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("gwae agent: cannot run {}: {e}", argv[0]);
            std::process::exit(127);
        }
    }
}

// ANSI used by the picker. The gateway paints plain text into its own pane, so
// it only needs colors and a couple of attributes, not a renderer.
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// The pane's width in columns, for laying the picker out. Panes are often a
/// quarter of the screen, so the full-width form does not fit and its most
/// important part (the list) would scroll away above the prompt.
fn term_cols() -> usize {
    // 80 when there is no tty (a pipe, `--print` into a file): the wide form
    // is the better default for something a human will read later.
    crossterm::terminal::size()
        .ok()
        .map(|(c, _)| c as usize)
        .filter(|c| *c > 0)
        .unwrap_or(80)
}

/// Render the plan as the text the user sees, and return the choices that the
/// on-screen numbers map to. Pure in `cols`, so the exact layout at any pane
/// width is testable.
pub fn render_at(plan: &Plan, cols: usize) -> (String, Vec<Found>) {
    let mut header = String::new();
    if !matches!(plan, Plan::Configured(_)) {
        let splash_w = crate::splash::art_width();
        if cols >= 50 && cols >= splash_w + 2 {
            let pal = crate::theme::Palette::default();
            header.push_str(&crate::splash::banner(usize::MAX, &pal, cols as u16));
        } else if cols >= 17 {
            header.push_str("gwae\n");
        }
    }
    // Under ~50 columns (a quarter-width pane on a typical screen) the paths
    // and the explanatory footer push the list off the top of the pane, which
    // leaves the user staring at a prompt with no visible options. The narrow
    // form drops both and keeps the choices adjacent to the prompt.
    let narrow = cols < 50;
    let mut s = String::new();
    let choices = match plan {
        Plan::Configured(_) => Vec::new(),
        Plan::Missing { want, found } => {
            if narrow {
                s.push_str(&format!(
                    "{YELLOW}{BOLD}`{want}` is not installed.{RESET}\n"
                ));
            } else {
                s.push_str(&format!(
                    "{YELLOW}{BOLD}`{want}` is not installed.{RESET}\n{DIM}Your config asks for it, but it is not on PATH. Pick another:{RESET}\n\n"
                ));
            }
            found.clone()
        }
        Plan::Choose(found) => {
            if narrow {
                s.push_str(&format!("{BOLD}Pick an agent:{RESET}\n"));
            } else {
                s.push_str(&format!(
                    "{BOLD}Which agent should {CYAN}⌥+;{RESET}{BOLD} launch?{RESET}\n{DIM}Found on your PATH:{RESET}\n\n"
                ));
            }
            found.clone()
        }
        Plan::NoneInstalled { want } => {
            match want {
                Some(w) if narrow => {
                    s.push_str(&format!("{YELLOW}{BOLD}`{w}` is not installed.{RESET}\n"))
                }
                Some(w) => s.push_str(&format!(
                    "{YELLOW}{BOLD}`{w}` is not installed{RESET}, and no other agent harness was found on your PATH.\n"
                )),
                None => s.push_str(&format!(
                    "{YELLOW}{BOLD}No agent harness found on your PATH.{RESET}\n"
                )),
            }
            if narrow {
                s.push_str(&format!(
                    "{DIM}Type its command, or Enter for a shell.{RESET}\n"
                ));
            } else {
                s.push_str(&format!(
                    "{DIM}Looked for: {}, plus anything on PATH that looks like an agent.{RESET}\n\nInstall one and press {CYAN}⌥+;{RESET} again, or type its command now\nif it lives somewhere we did not look. {DIM}Enter alone opens a shell.{RESET}\n",
                    KNOWN_AGENTS
                        .iter()
                        .map(|(c, _)| *c)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Vec::new()
        }
    };
    for (i, f) in choices.iter().enumerate() {
        // Enter takes the first entry, so it has to be labeled as such: an
        // unmarked default is one the user only discovers by triggering it.
        let dflt = if i == 0 {
            format!("  {DIM}(default){RESET}")
        } else {
            String::new()
        };
        if narrow {
            s.push_str(&format!(
                "  {CYAN}{}{RESET} {BOLD}{}{RESET}{dflt}\n",
                i + 1,
                f.label
            ));
        } else {
            s.push_str(&format!(
                "  {CYAN}{}{RESET}  {BOLD}{}{RESET}  {DIM}{}{RESET}{dflt}\n",
                i + 1,
                f.label,
                f.path.display()
            ));
        }
    }
    if !choices.is_empty() {
        if narrow {
            s.push_str(&format!(
                "  {CYAN}s{RESET} {BOLD}shell{RESET}\n{DIM}...or type a command.{RESET}\n"
            ));
        } else {
            s.push_str(&format!(
                "  {CYAN}s{RESET}  {BOLD}just a shell{RESET}  {DIM}skip, don't save{RESET}\n\n{DIM}Not listed? Type the command itself (e.g. {RESET}{CYAN}hermes --resume{RESET}{DIM}).\nYour choice is saved to {} as `default_agent`, so ⌥+; goes straight there next time.{RESET}\n",
                crate::config::Config::default_path().display()
            ));
        }
    }
    if !header.is_empty() {
        s = header + &s;
    }
    (s, choices)
}

/// [`render_at`] using the live terminal width.
pub fn render(plan: &Plan) -> (String, Vec<Found>) {
    render_at(plan, term_cols())
}

/// What the user typed at the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// One of the listed harnesses, by index.
    Listed(usize),
    /// A command they typed themselves, which resolved on `PATH`. This is the
    /// escape hatch that makes a harness gwae has never heard of usable
    /// today rather than after a release.
    Typed(String),
    /// Just a shell; save nothing.
    Shell,
}

/// Interpret one line of picker input. Pure, so every branch (including the
/// typo cases a user actually hits) is testable without a terminal.
pub fn parse_choice(line: &str, n: usize) -> Result<Choice, String> {
    let t = line.trim();
    if t.is_empty() {
        // Enter takes the first (most preferred) harness, or a shell when
        // there is nothing to take.
        return Ok(if n > 0 {
            Choice::Listed(0)
        } else {
            Choice::Shell
        });
    }
    if t.eq_ignore_ascii_case("s") || t.eq_ignore_ascii_case("shell") {
        return Ok(Choice::Shell);
    }
    // A bare number only means an index when it *is* one; otherwise fall
    // through, since a command could plausibly be named oddly.
    if let Ok(i) = t.parse::<usize>() {
        return if i >= 1 && i <= n {
            Ok(Choice::Listed(i - 1))
        } else {
            Err(format!("There is no {i}. Enter 1-{n}, a command, or s."))
        };
    }
    if command_available(t) {
        return Ok(Choice::Typed(t.to_string()));
    }
    Err(format!(
        "`{}` is not on your PATH. Enter 1-{n}, another command, or s for a shell.",
        shell_exe(t)
    ))
}

/// Read a choice, re-prompting until it is valid. Returns [`Choice::Shell`]
/// on EOF or a non-tty, so the gateway can never wedge a pane waiting for
/// input that will not come.
fn prompt(n: usize) -> Choice {
    use std::io::BufRead;
    if !std::io::stdin().is_terminal() {
        return Choice::Shell;
    }
    let stdin = std::io::stdin();
    loop {
        print!("\n{CYAN}>{RESET} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Choice::Shell,
            Ok(_) => {}
        }
        match parse_choice(&line, n) {
            Ok(c) => return c,
            Err(msg) => println!("{DIM}{msg}{RESET}"),
        }
    }
}

/// `gwae agent`: resolve, maybe ask, save, and exec. Never returns.
pub fn run(
    default_agent: &str,
    extra: &[String],
    input_poll_ms: u64,
    cfg_path: &Path,
    print_only: bool,
) -> ! {
    let p = plan(default_agent, detect_with(extra));

    if print_only {
        match &p {
            Plan::Configured(cmd) => println!(
                "default_agent: {cmd} [ok] -> {}",
                which(&crate::tui::shell_split(cmd)[0])
                    .unwrap_or_default()
                    .display()
            ),
            _ => {
                let (text, _) = render(&p);
                print!("{text}");
            }
        }
        std::process::exit(0);
    }

    let cmd = match p {
        Plan::Configured(cmd) => cmd,
        Plan::NoneInstalled { .. } => {
            // Still offer the typed escape hatch: "we found nothing" is a
            // statement about our search, not about the user's machine.
            let (text, _) = render(&p);
            print!("{text}");
            let _ = std::io::stdout().flush();
            match prompt(0) {
                Choice::Typed(cmd) => {
                    match save_default_agent(cfg_path, &cmd) {
                        Ok(()) => println!("{DIM}Saved default_agent = \"{cmd}\".{RESET}"),
                        Err(e) => println!(
                            "{YELLOW}Could not save to {}: {e}{RESET}",
                            cfg_path.display()
                        ),
                    }
                    crate::onboard::maybe_run(cfg_path, input_poll_ms);
                    cmd
                }
                _ => fallback_shell(),
            }
        }
        ref chooser => {
            let (text, choices) = render(chooser);
            print!("{text}");
            let _ = std::io::stdout().flush();
            let pick = match prompt(choices.len()) {
                Choice::Listed(i) => Some(choices[i].cmd.clone()),
                Choice::Typed(cmd) => Some(cmd),
                Choice::Shell => None,
            };
            match pick {
                Some(pick) => {
                    match save_default_agent(cfg_path, &pick) {
                        Ok(()) => println!("{DIM}Saved default_agent = \"{pick}\".{RESET}"),
                        Err(e) => println!(
                            "{YELLOW}Could not save to {}: {e}{RESET}",
                            cfg_path.display()
                        ),
                    }
                    crate::onboard::maybe_run(cfg_path, input_poll_ms);
                    pick
                }
                None => fallback_shell(),
            }
        }
    };
    // The harness owns the pane from here: same pid, same PTY, no wrapper.
    exec(&cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drop SGR escapes so assertions read the text a user sees.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn found(cmd: &str) -> Found {
        Found {
            cmd: cmd.into(),
            label: cmd.into(),
            path: PathBuf::from("/usr/bin").join(cmd),
        }
    }

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
    fn a_resolvable_configured_agent_short_circuits_every_prompt() {
        // The common case must never paint: config wins, no detection UI.
        assert_eq!(
            plan("sh", vec![found("jcode")]),
            Plan::Configured("sh".into())
        );
        // Whitespace is not a configuration.
        assert!(matches!(plan("   ", vec![]), Plan::NoneInstalled { .. }));
    }

    #[test]
    fn unset_agent_with_installs_offers_a_choice_and_without_them_falls_back() {
        assert_eq!(
            plan("", vec![found("jcode"), found("claude")]),
            Plan::Choose(vec![found("jcode"), found("claude")])
        );
        assert_eq!(plan("", vec![]), Plan::NoneInstalled { want: None });
    }

    #[test]
    fn a_configured_but_missing_agent_reports_what_was_wanted() {
        assert_eq!(
            plan("jcode-not-real", vec![found("claude")]),
            Plan::Missing {
                want: "jcode-not-real".into(),
                found: vec![found("claude")],
            }
        );
        assert_eq!(
            plan("jcode-not-real", vec![]),
            Plan::NoneInstalled {
                want: Some("jcode-not-real".into())
            }
        );
    }

    #[test]
    fn rendering_lists_every_choice_and_names_the_missing_agent() {
        let (text, choices) = render(&Plan::Missing {
            want: "jcode".into(),
            found: vec![found("claude"), found("codex")],
        });
        assert!(text.contains("`jcode` is not installed"));
        assert!(text.contains("/usr/bin/claude"));
        assert!(text.contains("/usr/bin/codex"));
        // Numbering is 1-based and matches the returned choice order.
        let plain = strip_ansi(&text);
        assert!(plain.contains("1  claude"), "{plain}");
        assert!(plain.contains("2  codex"), "{plain}");
        assert!(plain.find("1  claude") < plain.find("2  codex"));
        // "just a shell" is always an out, so the pane is never a dead end.
        assert!(plain.contains("just a shell"));
        // Enter picks the first entry, so the list must say which that is.
        assert!(plain.contains("1  claude"));
        let dflt = plain.find("(default)").expect("default marked");
        assert!(dflt > plain.find("1  claude").unwrap() && dflt < plain.find("2  codex").unwrap());
        assert_eq!(choices.len(), 2);
        // A resolved config renders nothing at all.
        let (text, choices) = render(&Plan::Configured("jcode".into()));
        assert!(text.is_empty());
        assert!(choices.is_empty());
    }

    #[test]
    fn the_none_installed_screen_lists_what_was_searched_for() {
        let (text, choices) = render(&Plan::NoneInstalled { want: None });
        assert!(text.contains("No agent harness found"));
        assert!(text.contains("jcode"));
        assert!(text.contains("aider"));
        assert!(choices.is_empty());
    }

    #[test]
    fn saving_replaces_an_existing_key_and_preserves_comments_and_order() {
        let before = "# my config\nstartup_panes = 1\ndefault_agent = \"jcode\"\nmouse = true\n";
        let after = set_default_agent_text(before, "claude");
        assert_eq!(
            after,
            "# my config\nstartup_panes = 1\ndefault_agent = \"claude\"\nmouse = true\n"
        );
    }

    #[test]
    fn saving_appends_when_absent_and_stays_above_any_table_header() {
        let before = "startup_panes = 1\n\n[theme]\npreset = \"nord\"\n";
        let after = set_default_agent_text(before, "claude");
        // Must land before `[theme]`, or it would become theme.default_agent.
        let agent_at = after.find("default_agent").unwrap();
        let table_at = after.find("[theme]").unwrap();
        assert!(agent_at < table_at, "{after}");
        assert!(after.contains("preset = \"nord\""));
        // And it must still parse as the value we asked for.
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn saving_into_a_flat_file_appends_at_the_end() {
        let after = set_default_agent_text("startup_panes = 1\n", "codex");
        assert_eq!(after, "startup_panes = 1\n\ndefault_agent = \"codex\"\n");
    }

    #[test]
    fn a_default_agent_inside_a_table_is_never_clobbered() {
        let before = "[theme]\ndefault_agent = \"decoy\"\n";
        let after = set_default_agent_text(before, "claude");
        assert!(after.contains("default_agent = \"decoy\""));
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["theme"]["default_agent"].as_str(), Some("decoy"));
    }

    #[test]
    fn saved_values_are_quoted_so_odd_commands_round_trip() {
        let after = set_default_agent_text("", "my agent");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("my agent"));
    }

    #[test]
    fn save_creates_the_file_and_its_parent_directory() {
        let dir = std::env::temp_dir().join(format!("gwae-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/gwae.toml");
        save_default_agent(&path, "jcode").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("jcode"));
        // Saving again over the fresh file replaces rather than duplicates.
        save_default_agent(&path, "claude").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("default_agent").count(), 1);
        assert!(text.contains("claude"));
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn a_narrow_pane_keeps_the_choices_next_to_the_prompt() {
        // A quarter-width pane is ~24 columns. The wide form's paths and
        // footer scroll the list off the top, leaving a prompt with no
        // visible options, which is the one thing the picker must never do.
        let plan = Plan::Choose(vec![found("claude"), found("aider")]);
        let (wide, _) = render_at(&plan, 100);
        let (narrow, _) = render_at(&plan, 24);
        assert!(
            narrow.lines().count() < wide.lines().count(),
            "narrow must be shorter:\n{narrow}"
        );
        let plain = strip_ansi(&narrow);
        // Everything needed to choose is still there.
        assert!(plain.contains("1 claude"), "{plain}");
        assert!(plain.contains("2 aider"), "{plain}");
        assert!(plain.contains("(default)"), "{plain}");
        assert!(plain.contains("s shell"), "{plain}");
        assert!(plain.contains("type a command"), "{plain}");
        // The long path and footer, which are what overflowed, are gone.
        assert!(!plain.contains("/usr/bin/claude"), "{plain}");
        assert!(!plain.contains("goes straight there"), "{plain}");
        // And it fits: every line inside the pane width.
        for l in plain.lines() {
            assert!(l.chars().count() <= 24, "line too wide: {l:?}");
        }
    }

    #[test]
    fn the_narrow_none_installed_screen_still_says_what_to_do() {
        let (narrow, _) = render_at(&Plan::NoneInstalled { want: None }, 24);
        let plain = strip_ansi(&narrow);
        assert!(plain.contains("No agent harness found"), "{plain}");
        assert!(plain.contains("Type its command"), "{plain}");
        assert!(plain.contains("Enter for a shell"), "{plain}");
    }

    #[test]
    fn parse_choice_accepts_indexes_typed_commands_and_shell() {
        assert_eq!(parse_choice("2", 3), Ok(Choice::Listed(1)));
        assert_eq!(parse_choice("  1  ", 3), Ok(Choice::Listed(0)));
        assert_eq!(parse_choice("", 3), Ok(Choice::Listed(0)), "Enter = first");
        assert_eq!(parse_choice("s", 3), Ok(Choice::Shell));
        assert_eq!(parse_choice("SHELL", 3), Ok(Choice::Shell));
        // With nothing listed, Enter can only mean a shell.
        assert_eq!(parse_choice("", 0), Ok(Choice::Shell));
        // The escape hatch: any real command, with or without args.
        assert_eq!(parse_choice("sh", 3), Ok(Choice::Typed("sh".into())));
        assert_eq!(
            parse_choice("sh --resume", 3),
            Ok(Choice::Typed("sh --resume".into()))
        );
    }

    #[test]
    fn parse_choice_explains_rejections_instead_of_silently_failing() {
        // An out-of-range number is a typo, not a command.
        let e = parse_choice("9", 3).unwrap_err();
        assert!(e.contains("no 9"), "{e}");
        assert!(e.contains("1-3"), "{e}");
        // A command that does not exist says so by name, so the user can see
        // the typo rather than wondering why nothing happened.
        let e = parse_choice("hermes-not-installed", 3).unwrap_err();
        assert!(e.contains("hermes-not-installed"), "{e}");
        assert!(e.contains("not on your PATH"), "{e}");
        // Args are stripped when naming the missing executable.
        let e = parse_choice("hermes-nope --resume", 3).unwrap_err();
        assert!(e.contains("`hermes-nope`"), "{e}");
    }

    #[test]
    fn fallback_shell_is_never_empty() {
        assert!(!fallback_shell().is_empty());
    }
}

#[cfg(test)]
mod save_edge_cases {
    use super::*;

    /// Every rewrite must leave a file that still parses and holds the new
    /// value: the config is the user's, and a corrupted one is silently
    /// ignored at startup, which would look like gwae losing settings.
    fn check(before: &str, agent: &str) -> toml::Value {
        let after = set_default_agent_text(before, agent);
        assert!(
            after.ends_with('\n'),
            "must stay newline-terminated: {after:?}"
        );
        let v: toml::Value =
            toml::from_str(&after).unwrap_or_else(|e| panic!("broke the file: {e}\n{after:?}"));
        assert_eq!(v["default_agent"].as_str(), Some(agent), "{after:?}");
        v
    }

    #[test]
    fn a_file_without_a_trailing_newline_is_still_valid_after() {
        check("startup_panes = 1", "claude");
    }

    #[test]
    fn crlf_line_endings_survive() {
        let v = check(
            "startup_panes = 1\r\ndefault_agent = \"jcode\"\r\nmouse = true\r\n",
            "claude",
        );
        assert_eq!(v["mouse"].as_bool(), Some(true));
        assert_eq!(v["startup_panes"].as_integer(), Some(1));
    }

    #[test]
    fn an_indented_key_is_still_the_key_we_replace() {
        let after = set_default_agent_text("  default_agent = \"jcode\"\n", "claude");
        assert_eq!(after.matches("default_agent").count(), 1, "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
    }

    #[test]
    fn a_comment_only_file_gains_the_key_and_keeps_its_comments() {
        let after = set_default_agent_text("# my notes\n# more notes\n", "claude");
        assert!(after.contains("# my notes"), "{after:?}");
        assert!(after.contains("# more notes"), "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
    }

    #[test]
    fn a_file_that_is_only_a_table_gets_the_key_above_it() {
        let v = check("[theme]\npreset = \"nord\"\n", "claude");
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn an_empty_string_is_handled_without_producing_a_stray_blank_line() {
        let after = set_default_agent_text("", "claude");
        assert_eq!(after, "default_agent = \"claude\"\n");
    }

    #[test]
    fn repeated_saves_never_accumulate_duplicate_keys() {
        // The picker can run many times; each must replace, not append.
        let mut text = "startup_panes = 1\n".to_string();
        for a in ["claude", "codex", "aider", "jcode"] {
            text = set_default_agent_text(&text, a);
        }
        assert_eq!(text.matches("default_agent").count(), 1, "{text:?}");
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("jcode"));
    }

    #[test]
    fn a_key_whose_name_merely_starts_the_same_is_left_alone() {
        let after = set_default_agent_text("default_agent_args = \"x\"\n", "claude");
        assert!(after.contains("default_agent_args = \"x\""), "{after:?}");
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["default_agent"].as_str(), Some("claude"));
        assert_eq!(v["default_agent_args"].as_str(), Some("x"));
    }
}
