//! Staying current: how an *already installed* gwae learns about a new
//! version, and how it moves to one (ADR-016).
//!
//! The rule this module exists to enforce is one sentence: **gwae updates
//! itself the way it was installed, or not at all.** A binary that overwrites
//! itself in place would be wrong for most of the ways gwae is actually on a
//! machine — Homebrew tracks file ownership, `cargo install` owns
//! `~/.cargo/bin`, Nix store paths are read-only by design, and a distro
//! package manager would silently disown a file it did not write. So the
//! upgrade *route* is a decision, and it is made here.
//!
//! Three separable questions, deliberately kept apart:
//!
//! 1. **Where did this binary come from?** [`Source`], decided by
//!    [`detect`] from facts ([`Facts`]) rather than probed inline, so every
//!    branch is testable without owning five differently-installed machines.
//!    Order of authority: the user's config, then the receipt
//!    `scripts/install.sh` leaves behind, then the path the running binary
//!    sits at. A guess is always labelled as one ([`Source::Unknown`]).
//! 2. **What would upgrading take?** [`plan`], pure, yielding either a
//!    command we are willing to run ([`Plan::commands`]) or an explanation of
//!    the one command *you* should run when the answer belongs to a package
//!    manager we must not fight.
//! 3. **Is there anything to upgrade to?** [`latest_version`] asks GitHub's
//!    `releases/latest` redirect for a tag. That request is a bare HTTP HEAD:
//!    it carries no version, no machine id, and nothing about the user, and
//!    it happens at most once a day ([`CHECK_INTERVAL`]) behind a config key
//!    that can turn it off entirely.
//!
//! What this module never does is install anything on its own. The check
//! notifies; `gwae upgrade` acts, and only after saying what it will run.
//! Software that replaces itself without being asked is a class of surprise a
//! terminal multiplexer has not earned the right to hand anyone.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The repository every install route ultimately points at.
pub const REPO: &str = "hongnoul/gwae";

/// The version this binary was built as.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How long a "latest version" answer is trusted before we ask again.
///
/// A day. Long enough that a machine that opens forty gwae sessions a day
/// makes one request, short enough that a release is noticed the next
/// morning.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long the network check may take before it is abandoned.
///
/// The check runs on a background thread and its result is *optional*, so the
/// only thing a slow answer can cost is the answer itself. Five seconds keeps
/// a stalled captive-portal DNS from leaving a thread alive for the whole
/// session.
const NET_TIMEOUT_SECS: u64 = 5;

/// Kill switch for the network check, for CI and for anyone scripting gwae.
///
/// Checked in addition to the config key so that a machine can be made quiet
/// without editing a config file it may not own.
pub const NO_CHECK_ENV: &str = "GWAE_NO_UPDATE_CHECK";

/// Override for the detected install source, e.g. `GWAE_UPDATE_SOURCE=brew`.
/// Same values the config key takes. Mostly for tests and for packagers who
/// vendor gwae somewhere the heuristics cannot see.
pub const SOURCE_ENV: &str = "GWAE_UPDATE_SOURCE";

// ---------------------------------------------------------------------------
// Where this binary came from
// ---------------------------------------------------------------------------

/// How gwae got onto this machine, which decides how it may leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `scripts/install.sh` put a release binary in a plain directory. We own
    /// that file outright, so re-running the installer is the upgrade.
    Script,
    /// Homebrew (a formula in the tap). `brew upgrade`.
    Homebrew,
    /// `cargo install gwae` from crates.io.
    Cargo,
    /// `cargo install --git https://github.com/hongnoul/gwae gwae`.
    CargoGit,
    /// Built in a checkout and installed by `make install` / `cargo install
    /// --path`. Upgrading means pulling and rebuilding, which is the user's
    /// call, not ours.
    Source_,
    /// A Nix store path. Immutable by design; the flake input is what moves.
    Nix,
    /// A distro package manager owns this file (`/usr/bin`, `/usr/local/bin`
    /// on Linux). AUR, apt, whatever it is: it is not ours to overwrite.
    System,
    /// Windows, where the release ships as a zip and there is no installer
    /// script to re-run yet.
    Windows,
    /// We could not tell. Never guessed *at*: the user is shown the routes
    /// and asked to pick one, and the answer can be written to the config.
    Unknown,
}

