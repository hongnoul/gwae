//! The open adapter setup system: one trait per machine concern.
//!
//! Every segment of first-run and ongoing setup (harness pick, appearance,
//! latency, focus repair, per-terminal bindings, companions, updates) is a
//! [`SetupStage`]: a probe of the machine, a pure plan, a doctor line, and a
//! health check. The loop in this module only knows the trait, never the
//! features.
//!
//! Adding a feature means adding one file under [`setup_stages`] that
//! implements the trait, plus one line in [`stages`]. Removing one means
//! deleting its line. Reordering means moving a line. No central enum, no
//! match on stage type, no shared mutable state between stages.
//!
//! Shape, deliberately mirroring `install.rs` and `latency.rs`:
//!
//! * [`SetupStage::probe`] reads the machine and returns facts. No writes.
//! * [`SetupStage::plan`] is pure: facts in, decision out. Unit-tested.
//! * [`SetupStage::doctor_line`] renders one `gwae doctor` line from facts.
//! * [`SetupStage::check`] reports whether the stage is healthy.
//!
//! P0 scope: the trait, the registry, and the doctor projection. `gwae
//! doctor` calls each stage's `doctor_line` instead of inlining the same
//! logic in `main.rs`; output text is byte-identical so `doctor_e2e.rs`
//! passes unchanged. Interactive rendering (`Screen`), apply, and the
//! `setup` command loop land in later phases.

pub mod setup_stages;
pub mod setup_support;

use crate::config::Config;
use std::path::Path;

/// What kind of effect a stage has when applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageKind {
    /// Writes only gwae's own TOML. Safe to apply silently or with `--yes`.
    Config,
    /// Touches other programs' files, LaunchAgents, or package managers.
    /// Always shows a diff and asks, unless `--yes`.
    Machine,
    /// Prints exact commands only. Never writes.
    Manual,
    /// Read-only facts for `doctor`. Never applies.
    Info,
}

/// Everything a stage needs that is not its own probe: the loaded config,
///
/// the config path, and the session's `--dir` override.
pub struct Ctx<'a> {
    /// The resolved config for this session.
    pub cfg: &'a Config,
    /// The config file path (for status lines that name it).
    pub cfg_path: &'a Path,
    /// The session `--dir` override, if any.
    pub dir: Option<&'a str>,
}

/// What applying one stage decided to do, in plain language for the
/// summary screen. Stages with nothing to change return an empty vec.
pub type Steps = Vec<String>;

/// One machine concern in the setup flow.
///
/// Implementations must keep `probe` side-effect free, `plan` pure, and
/// `doctor_line` in agreement with the live behavior: doctor renders the
/// same decision the flow would act on, so the two can never disagree.
///
/// Lifecycle for `gwae setup`: `probe` the machine, `plan` from the facts,
/// `render` the plan as a screen, `apply` the accepted plan. `doctor_line`
/// and `check` are the read-only projection used by `gwae doctor` and
/// `gwae setup --check`.
pub trait SetupStage {
    /// Stable id used by `--only <id>`. Lowercase, single word.
    fn id(&self) -> &'static str;
    /// What applying this stage is allowed to touch.
    fn kind(&self) -> StageKind;
    /// One `gwae doctor` line body (without the `  <id>: ` prefix).
    fn doctor_line(&self, ctx: &Ctx) -> String;
    /// True when there is nothing for the user to do.
    fn check(&self, ctx: &Ctx) -> bool;
    /// The human-readable steps this stage would take, for `--print` and
    /// the confirm screen. Empty when there is nothing to do.
    fn steps(&self, ctx: &Ctx) -> Steps {
        let _ = ctx;
        Vec::new()
    }
    /// Carry out the plan. Only called after the user confirms (or with
    /// `--yes` for `Config` stages). Returns lines for the summary screen.
    fn apply(&self, ctx: &Ctx, _yes: bool) -> Vec<String> {
        let _ = ctx;
        Vec::new()
    }
}

/// Every stage, in the order `gwae setup` runs them and `gwae doctor`
/// prints them.
pub fn stages() -> Vec<Box<dyn SetupStage>> {
    vec![
        Box::new(setup_stages::ConfigFileStage),
        Box::new(setup_stages::ThemeStage),
        Box::new(setup_stages::HarnessStage),
        Box::new(setup_stages::UpdatesStage),
        Box::new(setup_stages::SpawnDirStage),
        Box::new(setup_stages::LatencyStage),
    ]
}

/// The ids of every registered stage, in order. Used by `--only` validation
/// and tested to stay unique.
pub fn stage_ids() -> Vec<&'static str> {
    stages().iter().map(|s| s.id()).collect()
}

/// Render the full `gwae doctor` body through the registry. Each line is
/// produced by its owning stage; this function only adds the header and the
/// `  <id>: ` prefix convention.
pub fn doctor_body(ctx: &Ctx) -> Vec<(String, String)> {
    stages()
        .iter()
        .map(|s| (s.id().to_string(), s.doctor_line(ctx)))
        .collect()
}

