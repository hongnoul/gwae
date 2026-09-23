//! End-to-end: `⌥+;` when no agent harness is installed.
//!
//! The spawn-agent key used to run `default_agent` blind, so on a machine
//! without the harness the pane's child died instantly and left a blank box.
//! The acceptance behavior is that the pane instead explains itself, offers
//! whatever *is* installed, remembers the pick in the state file, and hands
//! the user a working shell if there is nothing to run. These tests drive the
//! real `gwae agent` binary under a real PTY, which is exactly what a pane is.

// Unix-only end to end: every session here drives a real PTY running
// `sh` syntax (and asserts with unix tooling); Windows has neither.
#![cfg(unix)]

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// A private HOME/XDG dir plus a fake PATH, so a case sees exactly the set of
/// "installed" harnesses it asks for and never the developer's real ones.
struct Sandbox {
    dir: std::path::PathBuf,
    bin: std::path::PathBuf,
}

impl Sandbox {
    fn new(agents: &[&str]) -> Sandbox {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        // Under the target dir, not `temp_dir()`: on macOS the system temp
        // lives in `/var/folders/...`, which the agent scan rightly treats
        // as an OS directory and skips, so stubs planted there would be
        // invisible to the discovery tests on a stock machine (and in CI).
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "gwae-agent-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
        for a in agents {
            let p = bin.join(a);
            // A stub that identifies itself, so a test can prove the gateway
            // really exec'd *this* harness and not something else.
            // Stay alive after announcing: a real harness holds the pane, and
            // a stub that exits would make gwae quit (last pane gone) and
            // wipe the alt screen before a test could read it.
            std::fs::write(
                &p,
                format!("#!/bin/sh\necho AGENT-RAN:{a}\nexec sleep 60\n"),
            )
            .expect("stub");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod stub");
            }
        }
        Sandbox { dir, bin }
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.dir.join("gwae/gwae.toml")
    }

    fn state_path(&self) -> std::path::PathBuf {
        self.dir.join("state/gwae/harness.json")
    }

    fn write_config(&self, body: &str) {
        std::fs::write(self.config_path(), body).expect("write config");
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.config_path()).unwrap_or_default()
    }

    fn read_state(&self) -> String {
        std::fs::read_to_string(self.state_path()).unwrap_or_default()
    }

    /// Run the full TUI with an explicit first-pane command.
    fn spawn_tui(&self) -> Pty {
        self.spawn_with(&["run", "sleep 60"])
    }

    /// Run the full TUI exactly as a bare `gwae` launch does.
    fn spawn_tui_bare(&self) -> Pty {
        self.spawn_with(&["run"])
    }

    /// Run `gwae agent` in a PTY with only the sandbox's bin on PATH.
    fn spawn(&self, args: &[&str]) -> Pty {
        let mut v = vec!["agent"];
        v.extend_from_slice(args);
        self.spawn_with(&v)
    }

    fn spawn_with(&self, args: &[&str]) -> Pty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", &self.dir);
        cmd.env("XDG_STATE_HOME", self.dir.join("state"));
        cmd.env("TERM", "xterm-256color");
        // `sh` must stay reachable: the gateway's last resort is $SHELL.
        cmd.env("PATH", format!("{}:/bin:/usr/bin", self.bin.display()));
        cmd.env("SHELL", "/bin/sh");
        for a in args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd).expect("spawn agent");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        let (tx, rx) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Pty {
            rx,
            writer,
            child,
            _master: pair.master,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Pty {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Pty {
    /// Read until `needle` shows up, or time out with what we did see.
    fn wait_for(&self, needle: &str) -> String {
        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if out.contains(needle) {
                return out;
            }
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(b) => out.push_str(&String::from_utf8_lossy(&b)),
                Err(_) => continue,
            }
        }
        assert!(out.contains(needle), "never saw {needle:?} in:\n{out}");
        out
    }

    /// Read until *every* needle has shown up. The prompt paints across
    /// several writes, so asserting sibling strings on the read that saw the
    /// first one is a race that only slow machines (CI) lose.
    fn wait_for_all(&self, needles: &[&str]) -> String {
        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if needles.iter().all(|n| out.contains(n)) {
                return out;
            }
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(b) => out.push_str(&String::from_utf8_lossy(&b)),
                Err(_) => continue,
            }
        }
        let missing: Vec<&&str> = needles.iter().filter(|n| !out.contains(**n)).collect();
        panic!("never saw {missing:?} in:\n{out}");
    }

    /// No guided setup remains: the gateway picker execs straight after a pick.
    /// Accumulate output until `done` holds, nudging the TUI with `poke` each
    /// second. The startup HUD covers the middle of the screen and only lifts
    /// on a keypress, so a test that needs the pane underneath has to ask more
    /// than once: the first nudge can land before the pane has painted.
    fn collect_until_poking(
        &mut self,
        timeout: Duration,
        poke: &str,
        done: impl Fn(&str) -> bool,
    ) -> String {
        let mut out = String::new();
        let deadline = Instant::now() + timeout;
        let mut next_poke = Instant::now();
        while Instant::now() < deadline {
            if done(&out) {
                return out;
            }
            if Instant::now() >= next_poke {
                self.send(poke);
                next_poke = Instant::now() + Duration::from_millis(900);
            }
            if let Ok(b) = self.rx.recv_timeout(Duration::from_millis(200)) {
                out.push_str(&String::from_utf8_lossy(&b));
            }
        }
        assert!(done(&out), "condition never met in:\n{out}");
        out
    }

    /// Accumulate output until `done` is satisfied, or the timeout expires.
    fn collect_until(&self, timeout: Duration, done: impl Fn(&str) -> bool) -> String {
        let mut out = String::new();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if done(&out) {
                return out;
            }
            if let Ok(b) = self.rx.recv_timeout(Duration::from_millis(250)) {
                out.push_str(&String::from_utf8_lossy(&b));
            }
        }
        assert!(done(&out), "condition never met in:\n{out}");
        out
    }

    fn send(&mut self, s: &str) {
        self.writer.write_all(s.as_bytes()).expect("write");
        self.writer.flush().expect("flush");
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn with_nothing_installed_the_pane_explains_itself_and_still_gives_a_shell() {
    // The exact scenario that used to produce a silent blank pane.
    let sb = Sandbox::new(&[]);
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for_all(&[
        "No agent harness found",
        "Enter alone opens a shell",
        "jcode",
    ]);
    assert!(
        seen.contains("Enter alone opens a shell"),
        "must say what it is doing instead; got:\n{seen}"
    );
    assert!(
        seen.contains("jcode"),
        "must name what it looked for; got:\n{seen}"
    );

    // And the pane is a *live shell*, not a dead box: it runs a command.
    p.send("\n");
    p.send("echo SHELL-IS-ALIVE\n");
    p.wait_for("SHELL-IS-ALIVE");

    // Nothing was remembered, since the user chose nothing.
    assert!(!sb.read_state().contains("aider"));
    assert!(!sb.read_config().contains("default_agent"));
    p.kill();
}

#[test]
fn an_installed_harness_is_offered_chosen_saved_and_executed() {
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for_all(&["agent", "claude", "aider"]);
    assert!(seen.contains("claude"), "got:\n{seen}");
    assert!(seen.contains("aider"), "got:\n{seen}");

    // Pick #2 (aider), proving the numbering maps to the listed order.
    // A bare digit answers the gateway picker; no Enter needed.
    p.send("2\n");
    // The stub prints this only if it was actually exec'd.
    p.wait_for("AGENT-RAN:aider");

    // The choice is remembered in the state file (not the config), so the
    // next ⌥+; skips the prompt entirely.
    let state = sb.read_state();
    assert!(
        state.contains("\"last\":\"aider\""),
        "choice must be remembered; got:\n{state}"
    );
    assert!(
        !sb.read_config().contains("default_agent"),
        "the config file must stay untouched"
    );
    p.kill();
}

#[test]
fn a_configured_harness_runs_immediately_with_no_prompt_at_all() {
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\n");
    let p = sb.spawn(&[]);
    let seen = p.wait_for("AGENT-RAN:claude");
    assert!(
        !seen.contains("Which agent"),
        "a resolved config must never prompt; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn a_configured_but_missing_harness_names_it_and_offers_what_exists() {
    // The configured harness is gone, but all is not lost: the gateway
    // offers what the machine does have, and the pick is saved over the
    // stale entry.
    let sb = Sandbox::new(&["codex"]);
    sb.write_config("default_agent = \"jcode\"\n");
    let mut p = sb.spawn(&[]);
    // Gateway lists what is actually installed.
    let seen = p.wait_for_all(&["agent", "codex"]);
    assert!(
        seen.contains("codex"),
        "must offer the alternative; got:\n{seen}"
    );

    p.send("1\n");
    p.wait_for("AGENT-RAN:codex");
    assert!(sb.read_state().contains("\"last\":\"codex\""));
    p.kill();
}

#[test]
fn choosing_a_shell_leaves_the_config_untouched() {
    let sb = Sandbox::new(&["claude", "aider"]);
    // `s` answers the gateway picker, opting out to a shell.
    sb.write_config("startup_panes = 1\n");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("s\n");

    p.send("echo SHELL-IS-ALIVE\n");
    p.wait_for("SHELL-IS-ALIVE");
    let cfg = sb.read_config();
    assert!(
        !cfg.contains("default_agent"),
        "opting out must not touch the config; got:\n{cfg}"
    );
    assert!(
        cfg.contains("startup_panes = 1"),
        "and must not disturb the file"
    );
    assert!(
        !sb.read_state().contains("claude"),
        "and must not remember anything"
    );
    p.kill();
}

#[test]
fn picking_a_harness_never_rewrites_the_config_file() {
    // The redesign's core promise: the config file is the user's, and a
    // pick must leave it byte-identical while the memory lands in state.
    // Two harnesses force the picker (a lone install launches itself).
    let sb = Sandbox::new(&["claude", "aider"]);
    let before = "# hand written\nstartup_panes = 3\n\n[minimap]\nmax_width = 31\n";
    sb.write_config(before);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.wait_for("AGENT-RAN:claude");

    assert_eq!(
        sb.read_config(),
        before,
        "the config file must be byte-identical after a pick"
    );
    assert!(
        sb.read_state().contains("\"last\":\"claude\""),
        "while the pick is remembered in state"
    );
    p.kill();
}

#[test]
fn print_reports_the_resolution_without_prompting_or_running_anything() {
    // The non-interactive path, for scripts and `doctor`-style checks.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\n");
    let p = sb.spawn(&["--print"]);
    let seen = p.wait_for_all(&["default_agent: claude", "[ok]"]);
    assert!(seen.contains("[ok]"), "got:\n{seen}");
    assert!(
        !seen.contains("AGENT-RAN"),
        "--print must not exec; got:\n{seen}"
    );
    p.kill();

    // A lone install with no override reports itself as the auto source.
    let sb = Sandbox::new(&["claude"]);
    let p = sb.spawn(&["--print"]);
    let seen = p.wait_for_all(&["auto: claude", "[ok]"]);
    assert!(seen.contains("[ok]"), "got:\n{seen}");
    p.kill();

    let sb = Sandbox::new(&[]);
    let p = sb.spawn(&["--print"]);
    let seen = p.wait_for("No agent harness found");
    assert!(!seen.contains("SHELL-IS-ALIVE"));
    p.kill();
}

/// Strip escape sequences and squeeze whitespace, so a phrase can be found in
/// TUI output where the renderer chops every line into positioned cells.
fn screen_text(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI/OSC/etc: consume up to the final byte of the sequence.
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    for c in chars.by_ref() {
                        if c == '\x07' || c == '\\' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn pressing_the_spawn_agent_key_with_one_harness_spawns_it_directly() {
    // The fast path end to end: a single installed harness means `⌥+;` never
    // opens any picker, in-pane or overlay; the new pane just is the harness.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn_tui();
    // Let the first pane settle so the spawn lands in a steady layout.
    std::thread::sleep(Duration::from_millis(700));

    // ⌥+; as a terminal actually sends it: ESC-prefixed (Meta).
    p.send("\x1b;");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("AGENT-RAN:claude")
    });
    let text = screen_text(&seen);
    assert!(
        !text.contains("just a shell") && !text.contains("pick agent:"),
        "a lone harness must spawn with no picker at all; got:\n{text}"
    );
    p.kill();
}

#[test]
fn pressing_the_spawn_agent_key_with_several_harnesses_opens_the_overlay() {
    // The overlay end to end: the binding, the native picker, a real
    // keypress pick, and the remembered state — all without touching the
    // config file.
    let sb = Sandbox::new(&["claude", "aider"]);
    // A config file must exist: a missing one triggers the native first-run
    // flow before the TUI, which is not what this test drives.
    sb.write_config("");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    p.send("\x1b;");
    // Wait for the whole overlay, not just its title: the title paints in
    // an earlier write than the rows, so matching on it alone is a race.
    // (The startup HUD dismisses on the chord itself; its stale bytes in
    // the accumulation below are harmless.)
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    let text = screen_text(&seen);
    assert!(text.contains("Claude Code"), "got:\n{text}");
    assert!(text.contains("just a shell"), "got:\n{text}");

    // ⏎ takes the default (first) entry and spawns it in the new pane.
    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("claude") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"claude\""),
        "got:\n{}",
        sb.read_state()
    );
    assert!(
        !sb.read_config().contains("default_agent"),
        "the overlay must not touch the config"
    );
    p.kill();
}

