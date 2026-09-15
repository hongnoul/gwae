//! Spawn path primitives: expansion, resolution, and validation.

use std::path::PathBuf;

/// Markers that identify a directory as a *project*, whatever it is called
/// and wherever it lives.
///
/// Discovery keys off these rather than off directory names. An earlier
/// version shipped a list of likely parents (`~/git`, `~/code`, `~/src`, ...)
/// and found nothing on a machine that used any other convention, which is
/// most of them: people keep work in `~/Documents/clients`, `~/w`, `/srv`,
/// or a company-mandated tree. A `.git` directory, by contrast, means the
/// same thing everywhere, so the feature works on a machine gwae has never
/// seen without the user configuring anything.
pub(super) const PROJECT_MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    // Not VCS, but unambiguous "this is a workspace you would open an agent
    // in", and they cover checkouts nested inside a monorepo.
    ".gwae",
    ".projectile",
];

/// Directories never worth descending into while looking for projects.
///
/// Two kinds: OS/library trees that hold tens of thousands of files and no
/// projects (`Library`, `Applications`), and dependency/build output that is
/// *inside* projects and would otherwise multiply every hit by its vendored
/// copies (`node_modules`, `target`, `vendor`). Matched by name at any depth,
/// because that is where they appear.
pub(super) const SKIP_DIRS: &[&str] = &[
    "Library",
    "Applications",
    "Music",
    "Pictures",
    "Movies",
    "Photos",
    "System",
    "Volumes",
    "node_modules",
    "target",
    "vendor",
    "venv",
    "__pycache__",
    "build",
    "dist",
    "Trash",
];

/// How deep below a search root the scan descends. Four levels reaches
/// `~/work/client/team/repo` while keeping the walk to a few dozen
/// `readdir`s on a normal machine (measured: ~35 repos in about 2ms on the
/// author's `$HOME`). The project-suggestion scan stops at a project, while
/// directory-name search also visits its children within the same limits.
pub(super) const MAX_DEPTH: usize = 4;

/// Hard ceiling on directories examined, so a pathological tree (a network
/// mount, a home full of generated data) cannot make `⌥+d` hang. Reaching it
/// yields fewer candidates, never a stall.
pub(super) const MAX_SCAN: usize = 4000;

/// Expand `~` and `$VAR` / `${VAR}` in a configured path.
///
/// Config files are written by hand, and a hand-written path is a `~` path.
/// A shell would expand it; a `CommandBuilder::cwd` would not, and would fail
/// with a baffling "no such directory: ~/git" instead.
pub fn expand(raw: &str) -> PathBuf {
    let s = raw.trim();
    let home = std::env::var("HOME").unwrap_or_default();
    let s = if s == "~" {
        home.clone()
    } else if let Some(rest) = s.strip_prefix("~/") {
        if home.is_empty() {
            rest.to_string()
        } else {
            format!("{home}/{rest}")
        }
    } else {
        s.to_string()
    };
    PathBuf::from(expand_vars(&s))
}

