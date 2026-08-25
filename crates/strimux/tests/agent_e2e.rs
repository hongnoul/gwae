//! End-to-end: `⌥+;` when no agent harness is installed.
//!
//! The spawn-agent key used to run `default_agent` blind, so on a machine
//! without the harness the pane's child died instantly and left a blank box.
//! The acceptance behavior is that the pane instead explains itself, offers
//! whatever *is* installed, saves the pick to the config, and hands the user a
//! working shell if there is nothing to run. These tests drive the real
//! `strimux agent` binary under a real PTY, which is exactly what a pane is.

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
        let dir = std::env::temp_dir().join(format!(
            "strimux-agent-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        std::fs::create_dir_all(dir.join("strimux")).expect("config dir");
        for a in agents {
            let p = bin.join(a);
            // A stub that identifies itself, so a test can prove the gateway
            // really exec'd *this* harness and not something else.
            // Stay alive after announcing: a real harness holds the pane, and
            // a stub that exits would make strimux quit (last pane gone) and
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

    /// Drop an executable stub onto the sandbox's PATH.
    fn write_stub(&self, name: &str, body: &str) {
        let p = self.bin.join(name);
        std::fs::write(&p, body).expect("stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }

    /// As [`Sandbox::spawn`], but with the `btm` offer live, so a case can
    /// drive the install against its own stubbed package manager.
    fn spawn_allowing_install(&self, args: &[&str]) -> Pty {
        let mut v = vec!["agent"];
        v.extend_from_slice(args);
        self.spawn_with_install(&v, true)
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.dir.join("strimux/strimux.toml")
    }

    fn write_config(&self, body: &str) {
        std::fs::write(self.config_path(), body).expect("write config");
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.config_path()).unwrap_or_default()
    }

    /// Run the full TUI with an explicit first-pane command.
    fn spawn_tui(&self) -> Pty {
        self.spawn_with(&["run", "sleep 60"])
    }

    /// Run the full TUI exactly as a bare `strimux` launch does.
    fn spawn_tui_bare(&self) -> Pty {
        self.spawn_with(&["run"])
    }

    /// Run `strimux agent` in a PTY with only the sandbox's bin on PATH.
    fn spawn(&self, args: &[&str]) -> Pty {
        let mut v = vec!["agent"];
        v.extend_from_slice(args);
        self.spawn_with(&v)
    }

    fn spawn_with(&self, args: &[&str]) -> Pty {
        self.spawn_with_install(args, false)
    }

    fn spawn_with_install(&self, args: &[&str], allow_install: bool) -> Pty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
        cmd.env("XDG_CONFIG_HOME", &self.dir);
        cmd.env("TERM", "xterm-256color");
        // `sh` must stay reachable: the gateway's last resort is $SHELL.
        cmd.env("PATH", format!("{}:/bin:/usr/bin", self.bin.display()));
        cmd.env("SHELL", "/bin/sh");
        // Onboarding offers to install `btm`, and the offer defaults to yes.
        // A test suite must never install software on the machine running it,
        // so the offer is off unless a case opts in with its own stubbed
        // package manager on the sandbox PATH.
        if !allow_install {
            cmd.env("STRIMUX_NO_INSTALL", "1");
        }
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

    /// Fast-forward the guided setup the gateway runs after the agent pick:
    /// `q` takes the default for every remaining question, then Enter
    /// dismisses the summary screen. The flow runs in raw mode, so these are
    /// bare keystrokes with no trailing newline - exactly what a keyboard
    /// sends. Tests that care about onboarding itself drive it explicitly.
    fn skip_onboarding(&mut self) {
        self.wait_for("Color theme");
        self.send("q");
        self.dismiss_summary();
    }

    /// Leave the closing summary screen, which only Enter and backspace act
    /// on. It drains keys typed during the flow before it listens, so this
    /// waits for the screen to appear rather than firing blind.
    ///
    /// Call [`Pty::press_done`] instead when the caller has already read the
    /// summary out of the stream: `wait_for` consumes what it reads, so a
    /// second wait for text that already went by would block until timeout.
    fn dismiss_summary(&mut self) {
        self.wait_for("strimux is configured");
        // The drain runs once the screen is up; give it a beat so the Enter
        // below is read as a keystroke rather than swallowed as backlog.
        std::thread::sleep(Duration::from_millis(250));
        self.send("\r");
    }

    /// Press Enter at a summary screen the caller has already seen.
    fn press_done(&mut self) {
        std::thread::sleep(Duration::from_millis(250));
        self.send("\r");
    }

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
    let seen = p.wait_for("No agent harness found");
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

    // Nothing was written to the config, since the user chose nothing.
    assert!(!sb.read_config().contains("default_agent"));
    p.kill();
}

#[test]
fn an_installed_harness_is_offered_chosen_saved_and_executed() {
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("Which agent");
    assert!(seen.contains("claude"), "got:\n{seen}");
    assert!(seen.contains("aider"), "got:\n{seen}");

    // Pick #2 (aider), proving the numbering maps to the listed order.
    p.send("2\n");
    // The stub prints this only if it was actually exec'd.
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:aider");

    // The choice persisted, so the next ⌥+; skips the prompt entirely.
    let cfg = sb.read_config();
    assert!(
        cfg.contains("default_agent = \"aider\""),
        "choice must be saved; got:\n{cfg}"
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
    let sb = Sandbox::new(&["codex"]);
    sb.write_config("default_agent = \"jcode\"\n");
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("`jcode` is not installed");
    assert!(
        seen.contains("codex"),
        "must offer the alternative; got:\n{seen}"
    );

    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:codex");
    assert!(sb.read_config().contains("default_agent = \"codex\""));
    p.kill();
}

#[test]
fn choosing_a_shell_leaves_the_config_untouched() {
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("startup_panes = 1\n");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("s\n");

    p.send("echo SHELL-IS-ALIVE\n");
    p.wait_for("SHELL-IS-ALIVE");
    let cfg = sb.read_config();
    assert!(
        !cfg.contains("default_agent"),
        "opting out must not save; got:\n{cfg}"
    );
    assert!(
        cfg.contains("startup_panes = 1"),
        "and must not disturb the file"
    );
    p.kill();
}

#[test]
fn saving_a_choice_preserves_the_rest_of_the_config_file() {
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("# hand written\nstartup_panes = 3\n\n[theme]\npreset = \"nord\"\n");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:claude");

    let cfg = sb.read_config();
    assert!(
        cfg.contains("# hand written"),
        "comments survive; got:\n{cfg}"
    );
    assert!(cfg.contains("startup_panes = 3"), "got:\n{cfg}");
    assert!(cfg.contains("preset = \"nord\""), "got:\n{cfg}");
    // And it must still be valid TOML with the key at top level.
    let v: toml::Value = toml::from_str(&cfg).expect("config stays valid TOML");
    assert_eq!(v["default_agent"].as_str(), Some("claude"));
    assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    p.kill();
}

#[test]
fn print_reports_the_resolution_without_prompting_or_running_anything() {
    // The non-interactive path, for scripts and `doctor`-style checks.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\n");
    let p = sb.spawn(&["--print"]);
    let seen = p.wait_for("default_agent: claude");
    assert!(seen.contains("[ok]"), "got:\n{seen}");
    assert!(
        !seen.contains("AGENT-RAN"),
        "--print must not exec; got:\n{seen}"
    );
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
fn pressing_the_spawn_agent_key_opens_the_gateway_in_the_new_pane() {
    // The binding itself, end to end: this is the path that used to produce a
    // blank pane, and no unit test of the gateway can prove the TUI reaches it.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn_tui();
    // Let the first pane settle so the spawn lands in a steady layout.
    std::thread::sleep(Duration::from_millis(700));

    // ⌥+; as a terminal actually sends it: ESC-prefixed (Meta).
    p.send("\x1b;");
    // The pane is a quarter of the screen, so the gateway's lines are wrapped
    // and split across cell-positioned writes; assert on fragments that fit.
    // A quarter-width pane gets the gateway's narrow layout, which keeps the
    // list next to the prompt instead of scrolling it away.
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("Pick an agent") && t.contains("Claude Code")
    });
    let text = screen_text(&seen);
    assert!(text.contains("1 Claude Code"), "got:\n{text}");

    // Pick it, and prove the choice reached the config from a real keypress.
    p.send("1\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_config().contains("default_agent") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_config().contains("default_agent = \"claude\""),
        "got:\n{}",
        sb.read_config()
    );
    p.kill();
}

#[test]
fn a_bare_enter_takes_the_listed_default() {
    // The prompt offers Enter as a shortcut, so it must land on entry #1 and
    // save it exactly as an explicit "1" would.
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("Which agent");
    assert!(
        seen.contains("(default)"),
        "the default must be labeled; got:\n{seen}"
    );

    p.send("\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:claude");
    assert!(sb.read_config().contains("default_agent = \"claude\""));
    p.kill();
}

#[test]
fn a_bad_entry_reprompts_instead_of_giving_up() {
    // A typo must not drop the user into a shell silently; the pane keeps
    // asking, since the whole point is to leave them with a working agent.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("9\n");
    p.wait_for("Enter 1-1");
    p.send("banana\n");
    p.wait_for("Enter 1-1");
    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:claude");
    p.kill();
}

#[test]
fn a_non_tty_never_wedges_the_pane_waiting_for_input() {
    // Belt and braces: if stdin is not a terminal the gateway must fall
    // straight through to a shell rather than block forever on a read.
    let sb = Sandbox::new(&["claude"]);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_strimux"))
        .arg("agent")
        .env("XDG_CONFIG_HOME", &sb.dir)
        .env("PATH", format!("{}:/bin:/usr/bin", sb.bin.display()))
        .env("SHELL", "/bin/sh")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("run agent with no tty");
    // It exec'd a shell, which with no stdin exits immediately and cleanly.
    assert!(out.status.success(), "status: {:?}", out.status);
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
    let seen = p.wait_for("ARGS:");
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
fn a_harness_strimux_has_never_heard_of_is_still_discovered() {
    // The whole point of the heuristic: a tool that did not exist when this
    // binary was built must show up without a strimux release.
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "hermes-agent");
    stub(&sb.bin, "frobnicator"); // not agent-shaped: must NOT be offered
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("hermes-agent");
    assert!(
        !seen.contains("frobnicator"),
        "an ordinary binary must not be offered; got:\n{seen}"
    );

    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:hermes-agent");
    assert!(sb
        .read_config()
        .contains("default_agent = \"hermes-agent\""));
    p.kill();
}

#[test]
fn muse_style_one_word_names_are_found_too() {
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "musecode");
    let mut p = sb.spawn(&[]);
    p.wait_for("musecode");
    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:musecode");
    p.kill();
}

#[test]
fn the_config_can_teach_it_a_name_it_could_never_guess() {
    // An agent whose command looks like nothing in particular.
    let sb = Sandbox::new(&[]);
    stub(&sb.bin, "zz");
    sb.write_config("agents = [\"zz\"]\n");
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("Which agent");
    assert!(seen.contains("zz"), "got:\n{seen}");
    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:zz");
    p.kill();
}

#[test]
fn typing_an_unlisted_command_works_and_is_saved() {
    // The escape hatch that makes any harness usable immediately.
    let sb = Sandbox::new(&["claude"]);
    stub(&sb.bin, "zz");
    let mut p = sb.spawn(&[]);
    let seen = p.wait_for("Which agent");
    assert!(
        seen.contains("Type the command"),
        "the option must be advertised; got:\n{seen}"
    );
    assert!(!seen.contains("zz"), "zz is not agent-shaped; got:\n{seen}");

    p.send("zz\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:zz");
    assert!(sb.read_config().contains("default_agent = \"zz\""));
    p.kill();
}

#[test]
fn a_typed_command_that_does_not_exist_says_so_and_reprompts() {
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("hermes\n");
    let seen = p.wait_for("not on your PATH");
    assert!(seen.contains("hermes"), "must name the typo; got:\n{seen}");
    p.send("1\n");
    p.skip_onboarding();
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
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:zz");
    assert!(sb.read_config().contains("default_agent = \"zz\""));
    p.kill();
}

#[test]
fn startup_pane_one_one_launches_the_configured_agent_directly() {
    // Case 1 of 2: a preferred agent is configured, so the very first pane is
    // that agent. No selector, no shell, nothing to type.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("default_agent = \"claude\"\n");
    let p = sb.spawn_tui_bare();
    let seen = p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("AGENT-RAN:claude")
    });
    let text = screen_text(&seen);
    assert!(
        !text.contains("Found on your PATH"),
        "a configured agent must not show the selector; got:\n{text}"
    );
    p.kill();
}

