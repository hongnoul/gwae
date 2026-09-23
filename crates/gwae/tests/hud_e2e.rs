//! End-to-end: the ⌥-hold dashboard, driven through a real PTY.
//!
//! The panel only exists while the modifier is down, so nothing about it can
//! be observed from a unit test of the layout: the reveal, its contents, and
//! its disappearance are all products of key *timing* in the real event loop.
//! These tests therefore run the actual binary and read the bytes it paints.

// Unix-only end to end: every session here drives a real PTY running
// `sh` syntax (and asserts with unix tooling); Windows has neither.
#![cfg(unix)]

use gwae_term::{Size, TermGrid, Vt100Grid};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

struct Session {
    rx: Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    screen: Vt100Grid,
    dir: std::path::PathBuf,
}

impl Session {
    fn start(config: &str) -> Session {
        Self::start_with_helper(
            config,
            "#!/bin/sh\nprintf '\\033]0;watchdog\\007'\nexec sleep 60\n",
            140,
            false,
        )
    }

    fn start_with_helper(config: &str, helper: &str, cols: u16, all_panes: bool) -> Session {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("JCODE_SCRATCH_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "gwae-hud-e2e-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
        std::fs::write(dir.join("gwae/gwae.toml"), config).expect("write config");
        let helper_path = dir.join("hud-helper.sh");
        std::fs::write(&helper_path, helper).expect("write pane helper");
        #[cfg(unix)]
        if all_panes {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&helper_path, std::fs::Permissions::from_mode(0o700))
                .expect("executable helper shell");
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let executable =
            std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
        let mut cmd = CommandBuilder::new(executable);
        cmd.env_clear();
        cmd.cwd(&dir);
        cmd.env("HOME", &dir);
        cmd.env("XDG_CONFIG_HOME", &dir);
        cmd.env("XDG_CACHE_HOME", dir.join("cache"));
        cmd.env("XDG_DATA_HOME", dir.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env(
            "SHELL",
            if all_panes {
                helper_path.as_os_str()
            } else {
                "/bin/sh".as_ref()
            },
        );
        cmd.env("PATH", "/usr/bin:/bin");
        cmd.env("ENV", "/dev/null");
        cmd.env("GWAE_NO_UPDATE_CHECK", "1");
        cmd.env("GWAE_NO_KEEP_AWAKE", "1");
        cmd.env("GWAE_LOG", "off");
        // The fixture exercises protocol chords, not the user's live keyboard.
        cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
        cmd.arg("run");
        // A pane that sets its own window title, so the dashboard has a real
        // OSC 0/2 name to show rather than a fixture we injected ourselves.
        // A file avoids relying on `run` to preserve nested shell quoting.
        cmd.arg("/bin/sh hud-helper.sh");
        let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
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
        Session {
            rx,
            writer,
            child,
            _master: pair.master,
            screen: Vt100Grid::new(Size { rows: 30, cols }),
            dir,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write keys");
        self.writer.flush().expect("flush");
    }

    /// Read until output goes quiet, and return it.
    fn drain(&mut self) -> String {
        let mut out = Vec::new();
        let mut idle = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(b) => {
                    self.screen.feed(&b);
                    out.extend_from_slice(&b);
                    idle = 0;
                }
                Err(_) => {
                    idle += 1;
                    if idle >= 3 {
                        break;
                    }
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Read for a fixed short window, without waiting for quiet. The panel is
    /// up for ~180ms after a chord on a terminal with no release reporting, so
    /// `drain` (which waits for silence) would always miss it.
    fn peek(&mut self, ms: u64) -> String {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            if let Ok(b) = self.rx.recv_timeout(Duration::from_millis(20)) {
                self.screen.feed(&b);
                out.extend_from_slice(&b);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn kill(self) {
        // Drop also tears down sessions when an assertion panics.
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Alt chords as a terminal that maps Option to Meta sends them: ESC + key.
fn alt(key: u8) -> Vec<u8> {
    vec![0x1b, key]
}
const ALT_ENTER: &[u8] = b"\x1b\r";

/// A quiet TUI without OSC 133. Redraw only for real SIGWINCH or input, and
/// log signals independently so the test cannot pass by hiding status changes.
#[cfg(unix)]
const RESIZE_HELPER: &str = r#"#!/bin/sh
stty -echo
printf '\033]2;pane-%s\007' "$GWAE_PANE"
redraw() {
    printf '\033[2J\033[HREADY %s' "$GWAE_PANE"
}
trap 'printf "%s\n" "$GWAE_PANE" >> winch.log; redraw' WINCH
trap 'exit 0' HUP TERM
redraw
while :; do
    if read -r key; then
        printf '\033]133;C\007\033[2;1HWORK %s' "$GWAE_PANE"
    fi
done
"#;

#[cfg(unix)]
#[test]
fn focusing_attention_panes_never_turns_redraws_into_work() {
    // Fractional quarter widths reproduce the one-cell rounding-phase resize.
    // Eight panes also leave startup panes offscreen until first visited.
    let mut s = Session::start_with_helper(
        "startup_panes = 8\n\
         ",
        RESIZE_HELPER,
        142,
        true,
    );
    // Hold the modifier via Kitty key reporting, independently of the user's
    // keyboard. This keeps the dashboard visible even on a slow test runner.
    s.send(b"\x1b[57443;3u");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        s.peek(50);
        let text = s.screen.visible_text();
        if (1..=8).all(|n| text.contains(&format!("·{n}"))) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "panes never became plain: {text}"
        );
    }
    let before = std::fs::read_to_string(s.dir.join("winch.log")).unwrap_or_default();
    // Both directions, including the first focus of previously hidden panes.
    for (key, pane) in (1..8)
        .map(|p| (b'l', p))
        .chain((0..7).rev().map(|p| (b'h', p)))
    {
        s.send(&alt(key));
        let mut painted = s.peek(250);
        // The move itself may paint late on a loaded runner: keep reading
        // until the title flips (bounded), but judge the no-`»` invariant
        // over everything painted since the chord.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while s.screen.title() != format!("pane-{pane}") {
            assert!(
                std::time::Instant::now() < deadline,
                "focus must actually move: title {:?}, wanted pane-{pane}",
                s.screen.title()
            );
            painted.push_str(&s.peek(100));
        }
        assert!(s.screen.visible_text().contains('╭'), "HUD must be visible");
        assert!(
            s.screen.visible_text().contains("·1"),
            "plain pane tiles must be visible"
        );
        assert!(
            !visible(&painted).contains('»'),
            "focus alone must never paint a running status: {}",
            s.screen.visible_text()
        );
    }
    assert_eq!(
        std::fs::read_to_string(s.dir.join("winch.log")).unwrap_or_default(),
        before,
        "navigation must not send SIGWINCH to idle children"
    );
    // `Running` is an agent claim: these panes are plain shells (spawned via
    // `run`, not `⌥+;`), so even a genuine OSC 133;C from one of them (an
    // editor like lazyvim, a pager, any foreground command) must not paint a
    // working status. The tile stays neutral.
    s.send(b"work\r");
    s.peek(250);
    assert!(
        !visible(&s.screen.visible_text()).contains('»'),
        "a plain pane's 133;C must never claim running: {}",
        s.screen.visible_text()
    );
    assert!(
        s.screen.visible_text().contains("·1"),
        "the plain pane's tile must stay neutral: {}",
        s.screen.visible_text()
    );
}

/// Add `n` columns to the focused strip, letting each PTY settle.
fn widen(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.send(ALT_ENTER);
        std::thread::sleep(Duration::from_millis(150));
    }
    let _ = s.drain();
}

/// Strip SGR/CSI escapes so assertions read the text the user sees. Panel
/// text is heavily styled per cell, so the raw stream interleaves colour
/// sequences between almost every character.
fn visible(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: consume through the final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: consume through BEL or ST.
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' || c == '\u{1b}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the centered dashboard frame is on screen. Pane frames start at
/// column 0; the dashboard is centered, so its `╭` sits at x > 0. Reads the
/// emulator grid (which tracks cursor positioning) rather than the raw byte
/// stream (which has no newlines).
fn panel_up(s: &Session) -> bool {
    use gwae_term::TermGrid;
    (0..30).any(|y| (1..140).any(|x| s.screen.cell(x, y).ch == '╭'))
}

#[test]
fn holding_the_modifier_reveals_a_dashboard_that_names_its_panes() {
    // Spatial-only: the panel shows geometry (color + address + marker), not
    // titles. Titles live in pane chrome; the HUD stays uncluttered.
    let mut s = Session::start("");
    let _ = s.drain();
    widen(&mut s, 3);

    // A chord holds the modifier open for a short window even on terminals
    // that never report a bare Option press, which is what most terminals do.
    // Re-reveal until the frame is up: on a loaded runner one peek can miss
    // the ~180ms hold window.
    let shown = visible(&reveal_raw(&mut s));
    // Spatial-only tiles: status glyph + column address, no hint text.
    // Plain panes (no harness, no protocol) carry the neutral `·`.
    assert!(
        shown.contains("»") || shown.contains("!") || shown.contains("·"),
        "dashboard should show spatial tiles (glyphs); got:\n{shown:?}"
    );
    for token in ["attention", "1-9", "hjkl"] {
        assert!(
            !shown.contains(token),
            "dashboard must not carry hints, found {token:?}:\n{shown:?}"
        );
    }
    // The panel is transient: once the hold lapses it must clean up after
    // itself rather than leaving a box painted over live panes.
    std::thread::sleep(Duration::from_millis(400));
    // A queued held-frame may arrive after peek's deadline. It is legitimate
    // as long as the later erase removes it from the final displayed screen.
    // Concatenating repaint bytes would falsely keep that old frame forever.
    let _ = s.drain();
    let after = s.screen.visible_text();
    assert!(
        !panel_up(&s),
        "the panel must not outlive the hold; got:\n{after:?}"
    );
    s.kill();
}

#[test]
fn a_lone_pane_still_reveals_its_dashboard() {
    // One pane still gets a one-tile dashboard with its tally, so the hold
    // always answers where focus is instead of going silent.
    let mut s = Session::start("startup_panes = 1\n");
    let _ = s.drain();

    // Retry: on a loaded runner one peek can miss the ~180ms hold window.
    let shown = visible(&reveal_raw(&mut s));
    assert!(
        panel_up(&s),
        "one pane still paints a dashboard; got:\n{shown:?}"
    );
    s.kill();
}

#[test]
fn option_digits_reach_the_pane_instead_of_jumping() {
    // Column jump is gone: Option+digits belong to the child (readline word
    // ops, vim counts) and must not focus anything or echo a jump toast.
    let mut s = Session::start("");
    let _ = s.drain();
    widen(&mut s, 3);

    let before = live_columns(&mut s);
    s.send(&alt(b'2'));
    let shown = visible(&s.peek(150));
    s.kill();
    assert!(
        !shown.contains("column 2"),
        "no jump toast for Option+2; got:\n{shown:?}"
    );
    assert_eq!(before.len(), 4, "four columns stay alive: {before:?}");
}

/// Count dashboard tiles by their column addresses: the first pane of each
/// column prints `glyph + column number` (`»1`, `·2`, `!3`, ...), so the set
/// of addresses present is the set of live columns.
fn tile_addresses(text: &str) -> Vec<usize> {
    (1..=16)
        .filter(|n| {
            let n = n.to_string();
            ["»", "·", "!", "✗"]
                .iter()
                .any(|g| text.contains(&format!("{g}{n}")))
        })
        .collect()
}

/// Reveal the dashboard and return the live column addresses.
fn live_columns(s: &mut Session) -> Vec<usize> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        s.send(&alt(b'h'));
        s.peek(150);
        let addrs = tile_addresses(&s.screen.visible_text());
        if !addrs.is_empty() {
            return addrs;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dashboard never revealed tiles: {}",
            s.screen.visible_text()
        );
    }
}

/// Reveal the dashboard and return the raw bytes of the window where the
/// frame was up, resending the chord until `panel_up` holds. Raw bytes (not
/// the emulator screen) because some assertions strip styling from the
/// stream itself.
fn reveal_raw(s: &mut Session) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        s.send(&alt(b'h'));
        let raw = s.peek(150);
        if panel_up(s) {
            return raw;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dashboard never appeared; last got:\n{:?}",
            visible(&raw)
        );
    }
}

/// A fake `jcode` that answers `debug clients:map` with one session whose
/// lifecycle status comes from `$GWAE_DOUBLE_STATUS` (`ready` or `running`).
/// The agent pane titles itself `jcode pond` (a real OSC 0/2 title, the same
/// channel a live harness uses).
///
/// The double serves both roles: with `debug clients:map` argv it answers
/// the poller; otherwise (spawned as the pane's harness) it titles the pane
/// and then behaves per `$GWAE_DOUBLE_MODE`: `chatty` ticks forever (a done
/// client the heuristic would hold at `Running`), `quiet` prints once and
/// goes silent (a generating client in a quiet stretch the heuristic would
/// flap to `!`).
#[cfg(unix)]
const DAEMON_DOUBLE: &str = r#"#!/bin/sh
if [ "$1" = "debug" ] && [ "$2" = "clients:map" ]; then
    printf '{"count":1,"clients":[{"session_id":"session_pond_1_abc","friendly_name":"pond","status":"%s","working_dir":"/tmp"}]}' "${GWAE_DOUBLE_STATUS:-ready}"
    exit 0
fi
printf '\033]0;jcode pond\007'
if [ "${GWAE_DOUBLE_MODE:-chatty}" = "quiet" ]; then
    printf 'working'
    exec sleep 60
fi
while :; do printf '\033[2;1Htick %s' "$(date +%S)"; sleep 1; done
"#;

/// Spawn gwae with the daemon double on PATH and no `run` arg, so pane 1.1
/// boots the startup-harness path as a genuine agent pane running the
/// double. `status` is the daemon verdict (`ready`/`running`), `mode` the
/// pane behavior (`chatty`/`quiet`). The double inherits both via env.
#[cfg(unix)]
fn spawn_with_double(status: &str, mode: &str, tag: &str) -> Session {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::var_os("JCODE_SCRATCH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = root.join(format!("gwae-hud-daemon-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("gwae")).expect("temp config dir");
    std::fs::create_dir_all(dir.join("bin")).expect("temp bin dir");
    std::fs::write(dir.join("bin/jcode"), DAEMON_DOUBLE).expect("write double");
    std::fs::set_permissions(
        dir.join("bin/jcode"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod double");
    // No `run` command: pane 1.1 resolves the lone `jcode` on PATH as its
    // startup harness, so it is a genuine agent pane running the double.
    std::fs::write(dir.join("gwae/gwae.toml"), "startup_panes = 1\n").expect("write config");

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 140,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let executable =
        std::env::var_os("GWAE_E2E_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_gwae").into());
    let mut cmd = CommandBuilder::new(executable);
    cmd.env_clear();
    cmd.cwd(&dir);
    cmd.env("HOME", &dir);
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd.env("XDG_CACHE_HOME", dir.join("cache"));
    cmd.env("XDG_DATA_HOME", dir.join("data"));
    cmd.env("TERM", "xterm-256color");
    cmd.env("SHELL", "/bin/sh");
    cmd.env(
        "PATH",
        format!("{}:/usr/bin:/bin", dir.join("bin").display()),
    );
    cmd.env("GWAE_DOUBLE_STATUS", status);
    cmd.env("GWAE_DOUBLE_MODE", mode);
    cmd.env("ENV", "/dev/null");
    cmd.env("GWAE_NO_UPDATE_CHECK", "1");
    cmd.env("GWAE_NO_KEEP_AWAKE", "1");
    cmd.env("GWAE_LOG", "off");
    cmd.env("GWAE_NO_NATIVE_MODIFIERS", "1");
    // No `run` arg: pane 1.1 boots the startup-harness path.
    let child = pair.slave.spawn_command(cmd).expect("spawn gwae");
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
    Session {
        rx,
        writer,
        child,
        _master: pair.master,
        screen: Vt100Grid::new(Size {
            rows: 30,
            cols: 140,
        }),
        dir,
    }
}

/// A finished agent TUI keeps repainting (spinner frames, ambient toasts),
/// so the activity heuristic alone holds `Running` forever. The daemon's
/// `ready` verdict must demote that tile to `!` (idle, wants attention).
#[cfg(unix)]
#[test]
fn a_settled_daemon_session_demotes_a_chatty_running_tile() {
    let mut s = spawn_with_double("ready", "chatty", "settled");
    let _ = s.drain();
    // The pane ticks forever, so the activity heuristic alone would claim
    // `Running`; the daemon says `ready`, so the tile must settle at `!`
    // within a few poll intervals and never stick at `»`.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let shown = visible(&reveal_raw(&mut s));
        assert!(
            !shown.contains("»1"),
            "daemon says ready but the tile is stuck running: {shown:?}"
        );
        if shown.contains("!1") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tile never settled to idle: {shown:?}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    s.kill();
}

/// A generating agent in a quiet stretch (a long tool call with no redraw)
/// goes silent past the quiet window. The heuristic alone would flap the
/// tile to `!` mid-turn; the daemon's `running` verdict must hold it at `»`.
#[cfg(unix)]
#[test]
fn a_busy_daemon_session_holds_a_quiet_tile_running() {
    let mut s = spawn_with_double("running", "quiet", "busy");
    let _ = s.drain();
    // Wait out the quiet window first: the tile may legitimately start at
    // `»` (fresh output) and only the daemon keeps it there once silence
    // passes 4s. From then on it must never flap to `!`.
    std::thread::sleep(Duration::from_secs(6));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let shown = visible(&reveal_raw(&mut s));
        assert!(
            !shown.contains("!1"),
            "daemon says running but the tile flapped to idle: {shown:?}"
        );
        if shown.contains("»1") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tile never held running: {shown:?}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    s.kill();
}

/// A harness pane has no OSC 133 (jcode emits none), so the daemon's
/// failure verdict is its *only* path to `✗`. A crashed session's tile
/// must paint the failed glyph, not the generic wants-attention `!` the
/// settled path produces.
#[cfg(unix)]
#[test]
fn a_failed_daemon_session_paints_the_failed_glyph() {
    let mut s = spawn_with_double("failed", "chatty", "failed");
    let _ = s.drain();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let shown = visible(&reveal_raw(&mut s));
        if shown.contains("✗1") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "tile never showed the failure verdict: {shown:?}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    s.kill();
}

/// The verdict-then-prompt read that made `Failed` unrenderable before the
/// sticky rule: a shell-integrated pane emits `133;D;1` (failed) and its
/// prompt's `133;A` in one burst. The ✗ must survive to the dashboard, and
/// a new command start must clear it back to a live status.
#[cfg(unix)]
#[test]
fn a_shell_failure_verdict_survives_its_own_prompt() {
    // The helper emits the exact burst a real fish/zsh integration writes
    // after a failed command, then waits; on any input it starts a new
    // "command" (133;C), which must clear the verdict.
    const FAILING_SHELL: &str = "#!/bin/sh\n\
        stty -echo\n\
        printf '\\033]2;pane-failed\\007'\n\
        printf '\\033]133;D;1\\007\\033]133;A\\007$ '\n\
        while read -r _; do\n\
            printf '\\033]133;C\\007working...'\n\
        done\n";
    let mut s = Session::start_with_helper("startup_panes = 1\n", FAILING_SHELL, 140, true);
    let _ = s.drain();
    // The ✗ verdict must reach the dashboard tile and stick there.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let shown = visible(&reveal_raw(&mut s));
        if shown.contains("✗1") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the failure verdict never survived the prompt: {shown:?}"
        );
    }
    // A new command start clears it: the pane moved on. (The helper is a
    // plain pane, so `133;C` reads neutral `·`, not the agent-only `»`.)
    s.send(b"\r");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let shown = visible(&reveal_raw(&mut s));
        if !shown.contains("✗1") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a new command must clear the stale verdict: {shown:?}"
        );
    }
    s.kill();
}

