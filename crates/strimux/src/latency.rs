//! Input-latency tuning across the three layers that actually matter.
//!
//! A keystroke's round trip is `OS -> terminal -> strimux -> pane -> echo
//! back out`, so latency is not strimux's alone to fix: the OS key-repeat
//! rate and the terminal's own input/paint delays dominate. This module
//! probes all three, says what is suboptimal, and applies the fixes that are
//! actually ours to apply.
//!
//! The split matters and is deliberate:
//!
//! - **strimux's own config** is ours to write, so `tune` fixes it directly.
//! - **kitty and macOS** belong to the user. macOS settings are global to the
//!   machine and need a logout; kitty's config is someone else's file. Those
//!   are *reported with the exact command*, never silently applied.
//!
//! Values here are from the vendors' own documentation, checked against
//! kitty 0.48.2 (`input_delay 3`, `repaint_delay 10`, `sync_to_monitor yes`)
//! and stock macOS (`KeyRepeat 6`, `InitialKeyRepeat 25`).

use std::path::{Path, PathBuf};

/// How good a single setting currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Already at (or better than) the value we would recommend.
    Optimal,
    /// Usable, but leaving measurable latency on the table.
    Suboptimal,
    /// Could not read it; say so rather than guess.
    Unknown,
}

/// One tunable knob on one layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// Layer name, for grouping in output.
    pub layer: &'static str,
    /// The key as the user would write it.
    pub key: &'static str,
    /// What it is set to now, if we could read it.
    pub current: Option<String>,
    /// What we recommend.
    pub want: &'static str,
    pub verdict: Verdict,
    /// Why it matters, in one line the user can act on.
    pub why: &'static str,
    /// The exact command to fix it, for settings we will not write ourselves.
    pub fix: Option<String>,
}

impl Setting {
    fn needs_work(&self) -> bool {
        self.verdict == Verdict::Suboptimal
    }
}

/// Read a global macOS default as a number. `None` when the key is unset or
/// the tool is unavailable, which is also the case on every other OS.
#[cfg(target_os = "macos")]
fn read_default(key: &str) -> Option<f64> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(target_os = "macos"))]
fn read_default(_key: &str) -> Option<f64> {
    None
}

/// The kitty config file kitty itself would load.
pub fn kitty_conf_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("KITTY_CONFIG_DIRECTORY") {
        return Some(PathBuf::from(dir).join("kitty.conf"));
    }
    let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(base.join("kitty/kitty.conf"))
}

/// Read one `key value` setting out of a kitty config's text.
///
/// kitty's format is whitespace-separated, one per line, `#` comments. The
/// last assignment wins, matching kitty's own precedence.
pub fn kitty_setting(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let mut parts = t.split_whitespace();
        if parts.next() == Some(key) {
            let v: Vec<&str> = parts.collect();
            if !v.is_empty() {
                found = Some(v.join(" "));
            }
        }
    }
    found
}

/// True when we are actually running under kitty, so its advice is relevant.
pub fn in_kitty() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM")
            .map(|t| t.contains("kitty"))
            .unwrap_or(false)
}

/// Compare a numeric setting against a "lower is better" target.
fn numeric_verdict(current: Option<f64>, want: f64) -> Verdict {
    match current {
        None => Verdict::Unknown,
        Some(c) if c <= want => Verdict::Optimal,
        Some(_) => Verdict::Suboptimal,
    }
}

fn fmt_num(v: Option<f64>) -> Option<String> {
    v.map(|n| {
        if n.fract() == 0.0 {
            format!("{n:.0}")
        } else {
            format!("{n}")
        }
    })
}

