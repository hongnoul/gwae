//! Leaving: `gwae uninstall` removes what the installer put down.
//!
//! The mirror of `update`: gwae uninstalls the way it was installed, or
//! explains how. Brew installs defer to brew (it owns the files); the curl
//! installer route removes the binary plus our own config/state dirs; cargo,
//! source, nix, and system routes print the exact command since we must not
//! fight the owner. Machine-written bookkeeping (receipt, update cache,
//! harness memory) is safe to delete; the user's config dir goes too, but
//! only with `--yes` or a tty confirm, since it holds hand edits.
//!
//! PATH lines the installer added to shell profiles are never removed
//! silently: they are listed for the user to delete, since the file belongs
//! to them.

use super::plan::plan;
use super::source::{detect, probe, state_dir, Source};
use std::path::PathBuf;

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

/// What uninstall decided to do. Pure, so tests describe a machine instead
/// of needing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uninstall {
    /// Hand off to the owner: print this command, remove nothing.
    Owner { cmd: String },
    /// Remove these paths ourselves (binary + our own dirs).
    Ours { paths: Vec<PathBuf> },
}

/// Decide the uninstall route from the source. Pure.
pub fn plan_uninstall(source: Source, exe: &PathBuf) -> Uninstall {
    match source {
        Source::Homebrew => Uninstall::Owner {
            cmd: "brew uninstall hongnoul/tap/gwae".to_string(),
        },
        Source::Script => {
            let mut paths = vec![exe.clone()];
            if let Some(dir) = config_dir() {
                paths.push(dir);
            }
            if let Some(dir) = state_dir() {
                paths.push(dir);
            }
            Uninstall::Ours { paths }
        }
        Source::Cargo => Uninstall::Owner {
            cmd: "cargo uninstall gwae".to_string(),
        },
        Source::CargoGit => Uninstall::Owner {
            cmd: "cargo uninstall gwae".to_string(),
        },
        Source::Source_ => Uninstall::Owner {
            cmd: "remove the installed binary (you built it from a checkout: see `make install` for where it went)".to_string(),
        },
        Source::Nix => Uninstall::Owner {
            cmd: "nix profile remove gwae (or drop the flake input)".to_string(),
        },
        Source::System => Uninstall::Owner {
            cmd: "ask your package manager (e.g. `paru -R gwae-bin`)".to_string(),
        },
        Source::Unknown => {
            // Unknown means "no owner claimed this path" (dev `make install`
            // into ~/.bun/bin, a hand copy, a stale receipt elsewhere). The
            // user asked to uninstall this exact binary, so remove it plus
            // our own dirs rather than punting.
            let mut paths = vec![exe.clone()];
            if let Some(dir) = config_dir() {
                paths.push(dir);
            }
            if let Some(dir) = state_dir() {
                paths.push(dir);
            }
            Uninstall::Ours { paths }
        }
    }
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

/// `gwae uninstall`: print the plan, confirm, and remove what we own.
/// Returns the process exit code.
pub fn run_uninstall(configured: Option<Source>, yes: bool) -> i32 {
    let facts = probe(configured);
    let source = detect(&facts);
    let _ = plan(source, &facts.exe);
    match plan_uninstall(source, &facts.exe) {
        Uninstall::Owner { cmd } => {
            println!("gwae came from {} — it owns these files, so run:", source.as_str());
            println!("  {cmd}");
            println!("\nThen, if you want a clean slate:");
            if let Some(d) = config_dir() {
                println!("  rm -rf {}", d.display());
            }
            if let Some(d) = state_dir() {
                println!("  rm -rf {}", d.display());
            }
            0
        }
        Uninstall::Ours { paths } => {
            println!("this will remove:");
            for p in &paths {
                println!("  {}", p.display());
            }
            let owned = owned_path_lines();
            if !owned.is_empty() {
                println!("\nPATH lines the installer added (left for you to delete):");
                for (file, line) in &owned {
                    println!("  {}: {line}", file.display());
                }
            }
            if !yes && !confirm() {
                println!("aborted; nothing removed.");
                return 1;
            }
            let mut failed = false;
            for p in &paths {
                if let Err(e) = remove_path(p) {
                    eprintln!("could not remove {}: {e}", p.display());
                    failed = true;
                } else {
                    println!("removed {}", p.display());
                }
            }
            if failed { 1 } else { 0 }
        }
    }
}

fn remove_path(p: &PathBuf) -> std::io::Result<()> {
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
    use std::path::Path;

    #[test]
    fn brew_cargo_and_managed_routes_defer_to_their_owner() {
        let exe = PathBuf::from("/opt/homebrew/bin/gwae");
        let Uninstall::Owner { cmd } = plan_uninstall(Source::Homebrew, &exe) else {
            panic!("brew must defer");
        };
        assert!(cmd.contains("brew uninstall"), "{cmd}");
        let exe = PathBuf::from("/Users/x/.cargo/bin/gwae");
        let Uninstall::Owner { cmd } = plan_uninstall(Source::Cargo, &exe) else {
            panic!("cargo must defer");
        };
        assert!(cmd.contains("cargo uninstall"), "{cmd}");
        let Uninstall::Owner { cmd } =
            plan_uninstall(Source::System, &PathBuf::from("/usr/bin/gwae"))
        else {
            panic!("system must defer");
        };
        assert!(!cmd.is_empty(), "managed routes must still say what to do");
    }

    #[test]
    fn unknown_route_removes_this_binary_not_a_guess() {
        // `make install` into ~/.bun/bin, a hand copy, a stale receipt
        // elsewhere: no owner claims it, so the binary the user invoked is
        // the thing to remove.
        let exe = PathBuf::from("/Users/x/.bun/bin/gwae");
        let Uninstall::Ours { paths } = plan_uninstall(Source::Unknown, &exe) else {
            panic!("unknown must remove itself, not punt");
        };
        assert_eq!(paths.first().unwrap(), &exe, "{paths:?}");
    }

    #[test]
    fn script_route_removes_the_binary_and_our_dirs() {
        let exe = PathBuf::from("/Users/x/.local/bin/gwae");
        let Uninstall::Ours { paths } = plan_uninstall(Source::Script, &exe) else {
            panic!("script route must remove itself");
        };
        assert!(paths.contains(&exe), "{paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("gwae") && p != &exe), "{paths:?}");
        assert_eq!(paths.first().unwrap(), &exe, "binary first: {paths:?}");
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
        let _ = Path::new("").to_path_buf();
    }
}
