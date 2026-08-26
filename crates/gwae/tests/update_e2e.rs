//! `gwae upgrade` must never surprise the machine it runs on.
//!
//! The unit tests in `update.rs` pin the pure decisions (which source, which
//! command). These run the *real binary* against real config files and real
//! state directories, because the properties that matter here are properties
//! of the shipped program:
//!
//! * a check-only run never executes an upgrade command;
//! * a binary another package manager owns is never written to;
//! * `doctor` tells the truth about which route is in effect;
//! * a config that pins the wrong source is reported, not silently obeyed.
//!
//! Every case pins `XDG_CONFIG_HOME` and `XDG_STATE_HOME` at a temp dir, so a
//! test can never read or write the developer's own gwae state, and sets
//! `GWAE_NO_UPDATE_CHECK` where the network is not the thing under test.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A private config + state directory for one case.
fn sandbox() -> PathBuf {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-update-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("config/gwae")).expect("config dir");
    std::fs::create_dir_all(dir.join("state/gwae")).expect("state dir");
    dir
}

/// Run gwae with `args` in a sandbox, returning `(stdout, stderr, code)`.
fn run(dir: &Path, config: Option<&str>, args: &[&str]) -> (String, String, i32) {
    if let Some(body) = config {
        std::fs::write(dir.join("config/gwae/gwae.toml"), body).expect("write config");
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"))
        .args(args)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .output()
        .expect("run gwae");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn doctor_reports_how_this_binary_would_upgrade() {
    let dir = sandbox();
    let (out, _, code) = run(&dir, None, &["doctor"]);
    assert_eq!(code, 0, "doctor should exit cleanly");
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("updates:"))
        .unwrap_or_else(|| panic!("doctor has no updates line:\n{out}"));
    // The test binary lives under target/, so detection must call it a
    // checkout - and must therefore refuse to drive the upgrade itself.
    assert!(line.contains("source"), "{line}");
    assert!(
        line.contains("git pull") || line.contains("make install"),
        "a binary built in a checkout must be told to rebuild, got: {line}"
    );
    assert!(
        line.contains("`gwae upgrade` ->"),
        "the route must be named, got: {line}"
    );
}

#[test]
fn doctor_says_when_the_check_is_switched_off() {
    let dir = sandbox();
    let (on, _, _) = run(&dir, Some("[update]\ncheck = true\n"), &["doctor"]);
    assert!(on.contains("checks daily"), "{on}");
    let (off, _, _) = run(&dir, Some("[update]\ncheck = false\n"), &["doctor"]);
    assert!(off.contains("check off"), "{off}");
    assert!(!off.contains("checks daily"), "{off}");
}

#[test]
fn a_pinned_source_is_used_and_shown_as_a_fact_not_a_guess() {
    let dir = sandbox();
    let (out, _, _) = run(&dir, Some("[update]\nsource = \"brew\"\n"), &["doctor"]);
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("updates:"))
        .expect("updates line");
    assert!(line.contains("brew (from config)"), "{line}");
    assert!(line.contains("brew upgrade gwae"), "{line}");
    assert!(
        !line.contains("detected from path"),
        "config must not be described as detection: {line}"
    );
}

#[test]
fn a_misspelled_source_is_reported_rather_than_silently_obeyed() {
    // The whole risk of this feature is running the wrong upgrade command, so
    // a source we do not understand must be loud, not quietly dropped.
    let dir = sandbox();
    let (out, _, code) = run(&dir, Some("[update]\nsource = \"homebrü\"\n"), &["doctor"]);
    assert_eq!(code, 0, "a bad key must not break doctor");
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("updates:"))
        .expect("updates line");
    assert!(line.contains("INVALID"), "{line}");
    assert!(
        line.contains("brew"),
        "the valid names must be listed: {line}"
    );
}

#[test]
fn an_install_receipt_decides_the_route_over_the_path() {
    // This is the case the receipt exists for: a binary in a directory whose
    // path says nothing (or says the wrong thing), where the installer knows
    // the answer for certain.
    let dir = sandbox();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_gwae"));
    let bin_dir = exe.parent().expect("bin dir");
    std::fs::write(
        dir.join("state/gwae/install.toml"),
        format!(
            "source = \"install.sh\"\ndir = {:?}\nversion = \"0.0.1\"\n",
            bin_dir
        ),
    )
    .expect("write receipt");
    let (out, _, _) = run(&dir, None, &["doctor"]);
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("updates:"))
        .expect("updates line");
    assert!(
        line.contains("install.sh (from install receipt)"),
        "the receipt must beat the target/ path heuristic: {line}"
    );
    assert!(line.contains("re-run the installer"), "{line}");
}

