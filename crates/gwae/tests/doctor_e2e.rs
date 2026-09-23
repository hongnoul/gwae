//! `gwae doctor` must tell the user the truth about their config.
//!
//! A malformed config file is discarded silently at startup by design, with
//! only a `tracing` warning that scrolls past (or never appears) before the
//! alternate screen takes over. `doctor` is the one place a user can find
//! out, so these tests run the real binary against real config files.

// Unix-only end to end: every session here drives a real PTY running
// `sh` syntax (and asserts with unix tooling); Windows has neither.
#![cfg(unix)]

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
    doctor_with_agents_and_state(config_body, agents, None)
}

/// As [`doctor_with_agents`], but with a pre-seeded harness memory file, so
/// cases can cover the remembered-pick paths deterministically.
fn doctor_with_agents_and_state(
    config_body: Option<&str>,
    agents: &[&str],
    state_body: Option<&str>,
) -> String {
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
    if let Some(state) = state_body {
        std::fs::create_dir_all(dir.join("state/gwae")).expect("temp state dir");
        std::fs::write(dir.join("state/gwae/harness.json"), state).expect("write state");
    }
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"));
    cmd.arg("doctor")
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_STATE_HOME", dir.join("state"));
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
fn retired_theme_keys_do_not_break_doctor() {
    // Old configs name retired presets. They parse as "no overrides" (retro
    // default), not fatal, and doctor must stay clean.
    for body in [
        "theme = \"nord\"\n",
        "theme = \"tokyonight-storm\"\n",
        "[theme]\npreset = \"nord\"\n",
        "[theme]\naccent = \"#ff0000\"\n",
        "focus_color = \"#ff0000\"\n",
        "startup_panes = 2\n",
    ] {
        let out = doctor(Some(body));
        assert!(
            !out.contains("UNKNOWN") && !out.contains("INVALID"),
            "retired theme keys in {body:?} should be ignored; got:\n{out}"
        );
    }
}

#[test]
fn flags_a_config_file_that_is_being_ignored() {
    // gwae discards an unparseable config wholesale, so every setting in it
    // is silently inert. doctor must say so, and point at the syntax error.
    let out = doctor(Some("startup_panes = 2\nthis is not valid toml <<<\n"));
    assert!(
        out.contains("INVALID"),
        "a broken config must be reported as ignored; got:\n{out}"
    );
    assert!(
        out.contains("line 2"),
        "the parse error should locate the problem; got:\n{out}"
    );
}

#[test]
fn a_valid_config_is_never_reported_as_a_problem() {
    for body in ["startup_panes = 2\n", "focus_color = \"#ff0000\"\n"] {
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
fn doctor_names_a_dead_override_even_when_memory_covers_it() {
    // Memory keeps ⌥+; working, so this is not MISSING, but the broken pin
    // must still be visible rather than sitting silently in the config.
    let out = doctor_with_agents_and_state(
        Some("default_agent = \"gwae-no-such-agent-xyz\"\n"),
        &["codex"],
        Some("{\"last\":\"codex\",\"mru\":[\"codex\"],\"custom\":[]}"),
    );
    assert!(
        out.contains("codex [remembered]") && out.contains("gwae-no-such-agent-xyz"),
        "got:\n{out}"
    );
    assert!(!out.contains("MISSING"), "memory covers it; got:\n{out}");

    // And a live override still reports plain ok, not remembered.
    let out = doctor_with_agents(Some("default_agent = \"sh\"\n"), &[]);
    assert!(out.contains("agent: sh [ok]"), "got:\n{out}");
    assert!(!out.contains("remembered"), "got:\n{out}");
}

#[test]
fn doctor_reports_whether_input_latency_is_tuned() {
    // Latency settings are invisible until you notice typing feels sluggish,
    // so doctor has to surface them alongside everything else it checks.
    // `input_poll_ms` is retired as a recommendation: the event-driven loop
    // wakes on keystrokes directly, so any configured value must read as
    // tuned rather than pointing at a fix that no longer helps.
    let out = doctor(Some("input_poll_ms = 10\n"));
    assert!(out.contains("latency:"), "got:\n{out}");
    assert!(
        !out.contains("input_poll_ms"),
        "retired knob must not be flagged; got:\n{out}"
    );
}