impl Source {
    /// The name used in config and in `GWAE_UPDATE_SOURCE`.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Script => "install.sh",
            Source::Homebrew => "brew",
            Source::Cargo => "cargo",
            Source::CargoGit => "cargo-git",
            Source::Source_ => "source",
            Source::Nix => "nix",
            Source::System => "system",
            Source::Windows => "windows",
            Source::Unknown => "unknown",
        }
    }

    /// Parse a config / env / receipt spelling. Tolerant of the obvious
    /// synonyms, because "homebrew" and "brew" are the same answer and
    /// bouncing one of them would be pedantry.
    pub fn parse(s: &str) -> Option<Source> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "install.sh" | "install-sh" | "script" | "installer" => Some(Source::Script),
            "brew" | "homebrew" => Some(Source::Homebrew),
            "cargo" | "crates.io" | "crates-io" => Some(Source::Cargo),
            "cargo-git" | "git" => Some(Source::CargoGit),
            "source" | "make" | "checkout" | "path" => Some(Source::Source_),
            "nix" => Some(Source::Nix),
            "system" | "apt" | "aur" | "pacman" | "dnf" | "distro" => Some(Source::System),
            "windows" | "winget" | "scoop" | "zip" => Some(Source::Windows),
            "unknown" | "auto" | "" => Some(Source::Unknown),
            _ => None,
        }
    }

    /// Every name a user may write, for error messages.
    pub const NAMES: &'static [&'static str] = &[
        "install.sh",
        "brew",
        "cargo",
        "cargo-git",
        "source",
        "nix",
        "system",
        "windows",
    ];
}

/// What [`detect`] decides from. Passed in rather than probed inside, so a
/// test describes a machine instead of needing one.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The path of the running binary (`std::env::current_exe`), canonical if
    /// that was possible.
    pub exe: PathBuf,
    /// `[update] source` from the config, or `GWAE_UPDATE_SOURCE`. Wins over
    /// everything: it is the user telling us directly.
    pub configured: Option<Source>,
    /// The receipt `install.sh` wrote, if this machine has one.
    pub receipt: Option<Receipt>,
    /// What `~/.cargo/.crates.toml` says about a `gwae` entry, when the
    /// binary lives in a cargo bin directory. Distinguishes a crates.io
    /// install from a `--git` one, which need different upgrade commands.
    pub cargo_origin: Option<CargoOrigin>,
    /// This is a Windows build.
    pub windows: bool,
}

/// Which cargo route installed gwae, read out of `.crates.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoOrigin {
    /// `registry+https://github.com/rust-lang/crates.io-index`
    Registry,
    /// `git+https://github.com/...`
    Git,
    /// `path+file:///...`, i.e. `cargo install --path`.
    Path,
}

/// The note `scripts/install.sh` leaves so the source is *known* rather than
/// inferred from a path that a user may well have moved the binary to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// How it was installed, as the installer spells it.
    pub source: Source,
    /// The directory the binary was installed into.
    pub dir: PathBuf,
    /// The version installed, for the record.
    pub version: String,
}

