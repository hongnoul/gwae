//! Where a new pane starts: the spawn directory.
//!
//! Every pane used to inherit gwae's own working directory, which is
//! whatever the terminal opened at (`$HOME`, usually). That made `⌥+;` a
//! two-step verb in practice: spawn the harness, then `cd` it to the repo you
//! actually meant. This module is the one place that answers "which
//! directory does a pane start in", for three inputs at three lifetimes:
//!
//! * `agent_dir` in the config file (persistent),
//! * `gwae run --dir` (this session),
//! * the `⌥+d` picker (this session, and optionally written back).
//!
//! The value is a *spawn-time* input only. Once a pane exists, its cwd
//! belongs to the child process: gwae cannot follow a `cd` without shell
//! integration it deliberately does not require, so there is nothing here to
//! keep in sync.

use std::path::{Path, PathBuf};

/// Directory roots scanned for candidate project directories when the config
/// does not name its own. These are where developers actually keep checkouts;
/// scanning one level down turns "~/git" into the ~30 repos inside it without
/// the user listing a single one.
pub const DEFAULT_ROOTS: &[&str] = &[
    "~/git",
    "~/code",
    "~/projects",
    "~/src",
    "~/dev",
    "~/Developer",
];

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

/// A directory offered by the `⌥+d` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Absolute path, used verbatim as the spawn directory.
    pub path: PathBuf,
    /// Short label (`~/git/gwae`), so the picker fits in a narrow panel.
    pub label: String,
    /// Why it is on the list (`current`, `config`, `~/git`), shown dimmed.
    pub origin: &'static str,
}

/// Shorten an absolute path for display by re-introducing `~`.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && s == home {
        return "~".into();
    }
    match (home.is_empty(), s.strip_prefix(&format!("{home}/"))) {
        (false, Some(rest)) => format!("~/{rest}"),
        _ => s,
    }
}

/// Everything the picker can offer, best-first and de-duplicated.
///
/// Order is deliberate: what you are using now, what you configured, the
/// pins, then the scanned roots alphabetically, then `$HOME` as the always-
/// available escape hatch. `current` is first so `⌥+d ↵` is a no-op rather
/// than a surprise.
pub fn candidates(
    current: Option<&Path>,
    cfg_dir: &str,
    pins: &[String],
    roots: &[String],
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let push = |out: &mut Vec<Candidate>,
                seen: &mut std::collections::HashSet<PathBuf>,
                p: PathBuf,
                origin: &'static str| {
        if !p.is_dir() {
            return;
        }
        let p = p.canonicalize().unwrap_or(p);
        if seen.insert(p.clone()) {
            out.push(Candidate {
                label: tilde(&p),
                path: p,
                origin,
            });
        }
    };

    if let Some(c) = current {
        push(&mut out, &mut seen, c.to_path_buf(), "current");
    }
    if let Ok(cwd) = std::env::current_dir() {
        push(&mut out, &mut seen, cwd, "cwd");
    }
    if !cfg_dir.trim().is_empty() {
        push(&mut out, &mut seen, expand(cfg_dir), "config");
    }
    for pin in pins {
        push(&mut out, &mut seen, expand(pin), "pinned");
    }
    let roots: Vec<String> = if roots.is_empty() {
        DEFAULT_ROOTS.iter().map(|s| s.to_string()).collect()
    } else {
        roots.to_vec()
    };
    for root in &roots {
        let rp = expand(root);
        let Ok(entries) = std::fs::read_dir(&rp) else {
            continue;
        };
        let mut kids: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                // Hidden directories under a project root are caches
                // (`.git` clones aside), never something you open an agent in.
                p.file_name()
                    .map(|n| !n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            })
            .collect();
        kids.sort();
        // The root itself is worth offering too, for a quick look around.
        push(&mut out, &mut seen, rp.clone(), "root");
        for k in kids {
            // Leaked as a 'static label via the same trick the rest of the
            // picker uses: the origin is one of a small fixed set.
            push(&mut out, &mut seen, k, "project");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        push(&mut out, &mut seen, PathBuf::from(home), "home");
    }
    out
}

/// Filter candidates by a typed query: a subsequence match on the label,
/// case-insensitive, like every fuzzy finder. Exact substring matches sort
/// first so typing a full repo name lands on it rather than on a longer path
/// that merely contains the letters.
pub fn filter(cands: &[Candidate], query: &str) -> Vec<Candidate> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return cands.to_vec();
    }
    let mut exact: Vec<Candidate> = Vec::new();
    let mut fuzzy: Vec<Candidate> = Vec::new();
    for c in cands {
        let l = c.label.to_ascii_lowercase();
        if l.contains(&q) {
            exact.push(c.clone());
        } else if subsequence(&l, &q) {
            fuzzy.push(c.clone());
        }
    }
    exact.extend(fuzzy);
    exact
}

/// Whether every char of `needle` appears in `hay`, in order.
fn subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

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
    fn candidates_lead_with_the_current_directory() {
        let tmp = std::env::temp_dir();
        let c = candidates(Some(&tmp), "", &[], &["/no/such/root".into()]);
        assert_eq!(c[0].origin, "current");
        assert_eq!(c[0].path, tmp.canonicalize().unwrap_or(tmp));
        // No duplicates, whatever the sources overlap on.
        let mut seen = std::collections::HashSet::new();
        for x in &c {
            assert!(seen.insert(x.path.clone()), "duplicate {:?}", x.path);
        }
    }

    #[test]
    fn scanned_roots_contribute_their_children() {
        let root = std::env::temp_dir().join("gwae-cand-test");
        let _ = std::fs::create_dir_all(root.join("alpha"));
        let _ = std::fs::create_dir_all(root.join(".hidden"));
        let c = candidates(None, "", &[], &[root.to_string_lossy().into()]);
        let labels: Vec<&str> = c.iter().map(|x| x.label.as_str()).collect();
        assert!(
            labels.iter().any(|l| l.ends_with("gwae-cand-test/alpha")),
            "{labels:?}"
        );
        assert!(!labels.iter().any(|l| l.ends_with(".hidden")), "{labels:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn filter_prefers_substring_over_subsequence() {
        let mk = |label: &str| Candidate {
            path: PathBuf::from(label),
            label: label.into(),
            origin: "project",
        };
        let cands = vec![mk("~/git/generic-workspace-a-e"), mk("~/git/gwae")];
        let got = filter(&cands, "gwae");
        assert_eq!(got[0].label, "~/git/gwae");
        assert_eq!(got.len(), 2);
        assert!(filter(&cands, "zzzz").is_empty());
        assert_eq!(filter(&cands, "  ").len(), 2);
    }
}