#[test]
fn startup_pane_one_one_shows_the_selector_when_no_agent_is_configured() {
    // Case 2 of 2: nothing configured, so pane 1.1 is the selector itself.
    let sb = Sandbox::new(&["claude", "aider"]);
    let mut p = sb.spawn_tui_bare();
    // Wait for the gateway to have painted, then dismiss the startup HUD,
    // which covers the middle of the screen (ESC is swallowed by strimux, so
    // it reaches no pane). What is left is what the user actually reads.
    // ⌥+/ dismisses the startup HUD without reaching the pane.
    let seen = p.collect_until_poking(Duration::from_secs(20), "\x1b/", |raw| {
        let t = screen_text(raw);
        t.contains("Claude Code") && t.contains("aider")
    });
    let text = screen_text(&seen);
    assert!(text.contains("Pick an agent"), "got:\n{text}");

    // And picking there works, exactly as it does for ⌥+;.
    p.send("1\n");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !sb.read_config().contains("default_agent") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    assert!(
        sb.read_config().contains("default_agent = \"claude\""),
        "got:\n{}",
        sb.read_config()
    );
    p.kill();
}

#[test]
fn an_explicit_run_command_still_beats_the_gateway_in_pane_one_one() {
    // Being specific must win: `strimux run <cmd>` is the user overriding the
    // default behavior, not asking for an agent.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn_with(&["run", "sh"]);
    std::thread::sleep(Duration::from_millis(700));
    p.send("echo SHELL-IS-ALIVE\n");
    let seen = p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("SHELL-IS-ALIVE")
    });
    assert!(
        !screen_text(&seen).contains("Found on your PATH"),
        "an explicit command must not be replaced by the gateway; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn startup_with_no_agents_at_all_still_lands_in_a_usable_shell() {
    // The degenerate case: strimux must never open onto a dead first pane.
    let sb = Sandbox::new(&[]);
    let mut p = sb.spawn_tui_bare();
    p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("No agent harness found")
    });
    p.send("\n");
    p.send("echo SHELL-IS-ALIVE\n");
    p.collect_until(Duration::from_secs(15), |raw| {
        screen_text(raw).contains("SHELL-IS-ALIVE")
    });
    p.kill();
}