/// Probe the macOS key-repeat settings. Empty on other platforms.
///
/// These are the largest single win for held-key delete: stock macOS repeats
/// at ~90ms/char, the fastest setting is ~15ms/char. No terminal or
/// multiplexer setting can compensate for the OS not sending the keys.
pub fn macos_settings() -> Vec<Setting> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let repeat = read_default("KeyRepeat");
    let initial = read_default("InitialKeyRepeat");
    // ApplePressAndHoldEnabled is a bool-ish 0/1; unset behaves as enabled.
    let press_hold = read_default("ApplePressAndHoldEnabled");
    vec![
        Setting {
            layer: "macOS",
            key: "KeyRepeat",
            current: fmt_num(repeat),
            want: "1",
            verdict: numeric_verdict(repeat, 1.0),
            why: "repeat rate for a held key (units of ~15ms; 2 is the fastest the UI offers, 1 is faster still)",
            fix: Some("defaults write -g KeyRepeat -int 1".into()),
        },
        Setting {
            layer: "macOS",
            key: "InitialKeyRepeat",
            current: fmt_num(initial),
            want: "10",
            verdict: numeric_verdict(initial, 10.0),
            why: "delay before a held key starts repeating (units of ~15ms)",
            fix: Some("defaults write -g InitialKeyRepeat -int 10".into()),
        },
        Setting {
            layer: "macOS",
            key: "ApplePressAndHoldEnabled",
            current: fmt_num(press_hold).or(Some("unset (on)".into())),
            want: "0",
            verdict: match press_hold {
                Some(0.0) => Verdict::Optimal,
                _ => Verdict::Suboptimal,
            },
            why: "when on, holding a key opens the accent popup instead of repeating",
            fix: Some("defaults write -g ApplePressAndHoldEnabled -bool false".into()),
        },
    ]
}

/// Probe kitty's latency-relevant settings. Empty when not under kitty.
///
/// `sync_to_monitor no` is normally a tearing tradeoff, but strimux emits
/// synchronized-update markers (`ESC[?2026h/l`) around every frame, so the
/// host applies each repaint atomically anyway. Under strimux you get the
/// latency win without the tearing, which is why this is recommended here and
/// might not be elsewhere.
pub fn kitty_settings() -> Vec<Setting> {
    if !in_kitty() {
        return Vec::new();
    }
    let text = kitty_conf_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let num = |k: &str, dflt: f64| -> Option<f64> {
        kitty_setting(&text, k)
            .and_then(|v| v.parse().ok())
            .or(Some(dflt))
    };
    // kitty 0.48 defaults, used when the key is absent from the file.
    let input_delay = num("input_delay", 3.0);
    let repaint_delay = num("repaint_delay", 10.0);
    let sync = kitty_setting(&text, "sync_to_monitor").unwrap_or_else(|| "yes".into());
    let sync_off = matches!(sync.as_str(), "no" | "n" | "false" | "0");
    vec![
        Setting {
            layer: "kitty",
            key: "input_delay",
            current: fmt_num(input_delay),
            want: "0",
            verdict: numeric_verdict(input_delay, 0.0),
            why: "delay before kitty processes what a program printed (the echo you are waiting to see)",
            fix: Some("input_delay 0".into()),
        },
        Setting {
            layer: "kitty",
            key: "repaint_delay",
            current: fmt_num(repaint_delay),
            want: "1",
            verdict: numeric_verdict(repaint_delay, 1.0),
            why: "minimum gap between screen updates; 10 caps you at ~100 FPS",
            fix: Some("repaint_delay 1".into()),
        },
        Setting {
            layer: "kitty",
            key: "sync_to_monitor",
            current: Some(sync.clone()),
            want: "no",
            verdict: if sync_off {
                Verdict::Optimal
            } else {
                Verdict::Suboptimal
            },
            why: "caps drawing at the monitor refresh; strimux already prevents tearing itself (ESC[?2026)",
            fix: Some("sync_to_monitor no".into()),
        },
    ]
}

/// Probe strimux's own knob. This is the one we may write.
pub fn strimux_settings(input_poll_ms: u64) -> Vec<Setting> {
    vec![Setting {
        layer: "strimux",
        key: "input_poll_ms",
        current: Some(input_poll_ms.to_string()),
        want: "1",
        verdict: if input_poll_ms <= 1 {
            Verdict::Optimal
        } else {
            Verdict::Suboptimal
        },
        why: "how long the loop waits for a keystroke; strimux is on the round trip twice, so it costs double",
        fix: Some("input_poll_ms = 1".into()),
    }]
}