#[test]
fn a_missing_override_opens_the_overlay_with_a_notice() {
    // The override names something uninstalled: live ⌥+; must say so in the
    // overlay, offer what exists, spawn the pick directly, and leave the
    // override itself alone (it is the user's explicit pin).
    let sb = Sandbox::new(&["claude", "aider"]);
    sb.write_config("default_agent = \"jcode\"\n");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    p.send("\x1b;");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    let text = screen_text(&seen);
    assert!(
        text.contains("`jcode` is not installed"),
        "the notice must name the missing override; got:\n{text}"
    );
    assert!(text.contains("Claude Code"), "got:\n{text}");

    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("claude") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"claude\""),
        "got:\n{}",
        sb.read_state()
    );
    assert!(
        sb.read_config().contains("default_agent = \"jcode\""),
        "the override must survive the pick"
    );
    p.kill();
}

#[test]
fn a_remembered_but_uninstalled_pick_opens_the_overlay_with_a_notice() {
    // Memory names something since uninstalled: live ⌥+; must say the
    // remembered pick is gone, offer what exists, and reheal on the pick.
    let sb = Sandbox::new(&["claude", "aider"]);
    // A config file must exist so first run is skipped; the stale memory
    // below is the live-TUI scenario under test.
    sb.write_config("");
    std::fs::create_dir_all(sb.dir.join("state/gwae")).expect("state dir");
    std::fs::write(
        sb.state_path(),
        "{\"last\":\"gone-xyz\",\"mru\":[\"gone-xyz\"],\"custom\":[]}",
    )
    .expect("stale memory");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    p.send("\x1b;");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    let text = screen_text(&seen);
    assert!(
        text.contains("remembered `gone-xyz` is gone"),
        "the notice must name the stale pick; got:\n{text}"
    );

    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("\"last\":\"claude\"") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"claude\""),
        "the pick must reheal memory; got:\n{}",
        sb.read_state()
    );
    p.kill();
}