/// Reveal the dashboard until `cond` holds on the emulator screen.
/// Re-sending the chord is free (each press re-opens the hold), so a loaded
/// runner that catches a mid-paint frame just retries instead of flaking.
fn reveal_until(s: &mut Session, cond: impl Fn(&str) -> bool, what: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        s.send(&alt(b'h'));
        s.peek(150);
        let text = s.screen.visible_text();
        if panel_up(s) && cond(&text) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}; last got:\n{text:?}"
        );
    }
}

#[test]
fn dashboard_window_follows_focus_past_max_rows() {
    // Eight strips with room for six (the default `max_rows`): the window
    // must follow focus so the focused strip always has a tile, instead of
    // pinning the first rows and stranding focus past line 6.
    //
    // Kitty CSI-u `ESC[13;4u` is the wire form of Alt+Shift+Enter (13 =
    // Enter, mods 4 = Alt+Shift); `ESC q`-style chords are deliberate
    // single presses, as in the kill-repeat test above.
    const NEW_STRIP: &[u8] = b"\x1b[13;4u";
    let mut s = Session::start("startup_panes = 1\n");
    let _ = s.drain();
    for _ in 0..7 {
        s.send(NEW_STRIP);
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = s.drain();
    // Eight strips exist: the dashboard must say exactly two are cut, which
    // also proves every Alt+Shift+Enter landed (fewer strips → "+1 strip").
    // New strips take focus, so the window hangs off the bottom: cut above
    // only, never below.
    let bottom = reveal_until(
        &mut s,
        |t| t.contains("+2 strip"),
        "eight strips never counted",
    );
    assert!(
        bottom.contains('↑') && !bottom.contains('↓'),
        "bottom focus cuts above only; got:\n{bottom:?}"
    );
    // Walk focus back to the first strip: the window returns to the top.
    for _ in 0..7 {
        s.send(&alt(b'k'));
        std::thread::sleep(Duration::from_millis(150));
    }
    let _ = s.drain();
    let top = reveal_until(&mut s, |t| t.contains("+2 strip"), "strips lost walking up");
    assert!(
        top.contains('↓') && !top.contains('↑'),
        "top focus cuts below only; got:\n{top:?}"
    );
    s.kill();
}

#[test]
fn held_kill_repeats_cannot_outrun_the_dashboard() {
    // Regression for the stale-HUD kill bug: holding ⌥+q fired key
    // auto-repeat faster than frames, so every queued repeat landed in one
    // drain batch and several panes died while the dashboard still showed
    // the first frame. Repeats of the kill chord must kill nothing; each
    // deliberate press kills exactly one pane, with a repaint between, so
    // the dashboard always shows what is actually alive.
    //
    // Kitty CSI-u `ESC[113;3:2u` is the wire form of Alt+q auto-repeat
    // (codepoint 113 = `q`, mods 3 = Alt, event type 2 = repeat); plain
    // `ESC q` is one deliberate press.
    const KILL_REPEAT: &[u8] = b"\x1b[113;3:2u";
    let mut s = Session::start("startup_panes = 4\n");
    let _ = s.drain();

    assert_eq!(
        live_columns(&mut s),
        vec![1, 2, 3, 4],
        "four panes start alive"
    );

    // A held key: five repeats must not kill anything.
    for _ in 0..5 {
        s.send(KILL_REPEAT);
        std::thread::sleep(Duration::from_millis(60));
    }
    let _ = s.drain();
    assert_eq!(
        live_columns(&mut s),
        vec![1, 2, 3, 4],
        "held ⌥+q repeats must kill nothing; the dashboard must still show all four"
    );

    // One deliberate press kills exactly one pane.
    s.send(&alt(b'q'));
    std::thread::sleep(Duration::from_millis(800));
    let _ = s.drain();
    assert_eq!(
        live_columns(&mut s),
        vec![1, 2, 3],
        "one ⌥+q press kills exactly one pane and the dashboard tracks it"
    );
    s.kill();
}

#[test]
fn dashboard_footer_is_tally_not_key_hints() {
    let mut s = Session::start("");
    let _ = s.drain();
    widen(&mut s, 3);
    // Retry: on a loaded runner one peek can catch the frame mid-paint or
    // after the hold lapses.
    let shown = visible(&reveal_raw(&mut s));
    assert!(panel_up(&s), "dashboard still appears: {shown:?}");
    assert!(
        shown.contains('»') || shown.contains('!') || shown.contains('·'),
        "tiles remain: {shown:?}"
    );
    for token in ["attention", "1-9 col", "hjkl", "keys"] {
        assert!(
            !shown.contains(token),
            "no hint footer, found {token:?}: {shown:?}"
        );
    }
    s.kill();
}

/// Drive the real binary through the modifier reveal and inspect rendered cells,
/// not just palette constants. Chrome is retro RGB unconditionally: a bare
/// config paints explicit 24-bit colors, never the terminal palette.
#[test]
fn terminal_dashboard_addresses_use_native_colors_without_palette_queries() {
    use gwae_term::{CColor, Size, TermGrid, Vt100Grid};
    // Chrome is retro unconditionally: a bare config paints white focus on
    // true black, whatever the host terminal is themed as. No OSC color-query
    // response is provided by this PTY.
    let mut s = Session::start("");
    let _ = s.drain();
    widen(&mut s, 3);
    // The panel paints over several frames; on a loaded runner one peek can
    // catch it mid-paint (frame up, map tiles not yet), so re-reveal and
    // re-collect until the map row parses rather than asserting on the
    // first window. Each chord re-opens the hold, so resending is free.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let raw = loop {
        s.send(&alt(b'h'));
        let raw = s.peek(150);
        if visible(&raw).contains('╭') {
            let mut probe = Vt100Grid::new(Size {
                cols: 140,
                rows: 30,
            });
            probe.feed(raw.as_bytes());
            let found = (0..30).any(|y| {
                (1..140)
                    .filter(|&x| {
                        probe.cell(x, y).ch.is_ascii_digit()
                            && matches!(probe.cell(x - 1, y).ch, '»' | '·' | '!' | '✗')
                    })
                    .count()
                    >= 2
            });
            if found {
                break raw;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dashboard never painted a populated minimap row; last got:\n{:?}",
            visible(&raw)
        );
    };
    s.kill();
    assert!(
        visible(&raw).contains('╭'),
        "dashboard must actually appear"
    );
    let mut grid = Vt100Grid::new(Size {
        cols: 140,
        rows: 30,
    });
    grid.feed(raw.as_bytes());
    let mut addresses = 0;
    let mut bold = 0;
    // Identify the map by geometry, not the old underline/color behavior.
    let map_y = (0..30)
        .find(|&y| {
            (1..140)
                .filter(|&x| {
                    grid.cell(x, y).ch.is_ascii_digit()
                        && matches!(grid.cell(x - 1, y).ch, '»' | '·' | '!' | '✗')
                })
                .count()
                >= 2
        })
        .expect("a populated minimap row");
    for y in [map_y] {
        for x in 0..140 {
            let c = grid.cell(x, y);
            // The dashboard's address signature is a status glyph + digit.
            if c.ch.is_ascii_digit()
                && x > 0
                && matches!(grid.cell(x - 1, y).ch, '»' | '·' | '!' | '✗')
            {
                addresses += 1;
                // Retro chrome: tiles carry explicit RGB fills (white focus,
                // full-intensity status tints) with black/white contrast ink,
                // never the terminal default pair.
                assert!(
                    matches!(c.style.bg, CColor::Rgb(..)),
                    "address at ({x}, {y}) should sit on a retro tint, got {:?}",
                    c.style.bg
                );
                assert!(
                    matches!(c.style.fg, CColor::Rgb(..)),
                    "address ink at ({x}, {y}) should be contrast ink, got {:?}",
                    c.style.fg
                );
                assert!(!c.style.underline, "no focus underline at ({x}, {y})");
                bold += usize::from(c.style.bold);
            }
        }
    }
    assert!(
        addresses >= 2,
        "must inspect real minimap addresses, got {addresses}"
    );
    assert!(bold > 0, "the focused tile stays bold");
}