#[test]
fn onboarding_tunes_latency_silently_before_it_asks_anything() {
    // Machine tuning is not a question: it has one right answer, so strimux
    // applies it to its own config *before* the first question is drawn,
    // rather than making the user adjudicate a number they cannot evaluate.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("input_poll_ms = 10\n");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    // By the time the first question is on screen, the fix is already on disk.
    let seen = p.wait_for("Color theme");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_config().contains("input_poll_ms = 1") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    let cfg = sb.read_config();
    assert!(cfg.contains("input_poll_ms = 1"), "got:\n{cfg}");
    // ...and it was never asked about, or announced mid-flow.
    assert!(
        !seen.contains("input_poll_ms"),
        "tuning must be silent; got:\n{seen}"
    );
    // The agent choice from the step before survives the tuning write.
    assert!(cfg.contains("default_agent = \"claude\""), "got:\n{cfg}");
    p.send("q");
    p.dismiss_summary();
    p.kill();
}

#[test]
fn the_summary_screen_reports_what_landed_in_the_file() {
    // The flow ends on one screen showing every setting as it now stands, so
    // a user who arrowed through eight questions can check the result without
    // opening the TOML.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.wait_for("Color theme");
    p.send("q");
    let seen = p.wait_for("strimux is configured");
    for line in [
        "Color theme",
        "Panes on screen at launch",
        "Width of a new column",
        "catppuccin-mocha",
    ] {
        assert!(seen.contains(line), "summary omits {line:?}; got:\n{seen}");
    }
    // And it names the file it wrote, which is the thing to edit next.
    assert!(seen.contains("strimux.toml"), "got:\n{seen}");
    p.press_done();
    p.wait_for("AGENT-RAN:claude");
    p.kill();
}