/// Every layer, in round-trip order: OS first, then terminal, then us.
pub fn audit(input_poll_ms: u64) -> Vec<Setting> {
    let mut v = macos_settings();
    v.extend(kitty_settings());
    v.extend(strimux_settings(input_poll_ms));
    v
}

/// The settings that are worth changing.
pub fn pending(settings: &[Setting]) -> Vec<&Setting> {
    settings.iter().filter(|s| s.needs_work()).collect()
}

/// Settings strimux will write itself, versus ones only the user can apply.
/// Splitting this out keeps the boundary explicit: we never reach into the
/// user's global system settings or another program's config file.
pub fn ours_and_theirs<'a>(pending: &[&'a Setting]) -> (Vec<&'a Setting>, Vec<&'a Setting>) {
    pending.iter().partition(|s| s.layer == "strimux")
}

/// Set `input_poll_ms` in a strimux config's text, preserving everything
/// else, the same way the agent gateway saves `default_agent`.
pub fn set_input_poll_text(text: &str, value: u64) -> String {
    crate::agent::set_scalar_text(text, "input_poll_ms", &value.to_string())
}

/// Persist `input_poll_ms` to the config file.
pub fn save_input_poll(path: &Path, value: u64) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let next = if existing.trim().is_empty() {
        format!("# strimux configuration\ninput_poll_ms = {value}\n")
    } else {
        set_input_poll_text(&existing, value)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)
}

// ANSI for the report. Same palette the agent gateway uses, so the two
// onboarding steps look like one product.
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Render the audit as the lines a user reads. Pure, so the exact wording is
/// pinned by tests instead of drifting.
pub fn render_report(settings: &[Setting]) -> String {
    let mut s = String::new();
    let mut layer = "";
    for set in settings {
        if set.layer != layer {
            layer = set.layer;
            s.push_str(&format!("\n{BOLD}{layer}{RESET}\n"));
        }
        let (mark, color) = match set.verdict {
            Verdict::Optimal => ("ok", GREEN),
            Verdict::Suboptimal => ("slow", YELLOW),
            Verdict::Unknown => ("?", DIM),
        };
        s.push_str(&format!(
            "  {color}{mark:>4}{RESET}  {}{} {DIM}(want {}){RESET}\n",
            set.key,
            set.current
                .as_deref()
                .map(|c| format!(" = {c}"))
                .unwrap_or_default(),
            set.want
        ));
        if set.verdict == Verdict::Suboptimal {
            s.push_str(&format!("        {DIM}{}{RESET}\n", set.why));
        }
    }
    s
}

/// The block telling the user how to apply what strimux will not apply
/// itself. Returns `None` when there is nothing for them to do.
pub fn render_manual_steps(theirs: &[&Setting]) -> Option<String> {
    if theirs.is_empty() {
        return None;
    }
    let mut s = String::new();
    let kitty: Vec<&&Setting> = theirs.iter().filter(|s| s.layer == "kitty").collect();
    let mac: Vec<&&Setting> = theirs.iter().filter(|s| s.layer == "macOS").collect();
    if !kitty.is_empty() {
        let path = kitty_conf_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/kitty/kitty.conf".into());
        s.push_str(&format!(
            "\n{BOLD}Add to {path}{RESET}{DIM} (kitty reloads it automatically){RESET}\n"
        ));
        for set in &kitty {
            if let Some(f) = &set.fix {
                s.push_str(&format!("  {CYAN}{f}{RESET}\n"));
            }
        }
    }
    if !mac.is_empty() {
        s.push_str(&format!(
            "\n{BOLD}Run once, then log out and back in{RESET}{DIM} (these are machine-wide, so strimux will not set them for you){RESET}\n"
        ));
        for set in &mac {
            if let Some(f) = &set.fix {
                s.push_str(&format!("  {CYAN}{f}{RESET}\n"));
            }
        }
    }
    Some(s)
}

