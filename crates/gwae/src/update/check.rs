//! Freshness checks: is there anything to upgrade to?

use super::plan::{plan, Plan, is_newer, parse_version, tag_from_url};
use super::source::{detect, probe, same_dir, state_dir, CargoOrigin, Facts, Receipt, Source};
use super::{CHECK_INTERVAL, CURRENT, NET_TIMEOUT_SECS, NO_CHECK_ENV, REPO, SOURCE_ENV};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Ask GitHub which release is current.
///
/// Uses the `releases/latest` **redirect**, not `api.github.com`: the API is
/// rate limited to 60 requests an hour per IP for unauthenticated callers,
/// which is a limit shared by everyone behind one office NAT, and being
/// silently rate-limited into "no updates ever" is the worst failure this
/// feature could have. The redirect lands on `/releases/tag/vX.Y.Z`, so the
/// tag is the answer.
///
/// The request is a HEAD with no body, no auth, and no query string: nothing
/// about the machine or the installed version leaves it.
pub fn latest_version() -> Result<String, String> {
    // Test seam. The branch that *runs* an upgrade command only exists when a
    // newer release exists, which no test can conjure on demand, so the one
    // path that can execute something on a user's machine would otherwise be
    // the only untested path in this module. Also useful for rehearsing an
    // upgrade before a release is cut.
    if let Ok(v) = std::env::var("GWAE_UPDATE_LATEST") {
        return match parse_version(&v) {
            Some(_) => Ok(v.trim().trim_start_matches('v').to_string()),
            None => Err(format!("GWAE_UPDATE_LATEST is not a version: {v:?}")),
        };
    }
    let out = std::process::Command::new("curl")
        .args([
            "-fsSLI",
            "-o",
            devnull(),
            "-w",
            "%{url_effective}",
            "--max-time",
            &NET_TIMEOUT_SECS.to_string(),
            &format!("https://github.com/{REPO}/releases/latest"),
        ])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not reach github.com{}",
            match String::from_utf8_lossy(&out.stderr).trim() {
                "" => String::new(),
                s => format!(": {s}"),
            }
        ));
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    tag_from_url(&url).ok_or_else(|| format!("unexpected release URL: {url}"))
}

/// The platform's bit bucket, for `curl -o`.
fn devnull() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

// ---------------------------------------------------------------------------
// Cadence: the cached answer
// ---------------------------------------------------------------------------

/// The cached result of the last check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cache {
    /// Unix seconds of the last completed check.
    pub last_check: u64,
    /// The version that check found.
    pub latest: String,
}

impl Cache {
    /// Parse the cache file.
    pub fn parse(text: &str) -> Cache {
        let Ok(v) = toml::from_str::<toml::Value>(text) else {
            return Cache::default();
        };
        Cache {
            last_check: v
                .get("last_check")
                .and_then(|x| x.as_integer())
                .unwrap_or(0)
                .max(0) as u64,
            latest: v
                .get("latest")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        }
    }

    /// Serialize the cache file.
    pub fn render(&self) -> String {
        format!(
            "# Written by gwae; safe to delete.\nlast_check = {}\nlatest = {:?}\n",
            self.last_check, self.latest
        )
    }

    /// Read the cache, or an empty one.
    pub fn load() -> Cache {
        cache_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Cache::parse(&t))
            .unwrap_or_default()
    }

    /// Write the cache, best effort. A machine with an unwritable state
    /// directory checks once per session instead of once per day, which is a
    /// fine outcome for a failure nobody should be told about.
    pub fn store(&self) {
        let Some(p) = cache_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.render());
    }

    /// Whether this cache is old enough to re-check, given "now".
    pub fn is_stale(&self, now: u64) -> bool {
        now.saturating_sub(self.last_check) >= CHECK_INTERVAL.as_secs()
    }
}

/// Where the cached answer lives.
pub fn cache_path() -> Option<PathBuf> {
    Some(state_dir()?.join("update.toml"))
}

/// Unix seconds now.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// When the startup check is allowed to touch the network. Pure, so the
/// policy is one testable function rather than a chain of `if`s at a call
/// site that also owns a thread.
///
/// `env_off` is the [`NO_CHECK_ENV`] kill switch, which beats the config: a
/// CI runner sets an env var, it does not rewrite a user's file.
pub fn should_check(startup_enabled: bool, env_off: bool, cache: &Cache, now: u64) -> bool {
    startup_enabled && !env_off && cache.is_stale(now)
}