#[test]
fn the_force_pick_chord_opens_the_overlay_and_spawns_a_normal_pane() {
    // ⌥+Shift+; ignores the fast paths and opens the overlay; the pick
    // lands in a normal pane and the choice is remembered.
    let sb = Sandbox::new(&["claude", "aider"]);
    // A config file must exist so first run is skipped and the chord is
    // handled by the live TUI.
    sb.write_config("");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    // ⌥+Shift+; as a terminal sends it: ESC-prefixed colon (Meta).
    p.send("\x1b:");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    assert!(
        screen_text(&seen).contains("Claude Code"),
        "got:\n{}",
        screen_text(&seen)
    );

    // Pick the second entry to prove overlay numbering still maps.
    p.send("\x1b[B");
    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:aider")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("aider") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"aider\""),
        "got:\n{}",
        sb.read_state()
    );
    p.kill();
}

#[test]
fn the_force_pick_chord_always_asks_even_with_a_remembered_pick() {
    // The force-pick promise: a healthy remembered pick makes ⌥+; spawn with
    // no UI, but ⌥+Shift+; must still open the overlay instead of repeating
    // the last harness. This is the exact stuck case the chord exists for.
    let sb = Sandbox::new(&["claude", "aider"]);
    std::fs::create_dir_all(sb.dir.join("state/gwae")).expect("state dir");
    std::fs::write(
        sb.state_path(),
        "{\"last\":\"claude\",\"mru\":[\"claude\"],\"custom\":[]}",
    )
    .expect("remembered pick");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    // ⌥+Shift+; as a terminal sends it: ESC-prefixed colon (Meta).
    p.send("\x1b:");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    let text = screen_text(&seen);
    assert!(text.contains("Claude Code"), "got:\n{text}");
    assert!(text.contains("aider"), "got:\n{text}");

    // Pick the second entry to prove a different harness is reachable.
    p.send("\x1b[B");
    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:aider")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("\"last\":\"aider\"") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"aider\""),
        "got:\n{}",
        sb.read_state()
    );
    p.kill();
}