#[test]
fn onboarding_says_nothing_about_latency_when_there_is_nothing_to_fix() {
    // Silence is the feature: a tuned machine must not be nagged, or the
    // message becomes noise people learn to skip.
    let sb = Sandbox::new(&["claude"]);
    sb.write_config("input_poll_ms = 1\n");
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.wait_for("Color theme");
    p.send("q");
    p.dismiss_summary();
    let seen = p.wait_for("AGENT-RAN:claude");
    // Only true when kitty/macOS are also clean; assert the strimux part,
    // which is the one this sandbox controls.
    assert!(
        !seen.contains("input_poll_ms"),
        "must not mention a fix that is already applied; got:\n{seen}"
    );
    p.kill();
}

#[test]
fn a_configured_agent_never_sees_the_latency_prompt() {
    // Past onboarding: going to your harness must not be interrupted, even
    // on a machine with untuned settings.
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
fn first_run_configures_the_whole_terminal_not_just_the_agent() {
    // The acceptance behavior for onboarding: one pass through `⌥+;` leaves a
    // config file with the *appearance* and *layout* settings decided, not
    // only `default_agent`. Driven through a real PTY, which is what a pane is.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    // Theme: arrow down to the fifth option (nord) and take it with Enter, so
    // we prove the *highlight* is what lands in the file, not the default.
    p.wait_for("Color theme");
    p.send("jjjj\r");
    // Panes: a digit needs no Enter at all.
    p.wait_for("Panes on screen at launch");
    p.send("2");
    // Everything else: defaults, then dismiss the summary.
    p.send("q");

    p.dismiss_summary();
    p.wait_for("AGENT-RAN:claude");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_config().contains("onboarded") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    let cfg = sb.read_config();
    assert!(cfg.contains("default_agent = \"claude\""), "got:\n{cfg}");
    assert!(
        cfg.contains("theme = \"nord\""),
        "answered theme; got:\n{cfg}"
    );
    assert!(
        cfg.contains("startup_panes = 2"),
        "answered panes; got:\n{cfg}"
    );
    assert!(
        cfg.contains("center_focus = "),
        "defaults written too; got:\n{cfg}"
    );
    assert!(cfg.contains("[cowsay]"), "table answers land; got:\n{cfg}");
    toml::from_str::<toml::Value>(&cfg).expect("generated config is valid TOML");
    p.kill();
}

#[test]
fn onboarding_happens_exactly_once() {
    // Second visit to the gateway must exec straight into the harness: a
    // setup flow you cannot get past is worse than no setup flow.
    let sb = Sandbox::new(&["claude"]);
    let mut p = sb.spawn(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.skip_onboarding();
    p.wait_for("AGENT-RAN:claude");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !sb.read_config().contains("onboarded") {
        let _ = p.rx.recv_timeout(Duration::from_millis(200));
    }
    p.kill();

    let p2 = sb.spawn(&[]);
    let seen = p2.wait_for("AGENT-RAN:claude");
    assert!(
        !seen.contains("Color theme"),
        "setup ran a second time; got:\n{seen}"
    );
    p2.kill();
}

#[test]
fn saying_yes_to_btm_installs_it_and_the_summary_reports_the_result() {
    // The one answer that changes the machine rather than a file. Driven with
    // a stub `brew` on the sandbox PATH, so the real command path runs (the
    // plan picks Brew, `brew install bottom` is executed, the result is
    // probed) without installing anything on the machine running the tests.
    let sb = Sandbox::new(&["claude"]);
    let marker = sb.dir.join("brew-ran.txt");
    // The stub records its arguments, then produces the binary the installer
    // was supposed to produce, so the post-install PATH probe finds it.
    sb.write_stub(
        "brew",
        &format!(
            "#!/bin/sh\necho \"$@\" > {}\nprintf '#!/bin/sh\\nexit 0\\n' > {}/btm\nchmod +x {}/btm\n",
            marker.display(),
            sb.bin.display(),
            sb.bin.display()
        ),
    );
    let mut p = sb.spawn_allowing_install(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    // `q` defaults every *remaining* question, but the btm offer is an action
    // on the machine, so drive to it explicitly: Enter through the questions
    // before it, taking each default.
    p.wait_for("Color theme");
    for _ in 0..7 {
        p.send("\r");
        std::thread::sleep(Duration::from_millis(120));
    }
    let at_offer = p.wait_for("Install btm");
    assert!(at_offer.contains("install it now"), "got:\n{at_offer}");
    p.send("\r");
    let seen = p.wait_for("strimux is configured");
    assert!(
        seen.contains("Install btm"),
        "the install is reported on the summary; got:\n{seen}"
    );
    assert!(
        seen.contains("installed") && !seen.contains("not installed"),
        "should report success; got:\n{seen}"
    );
    // The package manager really was invoked, with the formula name (which is
    // `bottom`, not the `btm` the binary is called).
    let ran = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(ran.contains("install bottom"), "brew args were: {ran:?}");
    p.press_done();
    p.kill();
}

#[test]
fn saying_no_to_btm_leaves_the_machine_alone() {
    // The default is yes, so "no" has to actually be reachable and actually
    // do nothing: a stub that records any invocation proves it.
    let sb = Sandbox::new(&["claude"]);
    let marker = sb.dir.join("brew-ran.txt");
    sb.write_stub(
        "brew",
        &format!("#!/bin/sh\necho \"$@\" > {}\n", marker.display()),
    );
    let mut p = sb.spawn_allowing_install(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.wait_for("Color theme");
    for _ in 0..7 {
        p.send("\r");
        std::thread::sleep(Duration::from_millis(120));
    }
    p.wait_for("Install btm");
    // Move off the default and take it: down, then Enter.
    p.send("j\r");
    let seen = p.wait_for("strimux is configured");
    assert!(
        seen.contains("skipped"),
        "should report skipped; got:\n{seen}"
    );
    assert!(
        !marker.exists(),
        "answering no still ran the package manager"
    );
    p.press_done();
    p.kill();
}

#[test]
fn a_machine_that_already_has_btm_is_never_asked() {
    // A question whose only honest answer is "it is already done" teaches
    // users that setup asks things it already knows.
    let sb = Sandbox::new(&["claude", "btm"]);
    let mut p = sb.spawn_allowing_install(&[]);
    p.wait_for("Which agent");
    p.send("1\n");
    p.wait_for("Color theme");
    p.send("q");
    let seen = p.wait_for("strimux is configured");
    assert!(
        !seen.contains("Install btm"),
        "offered an install to someone who already has it; got:\n{seen}"
    );
    p.press_done();
    p.kill();
}
