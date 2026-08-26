//! End-to-end: a gwae that nobody is touching must not heat the machine.
//!
//! The bug this locks down was invisible on screen and obvious on a laptop:
//! an idle mux sat at roughly 3.5% of a core *forever*, fans up and chassis
//! warm, for a screen that was not changing. Two things in the render loop
//! ran unconditionally at the input poll rate (500 iterations/second at the
//! default `input_poll_ms = 2`):
//!
//! 1. `refresh_size` — a `TIOCGWINSZ` that crossterm implements on macOS by
//!    opening and closing `/dev/tty`, so ~500 `open`/`close` syscall pairs a
//!    second. This dominated the profile.
//! 2. the 2 ms `event::poll` itself, waking the process 500x a second to
//!    find an empty queue.
//!
//! Unit tests cover the poll arithmetic. This file is the acceptance check:
//! it launches a *real* gwae on a real PTY, leaves it completely alone, and
//! measures the CPU time the kernel actually charged it. That is the only
//! measurement that corresponds to the user-visible symptom.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::{Duration, Instant};

/// CPU seconds the process has consumed so far, via `ps`.
///
/// `ps -o time=` reports cumulative CPU time as `[[dd-]hh:]mm:ss[.cc]`;
/// sampling it twice and differencing gives CPU *used over the interval*
/// without the smoothing that `%cpu` applies.
fn cpu_seconds(pid: u32) -> Option<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    // Split off any leading day field, then parse right-to-left so the
    // shortest form (`mm:ss`) and the longest both work.
    let s = s.rsplit('-').next().unwrap_or(&s);
    let mut secs = 0.0;
    for (i, part) in s.rsplit(':').enumerate() {
        let v: f64 = part.parse().ok()?;
        secs += v * 60f64.powi(i as i32);
    }
    Some(secs)
}

/// Launch gwae on a PTY running a silent command, and drain its output.
///
/// The pane runs `sleep`, so *nothing* in the session produces output after
/// the first paint: any CPU measured afterwards is the loop spinning on its
/// own, which is exactly what is under test.
struct Idle {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    dir: std::path::PathBuf,
}

impl Idle {
    fn start() -> Idle {
        let dir = std::env::temp_dir().join(format!("gwae-idle-cpu-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        // An explicit config so the default poll rate is what is measured
        // and no first-run onboarding flow can appear instead of the mux.
        std::fs::write(dir.join("gwae/gwae.toml"), "input_poll_ms = 2\n").expect("write config");

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 50,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gwae"));
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("TERM", "xterm-256color");
        cmd.env("GWAE_NO_INSTALL", "1");
        cmd.arg("run");
        cmd.arg("sleep 120");
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
        drop(pair.slave);

        // Drain continuously: a full PTY buffer would block gwae's writes and
        // make it look idle for the wrong reason.
        let mut reader = pair.master.try_clone_reader().expect("reader");
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });

        Idle {
            child,
            _master: pair.master,
            dir,
        }
    }

    fn pid(&self) -> u32 {
        self.child.process_id().expect("child pid")
    }
}

impl Drop for Idle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A gwae nobody is touching must be nearly free.
///
/// The threshold is deliberately loose (1.5% of one core) relative to both
/// the old behaviour (~3.5%, and higher on a bigger terminal, since the
/// syscall storm scaled with the poll rate rather than the screen) and the
/// fixed behaviour (~0.9%, essentially all of it the poll timer itself). A
/// regression that reinstates per-iteration `/dev/tty` opens or drops the
/// idle backoff lands far above this line; normal scheduling noise does not.
#[test]
fn an_untouched_session_barely_uses_the_cpu() {
    let s = Idle::start();
    let pid = s.pid();

    // Let startup finish: the first paint, PTY spawn, and config read are all
    // real work that must not be counted against the idle steady state.
    std::thread::sleep(Duration::from_secs(3));

    let Some(start) = cpu_seconds(pid) else {
        // `ps` is unavailable (or the child already exited): nothing to
        // assert about, and failing here would be a bug in the harness, not
        // in gwae.
        return;
    };
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(5));
    let end = cpu_seconds(pid).expect("gwae still running after the idle window");
    let wall = t0.elapsed().as_secs_f64();

    let used = end - start;
    let pct = used / wall * 100.0;
    assert!(
        pct < 1.5,
        "idle gwae burned {pct:.2}% of a core ({used:.2}s CPU over {wall:.2}s wall); \
         it should be near zero when nothing on screen is changing"
    );
}