#[test]
fn the_force_pick_chord_ignores_a_live_override_but_leaves_it_pinned() {
    // A resolved override makes ⌥+; spawn with no UI; ⌥+Shift+; must still
    // open the overlay (naming the override, since it keeps winning for ⌥+;)
    // and must not rewrite the config when the pick lands elsewhere.
    let sb = Sandbox::new(&["claude", "aider"]);
    sb.write_config("default_agent = \"claude\"\n");
    let mut p = sb.spawn_tui();
    std::thread::sleep(Duration::from_millis(700));

    p.send("\x1b:");
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("pick agent") && t.contains("just a shell")
    });
    let text = screen_text(&seen);
    assert!(
        text.contains("default_agent") && text.contains("claude"),
        "the notice must name the live override; got:\n{text}"
    );

    p.send("\x1b[B");
    p.send("\r");
    p.collect_until(Duration::from_secs(10), |raw| {
        screen_text(raw).contains("AGENT-RAN:aider")
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_state().contains("\"last\":\"aider\"") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"aider\""),
        "got:\n{}",
        sb.read_state()
    );
    assert!(
        sb.read_config().contains("default_agent = \"claude\""),
        "the override must survive the pick"
    );
    p.kill();
}

#[test]
fn a_bare_enter_takes_the_listed_default() {
    // The prompt offers Enter as a shortcut, so it must land on entry #1 and
    // save it exactly as an explicit "1" would.
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    // Question and default label paint across several writes; wait for both.
    let seen = p.wait_for_all(&["agent", "(default)"]);
    assert!(
        seen.contains("(default)"),
        "the default must be labeled; got:\n{seen}"
    );

    p.send("\n");
    // The gateway execs straight after the pick.
    p.wait_for("AGENT-RAN:claude");
    assert!(sb.read_state().contains("\"last\":\"claude\""));
    p.kill();
}

