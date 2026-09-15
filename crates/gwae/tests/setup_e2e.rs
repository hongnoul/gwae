//! `gwae setup` must audit without writing, and write only what it owns.
//!
//! These tests run the real binary against real (temp) config files: the
//! safety boundary (our TOML yes, kitty.conf and macOS globals never) is
//! exactly what needs a real process to believe.

use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// A sandbox config dir, returned with the dir so tests can inspect files.
fn sandbox(config_body: Option<&str>) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gwae-setup-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    if let Some(body) = config_body {
        std::fs::write(dir.join("gwae/gwae.toml"), body).expect("write config");
    }
    dir
}

fn setup(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.arg("setup").args(args).env("XDG_CONFIG_HOME", dir);
    // Never install software from a test, and never hold the Mac awake.
    cmd.env("GWAE_NO_INSTALL", "1");
    cmd.env("GWAE_NO_KEEP_AWAKE", "1");
    // Deterministic PATH: no agent harnesses, no swiftc.
    cmd.env("PATH", "/bin:/usr/bin");
    let out = cmd.output().expect("run gwae setup");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn print_lists_every_stage_without_writing() {
    let dir = sandbox(Some("theme = \"nord\"\n"));
    let before = std::fs::read_to_string(dir.join("gwae/gwae.toml")).unwrap();
    let (out, _, code) = setup(&dir, &["--print"]);
    assert_eq!(code, 0, "print should exit cleanly");
    for id in [
        "config file",
        "theme",
        "agent",
        "updates",
        "spawn dir",
        "latency",
    ] {
        assert!(out.contains(id), "print missing stage {id}:\n{out}");
    }
    let after = std::fs::read_to_string(dir.join("gwae/gwae.toml")).unwrap();
    assert_eq!(before, after, "print must not write");
}

#[test]
fn check_writes_nothing_and_reports_latency() {
    // input_poll_ms 10 is suboptimal on every platform, so --check must
    // fail with no writes even where macOS/kitty settings do not apply.
    let dir = sandbox(Some("input_poll_ms = 10\n"));
    let (out, _, code) = setup(&dir, &["--check"]);
    assert_ne!(code, 0, "check should fail on untuned latency");
    assert!(out.contains("latency"), "check should name the stage:\n{out}");
    let after = std::fs::read_to_string(dir.join("gwae/gwae.toml")).unwrap();
    assert!(after.contains("input_poll_ms = 10"), "check must not write:\n{after}");
}

#[test]
fn unknown_stage_is_an_error() {
    let dir = sandbox(None);
    let (_, err, code) = setup(&dir, &["--only", "no-such-stage"]);
    assert_eq!(code, 2, "unknown stage should exit 2, got {code}: {err}");
}

#[test]
fn only_latency_scopes_the_audit() {
    let dir = sandbox(Some("input_poll_ms = 10\n"));
    let (out, _, code) = setup(&dir, &["--check", "--only", "latency"]);
    assert_ne!(code, 0, "scoped check should still fail");
    assert!(out.contains("latency"), "scoped check names its stage:\n{out}");
    assert!(!out.contains("keep-awake"), "scoped check stays scoped:\n{out}");
}

#[test]
fn skip_env_disables_all_writes() {
    let dir = sandbox(Some("input_poll_ms = 10\n"));
    let (out, _, code) = setup(&dir, &["--yes"]);
    assert_eq!(code, 0, "skip env should exit cleanly");
    assert!(out.contains("GWAE_NO_INSTALL"), "should say why:\n{out}");
    let after = std::fs::read_to_string(dir.join("gwae/gwae.toml")).unwrap();
    assert!(after.contains("input_poll_ms = 10"), "no writes under skip env:\n{after}");
}

#[test]
fn doctor_contains_one_line_per_stage() {
    let dir = sandbox(None);
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.arg("doctor").env("XDG_CONFIG_HOME", &dir);
    cmd.env("GWAE_NO_KEEP_AWAKE", "1");
    cmd.env("PATH", "/bin:/usr/bin");
    let out = cmd.output().expect("run gwae doctor");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    for id in ["config file:", "theme:", "agent:", "latency:"] {
        assert!(text.contains(id), "doctor missing {id}:\n{text}");
    }
}