impl Receipt {
    /// Parse the receipt file. TOML, because gwae already speaks it and a
    /// second serialization format for four keys would be silly.
    pub fn parse(text: &str) -> Option<Receipt> {
        let v: toml::Value = toml::from_str(text).ok()?;
        let t = v.as_table()?;
        let source = Source::parse(t.get("source")?.as_str()?)?;
        Some(Receipt {
            source,
            dir: PathBuf::from(t.get("dir").and_then(|d| d.as_str()).unwrap_or_default()),
            version: t
                .get("version")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// Read the receipt from the state directory, if there is one.
    pub fn load() -> Option<Receipt> {
        let text = std::fs::read_to_string(receipt_path()?).ok()?;
        Receipt::parse(&text)
    }
}

/// Where the installer's receipt lives.
pub fn receipt_path() -> Option<PathBuf> {
    Some(state_dir()?.join("install.toml"))
}

/// gwae's state directory: cached answers and the install receipt.
///
/// State, not config: nothing in here is hand-edited, and losing it costs a
/// re-check and a re-detect. Kept out of `~/.config/gwae` for exactly that
/// reason - a directory the user is invited to edit should not fill up with
/// machine-written bookkeeping.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(x).join("gwae"));
    }
    if cfg!(windows) {
        if let Some(l) = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(l).join("gwae"));
        }
    }
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty())?;
    Some(PathBuf::from(home).join(".local/state/gwae"))
}

/// Decide the install source. Pure.
///
/// Authority order is *stated intent first*: config, then the installer's own
/// receipt, then the shape of the path. Path heuristics come last because
/// they are the only step that can be wrong - a binary copied from
/// `~/.local/bin` to `/usr/local/bin` by hand looks like a system package and
/// is not one.
pub fn detect(f: &Facts) -> Source {
    if let Some(s) = f.configured {
        return s;
    }
    if let Some(r) = &f.receipt {
        // The receipt only speaks for a binary still sitting where the
        // installer put it. Otherwise it is describing a different file.
        if r.dir.as_os_str().is_empty() || same_dir(f.exe.parent(), &r.dir) {
            return r.source;
        }
    }
    source_from_path(&f.exe, f.cargo_origin, f.windows)
}

/// Whether the binary's directory and the receipt's directory are the same
/// place.
///
/// Not a string compare: [`probe`] canonicalizes the running binary's path
/// (it must, or Homebrew's `bin` -> `Cellar` symlink hides the one marker
/// that identifies it), while the receipt holds whatever `$GWAE_INSTALL_DIR`
/// was spelled as. On macOS `/var` is a symlink to `/private/var`, `/tmp` to
/// `/private/tmp`, and plenty of people keep `$HOME` behind a symlink, so a
/// literal comparison silently voids a receipt that is perfectly correct -
/// and silently voiding the receipt means falling back to the path guessing
/// the receipt exists to replace.
fn same_dir(exe_dir: Option<&Path>, receipt_dir: &Path) -> bool {
    let Some(exe_dir) = exe_dir else {
        return false;
    };
    if exe_dir == receipt_dir {
        return true;
    }
    match (
        std::fs::canonicalize(exe_dir),
        std::fs::canonicalize(receipt_dir),
    ) {
        (Ok(a), Ok(b)) => a == b,
        // A receipt naming a directory that no longer exists cannot be
        // describing the binary we are running out of.
        _ => false,
    }
}

/// The path heuristics, split out so their exact edges are pinned by tests.
fn source_from_path(exe: &Path, cargo: Option<CargoOrigin>, windows: bool) -> Source {
    let p = exe.to_string_lossy().replace('\\', "/");
    // Nix first: a store path can *contain* any of the other markers.
    if p.starts_with("/nix/store/") {
        return Source::Nix;
    }
    // A cargo bin directory is unambiguous about *who* installed it; only
    // *which cargo route* needs the manifest.
    if p.contains("/.cargo/bin/") || p.contains("/.rustup/") {
        return match cargo {
            Some(CargoOrigin::Git) => Source::CargoGit,
            Some(CargoOrigin::Path) => Source::Source_,
            Some(CargoOrigin::Registry) => Source::Cargo,
            // In a cargo bin dir with nothing in `.crates.toml` to explain
            // it: crates.io is the overwhelmingly common route and its
            // upgrade command is also the least surprising thing to be told.
            None => Source::Cargo,
        };
    }
    // Homebrew, both prefixes, plus the Cellar the symlink points into.
    if p.starts_with("/opt/homebrew/") || p.contains("/Cellar/") || p.contains("/linuxbrew/") {
        return Source::Homebrew;
    }
    // A build tree: `target/release/gwae` is someone running their own
    // checkout, and telling them to `brew upgrade` would be absurd.
    if p.contains("/target/release/") || p.contains("/target/debug/") {
        return Source::Source_;
    }
    if windows {
        return Source::Windows;
    }
    // `/usr/local/bin` is shared ground: Homebrew on Intel macOS, hand-built
    // software on Linux, and some distro packages. It is claimed by the brew
    // branch above only when the prefix says so, and otherwise treated as
    // owned by *something else*, which is the safe reading.
    if p.starts_with("/usr/bin/") || p.starts_with("/bin/") || p.starts_with("/usr/local/bin/") {
        return Source::System;
    }
    Source::Unknown
}

