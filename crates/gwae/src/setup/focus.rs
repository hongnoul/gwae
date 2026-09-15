//! kitty Space focus repair: the Swift daemon from `docs/MACOS-FOCUS.md`.
//!
//! When kitty runs in native fullscreen it owns its own Space. Returning to
//! that Space activates the app but does not always make the window the
//! macOS key window, so GLFW discards keys until you click. The fix is a
//! small Swift agent that watches for kitty activation and re-asserts focus
//! with `kitty @ focus-window` over the remote-control socket.
//!
//! Shape, like `install.rs`: [`Facts`] is probed, [`plan`] is pure,
//! [`run`] is the only function that executes anything. Nothing here is
//! ever fatal: a failed repair costs the user a click, so it is reported
//! and stepped over.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::setup::setup_support::{embed, kitty_conf, terminal};

/// Launchd label for the daemon.
pub const LABEL: &str = "com.gwae.focus-fix";

/// The facts [`plan`] decides from. Probed, never assumed, so tests
/// describe a machine instead of needing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// This is macOS, where the bug and the fix live.
    pub macos: bool,
    /// Running under kitty, so the advice is relevant.
    pub in_kitty: bool,
    /// kitty.conf sets `allow_remote_control yes`.
    pub remote_control: bool,
    /// The `listen_on` socket kitty was configured with, if any.
    pub listen_on: Option<String>,
    /// `kitten @ --to <socket> ls` succeeds: kitty was restarted after the
    /// conf edit and the socket is live.
    pub socket_live: bool,
    /// `swiftc` resolves on PATH, so the daemon can be built.
    pub swiftc: bool,
    /// The compiled daemon exists at `~/.local/bin/gwae-focus-fix`.
    pub binary: bool,
    /// The LaunchAgent is loaded.
    pub agent_loaded: bool,
    /// kitty uses native fullscreen (its own Space), where the bug bites.
    /// `macos_traditional_fullscreen yes` opts out.
    pub native_fullscreen: bool,
}

impl Facts {
    /// Probe the real machine. No writes.
    pub fn probe() -> Facts {
        if std::env::var_os(crate::install::SKIP_ENV).is_some() {
            return Facts::absent();
        }
        let macos = cfg!(target_os = "macos");
        let in_kitty = terminal::in_kitty();
        let text = terminal::kitty_conf_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let remote_control = matches!(
            kitty_conf::get(&text, "allow_remote_control").as_deref(),
            Some("yes" | "y" | "true" | "1")
        );
        let listen_on = kitty_conf::get(&text, "listen_on");
        let socket = listen_on.clone().unwrap_or_else(embed::kitty_socket);
        let socket_live = socket_probe(&socket);
        let native_fullscreen = kitty_conf::get(&text, "macos_traditional_fullscreen")
            .map(|v| v != "yes")
            .unwrap_or(true);
        Facts {
            macos,
            in_kitty,
            remote_control,
            listen_on,
            socket_live,
            swiftc: crate::agent::which("swiftc").is_some(),
            binary: embed::focus_binary_path().map(|p| p.is_file()).unwrap_or(false),
            agent_loaded: agent_loaded(),
            native_fullscreen,
        }
    }

    /// Facts for a machine where the fix cannot apply: everything false
    /// except what the plan needs to report "not applicable".
    pub fn absent() -> Facts {
        Facts {
            macos: false,
            in_kitty: false,
            remote_control: false,
            listen_on: None,
            socket_live: false,
            swiftc: false,
            binary: false,
            agent_loaded: false,
            native_fullscreen: false,
        }
    }
}

/// What installing the repair would take on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Not macOS or not kitty: nothing to do, and setup says so.
    NotApplicable(&'static str),
    /// Traditional fullscreen is set: focus survives without a daemon.
    TraditionalFullscreen,
    /// Everything installed, loaded, and live.
    Ready,
    /// Something is missing; the steps name what `run` would do, in order.
    Install(Vec<Step>),
}

