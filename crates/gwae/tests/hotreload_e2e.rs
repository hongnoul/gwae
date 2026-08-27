//! End-to-end: hot reload swaps gwae's code without disturbing the panes.
//!
//! These tests exist because the two things hot reload can get wrong are both
//! invisible until they are catastrophic:
//!
//! 1. **Losing the session.** A reload that kills the panes is worse than the
//!    restart it replaced, because it happens without being asked for.
//! 2. **Leaking processes.** Signal handlers do not survive `execve`; they are
//!    reset to `SIG_DFL`. A reloaded gwae that forgets to re-arm
//!    `crate::reap` looks completely healthy and then orphans every pane's
//!    background jobs the next time the terminal window is closed. Nothing on
//!    screen would ever reveal it.
//!
//! So the tests below assert on the real process table around a real reload of
//! a real rebuilt binary, in the same spirit as `teardown_e2e.rs`.
//!
//! ## The macOS trap these tests encode
//!
//! Replacing a Mach-O binary in place invalidates its code signature. The
//! kernel does not fail the `execve` with an errno; it **SIGKILLs the process
//! mid-exec**, after the old image is already gone. The session dies silently,
//! with the only evidence in the kernel log:
//!
//! ```text
//! AMFI: '/path/to/gwae' has no CMS blob?
//! proc 1234: load code signature error 2 for file "gwae"
//! ```
//!
//! That is why `reload::is_loadable` proves the new image runs in a throwaway
//! child first, and why `a_broken_new_binary_is_refused` exists.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A gwae running from a *copy* of the test binary, so a test can replace
/// that copy underneath it without touching the build tree.
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    dir: PathBuf,
    /// The binary this gwae is running from; replacing it triggers a reload.
    bin: PathBuf,
    output: Arc<Mutex<Vec<u8>>>,
}

/// Install `src` at `dst` the way a real upgrade does: write a new file, make
/// it executable, re-sign it, then `rename` it into place.
///
/// The rename matters. Writing over the running file byte by byte can be
/// observed half-done, and on macOS an unsigned or stale-signed image is
/// SIGKILLed on exec rather than rejected cleanly.
fn install_binary(src: &Path, dst: &Path) {
    let tmp = dst.with_extension("new");
    std::fs::copy(src, &tmp).expect("copy new binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .expect("chmod new binary");
    }
    // Ad-hoc sign, as a locally built binary is. Without this the kernel
    // kills the exec; see the module docs.
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("codesign")
            .args(["-f", "-s", "-"])
            .arg(&tmp)
            .output();
    }
    std::fs::rename(&tmp, dst).expect("rename new binary into place");
}