/// Read a cargo origin out of `.crates.toml` text for the `gwae` package.
///
/// The file is TOML but its keys are `"name version (source)"` strings, so
/// this reads the key rather than the value.
pub fn cargo_origin(crates_toml: &str) -> Option<CargoOrigin> {
    for line in crates_toml.lines() {
        let line = line.trim();
        if !line.starts_with("\"gwae ") {
            continue;
        }
        if line.contains("(git+") {
            return Some(CargoOrigin::Git);
        }
        if line.contains("(path+") {
            return Some(CargoOrigin::Path);
        }
        if line.contains("(registry+") {
            return Some(CargoOrigin::Registry);
        }
    }
    None
}

/// Probe the real machine for [`Facts`].
pub fn probe(configured: Option<Source>) -> Facts {
    let exe = std::env::current_exe().unwrap_or_default();
    // Resolve symlinks: Homebrew installs into the Cellar and links into
    // `bin`, so the un-resolved path hides the one marker that identifies it.
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let configured = configured.or_else(|| {
        std::env::var(SOURCE_ENV)
            .ok()
            .and_then(|v| Source::parse(&v))
    });
    Facts {
        cargo_origin: cargo_crates_toml().as_deref().and_then(cargo_origin),
        exe,
        configured,
        receipt: Receipt::load(),
        windows: cfg!(windows),
    }
}

/// `~/.cargo/.crates.toml`, if it exists.
fn cargo_crates_toml() -> Option<String> {
    let base = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    std::fs::read_to_string(base.join(".crates.toml")).ok()
}

// ---------------------------------------------------------------------------
// What upgrading would take
// ---------------------------------------------------------------------------