/// `gwae setup`: audit, print, or apply the stages.
///
/// * `check`: print each unhealthy stage and return 1 when any fails. No
///   writes, for scripts and dotfile CI.
/// * `print`: print every stage's planned steps. No writes.
/// * `only`: restrict to one stage id; unknown ids are an error.
/// * Otherwise apply `Config` stages silently and report `Machine`/`Manual`
///   steps for the user. The interactive question loop arrives in P4; until
///   then this is the non-interactive path (`--yes` or nothing to confirm).
pub fn run_setup(ctx: &Ctx, check: bool, yes: bool, only: Option<&str>, print: bool) -> i32 {
    let all = stages();
    let picked: Vec<&Box<dyn SetupStage>> = match only {
        Some(want) => {
            let found: Vec<&Box<dyn SetupStage>> =
                all.iter().filter(|s| s.id() == want).collect();
            if found.is_empty() {
                eprintln!(
                    "unknown stage {want:?}; valid: {}",
                    stage_ids().join(", ")
                );
                return 2;
            }
            found
        }
        None => all.iter().collect(),
    };

    if print {
        for s in &picked {
            println!("{} [{}]", s.id(), kind_name(s.kind()));
            for step in s.steps(ctx) {
                println!("  {step}");
            }
        }
        return 0;
    }

    if check {
        let mut bad = 0;
        for s in &picked {
            if !s.check(ctx) {
                println!("{}: {}", s.id(), s.doctor_line(ctx));
                for step in s.steps(ctx) {
                    println!("  -> {step}");
                }
                bad += 1;
            }
        }
        if bad > 0 {
            return 1;
        }
        println!("all stages healthy [ok]");
        return 0;
    }

    // Non-interactive apply: Config stages write their own file; everything
    // else is reported for the user to run. Honors GWAE_NO_INSTALL.
    if std::env::var_os(crate::install::SKIP_ENV).is_some() {
        println!("setup: {} is set; no writes performed", crate::install::SKIP_ENV);
        return 0;
    }
    let mut code = 0;
    for s in &picked {
        if s.check(ctx) {
            continue;
        }
        match s.kind() {
            StageKind::Config => {
                if !yes && !at_tty() {
                    println!("{}: needs confirmation; re-run with --yes", s.id());
                    code = 1;
                    continue;
                }
                for line in s.apply(ctx, yes) {
                    println!("{}: {line}", s.id());
                }
            }
            StageKind::Machine | StageKind::Manual | StageKind::Info => {
                println!("{}: {}", s.id(), s.doctor_line(ctx));
                for step in s.steps(ctx) {
                    println!("  -> {step}");
                }
            }
        }
    }
    code
}

fn kind_name(k: StageKind) -> &'static str {
    match k {
        StageKind::Config => "config",
        StageKind::Machine => "machine",
        StageKind::Manual => "manual",
        StageKind::Info => "info",
    }
}

fn at_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `gwae setup --only <id>` and the doctor prefix both key off `id`, so
    /// a duplicate would make one stage unreachable and print two identical
    /// lines.
    #[test]
    fn stage_ids_are_unique_and_usable_as_cli_args() {
        let ids = stage_ids();
        assert!(!ids.is_empty(), "registry must not be empty");
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(!id.is_empty(), "stage id must not be empty");
            assert!(
                !id.contains('\n') && !id.contains(':'),
                "id {id:?} would break the `  <id>: <line>` format"
            );
            assert!(seen.insert(*id), "duplicate stage id {id:?}");
        }
    }

    /// The registry is the single place stages are added or removed: this
    /// pins the current set so a deletion is a deliberate diff, not drift.
    #[test]
    fn registry_lists_every_shipped_stage_in_doctor_order() {
        assert_eq!(
            stage_ids(),
            vec![
                "config file",
                "theme",
                "agent",
                "updates",
                "spawn dir",
                "latency",
            ]
        );
    }

    /// `doctor_body` returns one entry per stage, in registry order, with the
    /// same id the stage reports: the loop must not rename or drop anything.
    #[test]
    fn doctor_body_covers_every_stage_exactly_once() {
        let cfg = Config::default();
        let ctx = Ctx {
            cfg: &cfg,
            cfg_path: std::path::Path::new("/nonexistent/gwae.toml"),
            dir: None,
        };
        let body = doctor_body(&ctx);
        assert_eq!(body.len(), stage_ids().len());
        for (stage, (id, line)) in stages().iter().zip(body.iter()) {
            assert_eq!(id, stage.id());
            assert!(!line.is_empty(), "stage {} produced an empty line", id);
        }
    }

    /// Every stage declares what it is allowed to touch. Config stages stay
    /// the majority: a setup system that mostly edits other programs' files
    /// has the safety boundary backwards.
    #[test]
    fn most_stages_write_only_our_own_config() {
        let kinds: Vec<StageKind> = stages().iter().map(|s| s.kind()).collect();
        let config = kinds.iter().filter(|k| **k == StageKind::Config).count();
        assert!(
            config >= kinds.len() / 2,
            "expected Config to be the common kind, got {kinds:?}"
        );
    }
}