#[test]
fn a_bad_entry_reprompts_instead_of_giving_up() {
    // A typo must not drop the user into a shell silently; the pane keeps
    // asking, since the whole point is to leave them with a working agent.

    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("9\n");
    p.wait_for("Enter 1-2");
    p.send("banana\n");
    p.wait_for("Enter 1-2");
    p.send("1\n");
    // The gateway execs straight after a valid pick.
    p.wait_for("AGENT-RAN:claude");
    p.kill();
}

#[test]
fn a_non_tty_never_wedges_the_pane_waiting_for_input() {
    // Belt and braces: if stdin is not a terminal the gateway must fall
    // straight through to a shell rather than block forever on a read.
    let sb = Sandbox::new(&["claude"]);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gwae"))
        .arg("agent")
        .env("XDG_CONFIG_HOME", &sb.dir)
        .env("XDG_STATE_HOME", sb.dir.join("state"))
        .env("PATH", format!("{}:/bin:/usr/bin", sb.bin.display()))
        .env("SHELL", "/bin/sh")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("run agent with no tty");
    // It exec'd a shell, which with no stdin exits immediately and cleanly.
    assert!(out.status.success(), "status: {:?}", out.status);
    assert!(!sb.read_state().contains("claude"));
    assert!(!sb.read_config().contains("default_agent"));
}