/// The upgrade route for a given [`Source`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Re-run `scripts/install.sh` into the directory this binary lives in.
    /// The installer already owns download, checksum, and atomic install, so
    /// upgrading is deliberately the same code path as installing.
    Script { dir: PathBuf },
    /// `brew upgrade gwae`.
    Brew,
    /// `cargo install gwae --locked --force`.
    Cargo,
    /// `cargo install --git ... gwae --locked --force`.
    CargoGit,
    /// Someone else owns this file. We print the command *they* should run
    /// and touch nothing: a package manager whose files change underneath it
    /// is a broken package manager.
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
    pub fn commands(&self) -> Vec<(String, Vec<String>)> {
        match self {
            Plan::Script { dir } => {
                let url =
                    format!("https://raw.githubusercontent.com/{REPO}/main/scripts/install.sh");
                // Piping the installer to bash is exactly what the user did
                // to get here, and it keeps checksum verification in one
                // place. `GWAE_INSTALL_DIR` pins the destination so an
                // upgrade cannot silently relocate the binary.
                let script = format!(
                    "curl -fsSL {url} | GWAE_INSTALL_DIR={} bash",
                    shell_quote(&dir.to_string_lossy())
                );
                vec![("/bin/bash".into(), vec!["-c".into(), script])]
            }
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
            Plan::Script { dir } => {
                format!("re-run the installer into {}", dir.display())
            }
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
/// The `Managed` arms are the point of this whole module: three of the ways
/// gwae is installed are owned by something that would be actively damaged by
/// us writing over the file, so those turn into instructions rather than
/// actions.
pub fn plan(source: Source, exe: &Path) -> Plan {
    match source {
        Source::Script => Plan::Script {
            dir: exe
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        },
        Source::Homebrew => Plan::Brew,
        Source::Cargo => Plan::Cargo,
        Source::CargoGit => Plan::CargoGit,
        Source::Source_ => Plan::Managed {
            how: "you built this from a checkout: `git pull && make install`",
        },
        Source::Nix => Plan::Managed {
            how: "Nix owns this store path: `nix flake update` in your flake, \
                  or `nix profile upgrade gwae`",
        },
        Source::System => Plan::Managed {
            how: "your package manager owns this file: e.g. `paru -Syu gwae-bin`, \
                  or reinstall via install.sh",
        },
        Source::Windows => Plan::Managed {
            how: "download gwae-x86_64-pc-windows-msvc.zip from \
                  https://github.com/hongnoul/gwae/releases/latest and replace gwae.exe",
        },
        Source::Unknown => Plan::Ask,
    }
}

/// Minimal POSIX shell quoting for a path interpolated into `bash -c`.
///
/// Only ever applied to a directory *we* resolved from `current_exe`, but a
/// path with a space in it is ordinary on macOS and would otherwise split the
/// assignment into a command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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

/// `gwae upgrade`. Returns the process exit code.
///
/// `check_only` reports and stops. Otherwise the plan is printed *before* it
/// runs and, unless `assume_yes`, confirmed. There is no silent path: every
/// route out of this function has told the user what it did.
pub fn run_upgrade(configured: Option<Source>, check_only: bool, assume_yes: bool) -> i32 {
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
    if check_only {
        return 0;
    }
    if !assume_yes && !confirm("Proceed? [y/N] ") {
        println!("nothing done.");
        return 0;
    }

    for (prog, args) in &cmds {
        let status = std::process::Command::new(prog).args(args).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("gwae: `{prog}` exited with {s}; nothing else was run.");
                return 1;
            }
            Err(e) => {
                eprintln!("gwae: could not run `{prog}`: {e}");
                return 1;
            }
        }
    }
    println!("upgraded. Run `gwae --version` to confirm.");
    0
}

/// How the source was decided, appended to the `source:` line so the user can
/// tell a fact from a guess.
fn provenance(f: &Facts) -> String {
    if f.configured.is_some() {
        return " (from config)".to_string();
    }
    match &f.receipt {
        Some(r) if r.dir.as_os_str().is_empty() || same_dir(f.exe.parent(), &r.dir) => {
            " (from install receipt)".to_string()
        }
        _ => " (detected from path)".to_string(),
    }
}

