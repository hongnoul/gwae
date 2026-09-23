//! Leaving: `gwae uninstall` removes every trace of gwae.
//!
//! One command, clean machine: every `gwae` binary on PATH plus the known
//! install dirs, our config/state dirs, and (with confirm) the PATH lines
//! the curl installer added. Brew/cargo-owned installs are driven through
//! their owner first (`brew uninstall`, `cargo uninstall`) so the package
//! manager never ends up owning a ghost, then we sweep whatever is left.

use super::source::state_dir;
use std::path::{Path, PathBuf};

/// Shell profiles the curl installer may have touched.
const PROFILE_FILES: &[&str] = &[
    ".zshrc",
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".config/fish/conf.d/gwae.fish",
];

/// Marker left on lines the installer owns.
const OWN_MARK: &str = "added by gwae installer";

/// Every installed `gwae` binary: each `gwae` on PATH (resolved through
/// symlinks, so the brew Cellar target is found too) plus the known install
/// dirs, so a shadowed copy is never left behind.
fn all_binaries(current_exe: &Path) -> Vec<PathBuf> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    };
    push(current_exe.to_path_buf());
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if dir.is_empty() {
                continue;
            }
            let cand = PathBuf::from(dir).join("gwae");
            if cand.is_file() || cand.is_symlink() {
                // Resolve the symlink (brew Cellar) but keep the link path
                // too: removing the link is what takes it off PATH.
                push(cand.clone());
                if let Ok(real) = std::fs::canonicalize(&cand) {
                    if real.file_name().map(|n| n == "gwae").unwrap_or(false) {
                        push(real);
                    }
                }
            }
        }
    }
    // Known install dirs, even when off PATH right now.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for rel in [".local/bin/gwae", ".cargo/bin/gwae", ".bun/bin/gwae"] {
            let cand = home.join(rel);
            if cand.is_file() || cand.is_symlink() {
                push(cand);
            }
        }
    }
    out.sort();
    out
}

/// The full removal plan: binaries, our dirs, and profile files with our
/// lines. Pure, so tests assert on it without touching the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sweep {
    /// Owner commands to run first (brew/cargo), so the package manager
    /// releases what it owns before we sweep the leftovers.
    pub owner_cmds: Vec<String>,
    /// Paths we remove ourselves.
    pub paths: Vec<PathBuf>,
    /// Profile files holding lines we added.
    pub profiles: Vec<PathBuf>,
}

/// Decide what to remove. `current_exe` is the running binary, `path_bins`
/// the `gwae` files found on PATH, `known` the off-PATH install dirs that
/// exist, `brew_owned`/`cargo_owned` whether those managers claim gwae.
pub fn plan_sweep(
    current_exe: PathBuf,
    path_bins: Vec<PathBuf>,
    known: Vec<PathBuf>,
    brew_owned: bool,
    cargo_owned: bool,
) -> Sweep {
    use std::collections::HashSet;
    let mut owner_cmds = Vec::new();
    if brew_owned {
        owner_cmds.push("brew uninstall hongnoul/tap/gwae".to_string());
    }
    if cargo_owned {
        owner_cmds.push("cargo uninstall gwae".to_string());
    }
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for p in [current_exe].into_iter().chain(path_bins).chain(known) {
        if seen.insert(p.clone()) {
            paths.push(p);
        }
    }
    paths.sort();
    if let Some(dir) = config_dir() {
        paths.push(dir);
    }
    if let Some(dir) = state_dir() {
        paths.push(dir);
    }
    Sweep {
        owner_cmds,
        paths,
        profiles: profiles_with_our_lines(),
    }
}

/// Profile files on disk that hold lines the installer added.
fn profiles_with_our_lines() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = owned_path_lines().iter().map(|(f, _)| f.clone()).collect();
    out.sort();
    out.dedup();
    out
}

/// Our config dir, same resolution as `Config::default_path`'s parent.
fn config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("gwae"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/gwae"))
}

/// PATH lines we own, as `(file, line)` pairs still present on disk.
fn owned_path_lines() -> Vec<(PathBuf, String)> {
    let home = match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for rel in PROFILE_FILES {
        let p = home.join(rel);
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in text.lines() {
            if line.contains(OWN_MARK) {
                out.push((p.clone(), line.to_string()));
            }
        }
    }
    out
}