/// One action of the install, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Add the missing remote-control lines to kitty.conf.
    KittyConf { missing: Vec<String> },
    /// Install the Xcode command-line tools, then compile.
    Toolchain,
    /// Compile the embedded Swift source with swiftc.
    Compile,
    /// Write the plist with the absolute binary path and load it.
    Plist,
    /// kitty must be restarted before the socket goes live.
    RestartKitty,
}

/// Decide what to do from the facts. Pure.
pub fn plan(f: &Facts) -> Plan {
    if !f.macos {
        return Plan::NotApplicable("macOS only");
    }
    if !f.in_kitty {
        return Plan::NotApplicable("kitty only");
    }
    if !f.native_fullscreen {
        return Plan::TraditionalFullscreen;
    }
    let mut steps = Vec::new();
    if !f.remote_control || f.listen_on.is_none() {
        let mut missing = Vec::new();
        if !f.remote_control {
            missing.push("allow_remote_control yes".to_string());
        }
        if f.listen_on.is_none() {
            missing.push(format!("listen_on {}", embed::kitty_socket()));
        }
        steps.push(Step::KittyConf { missing });
    }
    if !f.socket_live {
        steps.push(Step::RestartKitty);
    }
    if !f.binary {
        // The compiler only matters when there is something to build: a
        // present binary needs no toolchain.
        if f.swiftc {
            steps.push(Step::Compile);
        } else {
            steps.push(Step::Toolchain);
        }
    }
    if !f.agent_loaded {
        steps.push(Step::Plist);
    }
    if steps.is_empty() {
        Plan::Ready
    } else {
        Plan::Install(steps)
    }
}