/// Apply the tuning strimux owns, silently, before onboarding asks anything.
///
/// `input_poll_ms` has exactly one right answer, so it was never a real
/// question: making it one only taught users that setup asks about things they
/// cannot evaluate. We fix our own config file without asking (one integer, in
/// our own file, trivially reversible) and *return* the steps only the user can
/// take, so the caller can show them once on the summary screen rather than
/// interrupting the flow.
///
/// Returns `None` when there is nothing left for the user to do, which is the
/// common case: silence is the feature.
pub fn apply_silently(input_poll_ms: u64, cfg_path: &Path) -> Option<String> {
    let settings = audit(input_poll_ms);
    let p = pending(&settings);
    if p.is_empty() {
        return None;
    }
    let (ours, theirs) = ours_and_theirs(&p);
    if !ours.is_empty() {
        // A failure here is not worth a screen of its own: the user gets a
        // working strimux either way, just a couple of milliseconds slower.
        if let Err(e) = save_input_poll(cfg_path, 1) {
            tracing::warn!("could not tune {}: {e}", cfg_path.display());
        }
    }
    render_manual_steps(&theirs)
}

/// One-line summary for `doctor`.
pub fn summary(settings: &[Setting]) -> String {
    let slow = pending(settings).len();
    if slow == 0 {
        "all layers tuned [ok]".to_string()
    } else {
        format!("{slow} setting(s) leaving latency on the table; run `strimux tune`")
    }
}