#[test]
fn a_receipt_pointing_somewhere_else_does_not_speak_for_this_binary() {
    // A receipt describes the file the installer wrote. If the running
    // binary is not that file, obeying the receipt would send the user's
    // upgrade at a completely different install.
    let dir = sandbox();
    std::fs::write(
        dir.join("state/gwae/install.toml"),
        "source = \"install.sh\"\ndir = \"/somewhere/else/bin\"\nversion = \"0.0.1\"\n",
    )
    .expect("write receipt");
    let (out, _, _) = run(&dir, None, &["doctor"]);
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("updates:"))
        .expect("updates line");
    assert!(
        line.contains("detected from path"),
        "a stale receipt must be ignored, got: {line}"
    );
}

#[test]
fn upgrade_check_never_runs_an_upgrade_command() {
    // `--check` is the mode people put in a shell prompt or a cron. It must
    // be observably read-only. The binary under test is a checkout build, so
    // the route is a Managed one that cannot run anything at all - and the
    // run must still succeed and explain itself.
    let dir = sandbox();
    let (out, err, code) = run(
        &dir,
        Some("[update]\nsource = \"nix\"\n"),
        &["upgrade", "--check"],
    );
    assert!(
        code == 0 || code == 1,
        "check exits cleanly or reports it could not reach github, got {code}: {err}"
    );
    assert!(out.contains("source:  nix"), "{out}");
    assert!(
        out.contains("nix flake update") || out.contains("nix profile upgrade"),
        "a Nix install must be handed to Nix: {out}"
    );
    // Nothing that looks like an install command was executed: the only
    // mention of one is inside the printed instructions.
    assert!(
        !out.contains("upgraded."),
        "check mode must never report having upgraded: {out}"
    );
}

#[test]
fn upgrade_refuses_to_guess_when_it_cannot_tell() {
    // A wrong guess here runs a package-manager command against a machine
    // that never installed gwae that way. Refusing, and naming the config key
    // that fixes it, is the only safe answer.
    let dir = sandbox();
    let (out, _, code) = run(
        &dir,
        Some("[update]\nsource = \"unknown\"\n"),
        &["upgrade", "--check"],
    );
    assert_eq!(code, 1, "an unresolvable route is a failure, not a no-op");
    assert!(
        out.contains("will not guess") || out.contains("latest:  unknown"),
        "{out}"
    );
}

#[test]
fn the_env_kill_switch_is_visible_in_doctor() {
    let dir = sandbox();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"))
        .arg("doctor")
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("GWAE_NO_UPDATE_CHECK", "1")
        .output()
        .expect("run gwae doctor");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("check off (env)"),
        "the kill switch must be visible, not silent:\n{text}"
    );
}

#[test]
fn update_is_accepted_as_an_alias_for_upgrade() {
    // `update` is what half of all users will type first; a "no such
    // subcommand" error for the feature's own name would be a bad joke.
    let dir = sandbox();
    let (out, _, _) = run(
        &dir,
        Some("[update]\nsource = \"nix\"\n"),
        &["update", "--check"],
    );
    assert!(out.contains("source:  nix"), "{out}");
}