impl Session {
    /// Start gwae with hot reload enabled, running `pane_cmd` in its pane.
    fn start(pane_cmd: &str) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gwae-reload-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), "").expect("write config");

        // Run from a copy: the test replaces this file, and the build tree's
        // binary must never be touched. Named `gwae-bin` because `<dir>/gwae`
        // is already the config directory.
        let bin = dir.join("gwae-bin");
        install_binary(Path::new(env!("CARGO_BIN_EXE_gwae")), &bin);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(&bin);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.env("SHELL", "/bin/sh");
        cmd.env("GWAE_DEV_RELOAD", "1");
        cmd.arg("run");
        cmd.arg(pane_cmd);
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("writer");
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let output = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut b = [0u8; 8192];
            while let Ok(n) = reader.read(&mut b) {
                if n == 0 {
                    break;
                }
                sink.lock().expect("lock").extend_from_slice(&b[..n]);
            }
        });

        std::thread::sleep(Duration::from_secs(3));
        Session {
            child,
            _master: pair.master,
            writer,
            dir,
            bin,
            output,
        }
    }

    fn type_line(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).expect("write");
        self.writer.write_all(b"\r").expect("newline");
        self.writer.flush().expect("flush");
    }

    /// Replace the running binary, which is what triggers a hot reload.
    fn trigger_reload(&self) {
        install_binary(Path::new(env!("CARGO_BIN_EXE_gwae")), &self.bin);
    }

    /// Replace the running binary with something that cannot be executed.
    fn install_broken_binary(&self) {
        let tmp = self.bin.with_extension("new");
        std::fs::write(&tmp, b"#!/nonexistent/interpreter\ngarbage\n").expect("write junk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::rename(&tmp, &self.bin).expect("rename junk into place");
    }

    fn pid(&self) -> u32 {
        self.child.process_id().expect("gwae pid")
    }

    fn signal(&self, sig: libc::c_int) {
        // Safety: `kill(2)` on a pid we spawned and still own.
        unsafe {
            libc::kill(self.pid() as libc::pid_t, sig);
        }
    }

    fn screen(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().expect("lock")).into_owned()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn any_running(needle: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-Ao", "command="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.contains("-Ao"))
        .any(|l| l.contains(needle))
}

fn wait_until_running(needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if any_running(needle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{needle} never started; the test cannot prove anything");
}

fn gone_within(needle: &str, how_long: Duration) -> bool {
    let deadline = Instant::now() + how_long;
    while Instant::now() < deadline {
        if !any_running(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// True while `pid` exists and is not a zombie.
///
/// A plain `kill(pid, 0)` is not enough: a child this process spawned but has
/// not waited on lingers as a zombie and still answers signal 0, which would
/// make a dead gwae look alive.
fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

struct Reaper(&'static str);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = std::process::Command::new("pkill")
            .args(["-9", "-f", self.0])
            .output();
    }
}

/// The headline promise: new code, same panes.
#[test]
fn a_reload_swaps_the_binary_and_keeps_the_pane_running() {
    const MARKER: &str = "sleep 24801";
    let _reap = Reaper(MARKER);

    let mut s = Session::start("sh -i");
    let gwae_pid = s.pid();
    // Something long-lived *inside* the pane, so "the pane survived" is a
    // claim about a real process rather than about pixels.
    s.type_line(&format!("{MARKER} &"));
    wait_until_running(MARKER);

    s.trigger_reload();
    // Settle window + exec + adopt.
    std::thread::sleep(Duration::from_secs(6));

    assert!(
        alive(gwae_pid),
        "gwae must still be running, with the same pid: execve replaces the \
         image, it does not create a process"
    );
    assert!(
        any_running(MARKER),
        "the pane's work must survive a reload; that is the entire point"
    );
}

/// The dangerous one. Signal handlers are reset by `execve`, so a reloaded
/// gwae that does not re-arm the reaper leaks every pane's detached jobs.
#[test]
fn teardown_still_reaps_detached_work_after_a_reload() {
    const MARKER: &str = "sleep 24802";
    let _reap = Reaper(MARKER);

    let mut s = Session::start("sh -i");
    // `nohup ... &` deliberately escapes the pane's process group: exactly
    // the case that survives a naive teardown.
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    s.trigger_reload();
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        any_running(MARKER),
        "precondition: the job should still be running after the reload"
    );

    // Now the thing that must still work: a fatal signal to the *reloaded*
    // image has to take the detached job with it.
    s.signal(libc::SIGTERM);

    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "a detached job survived SIGTERM after a hot reload: the new image \
         did not re-arm the reaper, so gwae now leaks processes it promises \
         to kill"
    );
}

/// SIGHUP is what a closed terminal window sends, so it is the likeliest way
/// a reloaded session ends on a real machine.
#[test]
fn sighup_still_reaps_after_a_reload() {
    const MARKER: &str = "sleep 24803";
    let _reap = Reaper(MARKER);

    let mut s = Session::start("sh -i");
    s.type_line(&format!("nohup {MARKER} >/dev/null 2>&1 &"));
    wait_until_running(MARKER);

    s.trigger_reload();
    std::thread::sleep(Duration::from_secs(6));

    s.signal(libc::SIGHUP);
    assert!(
        gone_within(MARKER, Duration::from_secs(10)),
        "closing the window after a reload left a detached job running"
    );
}

/// A binary that cannot be exec'd must not cost the user their session.
///
/// This is the macOS code-signature case in disguise: `is_loadable` runs the
/// candidate first, so a bad image is reported instead of SIGKILLing us
/// mid-`execve`.
#[test]
fn a_broken_new_binary_is_refused_and_the_session_survives() {
    const MARKER: &str = "sleep 24804";
    let _reap = Reaper(MARKER);

    let mut s = Session::start("sh -i");
    let gwae_pid = s.pid();
    s.type_line(&format!("{MARKER} &"));
    wait_until_running(MARKER);

    s.install_broken_binary();
    std::thread::sleep(Duration::from_secs(6));

    assert!(
        alive(gwae_pid),
        "a broken build must not take the session down; the reload should be \
         refused before this process commits to becoming the new image"
    );
    assert!(
        any_running(MARKER),
        "and the pane's work must be untouched by a refused reload"
    );
    let screen = s.screen();
    assert!(
        screen.contains("reload"),
        "the refusal should say so on screen rather than failing silently; \
         got tail: {:?}",
        &screen[screen.len().saturating_sub(400)..]
    );
}

/// Reload must be opt-in: a normal session ignores a changed binary entirely.
#[test]
fn reload_does_nothing_unless_it_is_enabled() {
    const MARKER: &str = "sleep 24805";
    let _reap = Reaper(MARKER);

    // Same setup, minus GWAE_DEV_RELOAD.
    let dir = std::env::temp_dir().join(format!("gwae-reload-off-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("gwae")).expect("config dir");
    std::fs::write(dir.join("gwae/gwae.toml"), "").expect("config");
    let bin = dir.join("gwae-bin");
    install_binary(Path::new(env!("CARGO_BIN_EXE_gwae")), &bin);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(&bin);
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd.env("TERM", "xterm-256color");
    cmd.env("SHELL", "/bin/sh");
    cmd.arg("run");
    cmd.arg("sh -i");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);
    let mut writer = pair.master.take_writer().expect("writer");
    let mut reader = pair.master.try_clone_reader().expect("reader");
    std::thread::spawn(move || {
        let mut b = [0u8; 8192];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 {
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_secs(3));
    let pid = child.process_id().expect("pid");
    writer
        .write_all(format!("{MARKER} &\r").as_bytes())
        .expect("write");
    writer.flush().expect("flush");
    wait_until_running(MARKER);

    // Replace the binary. With reload disabled this must be a non-event.
    install_binary(Path::new(env!("CARGO_BIN_EXE_gwae")), &bin);
    std::thread::sleep(Duration::from_secs(5));

    assert!(alive(pid), "the session should be untouched");
    assert!(
        any_running(MARKER),
        "and so should its panes: nothing opted into reloading"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The layout itself must survive, not just one pane.
///
/// Every other test here uses a single pane, which would pass even if the
/// handover dropped the tree entirely and started fresh with one pane. This
/// one builds a real grid first, so "the session survived" means the
/// *arrangement* survived too, and each pane still owns its own live child.
#[test]
fn a_multi_pane_layout_survives_a_reload() {
    const A: &str = "sleep 24806";
    const B: &str = "sleep 24807";
    let _ra = Reaper(A);
    let _rb = Reaper(B);

    let mut s = Session::start("sh -i");
    let gwae_pid = s.pid();
    // Mark pane 1, then open a second column and mark that one. Two distinct
    // markers means the test can tell "both panes survived" from "one pane
    // survived and the other was silently recreated".
    //
    // `nohup ... &` rather than a plain `&` in both panes, and that detail is
    // the whole test. A plain background job dies with its shell when the PTY
    // hangs up, so it disappears on SIGTERM whether or not the reaper knows
    // about that pane — which makes the leak check pass even when adoption
    // drops a pane entirely (confirmed by sabotage). A nohup'd job survives
    // the hangup, so it is only reaped if that pane's pid was genuinely
    // re-registered after the reload.
    s.type_line(&format!("nohup {A} >/dev/null 2>&1 &"));
    wait_until_running(A);
    // ⌥+Enter: new column, which spawns a shell in it.
    s.writer.write_all(b"\x1b\r").expect("new column");
    s.writer.flush().expect("flush");
    std::thread::sleep(Duration::from_secs(3));
    s.type_line(&format!("nohup {B} >/dev/null 2>&1 &"));
    wait_until_running(B);

    s.trigger_reload();
    std::thread::sleep(Duration::from_secs(6));

    assert!(
        alive(gwae_pid),
        "the session must survive with the same pid"
    );
    assert!(
        any_running(A) && any_running(B),
        "both panes' children must survive: A={} B={}",
        any_running(A),
        any_running(B)
    );

    // And the grid must still be a grid. Two columns paint two vertical
    // dividers' worth of chrome; a layout that reset to a single pane would
    // not.
    let screen = s.screen();
    assert!(
        screen.contains('│'),
        "the reloaded screen should still be drawing pane chrome"
    );

    // The decisive check: teardown still reaches *both* panes' detached work
    // after the reload, so a multi-pane session cannot leak on exit.
    s.signal(libc::SIGTERM);
    // Both waits are given their own full budget, and the results are
    // collected before asserting. Writing this as `gone_within(A) &&
    // gone_within(B)` short-circuits: when A is merely slow (six of these
    // tests reload real binaries in parallel), B is never polled at all and
    // the failure blames the wrong pane. Observed once as exactly that.
    let a_gone = gone_within(A, Duration::from_secs(15));
    let b_gone = gone_within(B, Duration::from_secs(15));
    assert!(
        a_gone && b_gone,
        "every adopted pane must be re-registered with the reaper, not just \
         the first: pane A reaped={a_gone}, pane B reaped={b_gone}"
    );
}
