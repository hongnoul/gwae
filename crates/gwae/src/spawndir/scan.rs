//! Project discovery: marker-based directory scan with bounds.

use super::path::{expand, PROJECT_MARKERS, SKIP_DIRS};
use std::path::{Path, PathBuf};

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
    walk(roots, max_depth, budget, true)
}

/// Searchable directories, including unversioned scaffolds and repo children.
/// Unlike project suggestions, these are only shown once the user types.
pub(super) fn scan_directories(roots: &[PathBuf], max_depth: usize, budget: usize) -> Vec<PathBuf> {
    walk(roots, max_depth, budget, false)
}

/// Both discovery passes share depth/budget limits, exclusions and cycle checks.
fn walk(roots: &[PathBuf], max_depth: usize, budget: usize, projects_only: bool) -> Vec<PathBuf> {
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
        if !projects_only || is_project(&dir) {
            found.push(dir.clone());
            if projects_only {
                continue;
            }
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
pub(super) const ZOXIDE_LIMIT: usize = 40;

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

#[cfg(test)]
mod tests {
    use super::super::path::inherited;
    use super::super::picker::{candidates, filter};
    use super::*;

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
    fn reopening_finds_unversioned_scaffolds_under_the_current_spawn_directory() {
        let root = tree("scaffold", &["workspace/.git", "search-elsewhere"]);
        let current = root.join("workspace");
        // The spawn directory need not be inside the configured search roots.
        let roots = vec![root.join("search-elsewhere").to_string_lossy().into_owned()];
        let before = candidates(Some(&current), "", &[], &roots);
        assert!(filter(&before, "fresh-scaffold").is_empty());

        let scaffold = current.join("fresh-scaffold");
        std::fs::create_dir_all(&scaffold).unwrap();
        let after = candidates(Some(&current), "", &[], &roots);
        let matches = filter(&after, "fresh-scaffold");
        assert_eq!(matches.len(), 1, "new plain directories must be searchable");
        assert_eq!(matches[0].path, scaffold.canonicalize().unwrap());
        assert!(
            !filter(&after, "").iter().any(|c| c.path == matches[0].path),
            "plain directories must not clutter the initial project suggestions"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn directory_search_reaches_inside_other_projects_and_keeps_skipping_dependencies() {
        let root = tree(
            "search-nested",
            &[
                "other/.git",
                "other/apps/fresh-scaffold",
                "other/node_modules/noisy-scaffold",
                "other/target/noisy-scaffold",
                "other/.cache/noisy-scaffold",
                "plain-scaffold",
            ],
        );
        // Candidate discovery also includes cwd/recent directories, which may
        // contain another concurrently running test's temporary tree.
        let canonical_root = root.canonicalize().unwrap();
        let all: Vec<_> = candidates(None, "", &[], &[root.to_string_lossy().into_owned()])
            .into_iter()
            .filter(|candidate| candidate.path.starts_with(&canonical_root))
            .map(|mut candidate| {
                // Random macOS temp prefixes can themselves fuzzy-match the
                // query. Test discovery using stable fixture-relative labels.
                candidate.label = candidate
                    .path
                    .strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                candidate
            })
            .collect();
        for query in ["fresh-scaffold", "plain-scaffold"] {
            assert_eq!(filter(&all, query).len(), 1, "missing {query}");
        }
        assert!(filter(&all, "noisy-scaffold").is_empty());
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
    fn directory_search_is_bounded_and_deduplicates_overlapping_roots() {
        let root = tree("directory-bounds", &["a/child", "b/child"]);
        let roots = vec![root.join("a"), root.clone(), root.join("a")];
        let shallow = scan_directories(&roots, 1, 1000);
        assert!(shallow.contains(&root.join("a/child")));
        assert!(!shallow.contains(&root.join("b/child")));
        assert_eq!(shallow.iter().filter(|p| **p == root.join("a")).count(), 1);
        assert!(scan_directories(&roots, 9, 0).is_empty());
        assert_eq!(scan_directories(&roots, 9, 2).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn directory_search_does_not_follow_child_symlinks_or_repeat_alias_roots() {
        let root = tree("directory-links", &["workspace/scaffold", "outside"]);
        std::os::unix::fs::symlink(root.join("workspace"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("workspace/link")).unwrap();
        let roots = vec![root.join("workspace"), root.join("alias")];
        let got = scan_directories(&roots, 4, 1000);
        assert_eq!(
            got,
            vec![root.join("workspace"), root.join("workspace/scaffold")]
        );
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
        let found = candidates(inherited().as_deref(), "", &[], &[]);
        let dt = t.elapsed();
        assert!(
            dt < std::time::Duration::from_millis(750),
            "discovery took {dt:?} and found {} candidates; ⌥+d must not stall",
            found.len()
        );
        eprintln!(
            "spawn-directory discovery: {} candidates in {dt:?}",
            found.len()
        );
    }

    // Asserts unix facts (`sh` on PATH, `$HOME`, unix paths/quoting).
    #[cfg(unix)]
    #[test]
    fn search_roots_default_to_home_and_are_overridable() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        assert_eq!(search_roots(&[]), vec![home]);
        assert_eq!(
            search_roots(&["~/work".into(), "/srv".into()]),
            vec![expand("~/work"), PathBuf::from("/srv")]
        );
    }
}
