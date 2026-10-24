//! The keep-awake lid helper (`gwae __lid-awake <pid>`) holds a kernel flag
//! that nothing else clears when its setter dies, so its exit paths are the
//! whole safety story: if any of them leaks, the Mac can no longer sleep on
//! lid close. These run the real binary and watch the real kernel state.
//!
//! The kernel flag itself is only observable indirectly (`ioreg` reports
//! `AppleClamshellCausesSleep`, which also folds in desktop mode and AC), so
//! the assertions here are about the helper's lifecycle and exit codes,
//! plus a final check that the flag is back off afterwards whenever the
//! machine reports a value at all.

#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_gwae")
}

/// Serializes the cases: they share one kernel flag.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn wait_gone(pid: u32, within: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < within {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !pid_alive(pid)
}

/// `AppleClamshellCausesSleep` from the root domain, when the machine has a
/// lid (`None` on desktops and in VMs).
fn clamshell_causes_sleep() -> Option<bool> {
    let out = Command::new("ioreg")
        .args(["-r", "-k", "AppleClamshellCausesSleep", "-d", "1"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains("\"AppleClamshellCausesSleep\""))?;
    Some(line.trim_end().ends_with("Yes"))
}

#[test]
fn helper_exits_when_its_parent_dies() {
    let _g = lock();
    let before = clamshell_causes_sleep();
    // `sh` is the parent: it starts the helper bound to its own pid, prints
    // the helper's pid, and exits. The helper must notice and leave.
    let script = format!("'{}' __lid-awake $$ & echo $!; sleep 1", bin());
    let out = Command::new("sh")
        .args(["-c", &script])
        .stderr(Stdio::inherit())
        .output()
        .expect("run sh");
    let helper: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("helper pid");
    assert!(
        wait_gone(helper, Duration::from_secs(3)),
        "lid helper {helper} outlived its parent: the override would stick"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        clamshell_causes_sleep(),
        before,
        "lid-close sleep policy must be back to where it started"
    );
}

#[test]
fn helper_clears_and_exits_zero_on_sigterm() {
    let _g = lock();
    let before = clamshell_causes_sleep();
    let mut child = Command::new(bin())
        .args(["__lid-awake", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "helper must stay up while its parent (this test) lives"
    );
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "helper ignored SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(status.code(), Some(0), "SIGTERM is an orderly release");
    assert_eq!(clamshell_causes_sleep(), before);
}

#[test]
fn helper_refuses_a_pid_that_is_not_its_parent() {
    let _g = lock();
    // pid 1 is never our parent: the helper must not hold anything for it.
    let start = Instant::now();
    let status = Command::new(bin())
        .args(["__lid-awake", "1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run helper");
    assert!(status.success());
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn helper_rejects_missing_or_garbage_pid() {
    for args in [&["__lid-awake"][..], &["__lid-awake", "nope"][..]] {
        let status = Command::new(bin())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run helper");
        assert_eq!(status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn helper_stays_out_of_help() {
    let out = Command::new(bin()).arg("--help").output().expect("help");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("lid-awake"),
        "internal helper leaked into --help"
    );
}