#[test]
fn a_configured_command_with_arguments_is_exec_d_with_them() {
    // `default_agent` is documented as a command, not just a binary name, so
    // args have to survive the gateway's shell-split and reach the harness.
    let sb = Sandbox::new(&["claude"]);
    // The stub echoes its argv, so this proves the args were passed through.
    std::fs::write(
        sb.bin.join("claude"),
        "#!/bin/sh\necho AGENT-RAN:claude ARGS:$*\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            sb.bin.join("claude"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    sb.write_config("default_agent = \"claude --resume --foo\"\n");
    let p = sb.spawn(&[]);
    let seen = p.wait_for_all(&["ARGS:", "--resume --foo"]);
    assert!(
        seen.contains("--resume --foo"),
        "args must reach the harness; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn an_absolute_path_as_the_configured_agent_runs_without_a_prompt() {
    // Someone pinning a specific install must not be sent to the picker.
    let sb = Sandbox::new(&["claude"]);
    let abs = sb.bin.join("claude");
    sb.write_config(&format!("default_agent = \"{}\"\n", abs.display()));
    let p = sb.spawn(&[]);
    let seen = p.wait_for("AGENT-RAN:claude");
    assert!(!seen.contains("Which agent"), "got:\n{seen}");
    p.kill();
}

/// Install an executable stub that announces itself when run.
fn stub(dir: &std::path::Path, name: &str) {
    let p = dir.join(name);
    std::fs::write(
        &p,
        format!("#!/bin/sh\necho AGENT-RAN:{name}\nexec sleep 60\n"),
    )
    .expect("stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

#[test]
fn a_harness_gwae_has_never_heard_of_is_still_discovered() {
    // The whole point of the heuristic: a tool that did not exist when this
    // binary was built must show up without a gwae release.
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "hermes-agent");
    stub(&sb.bin, "frobnicator"); // not agent-shaped: must NOT be offered
                                  // A lone install launches itself: no picker, no keypress needed.
    let p = sb.spawn(&[]);
    let seen = p.wait_for("AGENT-RAN:hermes-agent");
    assert!(
        !seen.contains("Which agent"),
        "a lone harness must never prompt; got:\n{seen}"
    );
    // ...and leaves no trace: an auto-launch is not a pick, so a second
    // harness installed later still gets its picker moment.
    assert!(
        !sb.read_state().contains("hermes-agent"),
        "auto-launch must not write memory"
    );
    p.kill();
}

#[test]
fn muse_style_one_word_names_are_found_too() {
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "musecode");
    // Lone install: straight to the harness, no picker.
    let p = sb.spawn(&[]);
    p.wait_for("AGENT-RAN:musecode");
    p.kill();
}

#[test]
fn a_typed_pick_is_remembered_and_offered_next_time() {
    // The replacement for the old `agents` config list: type any resolvable
    // command once, and it is offered alongside detected harnesses after.
    // Two harnesses force the picker (a lone install launches itself).
    let sb = Sandbox::new(&["claude", "aider"]);
    stub(&sb.bin, "zz");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("zz\n");
    p.wait_for("AGENT-RAN:zz");
    p.kill();

    let state = sb.read_state();
    assert!(state.contains("\"last\":\"zz\""), "got:\n{state}");
    assert!(state.contains("\"custom\""), "got:\n{state}");

    // Second run: the remembered pick execs with no picker at all.
    let p = sb.spawn(&[]);
    let seen = p.wait_for("AGENT-RAN:zz");
    assert!(
        !seen.contains("Which agent"),
        "a remembered pick must never prompt; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn typing_an_unlisted_command_works_and_is_saved() {
    // The escape hatch that makes any harness usable immediately.
    // Two harnesses force the picker (a lone install launches itself).
    let sb = Sandbox::new(&["claude", "aider"]);
    stub(&sb.bin, "zz");
    let mut p = sb.spawn(&[]);
    // Both strings, not just the first: the prompt paints across several
    // writes, and asserting the footer on the read that saw the header is a
    // race slow CI machines lose.
    let seen = p.wait_for_all(&["Which agent", "Type the command"]);
    assert!(
        seen.contains("Type the command"),
        "the option must be advertised; got:\n{seen}"
    );
    assert!(!seen.contains("zz"), "zz is not agent-shaped; got:\n{seen}");

    p.send("zz\n");
    // The gateway execs straight after the pick.
    p.wait_for("AGENT-RAN:zz");
    assert!(sb.read_state().contains("\"last\":\"zz\""));
    p.kill();
}

#[test]
fn a_typed_command_that_does_not_exist_says_so_and_reprompts() {
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("hermes\n");
    let seen = p.wait_for("not on your PATH");
    assert!(seen.contains("hermes"), "must name the typo; got:\n{seen}");
    p.send("1\n");
    // The gateway execs straight after a valid pick.
    p.wait_for("AGENT-RAN:claude");
    p.kill();
}

#[test]
fn even_with_nothing_found_you_can_type_a_command() {
    // "Nothing installed" is a claim about our search, not the machine, so
    // that screen must not be a dead end either.
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "zz");
    let mut p = sb.spawn(&[]);
    p.wait_for("No agent harness found");
    p.send("zz\n");
    // The gateway execs straight after the pick.
    p.wait_for("AGENT-RAN:zz");
    assert!(sb.read_state().contains("\"last\":\"zz\""));
    p.kill();
}

#[test]
fn startup_pane_one_one_launches_the_configured_agent_directly() {
    // Case 1 of 2: a preferred agent is configured, so the very first pane is
    // that agent. No selector, no shell, nothing to type.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\n");
    let mut p = sb.spawn_tui_bare();
    // The startup HUD covers the middle of the screen, and cells under it are
    // never emitted; poke ⌥+/ so the pane's full row gets painted at least
    // once between toggles.
    let seen = p.collect_until_poking(Duration::from_secs(15), "\x1b/", |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let text = screen_text(&seen);
    assert!(
        !text.contains("Found on your PATH") && !text.contains("Claude Code"),
        "a configured agent must not show the selector; got:\n{text}"
    );
    p.kill();
}

#[test]
fn startup_pane_one_one_shows_the_selector_when_no_agent_is_configured() {
    // Case 2 of 2: nothing configured, so pane 1.1 is the selector itself.
    // With a fresh config that selector is the gateway's own picker.
    let sb = Sandbox::new(&["claude", "aider"]);
    // "No agent configured" means an empty config, not a missing file: a
    // missing file is first run, which now configures natively before the
    // TUI ever opens. Pane 1.1's selector is the post-first-run behavior.
    sb.write_config("");
    let mut p = sb.spawn_tui_bare();
    // Wait for the gateway to have painted, then dismiss the startup HUD,
    // which covers the middle of the screen (ESC is swallowed by gwae, so
    // it reaches no pane). What is left is what the user actually reads.
    // ⌥+/ dismisses the startup HUD without reaching the pane.
    let seen = p.collect_until_poking(Duration::from_secs(20), "\x1b/", |raw| {
        let t = screen_text(raw);
        t.contains("Claude Code") && t.contains("aider")
    });
    let text = screen_text(&seen);
    assert!(text.contains("Claude Code"), "got:\n{text}");

    // And picking there works: answer the gateway picker and the choice
    // is remembered in state, not the config. Drive by sleeps past this
    // point, not pokes.
    p.send("1\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !sb.read_state().contains("claude") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_state().contains("\"last\":\"claude\""),
        "got:\n{}",
        sb.read_state()
    );
    assert!(
        !sb.read_config().contains("default_agent"),
        "the config must stay untouched"
    );
    p.kill();
}

#[test]
fn startup_with_a_lone_install_launches_it_with_no_picker() {
    // The zero-config case: one harness, no memory, no override. Pane 1.1
    // is the harness from the first frame, with no gateway in between.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn_tui_bare();
    let seen = p.collect_until_poking(Duration::from_secs(15), "\x1b/", |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let text = screen_text(&seen);
    assert!(
        !text.contains("Found on your PATH") && !text.contains("Which agent"),
        "a lone harness must not show any selector; got:\n{text}"
    );
    p.kill();
}

#[test]
fn an_explicit_run_command_still_beats_the_gateway_in_pane_one_one() {
    // Being specific must win: `gwae run <cmd>` is the user overriding the
    // default behavior, not asking for an agent.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn_with(&["run", "sh"]);
    std::thread::sleep(Duration::from_millis(700));
    p.send("echo SHELL-IS-ALIVE\n");
    let seen = p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("SHELL-IS-ALIVE")
    });
    assert!(
        !screen_text(&seen).contains("Found on your PATH")
            && !screen_text(&seen).contains("Which agent"),
        "an explicit command must not be replaced by the gateway; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn startup_with_no_agents_at_all_still_lands_in_a_usable_shell() {
    // The degenerate case: gwae must never open onto a dead first pane.
    // With nothing installed pane 1.1 runs the gateway's nothing-installed
    // screen; Enter falls through to a live shell.
    let sb = Sandbox::new(&[]);
    let mut p = sb.spawn_tui_bare();
    p.collect_until_poking(Duration::from_secs(15), "\x1b/", |raw| {
        let t = screen_text(raw);
        t.contains("No agent harness found") || t.contains("opens a shell")
    });
    p.send("\r");
    std::thread::sleep(Duration::from_millis(500));
    p.send("echo SHELL-IS-ALIVE\n");
    p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("SHELL-IS-ALIVE")
    });
    p.kill();
}