/// The one-line nudge shown when a newer version exists, e.g.
/// `gwae 1.0.2 is out (you have 1.0.1) · run: brew upgrade gwae`.
///
/// It always ends in the *exact* command for this machine, because "an update
/// is available" with no route is a notification that makes the reader do the
/// research we already did.
pub fn notice(current: &str, latest: &str, plan: &Plan) -> String {
    let how = match plan {
        Plan::Ask => "run: gwae upgrade".to_string(),
        Plan::Script { .. } => "run: gwae upgrade".to_string(),
        p => format!("run: {}", p.describe()),
    };
    format!("gwae {latest} is out (you have {current}) · {how}")
}

// ---------------------------------------------------------------------------
// The background check
// ---------------------------------------------------------------------------

/// Run the check on a background thread and hand back a slot that will hold
/// the notice, if there is one.
///
/// Non-blocking by construction: startup must not wait on a network round
/// trip, and a session that ends before the answer arrives simply never sees
/// it. The thread is detached and writes exactly two things - the cache file
/// and the slot.
pub fn spawn_check(
    startup_enabled: bool,
    source: Source,
) -> std::sync::Arc<std::sync::Mutex<Option<String>>> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let env_off = std::env::var_os(NO_CHECK_ENV).is_some();
    let cache = Cache::load();
    let now = now_unix();

    // A fresh cache can answer immediately, without a thread or a request.
    if !env_off && startup_enabled && !cache.is_stale(now) && is_newer(CURRENT, &cache.latest) {
        let exe = std::env::current_exe().unwrap_or_default();
        let p = plan(source, &exe);
        *slot.lock().unwrap() = Some(notice(CURRENT, &cache.latest, &p));
        return slot;
    }
    if !should_check(startup_enabled, env_off, &cache, now) {
        return slot;
    }

    let out = std::sync::Arc::clone(&slot);
    std::thread::spawn(move || {
        let Ok(latest) = latest_version() else {
            // A failed check is not news. It costs the user nothing and
            // telling them their network is flaky is not gwae's job.
            return;
        };
        Cache {
            last_check: now_unix(),
            latest: latest.clone(),
        }
        .store();
        if is_newer(CURRENT, &latest) {
            let exe = std::env::current_exe().unwrap_or_default();
            let p = plan(source, &exe);
            if let Ok(mut g) = out.lock() {
                *g = Some(notice(CURRENT, &latest, &p));
            }
        }
    });
    slot
}

// ---------------------------------------------------------------------------
// `gwae upgrade`
// ---------------------------------------------------------------------------

/// `gwae upgrade`: report the version, source, route, and latest release.
/// Always check-only: prints the exact upgrade command but never executes it.
/// Returns the process exit code.
pub fn run_upgrade(configured: Option<Source>) -> i32 {
    let facts = probe(configured);
    let source = detect(&facts);
    let p = plan(source, &facts.exe);

    println!("gwae {CURRENT}");
    println!("  binary:  {}", facts.exe.display());
    println!("  source:  {}{}", source.as_str(), provenance(&facts));
    // Printed on every path, including "up to date". "How would this machine
    // upgrade?" is worth answering before the day it matters; an answer that
    // only appears once a release exists is one nobody can check in advance.
    println!("  route:   {}", p.describe());

    let latest = match latest_version() {
        Ok(v) => v,
        Err(e) => {
            println!("  latest:  unknown ({e})");
            return 1;
        }
    };
    Cache {
        last_check: now_unix(),
        latest: latest.clone(),
    }
    .store();

    // An unknown source is a real defect even when there is nothing to
    // install today: the next release will find this machine unable to
    // upgrade, and the fix is one config key the user can write right now.
    // So this is checked before the up-to-date exit, not after it.
    if matches!(p, Plan::Ask) {
        println!("  latest:  {latest}");
        if let Some(r) = ignored_receipt(&facts) {
            println!(
                "\nnote: there is an install receipt for {} ({} {}), but this binary",
                r.dir.display(),
                r.source.as_str(),
                r.version
            );
            println!(
                "runs from {}, so the receipt does not apply to it.",
                facts.exe.display()
            );
        }
        println!("\ngwae cannot tell how it was installed, so it will not guess.");
        println!("Set the route in your config and re-run:");
        println!("  [update]");
        println!(
            "  source = \"brew\"   # one of: {}",
            Source::NAMES.join(", ")
        );
        return 1;
    }

    if !is_newer(CURRENT, &latest) {
        println!("  latest:  {latest} — you are up to date");
        return 0;
    }
    println!("  latest:  {latest}");

    let cmds = p.commands();
    if cmds.is_empty() {
        println!("\nThis install is managed elsewhere, so gwae will not touch it.");
        println!("  {}", p.describe());
        return 0;
    }

    println!("\nUpgrade {CURRENT} -> {latest} by running:");
    for (prog, args) in &cmds {
        println!("  {prog} {}", args.join(" "));
    }
    // Check-only by design: gwae prints the exact command but never executes
    // a package manager itself.
    let _ = check_only;
    let _ = assume_yes;
    0
}

