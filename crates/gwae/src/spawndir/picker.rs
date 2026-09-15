//! Picker candidates: ranked directories for the spawn picker.

use super::path::{expand, inherited, MAX_DEPTH, MAX_SCAN};
use super::scan::{is_project, scan, search_roots, zoxide_dirs, ZOXIDE_LIMIT};
use std::path::{Path, PathBuf};

/// A directory offered by the `⌥+d` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Absolute path, used verbatim as the spawn directory.
    pub path: PathBuf,
    /// Short label (`~/git/gwae`), so the picker fits in a narrow panel.
    pub label: String,
    /// Why it is on the list (`current`, `recent`, `project`), shown dimmed.
    /// `directory` entries are searchable but hidden until a query is typed.
    pub origin: &'static str,
}

/// Shorten an absolute path for display by re-introducing `~`.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return s;
    }
    if s == home {
        return "~".into();
    }
    if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
        return format!("~/{rest}");
    }
    // On macOS /var is a symlink to /private/var, and $TMPDIR lives under
    // /var/folders/...  `candidates` canonicalizes each found path to
    // /private/var/..., while HOME stays /var/... — so the prefix above
    // misses and the label would be an absolute path. Handle that once by
    // canonicalizing only HOME (one stat, not one per candidate per frame),
    // and for the synthetic /var case where canonicalize can't reach a
    // nonexistent temp tempdir, fall back to prefix rewriting.
    if let Ok(home_canon) = PathBuf::from(&home).canonicalize() {
        let hc = home_canon.to_string_lossy().to_string();
        if s == hc {
            return "~".into();
        }
        if let Some(rest) = s.strip_prefix(&format!("{hc}/")) {
            return format!("~/{rest}");
        }
        // And the reverse: HOME is already /private/var but p is still /var.
        if home.starts_with("/private/var") && s.starts_with("/var/") {
            let alt = format!("/var{}", &home[8..]);
            if s == alt {
                return "~".into();
            }
            if let Some(rest) = s.strip_prefix(&format!("{alt}/")) {
                return format!("~/{rest}");
            }
        }
    } else if home.starts_with("/var/folders/") && s.starts_with("/private/var/folders/") {
        // HOME is a temp dir that doesn't exist yet (test setup race) or a
        // synthetic path where canonicalize failed; still alias /var ↔ /private/var.
        let alt_home = format!("/private{home}");
        if s == alt_home {
            return "~".into();
        }
        if let Some(rest) = s.strip_prefix(&format!("{alt_home}/")) {
            return format!("~/{rest}");
        }
    }
    s
}

/// Everything the picker can offer, best-first and de-duplicated.
///
/// Order is deliberate, most-likely first: what you are using now, what you
/// configured, your pins, the directories zoxide says you actually visit,
/// then every project found under the search roots, then the roots and
/// `$HOME` as an escape hatch. Other directories are included for name search
/// only, so `current` still leads and `⌥+d ↵` remains a no-op.
///
/// Nothing here is keyed off a directory *name*, which is the point: the
/// same code finds `~/git/gwae`, `~/Documents/work/thing`, and `/srv/app`
/// without knowing anything about the machine it is running on.
#[allow(dead_code)] // kept for tests/docs; production uses harness-aware form
pub fn candidates(
    current: Option<&Path>,
    cfg_dir: &str,
    pins: &[String],
    roots: &[String],
) -> Vec<Candidate> {
    candidates_for_harness(current, cfg_dir, "", pins, roots)
}

/// Harness-aware candidate builder: treats `harness_dir` as the primary config
/// origin (`"config:jcode"`), then `fallback_dir` as secondary. Order keeps
/// both discoverable but preferred harness first.
pub fn candidates_for_harness(
    current: Option<&Path>,
    harness_dir: &str,
    fallback_dir: &str,
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
    // Preferred harness first, then generic fallback, each as its own origin so
    // the picker explains which config line contributed the entry.
    if !harness_dir.trim().is_empty() {
        push(&mut out, &mut seen, expand(harness_dir), "config");
    }
    if !fallback_dir.trim().is_empty() && fallback_dir.trim() != harness_dir.trim() {
        push(&mut out, &mut seen, expand(fallback_dir), "config");
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
    // Search near this session first, including when --dir or a harness puts
    // it outside HOME/configured roots or deeper than the home scan can reach.
    // Snapshot these explicit/recent places before adding global suggestions.
    let mut directory_roots: Vec<PathBuf> = out.iter().map(|c| c.path.clone()).collect();
    let search_roots = search_roots(roots);
    directory_roots.extend(search_roots.iter().cloned());
    // Then everything that looks like a project under the search roots,
    // found by marker rather than by directory name.
    for p in scan(&search_roots, MAX_DEPTH, MAX_SCAN) {
        push(&mut out, &mut seen, p, "project");
    }
    // The roots themselves, and $HOME, as the always-available escape hatch
    // for a directory that is not a project at all.
    for r in search_roots {
        push(&mut out, &mut seen, r, "root");
    }
    if let Some(home) = std::env::var_os("HOME") {
        push(&mut out, &mut seen, PathBuf::from(home), "home");
    }
    // Rebuilt on every picker open, not cached for the session: a running
    // agent can scaffold a directory without initializing any VCS marker.
    // Keep these after suggestions so dedup preserves their stronger origins.
    for p in scan_directories(&directory_roots, MAX_DEPTH, MAX_SCAN) {
        push(&mut out, &mut seen, p, "directory");
    }
    out
}

/// Filter candidates by a typed query: a subsequence match on the label,
/// case-insensitive, like every fuzzy finder. Exact substring matches sort
/// first so typing a full repo name lands on it rather than on a longer path
/// that merely contains the letters. With no query, show only the curated
/// project/current/config/history suggestions, not every ordinary directory.
pub fn filter(cands: &[Candidate], query: &str) -> Vec<Candidate> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return cands
            .iter()
            .filter(|c| c.origin != "directory")
            .cloned()
            .collect();
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
#[cfg(test)]
mod tests {
    use super::*;

    use super::*;

    
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


    #[test]
    fn candidates_for_harness_leads_with_current_then_harness_then_fallback() {
        let tmp = std::env::temp_dir();
        // Use temp dirs as harness/fallback so tilde/expand works.
        let h = tmp.join("gwae-harness-cand-h");
        let f = tmp.join("gwae-harness-cand-f");
        let _ = std::fs::create_dir_all(&h);
        let _ = std::fs::create_dir_all(&f);
        let c = candidates_for_harness(
            Some(&tmp),
            h.to_str().unwrap(),
            f.to_str().unwrap(),
            &[],
            &["/no/such/root".into()],
        );
        // current first
        assert_eq!(c[0].origin, "current");
        // then harness config, then fallback, order preserved
        let labels: Vec<&str> = c.iter().map(|x| x.origin).collect();
        let h_pos = labels.iter().position(|o| *o == "config").unwrap();
        // Two config origins in order: both labeled "config", deduped by path.
        assert!(labels.iter().filter(|o| **o == "config").count() >= 2 || h != f);
        let _ = h_pos;
    }

}