/// Whether the LaunchAgent is loaded.
fn agent_loaded() -> bool {
    Command::new("launchctl")
        .args(["list", LABEL])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Whether `kitten @ --to <socket> ls` answers: kitty is listening.
fn socket_probe(socket: &str) -> bool {
    let kitten = kitten_path();
    Command::new(kitten)
        .args(["@", "--to", socket, "ls"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The kitty remote-control client, wherever it lives.
fn kitten_path() -> PathBuf {
    for dir in [
        "/Applications/kitty.app/Contents/MacOS",
        "/Users/Shared/Applications/kitty.app/Contents/MacOS",
    ] {
        let c = PathBuf::from(dir).join("kitten");
        if c.is_file() {
            return c;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let c = PathBuf::from(home)
            .join("Applications/kitty.app/Contents/MacOS/kitten");
        if c.is_file() {
            return c;
        }
    }
    PathBuf::from("kitten")
}

/// Run the install steps that do not need the user's hands: compile the
/// daemon, write the plist, load it. Returns lines for the summary screen.
///
/// kitty.conf is never edited here: it belongs to the user, so the flow
/// prints the diff and asks. Restarting kitty is the user's call too.
pub fn run(p: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    let steps = match p {
        Plan::Install(s) => s,
        _ => return out,
    };
    for step in steps {
        match step {
            Step::KittyConf { .. } | Step::RestartKitty | Step::Toolchain => {}
            Step::Compile => out.push(compile()),
            Step::Plist => out.push(install_plist()),
        }
    }
    out
}

fn compile() -> String {
    let Some(dest) = embed::focus_binary_path() else {
        return "no HOME, cannot place the daemon".to_string();
    };
    let src_path = dest.with_extension("swift");
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return format!("could not create {}", parent.display());
        }
    }
    if std::fs::write(&src_path, embed::FOCUS_FIX_SWIFT).is_err() {
        return format!("could not write {}", src_path.display());
    }
    let ok = Command::new("swiftc")
        .args([
            "-O",
            &src_path.display().to_string(),
            "-o",
            &dest.display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        format!("compiled {}", dest.display())
    } else {
        "swiftc failed; install Xcode command-line tools and re-run".to_string()
    }
}

fn install_plist() -> String {
    let Some(bin) = embed::focus_binary_path() else {
        return "no HOME, cannot place the daemon".to_string();
    };
    let Some(home) = std::env::var_os("HOME") else {
        return "no HOME, cannot install the LaunchAgent".to_string();
    };
    let dest = PathBuf::from(home).join("Library/LaunchAgents/com.gwae.focus-fix.plist");
    let body = embed::render_focus_plist(&bin, &embed::kitty_socket());
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return format!("could not create {}", parent.display());
        }
    }
    if std::fs::write(&dest, body).is_err() {
        return format!("could not write {}", dest.display());
    }
    let ok = Command::new("launchctl")
        .args(["load", "-w", &dest.display().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        format!("loaded {}", dest.display())
    } else {
        format!("wrote {}, but `launchctl load` failed", dest.display())
    }
}

/// The `gwae doctor` line for the focus repair.
pub fn doctor_line(f: &Facts) -> String {
    match plan(f) {
        Plan::NotApplicable(why) => format!("not applicable ({why})"),
        Plan::TraditionalFullscreen => {
            "traditional fullscreen; focus survives without a daemon [ok]".to_string()
        }
        Plan::Ready => "daemon loaded, socket live [ok]".to_string(),
        Plan::Install(steps) => {
            let names: Vec<&str> = steps
                .iter()
                .map(|s| match s {
                    Step::KittyConf { .. } => "kitty.conf",
                    Step::Toolchain => "xcode tools",
                    Step::Compile => "compile",
                    Step::Plist => "launchd",
                    Step::RestartKitty => "restart kitty",
                })
                .collect();
            format!("missing: {}; run `gwae setup --only focus`", names.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            macos: true,
            in_kitty: true,
            remote_control: true,
            listen_on: Some("unix:/tmp/mykitty".to_string()),
            socket_live: true,
            swiftc: true,
            binary: true,
            agent_loaded: true,
            native_fullscreen: true,
        }
    }

    #[test]
    fn ready_when_everything_is_in_place() {
        assert_eq!(plan(&facts()), Plan::Ready);
    }

    #[test]
    fn not_applicable_off_macos_or_off_kitty() {
        assert!(matches!(
            plan(&Facts { macos: false, ..facts() }),
            Plan::NotApplicable(_)
        ));
        assert!(matches!(
            plan(&Facts { in_kitty: false, ..facts() }),
            Plan::NotApplicable(_)
        ));
    }

    #[test]
    fn traditional_fullscreen_needs_no_daemon() {
        assert_eq!(
            plan(&Facts { native_fullscreen: false, ..facts() }),
            Plan::TraditionalFullscreen
        );
    }

    #[test]
    fn missing_conf_comes_first_then_restart_then_build() {
        let p = plan(&Facts {
            remote_control: false,
            listen_on: None,
            socket_live: false,
            binary: false,
            agent_loaded: false,
            ..facts()
        });
        let Plan::Install(steps) = p else {
            panic!("expected install, got {p:?}");
        };
        assert!(matches!(steps[0], Step::KittyConf { .. }), "{steps:?}");
        assert_eq!(steps[1], Step::RestartKitty);
        assert_eq!(steps[2], Step::Compile);
        assert_eq!(steps[3], Step::Plist);
    }

    #[test]
    fn a_present_binary_needs_no_toolchain() {
        // The compiler only matters when there is something to build.
        let p = plan(&Facts { swiftc: false, ..facts() });
        assert_eq!(p, Plan::Ready);
    }

    #[test]
    fn without_swiftc_the_toolchain_is_the_actionable_step() {
        let p = plan(&Facts {
            swiftc: false,
            binary: false,
            ..facts()
        });
        let Plan::Install(steps) = p else {
            panic!("expected install, got {p:?}");
        };
        assert!(steps.contains(&Step::Toolchain), "{steps:?}");
        assert!(!steps.contains(&Step::Compile), "{steps:?}");
    }

    #[test]
    fn every_state_says_something_actable() {
        assert!(doctor_line(&Facts::absent()).contains("not applicable"));
        assert!(doctor_line(&facts()).contains("[ok]"));
        let broken = Facts {
            remote_control: false,
            ..facts()
        };
        let line = doctor_line(&broken);
        assert!(line.contains("gwae setup --only focus"), "{line}");
    }
}