/// How the source was decided, appended to the `source:` line so the user can
/// tell a fact from a guess.
pub(crate) fn provenance(f: &Facts) -> String {
    if f.configured.is_some() {
        return " (from config)".to_string();
    }
    if let Some(r) = &f.receipt {
        if r.dir.as_os_str().is_empty() || same_dir(f.exe.parent(), &r.dir) {
            return " (from install receipt)".to_string();
        }
        // A receipt exists but describes a different install (e.g. the binary
        // was copied elsewhere, or a second install lives on PATH first).
        // Say so: otherwise "detected from path" reads as "gwae knows nothing",
        // when it actually knows something that just does not apply here.
        return format!(
            " (detected from path; install receipt for {} ignored)",
            r.dir.display()
        );
    }
    " (detected from path)".to_string()
}

/// A receipt that exists but must not speak for this binary, because the
/// binary no longer sits in the directory the receipt names.
pub(crate) fn ignored_receipt(f: &Facts) -> Option<&Receipt> {
    let r = f.receipt.as_ref()?;
    if r.dir.as_os_str().is_empty() || same_dir(f.exe.parent(), &r.dir) {
        return None;
    }
    Some(r)
}

/// A y/N prompt on stdin. `false` when stdin is not a terminal, so a piped
/// `gwae upgrade` reports the plan and stops rather than acting on a
/// The `gwae doctor` line: source, route, and what the last check found.
pub fn doctor_line(configured: Option<Source>, startup_enabled: bool) -> String {
    let facts = probe(configured);
    let source = detect(&facts);
    let p = plan(source, &facts.exe);
    let cache = Cache::load();
    let checked = match (cache.last_check, cache.latest.as_str()) {
        (0, _) | (_, "") => "never checked".to_string(),
        (_, latest) if is_newer(CURRENT, latest) => format!("{latest} available"),
        (_, latest) => format!("latest is {latest}"),
    };
    let auto = if std::env::var_os(NO_CHECK_ENV).is_some() {
        "check off (env)"
    } else if startup_enabled {
        "checks daily"
    } else {
        "check off"
    };
    let ok = if matches!(p, Plan::Ask) { "" } else { " [ok]" };
    format!(
        "{}{} · {} · {} · `gwae upgrade` -> {}",
        source.as_str(),
        provenance(&facts),
        auto,
        checked,
        p.describe()
    ) + ok
}

#[cfg(test)]

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::plan::Plan;
    use crate::update::source::Source;
    use std::path::PathBuf;

    use super::*;

    fn facts(exe: &str) -> Facts {
        Facts {
            exe: PathBuf::from(exe),
            ..Default::default()
        }
    }

    
    #[test]
    fn the_cache_round_trips_and_expires_after_a_day() {
        let c = Cache {
            last_check: 1_700_000_000,
            latest: "1.2.3".into(),
        };
        assert_eq!(Cache::parse(&c.render()), c);
        assert!(!c.is_stale(c.last_check + 3600));
        assert!(c.is_stale(c.last_check + CHECK_INTERVAL.as_secs()));
        // A corrupt cache reads as "never checked", which re-checks at any
        // real clock time (the epoch itself is not a time anyone runs at).
        assert!(Cache::parse("garbage {").is_stale(now_unix()));
        assert_eq!(Cache::parse("garbage {"), Cache::default());
    }


    #[test]
    fn the_env_kill_switch_beats_an_enabled_config() {
        let stale = Cache::default();
        assert!(should_check(true, false, &stale, now_unix()));
        assert!(!should_check(true, true, &stale, now_unix()));
        assert!(!should_check(false, false, &stale, now_unix()));
    }

}
