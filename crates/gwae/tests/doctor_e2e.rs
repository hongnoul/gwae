//! `gwae doctor` must tell the user the truth about their config.
//!
//! Both failure modes here are silent at startup by design: a malformed config
//! file is discarded and an unknown theme name falls back to the default, each
//! with only a `tracing` warning that scrolls past (or never appears) before
//! the alternate screen takes over. `doctor` is the one place a user can find
//! out, so these tests run the real binary against real config files.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Run `gwae doctor` with `config_body` as the config file (or no file at
/// all when `None`) and return its stdout.
fn doctor(config_body: Option<&str>) -> String {
    doctor_with_agents(config_body, &[])
}

/// As [`doctor`], but with `agents` planted as stub executables on a pinned
/// PATH. The agent line's wording depends on what is installed, so a case
/// asserting on it must control PATH rather than inherit the machine's: a
/// developer laptop has `claude` on PATH and a CI runner has nothing, and
/// `plan` answers differently for each.
fn doctor_with_agents(config_body: Option<&str>, agents: &[&str]) -> String {
    // Unique per case: these tests are threads of one process, so a shared
    // directory would let cases clobber each other's config.
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "gwae-doctor-e2e-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    if let Some(body) = config_body {
        std::fs::write(dir.join("gwae/gwae.toml"), body).expect("write config");
    }
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.arg("doctor").env("XDG_CONFIG_HOME", &dir);
    if !agents.is_empty() {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        for a in agents {
            let p = bin.join(a);
            std::fs::write(&p, "#!/bin/sh\nexit 0\n").expect("stub");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod stub");
            }
        }
        cmd.env("PATH", format!("{}:/bin:/usr/bin", bin.display()));
    }
    let out = cmd.output().expect("run gwae doctor");
    assert!(out.status.success(), "doctor should exit cleanly");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn reports_the_default_theme_when_no_config_exists() {
    let out = doctor(None);
    assert!(
        out.contains("not present"),
        "should say there is no config file; got:\n{out}"
    );
    assert!(
        out.contains("catppuccin-mocha"),
        "should name the default theme; got:\n{out}"
    );
}

#[test]
fn reports_a_configured_theme_by_name() {
    let out = doctor(Some("theme = \"nord\"\n"));
    assert!(
        out.contains("theme: nord"),
        "should report the configured theme; got:\n{out}"
    );
    assert!(
        out.contains("parses [ok]"),
        "a valid config should be reported as parsing; got:\n{out}"
    );
}

#[test]
fn flags_an_unknown_theme_and_lists_the_real_ones() {
    // Silently falling back leaves the user staring at default colors with no
    // idea their theme name was wrong (a typo like "tokyonight-storm").
    let out = doctor(Some("theme = \"tokyonight-storm\"\n"));
    assert!(
        out.contains("UNKNOWN"),
        "an unknown theme must be called out; got:\n{out}"
    );
    assert!(
        out.contains("tokyonight-storm"),
        "the offending name should be echoed back; got:\n{out}"
    );
    for name in ["catppuccin-mocha", "tokyo-night", "nord", "terminal"] {
        assert!(
            out.contains(name),
            "the available themes should be listed (missing {name}); got:\n{out}"
        );
    }
}

#[test]
fn flags_a_config_file_that_is_being_ignored() {
    // gwae discards an unparseable config wholesale, so every setting in it
    // is silently inert. doctor must say so, and point at the syntax error.
    let out = doctor(Some("theme = \"nord\"\nthis is not valid toml <<<\n"));
    assert!(
        out.contains("INVALID"),
        "a broken config must be reported as ignored; got:\n{out}"
    );
    assert!(
        out.contains("line 2"),
        "the parse error should locate the problem; got:\n{out}"
    );
    assert!(
        out.contains("catppuccin-mocha"),
        "and the theme shown must be the fallback, not the ignored one; got:\n{out}"
    );
}

#[test]
fn a_valid_config_is_never_reported_as_a_problem() {
    for body in [
        "theme = \"gruvbox\"\n",
        "[theme]\npreset = \"nord\"\naccent = \"#ff0000\"\n",
        "focus_color = \"#ff0000\"\n",
        "startup_panes = 2\n",
    ] {
        let out = doctor(Some(body));
        assert!(
            !out.contains("UNKNOWN") && !out.contains("INVALID"),
            "valid config {body:?} should be clean; got:\n{out}"
        );
    }
}

#[test]
fn doctor_reports_how_the_spawn_agent_key_will_resolve() {
    // `default_agent` failures are invisible until you press ⌥+;, so doctor
    // has to say what that key will actually do right now.
    let out = doctor(Some("default_agent = \"sh\"\n"));
    assert!(out.contains("agent: sh [ok]"), "got:\n{out}");

    // A configured-but-absent harness must be called out, not silently ok'd.
    // An alternative is planted on a pinned PATH so the wording is `MISSING
    // ...; will offer ...` on every machine, installed agents or none.
    let out = doctor_with_agents(
        Some("default_agent = \"gwae-no-such-agent-xyz\"\n"),
        &["codex"],
    );
    assert!(
        out.contains("MISSING \"gwae-no-such-agent-xyz\""),
        "got:\n{out}"
    );

    // Unset is a normal state now, not an error: the gateway handles it.
    let out = doctor(Some("startup_panes = 1\n"));
    assert!(out.contains("agent:"), "got:\n{out}");
    assert!(!out.contains("MISSING"), "got:\n{out}");
}

#[test]
fn doctor_reports_whether_input_latency_is_tuned() {
    // Latency settings are invisible until you notice typing feels sluggish,
    // so doctor has to surface them alongside everything else it checks.
    let out = doctor(Some("input_poll_ms = 10\n"));
    assert!(out.contains("latency:"), "got:\n{out}");
    assert!(
        out.contains("gwae tune"),
        "must point at the fix; got:\n{out}"
    );
}