/// `gwae uninstall [--yes]`: drive owner uninstalls, then sweep every
/// leftover binary, our dirs, and our PATH lines. Returns exit code.
pub fn run_uninstall(_configured: Option<super::source::Source>, yes: bool) -> i32 {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gwae"));
    let bins = all_binaries(&exe);
    let brew_owned = std::process::Command::new("brew")
        .args(["list", "gwae"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let cargo_owned = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cargo/.crates.toml"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| super::source::cargo_origin(&t).is_some())
        .unwrap_or(false);
    // Off-PATH known dirs are already inside all_binaries; plan_sweep takes
    // an empty `known` here and stays pure for tests.
    let sweep = plan_sweep(exe, bins, Vec::new(), brew_owned, cargo_owned);

    if !sweep.owner_cmds.is_empty() {
        println!("handing owned installs to their manager first:");
        for cmd in &sweep.owner_cmds {
            println!("  running: {cmd}");
            let mut parts = cmd.split_whitespace();
            let prog = parts.next().unwrap_or("");
            let args: Vec<&str> = parts.collect();
            match std::process::Command::new(prog).args(&args).status() {
                Ok(s) if s.success() => println!("  ok: {cmd}"),
                Ok(s) => eprintln!("  warning: {cmd} exited {s}; continuing sweep"),
                Err(e) => eprintln!("  warning: could not run {cmd}: {e}; continuing sweep"),
            }
        }
        println!();
    }

    println!("this will remove:");
    for p in &sweep.paths {
        println!("  {}", p.display());
    }
    if !sweep.profiles.is_empty() {
        println!("\nPATH lines the installer added (these files will be cleaned):");
        for f in &sweep.profiles {
            println!("  {}", f.display());
        }
    }
    if !yes && !confirm() {
        println!("aborted; nothing removed.");
        return 1;
    }
    let mut failed = false;
    for p in &sweep.paths {
        if let Err(e) = remove_path(p) {
            eprintln!("could not remove {}: {e}", p.display());
            failed = true;
        } else {
            println!("removed {}", p.display());
        }
    }
    for f in &sweep.profiles {
        if let Err(e) = strip_our_lines(f) {
            eprintln!("could not clean {}: {e}", f.display());
            failed = true;
        } else {
            println!("cleaned {}", f.display());
        }
    }
    // The fish conf.d file is ours alone (never config.fish itself): an
    // emptied one is removed rather than left as a husk.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let fish = home.join(".config/fish/conf.d/gwae.fish");
        if fish.is_file() {
            let empty = std::fs::read_to_string(&fish)
                .map(|t| {
                    t.lines()
                        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
                })
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_file(&fish);
            }
        }
    }
    if failed {
        1
    } else {
        0
    }
}

/// Remove our marked lines from a profile file, keeping everything else.
/// Both the snippet and its `# added by gwae installer` comment contain the
/// mark, so one filter drops both.
fn strip_our_lines(path: &Path) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = text.lines().filter(|l| !l.contains(OWN_MARK)).collect();
    let mut s = kept.join("\n");
    if !text.is_empty() && !s.is_empty() {
        s.push('\n');
    }
    std::fs::write(path, s)
}

fn remove_path(p: &Path) -> std::io::Result<()> {
    if !p.exists() && !p.is_symlink() {
        return Ok(());
    }
    if p.is_dir() && !p.is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// y/N on stdin. False without a tty, so a piped uninstall prints the plan
/// and stops rather than deleting on a guess.
fn confirm() -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("remove these? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(_) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn sweep_drives_owners_first_then_lists_everything() {
        let exe = PathBuf::from("/Users/x/.bun/bin/gwae");
        let bins = vec![
            PathBuf::from("/opt/homebrew/bin/gwae"),
            PathBuf::from("/Users/x/.local/bin/gwae"),
        ];
        let s = plan_sweep(exe.clone(), bins, Vec::new(), true, true);
        assert!(
            s.owner_cmds.iter().any(|c| c.contains("brew uninstall")),
            "{s:?}"
        );
        assert!(
            s.owner_cmds.iter().any(|c| c.contains("cargo uninstall")),
            "{s:?}"
        );
        // Every binary plus our two dirs; dirs last.
        assert!(s.paths.contains(&exe), "{s:?}");
        let n = s.paths.len();
        assert!(s.paths[n - 2].ends_with(".config/gwae"), "{s:?}");
        assert!(s.paths[n - 1].ends_with("state/gwae"), "{s:?}");
        let bins = &s.paths[..n - 2];
        let mut sorted = bins.to_vec();
        sorted.sort();
        assert_eq!(bins, sorted, "binaries sorted: {s:?}");
    }

    #[test]
    fn sweep_without_owners_removes_directly() {
        let exe = PathBuf::from("/Users/x/.bun/bin/gwae");
        let s = plan_sweep(exe.clone(), Vec::new(), Vec::new(), false, false);
        assert!(s.owner_cmds.is_empty(), "{s:?}");
        assert!(s.paths.contains(&exe), "{s:?}");
    }

    #[test]
    fn strip_keeps_hand_edits_and_drops_our_lines() {
        let dir = std::env::temp_dir().join(format!(
            "gwae-strip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("zshrc");
        std::fs::write(
            &f,
            "export MINE=1\n# added by gwae installer\ncase \":$PATH:\" in blah # added by gwae installer\n",
        )
        .unwrap();
        strip_our_lines(&f).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("MINE=1"), "{after:?}");
        assert!(!after.contains(OWN_MARK), "{after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_path_tolerates_absence_and_removes_files_and_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "gwae-uninstall-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        // Absent is fine.
        assert!(remove_path(&dir.join("nope")).is_ok());
        // File and dir both go.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/f"), "x").unwrap();
        assert!(remove_path(&dir.join("sub/f")).is_ok());
        assert!(!dir.join("sub/f").exists());
        assert!(remove_path(&dir.join("sub")).is_ok());
        assert!(!dir.join("sub").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