/// `strimux tune`: report every layer, apply what is ours, print the rest.
/// Returns the process exit code.
pub fn run_tune(input_poll_ms: u64, cfg_path: &Path, apply: bool) -> i32 {
    let settings = audit(input_poll_ms);
    println!(
        "{BOLD}Input latency{RESET}{DIM} — a keystroke crosses all three layers, twice{RESET}"
    );
    print!("{}", render_report(&settings));

    let p = pending(&settings);
    if p.is_empty() {
        println!("\n{GREEN}Everything is already tuned.{RESET}");
        return 0;
    }
    let (ours, theirs) = ours_and_theirs(&p);

    if apply && !ours.is_empty() {
        match save_input_poll(cfg_path, 1) {
            Ok(()) => println!(
                "\n{GREEN}Applied:{RESET} input_poll_ms = 1 {DIM}({}){RESET}",
                cfg_path.display()
            ),
            Err(e) => println!(
                "\n{YELLOW}Could not write {}: {e}{RESET}",
                cfg_path.display()
            ),
        }
    } else if !ours.is_empty() {
        println!(
            "\n{DIM}strimux can fix its own setting: run {RESET}{CYAN}strimux tune --apply{RESET}"
        );
    }
    if let Some(steps) = render_manual_steps(&theirs) {
        print!("{steps}");
    }
    println!("\n{DIM}Why these, and why strimux is on the path at all: `docs/LATENCY.md`.{RESET}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_settings_are_parsed_with_the_last_assignment_winning() {
        let text = "# a comment\nrepaint_delay 10\nfont_size 13\nrepaint_delay 1\n";
        assert_eq!(kitty_setting(text, "repaint_delay").as_deref(), Some("1"));
        assert_eq!(kitty_setting(text, "font_size").as_deref(), Some("13"));
        assert_eq!(kitty_setting(text, "missing"), None);
    }

    #[test]
    fn commented_out_settings_are_not_read_as_active() {
        // The common case of a user "trying" a setting by commenting it.
        let text = "#repaint_delay 1\n# input_delay 0\n";
        assert_eq!(kitty_setting(text, "repaint_delay"), None);
        assert_eq!(kitty_setting(text, "input_delay"), None);
    }

    #[test]
    fn a_key_with_no_value_is_not_treated_as_set() {
        assert_eq!(kitty_setting("repaint_delay\n", "repaint_delay"), None);
    }

    #[test]
    fn multi_word_values_survive_parsing() {
        let text = "shell /opt/homebrew/bin/fish\nmouse_hide_wait 0.1 3.0 40 yes\n";
        assert_eq!(
            kitty_setting(text, "shell").as_deref(),
            Some("/opt/homebrew/bin/fish")
        );
        assert_eq!(
            kitty_setting(text, "mouse_hide_wait").as_deref(),
            Some("0.1 3.0 40 yes")
        );
    }

    #[test]
    fn a_faster_than_recommended_value_is_still_optimal_not_wrong() {
        // Someone who set repaint_delay 0 must not be told to raise it.
        assert_eq!(numeric_verdict(Some(0.0), 1.0), Verdict::Optimal);
        assert_eq!(numeric_verdict(Some(1.0), 1.0), Verdict::Optimal);
        assert_eq!(numeric_verdict(Some(10.0), 1.0), Verdict::Suboptimal);
        assert_eq!(numeric_verdict(None, 1.0), Verdict::Unknown);
    }

    #[test]
    fn strimux_own_setting_is_judged_against_one_millisecond() {
        assert_eq!(strimux_settings(1)[0].verdict, Verdict::Optimal);
        assert_eq!(strimux_settings(0)[0].verdict, Verdict::Optimal);
        assert_eq!(strimux_settings(2)[0].verdict, Verdict::Suboptimal);
        assert_eq!(strimux_settings(10)[0].verdict, Verdict::Suboptimal);
    }

    #[test]
    fn only_strimux_settings_are_ever_applied_automatically() {
        // The safety boundary: we must never silently write a macOS global or
        // another program's config file.
        let all = audit(2);
        let p = pending(&all);
        let (ours, theirs) = ours_and_theirs(&p);
        assert!(
            ours.iter().all(|s| s.layer == "strimux"),
            "only strimux settings are auto-applied"
        );
        assert!(
            theirs.iter().all(|s| s.layer != "strimux"),
            "non-strimux settings must be left to the user"
        );
        // And everything we will not apply still tells the user how to do it.
        for s in &theirs {
            assert!(s.fix.is_some(), "{} has no fix command", s.key);
        }
    }

    #[test]
    fn every_setting_explains_itself() {
        // A tuner that says "change this" without saying why is cargo cult.
        for s in audit(2) {
            assert!(!s.why.is_empty(), "{} has no rationale", s.key);
            assert!(!s.want.is_empty());
        }
    }

    #[test]
    fn saving_input_poll_preserves_the_rest_of_the_config() {
        let before = "# mine\nstartup_panes = 1\ninput_poll_ms = 10\nmouse = true\n";
        let after = set_input_poll_text(before, 1);
        assert_eq!(
            after,
            "# mine\nstartup_panes = 1\ninput_poll_ms = 1\nmouse = true\n"
        );
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["input_poll_ms"].as_integer(), Some(1));
        assert_eq!(v["mouse"].as_bool(), Some(true));
    }

    #[test]
    fn saving_input_poll_appends_when_absent_and_stays_above_tables() {
        let after = set_input_poll_text("startup_panes = 1\n\n[theme]\npreset = \"nord\"\n", 1);
        assert!(after.find("input_poll_ms").unwrap() < after.find("[theme]").unwrap());
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["input_poll_ms"].as_integer(), Some(1));
        assert_eq!(v["theme"]["preset"].as_str(), Some("nord"));
    }

    #[test]
    fn the_value_written_is_an_integer_not_a_string() {
        // `input_poll_ms = "1"` would fail to deserialize and be discarded.
        let after = set_input_poll_text("", 1);
        assert!(after.contains("input_poll_ms = 1"), "{after:?}");
        assert!(!after.contains("\"1\""), "{after:?}");
    }

    #[test]
    fn kitty_advice_is_withheld_when_not_running_under_kitty() {
        // Telling an Alacritty user to edit kitty.conf is noise, and worse,
        // it implies strimux does not know what it is talking about.
        if !in_kitty() {
            assert!(kitty_settings().is_empty());
        }
    }
}