/// Run gwae with a pretend newer release and a `PATH` whose package managers
/// are stubs that announce themselves.
///
/// This is the only way to exercise the branch that actually *executes*
/// something, since a real newer release cannot be conjured on demand - and
/// that branch is precisely the one that can change a user's machine, so
/// leaving it untested would be leaving the risky half unverified.
fn run_with_fake_release(
    dir: &Path,
    config: &str,
    latest: &str,
    args: &[&str],
) -> (String, String, i32) {
    let fakebin = dir.join("fakebin");
    std::fs::create_dir_all(&fakebin).expect("fakebin");
    for tool in ["brew", "cargo"] {
        let p = fakebin.join(tool);
        std::fs::write(&p, format!("#!/bin/sh\necho RAN-{tool} \"$@\"\n")).expect("stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    std::fs::write(dir.join("config/gwae/gwae.toml"), config).expect("write config");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"))
        .args(args)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("GWAE_UPDATE_LATEST", latest)
        .env("PATH", format!("{}:/usr/bin:/bin", fakebin.display()))
        .output()
        .expect("run gwae");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn with_a_newer_release_check_mode_still_runs_nothing() {
    // The dangerous combination: an upgrade genuinely is available *and* the
    // package manager is right there on PATH. `--check` must still only talk.
    let dir = sandbox();
    let (out, _, code) = run_with_fake_release(
        &dir,
        "[update]\nsource = \"brew\"\n",
        "99.0.0",
        &["upgrade", "--check"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("99.0.0"), "the new version is reported: {out}");
    assert!(out.contains("brew upgrade gwae"), "{out}");
    assert!(
        !out.contains("RAN-brew"),
        "check mode executed the upgrade: {out}"
    );
    assert!(!out.contains("upgraded."), "{out}");
}

#[test]
fn with_a_newer_release_an_approved_upgrade_runs_exactly_the_printed_command() {
    // The whole contract in one test: what gwae printed is what gwae ran.
    let dir = sandbox();
    let (out, err, code) = run_with_fake_release(
        &dir,
        "[update]\nsource = \"brew\"\n",
        "99.0.0",
        &["upgrade", "-y"],
    );
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert!(
        out.contains("RAN-brew upgrade gwae"),
        "the approved upgrade must actually run: {out}"
    );
    assert!(out.contains("upgraded."), "{out}");
    // Nothing beyond the one printed command was executed.
    assert!(!out.contains("RAN-cargo"), "{out}");
}

#[test]
fn with_a_newer_release_a_managed_install_is_still_never_touched() {
    // A Nix user with brew also installed is the case where a careless
    // implementation reaches for whatever package manager it can find.
    let dir = sandbox();
    let (out, _, code) = run_with_fake_release(
        &dir,
        "[update]\nsource = \"nix\"\n",
        "99.0.0",
        &["upgrade", "-y"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        !out.contains("RAN-"),
        "a Nix install must never be upgraded by gwae: {out}"
    );
    assert!(out.contains("managed elsewhere"), "{out}");
    assert!(out.contains("nix"), "{out}");
}

#[test]
fn a_cargo_install_is_upgraded_with_locked_and_force() {
    // `--locked` keeps the build reproducible and `--force` is what makes
    // cargo replace an existing binary rather than refusing.
    let dir = sandbox();
    let (out, _, code) = run_with_fake_release(
        &dir,
        "[update]\nsource = \"cargo\"\n",
        "99.0.0",
        &["upgrade", "-y"],
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("RAN-cargo install gwae --locked --force"),
        "{out}"
    );
}

#[test]
fn the_check_writes_a_cache_so_the_next_session_stays_quiet() {
    // The daily-cadence promise is only real if the answer is persisted.
    let dir = sandbox();
    let (_, _, _) = run_with_fake_release(
        &dir,
        "[update]\nsource = \"nix\"\n",
        "99.0.0",
        &["upgrade", "--check"],
    );
    let cache = std::fs::read_to_string(dir.join("state/gwae/update.toml"))
        .expect("the check must record what it found");
    assert!(cache.contains("99.0.0"), "{cache}");
    assert!(cache.contains("last_check"), "{cache}");
    // And doctor reads it back rather than asking the network again.
    let (out, _, _) = run(&dir, None, &["doctor"]);
    assert!(
        out.contains("99.0.0 available"),
        "doctor must report the cached finding:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// The notice on a running session
// ---------------------------------------------------------------------------

/// The last unproven link: a check that finds something must actually reach
/// the user's eyes.
///
/// Everything above tests the subcommand, which only runs when someone
/// already suspects there is an update. The startup notice is the path that
/// tells the other 99% of users, and it crosses a background thread, a mutex,
/// and the toast renderer - none of which a pure test can stand in for. So
/// this drives a real gwae over a real PTY and looks for the text on the
/// wire.
#[test]
#[cfg(unix)]
fn a_running_session_shows_the_notice_and_names_the_command() {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::Read;
    use std::time::Duration;

    let dir = sandbox();
    std::fs::write(
        dir.join("config/gwae/gwae.toml"),
        "[update]\ncheck = true\nsource = \"brew\"\n",
    )
    .expect("write config");

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.env("XDG_CONFIG_HOME", dir.join("config"));
    cmd.env("XDG_STATE_HOME", dir.join("state"));
    cmd.env("TERM", "xterm-256color");
    // A release that will always be newer than whatever this build is.
    cmd.env("GWAE_UPDATE_LATEST", "99.0.0");
    cmd.arg("run");
    cmd.arg("sleep 60");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn gwae");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // Read until the notice shows up or we run out of patience. The check is
    // asynchronous by design, so this waits for content rather than for a
    // fixed delay.
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(250)) {
            seen.push_str(&String::from_utf8_lossy(&b));
        }
        if seen.contains("99.0.0") {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    // The toast is drawn into a cell grid, so the text arrives interleaved
    // with SGR sequences; strip everything but the glyphs before matching.
    let flat: String = seen
        .chars()
        .filter(|c| !c.is_control() && *c != '\u{1b}')
        .collect();
    assert!(
        flat.contains("99.0.0 is out"),
        "the update notice never reached the screen; saw:\n{flat}"
    );
    assert!(
        flat.contains("brew upgrade gwae"),
        "the notice must name this machine's command; saw:\n{flat}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
