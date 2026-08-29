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
const PROJECT_MARKERS: &[&str] = &[
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
const SKIP_DIRS: &[&str] = &[
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
/// author's `$HOME`). The scan also stops descending as soon as it finds a
/// project, so a big monorepo costs one entry, not thousands.
const MAX_DEPTH: usize = 4;

/// Hard ceiling on directories examined, so a pathological tree (a network
/// mount, a home full of generated data) cannot make `⌥+d` hang. Reaching it
/// yields fewer candidates, never a stall.
const MAX_SCAN: usize = 4000;

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

/// True when `dir` is itself a project (holds one of [`PROJECT_MARKERS`]).
pub fn is_project(dir: &Path) -> bool {
    PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
}

/// Whether the walk should descend into a directory of this name.
fn descendable(name: &str) -> bool {
    // Hidden directories are caches, VCS internals and app state; none of
    // them is somewhere you would start an agent.
    !name.starts_with('.') && !SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// Find project directories under `roots`, breadth-first.
///
/// Breadth-first rather than depth-first so that when the budget runs out,
/// what survives is the shallow directories nearest the roots, which are the
/// ones a person actually means. It also stops descending at a project: the
/// submodules and vendored checkouts inside a repo are noise in a list whose
/// job is to name the repo.
pub fn scan(roots: &[PathBuf], max_depth: usize, budget: usize) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = roots
        .iter()
        .filter(|r| r.is_dir())
        .map(|r| (r.clone(), 0))
        .collect();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut scanned = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        if scanned >= budget {
            break;
        }
        // Symlinked and bind-mounted trees can otherwise be walked twice, or
        // (worse) cycle forever.
        let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !seen.insert(key) {
            continue;
        }
        scanned += 1;
        if is_project(&dir) {
            found.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut kids: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| {
                // `file_type` does not follow symlinks, so a link into a huge
                // tree (or back up the tree) is skipped rather than walked.
                e.file_type().map(|t| t.is_dir()).unwrap_or(false)
            })
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| descendable(&n.to_string_lossy()))
                    .unwrap_or(false)
            })
            .collect();
        kids.sort();
        for k in kids {
            queue.push_back((k, depth + 1));
        }
    }
    found.sort();
    found
}

/// Directories `zoxide` says the user actually visits, most-used first.
///
/// This is the highest-signal source available and it needs no configuration:
/// if someone has zoxide, their frecency database already knows the places
/// they work, including the ones outside `$HOME` that no scan of the home
/// directory would ever reach. Absent or broken zoxide is not an error, it
/// just contributes nothing.
pub fn zoxide_dirs(limit: usize) -> Vec<PathBuf> {
    let Ok(out) = std::process::Command::new("zoxide")
        .args(["query", "--list"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .take(limit)
        .collect()
}

/// How many zoxide entries to take. Enough to cover the places anyone works
/// in regularly, small enough that the picker stays a list rather than a
/// history dump.
const ZOXIDE_LIMIT: usize = 40;

/// The roots a scan starts from: whatever the user configured, else `$HOME`.
///
/// `$HOME` is the right default precisely because it makes no assumption
/// about layout: the marker scan finds projects wherever this particular
/// person happens to keep them.
pub fn search_roots(configured: &[String]) -> Vec<PathBuf> {
    if !configured.is_empty() {
        return configured.iter().map(|r| expand(r)).collect();
    }
    std::env::var_os("HOME")
        .map(|h| vec![PathBuf::from(h)])
        .unwrap_or_default()
}

/// A directory offered by the `⌥+d` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Absolute path, used verbatim as the spawn directory.
    pub path: PathBuf,
    /// Short label (`~/git/gwae`), so the picker fits in a narrow panel.
    pub label: String,
    /// Why it is on the list (`current`, `recent`, `project`), shown dimmed.
    pub origin: &'static str,
}

/// Shorten an absolute path for display by re-introducing `~`.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return s;
    }
    // HOME and p may be canonicalized differently (`/var` vs `/private/var`
    // on macOS, or a symlinked temp dir in tests).  Canonicalize HOME for the
    // prefix check so `~/...` still wins and the label stays short.
    let home_canon = PathBuf::from(&home)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&home))
        .to_string_lossy()
        .to_string();
    // `p` is already canonicalized by `candidates` and by the TUI's chosen
    // path; canonicalize defensively in case a caller passes a non-canonical
    // path.
    let s_canon = PathBuf::from(&s)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&s))
        .to_string_lossy()
        .to_string();
    if s_canon == home_canon {
        return "~".into();
    }
    if let Some(rest) = s_canon.strip_prefix(&format!("{home_canon}/")) {
        return format!("~/{rest}");
    }
    // Fall back to the non-canonical home prefix (covers non-existent HOME
    // in tests where canonicalize fails for one side only).
    if s == home {
        return "~".into();
    }
    if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
        return format!("~/{rest}");
    }
    s
}

