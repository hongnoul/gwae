//! Install-source detection: where did this binary come from?

use super::SOURCE_ENV;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Where this binary came from
// ---------------------------------------------------------------------------

/// How gwae got onto this machine, which decides how it may leave.
///
/// gwae runs on macOS, Linux, and Windows. The curl installer
/// (`scripts/install.sh`, served from the site) is the primary route on
/// macOS and Linux; the PowerShell installer (`scripts/install.ps1`) covers
/// Windows; Homebrew stays supported on macOS. The other variants exist so a
/// binary installed any other way is still told the truth about its own
/// route instead of being guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `scripts/install.sh` (or `install.ps1` on Windows) put a release
    /// binary in a plain directory. We own that file outright, so re-running
    /// the installer is the upgrade. The primary route.
    Script,
    /// Homebrew (the tap). `brew upgrade gwae`. Supported on macOS.
    Homebrew,
    /// `cargo install gwae` from crates.io. Legacy: new installs should use
    /// Homebrew.
    Cargo,
    /// `cargo install --git https://github.com/hongnoul/gwae gwae`.
    /// Legacy: new installs should use Homebrew.
    CargoGit,
    /// Built in a checkout and installed by `make install` / `cargo install
    /// --path`. Upgrading means pulling and rebuilding, which is the user's
    /// call, not ours.
    Source_,
    /// A Nix store path. Immutable by design; the flake input is what moves.
    /// Legacy: new installs should use Homebrew.
    Nix,
    /// A distro package manager owns this file (`/usr/bin`, `/usr/local/bin`
    /// on Linux). AUR, apt, whatever it is: it is not ours to overwrite.
    /// Legacy: new installs should use Homebrew.
    System,
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
            Source::Unknown => "unknown",
        }
    }

    /// Parse a config / env / receipt spelling. Tolerant of the obvious
    /// synonyms, because "homebrew" and "brew" are the same answer and
    /// bouncing one of them would be pedantry.
    pub fn parse(s: &str) -> Option<Source> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "install.sh" | "install-sh" | "install.ps1" | "install-ps1" | "script"
            | "installer" => Some(Source::Script),
            "brew" | "homebrew" => Some(Source::Homebrew),
            "cargo" | "crates.io" | "crates-io" => Some(Source::Cargo),
            "cargo-git" | "git" => Some(Source::CargoGit),
            "source" | "make" | "checkout" | "path" => Some(Source::Source_),
            "nix" => Some(Source::Nix),
            "system" | "apt" | "aur" | "pacman" | "dnf" | "distro" => Some(Source::System),
            // These spellings once pinned Windows-specific routes that no
            // longer exist. Fall through to detection, which will say what
            // it sees.
            "windows" | "winget" | "scoop" | "zip" => Some(Source::Unknown),
            "unknown" | "auto" | "" => Some(Source::Unknown),
            _ => None,
        }
    }

    /// Every name a user may write, for error messages.
    /// The installer script leads: it is the primary route on every OS.
    /// Brew follows for macOS. The rest are legacy routes detection still
    /// understands.
    pub const NAMES: &'static [&'static str] = &[
        "install.sh",
        "brew",
        "cargo",
        "cargo-git",
        "source",
        "nix",
        "system",
        "unknown",
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
    /// (Legacy: new installs should use Homebrew.)
    pub cargo_origin: Option<CargoOrigin>,
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

/// The note `scripts/install.sh` leaves, so the source is *known* rather than
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
    // Windows: `install.ps1` writes its receipt under `%LOCALAPPDATA%\gwae\state`.
    #[cfg(windows)]
    if let Some(x) = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(x).join("gwae").join("state"));
    }
    Some(crate::config::home_dir()?.join(".local/state/gwae"))
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
    source_from_path(&f.exe, f.cargo_origin)
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
pub fn same_dir(exe_dir: Option<&Path>, receipt_dir: &Path) -> bool {
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
///
/// macOS-only tree: `/opt/homebrew` and the Cellar are the canonical
/// markers. Linuxbrew and Linux system prefixes are kept as legacy
/// readings so old installs still get a truthful answer.
fn source_from_path(exe: &Path, cargo: Option<CargoOrigin>) -> Source {
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
    }
}

/// `~/.cargo/.crates.toml`, if it exists.
fn cargo_crates_toml() -> Option<String> {
    let base = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    std::fs::read_to_string(base.join(".crates.toml")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::check::{ignored_receipt, provenance};
    use crate::update::plan::{plan, Plan};
    use std::path::{Path, PathBuf};

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
        // ...but the reason must be visible, not "detected from path" as if
        // there were nothing else to say. This is the user-visible half of
        // the stale-receipt guarantee: ignoring must be loud.
        assert!(
            provenance(&moved).contains("/Users/x/.local/bin"),
            "ignored receipt must be named: {}",
            provenance(&moved)
        );
        assert!(
            ignored_receipt(&moved).is_some(),
            "the Ask branch must have a note to print"
        );
        assert!(
            ignored_receipt(&f).is_none(),
            "an applying receipt is not an ignored one"
        );
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
    fn windows_spellings_no_longer_pin_a_route() {
        // Windows is sunset: old configs spelling it must not pin a dead
        // route. They resolve to Unknown so detection runs instead.
        for name in ["windows", "winget", "scoop", "zip"] {
            assert_eq!(Source::parse(name), Some(Source::Unknown), "{name}");
        }
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
}
