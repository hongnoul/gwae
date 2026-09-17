//! Terminal setup/teardown + poll intervals + size backstop (verbatim move from `tui/mod.rs`).

use std::io::Write;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    PopKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
    LeaveAlternateScreen,
};

/// Hand the terminal back to the host: leave the alt screen, drop raw mode,
/// and undo every mode gwae turned on.
///
/// Shared by the normal exit path and by hot reload. Terminal modes are
/// kernel tty state, so they survive an `execve`: a reload that skipped this
/// would leave the new image in raw mode on an alt screen it never entered,
/// which looks exactly like a hung terminal.
pub(crate) fn restore_terminal(stdout: &mut std::io::Stdout, kitty_keyboard: bool) {
    if kitty_keyboard {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = stdout.write_all(b"\x1b[?7h");
    let _ = stdout.flush();
    let _ = execute!(stdout, DisableBracketedPaste, DisableMouseCapture);
    let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

/// Re-acquire the terminal after [`restore_terminal`], for the one case where
/// a reload was attempted and the `execve` failed: this image is still alive
/// and still owns every pane, so it has to take the screen back rather than
/// exit and take the panes with it.
#[cfg(unix)]
pub(crate) fn re_enter_terminal(stdout: &mut std::io::Stdout) -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    execute!(stdout, EnterAlternateScreen, cursor::Hide).map_err(|e| format!("alt screen: {e}"))?;
    let _ = stdout.write_all(b"\x1b[?7l");
    let _ = stdout.flush();
    let _ = execute!(stdout, EnableBracketedPaste, EnableMouseCapture);
    Ok(())
}

/// Re-read the live terminal size and adopt it whenever the OS reports
/// anything different from what we last knew. Called every loop so a resize
/// that crossterm never delivers as an `Event::Resize` (or that is coalesced
/// away) can't leave the frame short of the terminal's true right edge. This
/// is what makes the panes truly full-bleed to the right margin: the frame is
/// always sized to the currently-rendered column count, never to a cached one.
pub(crate) fn refresh_size(cols: &mut u16, rows: &mut u16) -> bool {
    match term_size() {
        Ok((c, r)) => {
            let c = c.max(1);
            let r = r.max(2);
            if c != *cols || r != *rows {
                if std::env::var_os("GWAE_DEBUG_SIZE").is_some() {
                    eprintln!("[gwae] terminal size -> {c} cols x {r} rows");
                }
                *cols = c;
                *rows = r;
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// How often the config file is checked for edits.
///
/// Fast enough that saving the config feels immediate, slow enough that the
/// `stat` never shows up next to the render loop's own work.
pub(crate) const CONFIG_POLL: Duration = Duration::from_millis(400);

/// How long the binary's mtime must hold still before a hot reload fires.
///
/// A link is not atomic: the new image is truncated and written over some
/// milliseconds, so a poll can catch a fresh mtime on a file that is not yet
/// a valid executable. Requiring quiet costs one extra poll and turns "exec a
/// half-written binary" (which kills the session and every pane with it) into
/// "reload a moment later".
#[cfg(unix)]
pub(crate) const BINARY_SETTLE: Duration = Duration::from_millis(300);

/// How often the safety-net size re-measure runs.
///
/// `refresh_size` is a *backstop* for resizes crossterm never delivers as an
/// `Event::Resize`; the event path already handles every resize the host does
/// report, and it stays instant. Running the backstop on every loop iteration
/// meant a `TIOCGWINSZ` — which on macOS opens and closes `/dev/tty` — at the
/// input poll rate (1000/s at the default `input_poll_ms = 1`). That syscall
/// storm was the bulk of gwae's idle CPU: a completely idle mux sat at ~3.5%
/// of a core forever, which on a laptop is a warm chassis and a spinning fan
/// for no work at all. A quarter second is far below human resize perception
/// for the rare dropped-event case, and costs 4 stats/s instead of 500.
pub(crate) const SIZE_POLL: Duration = Duration::from_millis(250);

/// How long the whole screen must be quiet before the input poll relaxes.
pub(crate) const IDLE_AFTER: Duration = Duration::from_millis(750);

/// The relaxed poll interval used once idle (~33 wakeups/s).
pub(crate) const IDLE_POLL_MS: u64 = 30;

/// How long to block in `event::poll` this iteration.
///
/// `event::poll` returns *immediately* when a key arrives, so a longer
/// timeout never costs keystroke latency; all it delays is the loop's own
/// periodic work. So: run at the configured (tight) rate while anything is
/// happening, and back off once every pane and the keyboard have been silent
/// for `IDLE_AFTER`. At the default 1 ms the loop would wake 1000x a second
/// forever to find nothing, which on a laptop is a warm chassis for a screen
/// that is not changing. The relaxed ceiling still repaints within one frame
/// at 30 fps, and the very first byte of output or keystroke snaps the rate
/// back to tight.
pub(crate) fn input_poll_interval(configured_ms: u64, since_activity: Duration) -> Duration {
    let base = configured_ms.clamp(1, 50);
    let ms = if since_activity < IDLE_AFTER {
        base
    } else {
        base.max(IDLE_POLL_MS)
    };
    Duration::from_millis(ms)
}

/// How long the reload note stays on screen.
pub(crate) const NOTE_LINGER: Duration = Duration::from_millis(2500);

/// The first line of a multi-line error, for one-line status display.
pub(crate) fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Typing must never be slowed down by the idle backoff: while anything
    /// is happening the loop polls at exactly the configured rate.
    #[test]
    fn a_busy_session_polls_at_the_configured_rate() {
        for ms in [1u64, 2, 8, 50] {
            assert_eq!(
                input_poll_interval(ms, Duration::from_millis(0)),
                Duration::from_millis(ms),
                "fresh activity at {ms}ms"
            );
            // Still tight just before the idle threshold.
            assert_eq!(
                input_poll_interval(ms, IDLE_AFTER - Duration::from_millis(1)),
                Duration::from_millis(ms)
            );
        }
    }

    /// The bug this guards: at the default 1 ms the loop would wake 1000x a
    /// second forever, burning CPU on a mux nobody was touching. Once the
    /// screen is quiet the wakeups must drop by more than an order of
    /// magnitude.
    #[test]
    fn an_idle_session_backs_off_to_far_fewer_wakeups() {
        let idle = input_poll_interval(2, IDLE_AFTER);
        assert_eq!(idle, Duration::from_millis(IDLE_POLL_MS));
        let busy = input_poll_interval(2, Duration::from_millis(0));
        assert!(
            idle.as_micros() >= busy.as_micros() * 10,
            "idle backoff {idle:?} must be >=10x the busy interval {busy:?}"
        );
        // Still fast enough to repaint within one frame at 30 fps.
        assert!(idle <= Duration::from_millis(33), "{idle:?} too sluggish");
    }

    /// A user who deliberately configured a *slower* poll than the idle
    /// backoff keeps their setting: the backoff is a floor, never a ceiling,
    /// so it can only ever reduce wakeups.
    #[test]
    fn the_backoff_never_polls_more_often_than_configured() {
        let cfg = 50;
        assert_eq!(
            input_poll_interval(cfg, Duration::from_secs(60)),
            Duration::from_millis(cfg)
        );
        // Out-of-range config values are still clamped the way they were.
        assert_eq!(
            input_poll_interval(0, Duration::from_millis(0)),
            Duration::from_millis(1)
        );
        assert_eq!(
            input_poll_interval(9999, Duration::from_millis(0)),
            Duration::from_millis(50)
        );
    }
}