/// Substitute `$VAR` and `${VAR}` from the environment. Unset variables
/// expand to the empty string, matching a shell, so a typo yields a path that
/// visibly does not exist rather than a literal `$FOO` directory.
fn expand_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            let ok = if braced {
                n != '}'
            } else {
                n.is_ascii_alphanumeric() || n == '_'
            };
            if !ok {
                break;
            }
            name.push(n);
            chars.next();
        }
        if braced {
            // Consume the closing brace if it is there; an unterminated
            // `${FOO` is a typo, and dropping it is kinder than emitting it.
            let _ = chars.next_if_eq(&'}');
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

/// The directory a pane should start in. Never `None`: see [`inherited`].
///
/// `cli` (from `--dir`) wins over `cfg` (from `agent_dir`), because a flag is
/// this-run-only intent and the file is a standing preference. A configured
/// directory that does not exist resolves to `None`: a pane in the inherited
/// directory is a visible, recoverable wrong; a pane that fails to spawn at
/// all reads as a gwae bug.
/// Resolve where new panes start: `cli > agent_dir > inherited`.
///
/// Empty strings are ignored. A configured directory that does not exist is
/// skipped with a warning rather than breaking pane spawn.
pub fn resolve(cli: Option<&str>, cfg: &str) -> Option<PathBuf> {
    for raw in [cli.unwrap_or(""), cfg] {
        if raw.trim().is_empty() {
            continue;
        }
        let p = expand(raw);
        if p.is_dir() {
            return Some(p);
        }
        tracing::warn!("spawn dir {p:?} is not a directory; inheriting cwd");
    }
    inherited()
}

/// gwae's own working directory, used when nothing is configured.
///
/// This has to be passed *explicitly* rather than left unset: `portable-pty`
/// does not inherit the parent's cwd when a `CommandBuilder` has none, it
/// falls back to the user's home directory. So "no `agent_dir`" used to mean
/// "every pane opens in `$HOME`", even when gwae itself was launched from a
/// repo — a silent surprise, and half of the reason this feature exists.
pub fn inherited() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

/// Why a configured spawn directory was rejected, for `doctor` and for the
/// transient TUI note. Split from [`resolve`] so both can explain themselves
/// without re-deriving the decision.
pub fn check(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("unset".into());
    }
    let p = expand(raw);
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!("{} does not exist", p.display()))
    }
}

/// True when `dir` is itself a project (holds one of [`PROJECT_MARKERS`]).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_paths_expand_to_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand("~"), PathBuf::from(&home));
        assert_eq!(expand("~/git"), PathBuf::from(format!("{home}/git")));
        // A bare `~name` is another user's home, which we do not resolve; it
        // must be left alone rather than mangled into `$HOME.name`.
        assert_eq!(expand("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn env_vars_expand_like_a_shell() {
        std::env::set_var("GWAE_TEST_DIR", "/tmp/zzz");
        assert_eq!(expand("$GWAE_TEST_DIR/a"), PathBuf::from("/tmp/zzz/a"));
        assert_eq!(expand("${GWAE_TEST_DIR}b"), PathBuf::from("/tmp/zzzb"));
        assert_eq!(expand("$GWAE_UNSET_XYZ/a"), PathBuf::from("/a"));
        assert_eq!(expand("100$"), PathBuf::from("100$"));
    }

    #[test]
    fn cli_beats_config_and_missing_dirs_are_ignored() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(resolve(Some("~"), ""), Some(PathBuf::from(&home)));
        // A missing --dir falls through to the config rather than aborting.
        assert_eq!(
            resolve(Some("/no/such/dir/xyz"), "~"),
            Some(PathBuf::from(&home))
        );
        // Nothing usable falls back to gwae's own cwd, explicitly, because
        // leaving it unset would send the pane to $HOME instead.
        let cwd = std::env::current_dir().ok();
        assert_eq!(resolve(Some("/no/such/dir/xyz"), "/also/not/here"), cwd);
        assert_eq!(resolve(None, ""), cwd);
    }

    #[test]
    fn check_explains_itself() {
        assert!(check("").is_err());
        assert!(check("~").is_ok());
        let e = check("/definitely/not/here").unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
    }

    #[test]
    fn cli_beats_config_and_missing_config_falls_back() {
        let tmp = std::env::temp_dir();
        let _ = std::fs::create_dir_all(tmp.join("gwae-harness-test-a"));
        let _ = std::fs::create_dir_all(tmp.join("gwae-harness-test-b"));
        let a = tmp.join("gwae-harness-test-a");
        let b = tmp.join("gwae-harness-test-b");
        // config dir resolves
        assert_eq!(resolve(None, a.to_str().unwrap()), Some(a.clone()));
        // cli > config
        assert_eq!(
            resolve(Some(b.to_str().unwrap()), a.to_str().unwrap()),
            Some(b.clone())
        );
        // missing config falls through to cwd
        assert_eq!(resolve(None, "/no/such/dir"), inherited());
    }
}