#[test]
fn a_configured_agent_never_sees_the_latency_prompt() {
    // Going to your harness must not be interrupted.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\ninput_poll_ms = 10\n");
    let p = sb.spawn(&[]);
    let seen = p.wait_for("AGENT-RAN:claude");
    assert!(
        !seen.contains("input-latency"),
        "the fast path must stay silent; got:\n{seen}"
    );
    // And the config is untouched.
    assert!(sb.read_config().contains("input_poll_ms = 10"));
    p.kill();
}

#[test]
fn a_remembered_command_that_dies_instantly_falls_back_to_the_picker() {
    // The stale-memory footgun: `harness.json` remembers a command that
    // still resolves on PATH but exits immediately (a broken wrapper, a
    // one-shot command recorded by accident). Pane 1.1 spawns it directly,
    // it dies, and — being the last pane — it used to take the whole
    // session down: a flash of a frame, exit 0, no explanation. The
    // acceptance behavior is a session that stays up, falls back to the
    // gateway picker in the same pane, names the dead command, and drops
    // the poisoned memory so the next bare launch is clean.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("");
    // A remembered pick that resolves but dies at once. It must be in the
    // sandbox bin (so `command_available` accepts it) and exit 0 instantly.
    let dud = sb.bin.join("dud-agent");
    std::fs::write(&dud, "#!/bin/sh\nexit 0\n").expect("dud stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dud, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    std::fs::create_dir_all(sb.dir.join("state/gwae")).expect("state dir");
    std::fs::write(
        sb.state_path(),
        "{\"last\":\"dud-agent\",\"mru\":[\"dud-agent\"],\"custom\":[\"dud-agent\"]}",
    )
    .expect("stale memory");

    let mut p = sb.spawn_tui_bare();
    // The session must survive and re-open pane 1.1 on the gateway picker
    // (which lists the real install). Poke ⌥+/ past the startup HUD.
    let seen = p.collect_until_poking(Duration::from_secs(20), "\x1b/", |raw| {
        screen_text(raw).contains("Claude Code")
    });
    let text = screen_text(&seen);
    assert!(
        text.contains("Claude Code"),
        "expected the gateway picker after the dud died; got:\n{text}"
    );
    // The poisoned memory is gone: `last` no longer names the dud, so the
    // next bare launch will not spawn it again.
    let state = sb.read_state();
    assert!(
        !state.contains("\"last\":\"dud-agent\""),
        "stale `last` must be dropped; got: {state}"
    );
    // And the fallback is genuinely usable: pick the real harness.
    p.send("1\n");
    let seen = p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    assert!(screen_text(&seen).contains("AGENT-RAN:claude"));
    p.kill();
}