/// A y/N prompt on stdin. `false` when stdin is not a terminal, so a piped
/// `gwae upgrade` reports the plan and stops rather than acting on a
/// confirmation nobody gave.
fn confirm(prompt: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

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
mod tests {
    use super::*;

    fn facts(exe: &str) -> Facts {
        Facts {
            exe: PathBuf::from(exe),
            ..Default::default()
        }
    }

    #[test]
    fn homebrew_is_detected_through_the_cellar_symlink() {
        // `brew` links `bin/gwae` into the Cellar, and `probe` canonicalizes,
        // so this is the path detection actually sees.
        let f = facts("/opt/homebrew/Cellar/gwae/1.0.1/bin/gwae");
        assert_eq!(detect(&f), Source::Homebrew);
        assert_eq!(detect(&facts("/opt/homebrew/bin/gwae")), Source::Homebrew);
        assert_eq!(
            detect(&facts("/home/linuxbrew/.linuxbrew/bin/gwae")),
            Source::Homebrew
        );
    }

    #[test]
    fn nix_store_paths_are_never_something_we_write_to() {
        let f = facts("/nix/store/abc123-gwae-1.0.1/bin/gwae");
        assert_eq!(detect(&f), Source::Nix);
        assert!(plan(Source::Nix, &f.exe).commands().is_empty());
    }

    #[test]
    fn cargo_bin_splits_by_what_the_manifest_says() {
        let mut f = facts("/Users/x/.cargo/bin/gwae");
        assert_eq!(detect(&f), Source::Cargo);
        f.cargo_origin = Some(CargoOrigin::Git);
        assert_eq!(detect(&f), Source::CargoGit);
        f.cargo_origin = Some(CargoOrigin::Path);
        assert_eq!(detect(&f), Source::Source_);
    }

    #[test]
    fn a_system_prefix_is_treated_as_someone_elses_file() {
        assert_eq!(detect(&facts("/usr/bin/gwae")), Source::System);
        assert_eq!(detect(&facts("/usr/local/bin/gwae")), Source::System);
        assert!(plan(Source::System, Path::new("/usr/bin/gwae"))
            .commands()
            .is_empty());
    }

    #[test]
    fn a_build_tree_is_a_checkout_not_a_package() {
        assert_eq!(
            detect(&facts("/Users/x/git/gwae/target/release/gwae")),
            Source::Source_
        );
    }

    #[test]
    fn an_unrecognized_path_is_admitted_as_unknown() {
        assert_eq!(detect(&facts("/Users/x/.local/bin/gwae")), Source::Unknown);
        assert_eq!(plan(Source::Unknown, Path::new("/x")), Plan::Ask);
    }

    #[test]
    #[cfg(unix)]
    fn a_receipt_survives_a_symlinked_install_dir() {
        // Regression: `probe` canonicalizes the exe path (it must, or
        // Homebrew's Cellar symlink is invisible) while the receipt holds the
        // path as the installer spelled it. On macOS `/var` *is* a symlink to
        // `/private/var`, so a literal comparison threw away a correct
        // receipt for every install under a symlinked directory - and the
        // fallback is exactly the path guessing the receipt exists to
        // replace. Found by installing into a temp dir, not by a unit test,
        // so this one uses the real filesystem.
        let root =
            std::env::temp_dir().join(format!("gwae-symlink-receipt-{}", std::process::id()));
        let real = root.join("real/bin");
        let link = root.join("link");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&real).expect("real dir");
        std::os::unix::fs::symlink(root.join("real"), &link).expect("symlink");

        let mut f = facts(&link.join("bin/gwae").to_string_lossy());
        f.receipt = Some(Receipt {
            source: Source::Script,
            // The receipt names the resolved path; the exe came in via the
            // symlink. Same directory, spelled two ways.
            dir: real.clone(),
            version: "1.0.1".into(),
        });
        assert_eq!(
            detect(&f),
            Source::Script,
            "two spellings of one directory must not void the receipt"
        );
        assert!(provenance(&f).contains("receipt"), "{}", provenance(&f));

        // And the guarantee still holds in the other direction: a receipt for
        // a genuinely different directory must not speak for this binary.
        let mut elsewhere = f.clone();
        elsewhere.receipt = Some(Receipt {
            source: Source::Script,
            dir: root.join("real"),
            version: "1.0.1".into(),
        });
        assert_ne!(detect(&elsewhere), Source::Script);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_receipt_beats_the_path_but_only_where_it_points() {
        let receipt = Receipt {
            source: Source::Script,
            dir: PathBuf::from("/Users/x/.local/bin"),
            version: "1.0.1".into(),
        };
        let mut f = facts("/Users/x/.local/bin/gwae");
        f.receipt = Some(receipt.clone());
        assert_eq!(detect(&f), Source::Script);

        // Same receipt, binary somewhere else: the receipt describes a
        // different file and must not speak for this one.
        let mut moved = facts("/opt/homebrew/bin/gwae");
        moved.receipt = Some(receipt);
        assert_eq!(detect(&moved), Source::Homebrew);
    }

    #[test]
    fn config_beats_everything_including_a_receipt() {
        let mut f = facts("/nix/store/abc-gwae/bin/gwae");
        f.receipt = Some(Receipt {
            source: Source::Script,
            dir: PathBuf::from("/nix/store/abc-gwae/bin"),
            version: "1.0.1".into(),
        });
        f.configured = Some(Source::Homebrew);
        assert_eq!(detect(&f), Source::Homebrew);
    }

    #[test]
    fn the_installer_receipt_round_trips() {
        let text = "source = \"install.sh\"\ndir = \"/home/u/.local/bin\"\nversion = \"1.0.1\"\n";
        let r = Receipt::parse(text).expect("parses");
        assert_eq!(r.source, Source::Script);
        assert_eq!(r.dir, PathBuf::from("/home/u/.local/bin"));
        assert_eq!(r.version, "1.0.1");
        assert!(Receipt::parse("not = [toml").is_none());
        assert!(Receipt::parse("source = \"martians\"").is_none());
    }

    #[test]
    fn source_names_parse_back_to_themselves() {
        for name in Source::NAMES {
            let s = Source::parse(name).expect("known name parses");
            assert_eq!(s.as_str(), *name, "{name} should round trip");
        }
        assert_eq!(Source::parse("Homebrew"), Some(Source::Homebrew));
        assert_eq!(Source::parse("AUR"), Some(Source::System));
        assert_eq!(Source::parse("nonsense"), None);
    }

    #[test]
    fn the_script_plan_pins_the_install_dir_and_quotes_it() {
        let p = plan(Source::Script, Path::new("/Users/my name/.local/bin/gwae"));
        let cmds = p.commands();
        let script = &cmds[0].1[1];
        assert!(
            script.contains("GWAE_INSTALL_DIR='/Users/my name/.local/bin'"),
            "install dir must survive a space: {script}"
        );
        assert!(script.contains("scripts/install.sh"));
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

    #[test]
    fn the_notice_always_ends_in_a_command_for_this_machine() {
        let n = notice("1.0.1", "1.0.2", &Plan::Brew);
        assert!(n.contains("1.0.2 is out"), "{n}");
        assert!(n.contains("brew upgrade gwae"), "{n}");
        // Routes we drive ourselves point at the subcommand, not at a raw
        // curl-to-bash the user would have to trust twice.
        let n = notice("1.0.1", "1.0.2", &Plan::Script { dir: "/b".into() });
        assert!(n.ends_with("run: gwae upgrade"), "{n}");
    }

    #[test]
    fn cargo_origin_reads_the_crates_manifest_key() {
        let registry = "[v1]\n\"gwae 1.0.1 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"gwae\"]\n";
        assert_eq!(cargo_origin(registry), Some(CargoOrigin::Registry));
        let git = "[v1]\n\"gwae 1.0.1 (git+https://github.com/hongnoul/gwae#abc)\" = [\"gwae\"]\n";
        assert_eq!(cargo_origin(git), Some(CargoOrigin::Git));
        let path =
            "[v1]\n\"gwae 1.0.1 (path+file:///Users/x/git/gwae/crates/gwae)\" = [\"gwae\"]\n";
        assert_eq!(cargo_origin(path), Some(CargoOrigin::Path));
        // Another crate's entry must not answer for gwae.
        let other = "[v1]\n\"ripgrep 14.0.0 (git+https://github.com/x/y)\" = [\"rg\"]\n";
        assert_eq!(cargo_origin(other), None);
    }

    #[test]
    fn every_managed_source_refuses_to_run_anything() {
        for s in [
            Source::Nix,
            Source::System,
            Source::Windows,
            Source::Source_,
        ] {
            let p = plan(s, Path::new("/usr/bin/gwae"));
            assert!(
                p.commands().is_empty(),
                "{s:?} must never be driven by gwae"
            );
            assert!(!p.describe().is_empty(), "{s:?} must still say what to do");
        }
    }
}
