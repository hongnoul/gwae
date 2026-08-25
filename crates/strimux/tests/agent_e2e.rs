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
            std::fs::write(&p, format!("#!/bin/sh\necho AGENT-RAN:{a}\n")).expect("stub");
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
        self.dir.join("strimux/strimux.toml")
    }

    fn write_config(&self, body: &str) {
        std::fs::write(self.config_path(), body).expect("write config");
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.config_path()).unwrap_or_default()
    }

    /// Run the full TUI, so `⌥+;` is exercised the way a user presses it.
    fn spawn_tui(&self) -> Pty {
        self.spawn_with(&["run", "sleep 60"])
    }

    /// Run `strimux agent` in a PTY with only the sandbox's bin on PATH.
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
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_strimux"));
        cmd.env("XDG_CONFIG_HOME", &self.dir);
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
        seen.contains("Opening a shell instead"),
        "must say what it is doing instead; got:\n{seen}"
    );
    assert!(
        seen.contains("jcode"),
        "must name what it looked for; got:\n{seen}"
    );

    // And the pane is a *live shell*, not a dead box: it runs a command.
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
    let seen = p.collect_until(Duration::from_secs(10), |raw| {
        let t = screen_text(raw);
        t.contains("Which agent should") && t.contains("claude")
    });
    let text = screen_text(&seen);
    assert!(text.contains("Found on your PATH"), "got:\n{text}");

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