/// Everything the picker can offer, best-first and de-duplicated.
///
/// Order is deliberate, most-likely first: what you are using now, what you
/// configured, your pins, the directories zoxide says you actually visit,
/// then every project found under the search roots, then the roots and
/// `$HOME` as an escape hatch. `current` leads so `⌥+d ↵` is a no-op rather
/// than a surprise.
///
/// Nothing here is keyed off a directory *name*, which is the point: the
/// same code finds `~/git/gwae`, `~/Documents/work/thing`, and `/srv/app`
/// without knowing anything about the machine it is running on.
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
    // Places this user demonstrably works in. Ahead of the scan because
    // frecency beats alphabetical: zoxide knows which repo you opened an
    // hour ago, and reaches trees outside $HOME entirely.
    for d in zoxide_dirs(ZOXIDE_LIMIT) {
        push(&mut out, &mut seen, d, "recent");
    }
    // Then everything that looks like a project under the search roots,
    // found by marker rather than by directory name.
    for p in scan(&search_roots(roots), MAX_DEPTH, MAX_SCAN) {
        push(&mut out, &mut seen, p, "project");
    }
    // The roots themselves, and $HOME, as the always-available escape hatch
    // for a directory that is not a project at all.
    for r in search_roots(roots) {
        push(&mut out, &mut seen, r, "root");
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

    /// A throwaway tree, so scan tests never touch the real machine.
    fn tree(name: &str, dirs: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gwae-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in dirs {
            std::fs::create_dir_all(root.join(d)).expect("make tree");
        }
        root
    }

    #[test]
    fn projects_are_found_by_marker_not_by_directory_name() {
        // The whole point of the redesign: none of these parents is named
        // anything gwae could have guessed.
        let root = tree(
            "markers",
            &[
                "wherever/my-thing/.git",
                "Documents/clients/acme/.hg",
                "srv/app/.jj",
                "notes/not-a-project",
            ],
        );
        let got = scan(std::slice::from_ref(&root), 4, 1000);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into())
            .collect();
        assert!(names.contains(&"my-thing".to_string()), "{names:?}");
        assert!(names.contains(&"acme".to_string()), "{names:?}");
        assert!(names.contains(&"app".to_string()), "{names:?}");
        assert!(
            !names.contains(&"not-a-project".to_string()),
            "a plain directory is not a project: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_scan_stops_at_a_project_and_skips_dependency_trees() {
        let root = tree(
            "nested",
            &[
                "app/.git",
                // A vendored checkout inside a repo: real, and pure noise in
                // a list whose job is to name the repo.
                "app/node_modules/dep/.git",
                "app/sub/.git",
            ],
        );
        let got = scan(std::slice::from_ref(&root), 4, 1000);
        assert_eq!(got.len(), 1, "only the outer repo: {got:?}");
        assert!(got[0].ends_with("app"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_and_budget_bound_the_walk() {
        let root = tree("deep", &["a/b/c/d/e/deep-one/.git", "top/.git"]);
        // Too deep to reach at depth 2, so it is simply not offered; the
        // shallow one still is.
        let shallow = scan(std::slice::from_ref(&root), 2, 1000);
        let names: Vec<String> = shallow
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into())
            .collect();
        assert!(names.contains(&"top".to_string()), "{names:?}");
        assert!(!names.contains(&"deep-one".to_string()), "{names:?}");
        // A budget of zero yields nothing and, crucially, returns.
        assert!(scan(std::slice::from_ref(&root), 9, 0).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_and_system_directories_are_never_descended() {
        assert!(descendable("git"));
        assert!(descendable("my-work"));
        assert!(!descendable(".cache"));
        assert!(!descendable("node_modules"));
        assert!(!descendable("Library"));
        // Case-insensitively, because macOS filesystems are.
        assert!(!descendable("library"));
    }

    #[test]
    fn scanning_a_real_home_is_fast_enough_for_a_keypress() {
        // `⌥+d` scans on open, so the walk has to be imperceptible or the
        // picker feels broken. This runs against the actual machine, which
        // is the only place the bound is meaningful.
        let t = std::time::Instant::now();
        let found = scan(&search_roots(&[]), MAX_DEPTH, MAX_SCAN);
        let dt = t.elapsed();
        assert!(
            dt < std::time::Duration::from_millis(750),
            "scan took {dt:?} and found {} projects; ⌥+d must not stall",
            found.len()
        );
    }

    #[test]
    fn search_roots_default_to_home_and_are_overridable() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        assert_eq!(search_roots(&[]), vec![home]);
        assert_eq!(
            search_roots(&["~/work".into(), "/srv".into()]),
            vec![expand("~/work"), PathBuf::from("/srv")]
        );
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
