//! Upgrade plans: what would upgrading take on this machine?

use super::source::Source;
use super::REPO;
use std::path::Path;

// ---------------------------------------------------------------------------
// What upgrading would take
// ---------------------------------------------------------------------------

/// The upgrade route for a given [`Source`].
///
/// Homebrew is the canonical route. Everything else is legacy: detected so
/// old installs get a truthful answer, but new installs should use
/// `brew install hongnoul/tap/gwae`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `brew upgrade gwae`.
    Brew,
    /// `cargo install gwae --locked --force` (legacy).
    Cargo,
    /// `cargo install --git ... gwae --locked --force` (legacy).
    CargoGit,
    /// Someone else owns this file, or the route no longer ships. We print
    /// the command *you* should run and touch nothing.
    Managed { how: &'static str },
    /// We do not know, so we ask instead of acting.
    Ask,
}

impl Plan {
    /// The commands this plan would run, as `(program, args)`.
    ///
    /// Empty for the plans gwae refuses to drive, which is what makes
    /// "would this run something?" a single `is_empty()` at the call site
    /// rather than a match that has to be kept in sync.
    ///
    /// Only [`Plan::Brew`] and the legacy cargo routes run anything;
    /// everything else prints. `gwae upgrade` itself is check-only and never
    /// executes even these: it prints the command for the user to run.
    pub fn commands(&self) -> Vec<(String, Vec<String>)> {
        match self {
            Plan::Brew => vec![("brew".into(), vec!["upgrade".into(), "gwae".into()])],
            Plan::Cargo => vec![(
                "cargo".into(),
                vec![
                    "install".into(),
                    "gwae".into(),
                    "--locked".into(),
                    "--force".into(),
                ],
            )],
            Plan::CargoGit => vec![(
                "cargo".into(),
                vec![
                    "install".into(),
                    "--git".into(),
                    format!("https://github.com/{REPO}"),
                    "gwae".into(),
                    "--locked".into(),
                    "--force".into(),
                ],
            )],
            Plan::Managed { .. } | Plan::Ask => vec![],
        }
    }

    /// The one line shown before anything runs: what is about to happen, in
    /// the words of the tool that will do it.
    pub fn describe(&self) -> String {
        match self {
            Plan::Brew => "brew upgrade gwae".to_string(),
            Plan::Cargo => "cargo install gwae --locked --force".to_string(),
            Plan::CargoGit => {
                format!("cargo install --git https://github.com/{REPO} gwae --locked --force")
            }
            Plan::Managed { how } => (*how).to_string(),
            Plan::Ask => "unknown install source".to_string(),
        }
    }
}

/// Decide the upgrade route from the source. Pure.
///
/// Homebrew is the canonical route. The `Managed` arms are the legacy
/// installs owned by something that would be actively damaged by us writing
/// over the file, so those turn into instructions rather than actions.
pub fn plan(source: Source, _exe: &Path) -> Plan {
    match source {
        Source::Script => Plan::Managed {
            how: "the install.sh route is retired: reinstall with `brew install hongnoul/tap/gwae`",
        },
        Source::Homebrew => Plan::Brew,
        Source::Cargo => Plan::Cargo,
        Source::CargoGit => Plan::CargoGit,
        Source::Source_ => Plan::Managed {
            how: "you built this from a checkout: `git pull && make install`",
        },
        Source::Nix => Plan::Managed {
            how: "Nix owns this store path: `nix flake update` in your flake, \
                  or `nix profile upgrade gwae`. New installs should use `brew install hongnoul/tap/gwae`.",
        },
        Source::System => Plan::Managed {
            how: "your package manager owns this file: e.g. `paru -Syu gwae-bin`. \
                  New installs should use `brew install hongnoul/tap/gwae`.",
        },
        Source::Unknown => Plan::Ask,
    }
}

// ---------------------------------------------------------------------------
// Is there anything to upgrade to
// ---------------------------------------------------------------------------

/// A parsed `major.minor.patch`, ignoring any pre-release suffix.
///
/// Hand-rolled rather than pulling in `semver`: gwae's whole dependency list
/// fits on a screen and stays there, and the comparison needed here is three
/// integers.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    // `gwae --version` prints "gwae 1.0.1"; accept that spelling too so the
    // caller can hand us either.
    let s = s.rsplit(' ').next().unwrap_or(s);
    // Drop `-rc.1` / `+build`.
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Whether `latest` is strictly newer than `current`.
///
/// Unparseable input answers "no". A version string we cannot read is not
/// evidence of a new release, and nagging someone to upgrade to a version
/// that may not exist is worse than staying quiet.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Pull `1.0.2` out of `https://github.com/o/r/releases/tag/v1.0.2`.
pub fn tag_from_url(url: &str) -> Option<String> {
    let tag = url.rsplit("/tag/").next()?;
    if tag == url {
        return None;
    }
    let tag = tag.trim_end_matches('/');
    let v = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(v).map(|_| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::check::notice;
    use crate::update::source::Source;
    use std::path::Path;

    #[test]
    fn the_retired_script_route_points_at_brew_and_runs_nothing() {
        let p = plan(Source::Script, Path::new("/Users/my name/.local/bin/gwae"));
        assert!(
            p.commands().is_empty(),
            "retired route must never run: {p:?}"
        );
        assert!(
            p.describe().contains("brew install"),
            "must point at brew: {}",
            p.describe()
        );
    }

    #[test]
    fn versions_compare_on_numbers_not_strings() {
        assert!(is_newer("1.0.9", "1.0.10"));
        assert!(is_newer("1.0.1", "v1.1.0"));
        assert!(!is_newer("1.0.1", "1.0.1"));
        assert!(!is_newer("2.0.0", "1.9.9"));
        // A version we cannot read is never a reason to nag.
        assert!(!is_newer("1.0.1", "banana"));
        assert!(!is_newer("banana", "1.0.2"));
    }

    #[test]
    fn a_version_string_may_be_spelled_as_the_binary_prints_it() {
        assert_eq!(parse_version("gwae 1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
    }

    #[test]
    fn the_release_tag_comes_out_of_the_redirect_url() {
        assert_eq!(
            tag_from_url("https://github.com/hongnoul/gwae/releases/tag/v1.0.2").as_deref(),
            Some("1.0.2")
        );
        // The bare /latest URL (no redirect followed) must not read as a
        // version, or every offline machine would think 'latest' is a release.
        assert_eq!(
            tag_from_url("https://github.com/hongnoul/gwae/releases/latest"),
            None
        );
        assert_eq!(tag_from_url(""), None);
    }

    #[test]
    fn the_notice_always_ends_in_a_command_for_this_machine() {
        let n = notice("1.0.1", "1.0.2", &Plan::Brew);
        assert!(n.contains("1.0.2 is out"), "{n}");
        assert!(n.contains("brew upgrade gwae"), "{n}");
        // Retired routes print their reinstall advice, not a runnable command.
        let n = notice(
            "1.0.1",
            "1.0.2",
            &plan(Source::Script, Path::new("/b/gwae")),
        );
        assert!(n.contains("brew install"), "{n}");
    }

    #[test]
    fn every_managed_source_refuses_to_run_anything() {
        for s in [Source::Nix, Source::System, Source::Source_] {
            let p = plan(s, Path::new("/usr/bin/gwae"));
            assert!(
                p.commands().is_empty(),
                "{s:?} must never be driven by gwae"
            );
            assert!(!p.describe().is_empty(), "{s:?} must still say what to do");
        }
    }
}
