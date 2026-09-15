//! TOML configuration (ADR-008).
//!
//! Loaded from `$XDG_CONFIG_HOME/gwae/gwae.toml` (or
//! `$HOME/.config/gwae/gwae.toml`). The schema is intentionally small in
//! M0 and grows with the layout. `docs/CONFIG.md` is generated from the doc
//! comments here.

#[cfg(test)]
use crate::keys;
use crate::theme::Palette;
use gwae_layout::Width;
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

fn default_input_poll_ms() -> u64 {
    1
}

fn default_keep_awake() -> bool {
    true
}

/// The resolved view of the config file, with defaults filled in.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default width of a newly created column (default: Half).
    pub default_column_width: Width,
    /// Cells of context kept visible around the focused column when scrolling.
    pub scroll_margin: u16,
    /// Always center the focused column instead of scrolling minimally.
    pub center_focus: bool,
    /// Logical grid content width (cells) of every pane, decoupled from the
    /// visible column width. Long lines up to this width do not wrap and can
    /// be revealed with horizontal pane scroll (Alt+Left/Right). `0` (the
    /// default) follows the visible column width so lines wrap normally and
    /// there is no horizontal overflow to manage in a pane.
    pub content_width: u16,
    /// The agent harness command that `;` (spawn-agent) launches. Empty (the
    /// default) means "not chosen yet": `;` then runs the agent gateway, which
    /// offers the harnesses found on PATH and writes the choice back here.
    pub default_agent: String,
    /// The directory new panes (agent and shell) start in. Empty (the
    /// default) inherits gwae's own working directory, which is whatever
    /// your terminal opened at. `~` and `$VAR` expand, so
    /// `agent_dir = "~/git"` works as written. A path that does not exist is
    /// ignored with a warning rather than breaking pane spawn.
    pub agent_dir: String,
    /// Per-harness default spawn directories. Key is the harness command name
    /// as it appears in `default_agent` (`"jcode"`, `"claude"`, …). When set,
    /// new panes that will run that harness start here; other harnesses fall
    /// back to `agent_dir`. `~` and `$VAR` expand. Example:
    /// `harness_dirs = { jcode = "~/git/gwae", Muse = "~/src/foo" }`.
    /// Preferred harness = `default_agent`; any harness is a key, so the table
    /// is not jcode-specific.
    #[serde(default)]
    pub harness_dirs: HashMap<String, String>,
    /// Directories always offered in the `⌥+d` spawn-directory picker, on top
    /// of the ones found by scanning `agent_dir_roots`.
    pub agent_dirs: Vec<String>,
    /// Where the `⌥+d` picker looks for projects and directories. Default: your home
    /// directory. gwae finds projects by looking for `.git`/`.hg`/`.jj`
    /// markers rather than by directory name, so it works whatever your
    /// layout is; set this to narrow (or widen) the search, e.g.
    /// `["~/work", "/srv/checkouts"]`. Roots that do not exist are skipped.
    /// Typing also searches ordinary directories, including inside repos and
    /// near the session's spawn directory, without requiring a project marker.
    pub agent_dir_roots: Vec<String>,
    /// Extra agent commands to offer in the `;` picker, on top of the ones
    /// gwae knows and the ones it finds by scanning `PATH`. Use this to
    /// teach it a harness with a name it cannot guess, or a wrapper script.
    /// Entries that are not installed are simply not shown.
    pub agents: Vec<String>,
    /// Number of equal-width panes on screen at first launch. Default: 1 (a
    /// single quarter-width pane; the skeleton's placeholder boxes show the
    /// rest of the container).
    pub startup_panes: usize,
    /// The minimap: a small bottom-right grid showing each strip (row) and its
    /// panes (columns), with the focused strip and column highlighted.
    pub minimap: Minimap,
    /// Milliseconds to wait in `event::poll` before checking PTY output and
    /// repainting. Lower values reduce perceived typing and backspace latency
    /// at the cost of more frequent wakeups. Default is 1ms for minimum
    /// input latency (backspace/delete feels instant); the loop backs off to
    /// 30ms once the screen has been quiet for 750ms, so an idle session stays
    /// cheap. Valid range 1..50.
    #[serde(default = "default_input_poll_ms")]
    pub input_poll_ms: u64,
    /// Hold a macOS `caffeinate` assertion while gwae runs, so idle and
    /// display sleep never pause the panes. macOS-only; elsewhere this key
    /// does nothing. Default `true`: agents keep working while you are away;
    /// set `keep_awake = false` to let the machine sleep as normal.
    /// Note the honest limit: a closed
    /// lid still sleeps outside clamshell mode (power + external
    /// display + external input).
    #[serde(default = "default_keep_awake")]
    pub keep_awake: bool,
    /// Staying current: whether gwae checks for new releases, and how it is
    /// allowed to upgrade itself. See `docs/UPDATES.md`.
    pub update: Update,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_column_width: Width::DEFAULT,
            scroll_margin: 2,
            center_focus: false,
            content_width: 0,
            default_agent: String::new(),
            agent_dir: String::new(),
            harness_dirs: HashMap::new(),
            agent_dirs: Vec::new(),
            agent_dir_roots: Vec::new(),
            agents: Vec::new(),
            startup_panes: 1,
            minimap: Minimap::default(),
            input_poll_ms: default_input_poll_ms(),
            keep_awake: default_keep_awake(),
            update: Update::default(),
        }
    }
}

impl Config {
    /// The default config file path for this user.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("gwae/gwae.toml");
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/gwae/gwae.toml")
    }

    /// The chrome palette: the host terminal's own colors. There is no
    /// theme key; change the terminal's scheme and gwae follows.
    pub fn palette(&self) -> Palette {
        Palette::default()
    }

    /// Adopt the appearance settings from `new`, keeping everything that
    /// cannot safely change while the TUI is running.
    ///
    /// Live reload only re-reads the file; it does not re-run startup. So
    /// settings that were *consumed once* at launch are deliberately kept:
    /// `startup_panes` (the panes already exist). `default_agent` is kept too,
    /// but only because nothing in the TUI reads it: the agent gateway loads
    /// the file itself in the new pane, so an edited value applies to the next
    /// agent pane regardless. Everything that is read afresh
    /// every frame - minimap, scroll behavior - is adopted.
    pub fn adopt_appearance(&mut self, new: Config) {
        let Config {
            startup_panes,
            default_agent,
            agent_dir,
            harness_dirs,
            ..
        } = self.clone();
        *self = Config {
            startup_panes,
            default_agent,
            // Kept for the same reason as `default_agent`: the running
            // session may have overridden it via `--dir` or `⌥+d`, and a
            // config edit must not yank panes back to the file's value.
            agent_dir,
            harness_dirs,
            ..new
        };
        // `keep_awake` rides along with the reload rather than being pinned:
        // it is a behavior toggle applied live to the running
        // session (the guard is reconciled in the render loop). `startup_panes`
        // above stays pinned because it was consumed once at launch.
    }

    /// Directory configured for a particular harness, falling back to `agent_dir`.
    ///
    /// `harness` is the `default_agent` value (`"jcode"`, `"claude"`, …). Lookup is
    /// exact and case-sensitive on the stored key; arity matches what `agent.rs` saves.
    pub fn dir_for_harness(&self, harness: &str) -> &str {
        let h = harness.trim();
        if h.is_empty() {
            return &self.agent_dir;
        }
        // Keys come from `default_agent` which `agent.rs` stores verbatim; but
        // tolerate `jcode --resume` style by stripping args for lookup, since a
        // user writes `harness_dirs = { jcode = "..." }` not `harness_dirs = { "jcode --resume" = … }`.
        let exe = crate::tui::shell_split(h)
            .first()
            .cloned()
            .unwrap_or_default();
        for key in [exe.as_str(), h] {
            if key.is_empty() {
                continue;
            }
            if let Some(v) = self.harness_dirs.get(key) {
                return v;
            }
        }
        &self.agent_dir
    }

    /// Load config from `path`, falling back to defaults if the file is
    /// missing or unparseable (with a warning).
    pub fn load(path: &std::path::Path) -> Config {
        match Config::load_checked(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("ignoring config {path:?}: {e}");
                Config::default()
            }
        }
    }

    /// Load config from `path`, returning the parse error instead of
    /// swallowing it.
    ///
    /// A missing file is not an error: it yields the defaults, same as
    /// [`Config::load`]. Live reload uses this so it can tell the user their
    /// edit is broken rather than silently reverting to the
    /// defaults, which would look like the reload itself had misbehaved.
    pub fn load_checked(path: &std::path::Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| e.to_string()),
            Err(_) => Ok(Config::default()),
        }
    }

    /// The config file's last-modified time, or `None` when there is no file
    /// (or the filesystem does not report one).
    ///
    /// Used to detect edits without a filesystem-watch dependency: one `stat`
    /// per poll is cheap next to the render loop's existing work.
    pub fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(path).ok()?.modified().ok()
    }
}

/// Where and how the minimap/status chrome is shown.
///
/// The bottom reserved status row (`reserved` / `reserved_quasimode`) has been
/// removed. Status is shown via the centered Alt HUD/minimap and (optionally)
/// `overlay` / `edge_ticks`. Legacy values `reserved` and
/// `reserved_quasimode` parse as `off` for compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MinimapMode {
    /// Classic bottom-right overlay (pre-redesign).
    Overlay,
    /// Single-cell ticks on the outer frame (no box).
    EdgeTicks,
    /// No minimap/status chrome at all. Hold ⌥/Alt to see the centered HUD
    /// (attention hint + cheat-sheet) and centered minimap.
    #[default]
    Off,
}

impl<'de> Deserialize<'de> for MinimapMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = MinimapMode;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "overlay, edge_ticks, off (plus legacy reserved / reserved_quasimode as off)",
                )
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match v.to_ascii_lowercase().as_str() {
                    "overlay" => Ok(MinimapMode::Overlay),
                    "edge_ticks" | "edgeticks" => Ok(MinimapMode::EdgeTicks),
                    "off" => Ok(MinimapMode::Off),
                    // legacy bottom-row modes → off
                    "reserved" | "reserved_quasimode" => Ok(MinimapMode::Off),
                    _ => Err(de::Error::unknown_variant(
                        v,
                        &["overlay", "edge_ticks", "off"],
                    )),
                }
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&v)
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// How gwae keeps itself current (see `crate::update`, ADR-016).
///
/// The defaults are "tell me, never act": a once-a-day check that only ever
/// results in a one-line notice, and an upgrade that happens when the user
/// runs `gwae upgrade` and not before. Nothing in this table can cause
/// software to be installed on the machine on its own.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Update {
    /// Check GitHub for a newer release at startup, at most once a day, and
    /// show a one-line notice if there is one. The request is an
    /// unauthenticated HEAD of the `releases/latest` redirect: it sends
    /// nothing about you or your machine, not even the installed version.
    /// `GWAE_NO_UPDATE_CHECK=1` turns it off without editing this file.
    pub check: bool,
    /// How this gwae was installed, when the automatic detection is wrong or
    /// cannot tell. One of `install.sh`, `brew`, `cargo`, `cargo-git`,
    /// `source`, `nix`, `system`, `windows`. Empty (the default) means
    /// "detect it", which uses the installer's receipt when there is one and
    /// the binary's path otherwise. `gwae doctor` prints what it decided and
    /// whether it was a fact or a guess.
    pub source: String,
}

impl Default for Update {
    fn default() -> Self {
        Update {
            check: true,
            source: String::new(),
        }
    }
}

impl Update {
    /// The configured source, parsed. `None` means "detect", including when
    /// the user wrote something unrecognized, which `gwae doctor` reports
    /// rather than letting it silently pin the wrong route.
    pub fn source(&self) -> Option<crate::update::Source> {
        if self.source.trim().is_empty() {
            return None;
        }
        match crate::update::Source::parse(&self.source) {
            Some(s) => Some(s),
            None => {
                tracing::warn!(
                    "unknown update.source {:?}; detecting instead. Valid: {}",
                    self.source,
                    crate::update::Source::NAMES.join(", ")
                );
                None
            }
        }
    }

    /// Whether `update.source` was written but not understood, for `doctor`.
    pub fn bad_source(&self) -> Option<&str> {
        let s = self.source.trim();
        if s.is_empty() || crate::update::Source::parse(s).is_some() {
            return None;
        }
        Some(s)
    }

    /// The install source actually in effect: the configured one, or what
    /// detection makes of this machine.
    ///
    /// Resolved once at startup rather than at notice time so the background
    /// check does not have to re-probe the filesystem to phrase its one line.
    pub fn source_detected(&self) -> crate::update::Source {
        crate::update::detect(&crate::update::probe(self.source()))
    }
}

/// The minimap widget: which strips (rows) and panes (columns) exist and
/// which is focused.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Minimap {
    /// Draw the minimap at all (kill-switch, kept for backward compat).
    pub show: bool,
    /// Presentation mode. `overlay` is the legacy bottom-right overlay;
    /// `edge_ticks` are single-cell frame ticks; `off` (default) shows only
    /// the centered Alt HUD/minimap on ⌥ hold. Legacy `reserved` /
    /// `reserved_quasimode` parse as `off` and the bottom row is reclaimed.
    pub mode: MinimapMode,
    /// Maximum width (in cells) of the minimap. Used for `overlay` and the
    /// centered minimap shown while holding Option/Alt.
    pub max_width: u16,
    /// Maximum number of strips (rows) shown; extra strips are cut off. Used
    /// for `overlay` and the centered minimap shown while holding Option/Alt.
    pub max_rows: u16,
    /// Draw the one-line status summary (above the map or in the HUD).
    pub show_counts: bool,
}

impl Minimap {
    /// How many bottom rows are reserved for chrome. No longer used: the
    /// bottom status row has been removed, so this always returns 0.
    #[allow(dead_code)]
    pub fn chrome_rows(&self) -> u16 {
        0
    }

    /// Whether the chrome strip should actually paint content this frame.
    /// With the bottom row removed this is true only for overlay-style
    /// chrome; the centered Alt HUD/minimap is gated elsewhere.
    #[allow(dead_code)]
    pub fn should_paint(&self, _alt_held: bool, _has_attention: bool) -> bool {
        if !self.show {
            return false;
        }
        match self.mode {
            MinimapMode::Off => false,
            MinimapMode::Overlay | MinimapMode::EdgeTicks => true,
        }
    }
}

impl Default for Minimap {
    fn default() -> Self {
        Minimap {
            show: true,
            mode: MinimapMode::default(),
            max_width: 32,
            max_rows: 6,
            show_counts: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gwae_term::CColor;

    fn parse(toml: &str) -> Config {
        toml::from_str(toml).expect("config parses")
    }

    #[test]
    fn key_hints_come_from_the_binding_table() {
        let hints = crate::binds::key_hints();
        assert!(!hints.is_empty(), "hint list must exist");
        assert_eq!(hints.len(), crate::binds::BINDS.len(), "one hint per binding");
    }

    #[test]
    fn key_hints_name_the_platform_modifier() {
        // The hints are the only keybinding docs many users ever read, so they
        // must speak the local keyboard's vocabulary: `⌥` on macOS, `Alt`
        // elsewhere, never both and never the wrong one. The two Ctrl+Shift
        // scroll hints and the host's native paste are exceptions: they name
        // their own platform-appropriate modifier instead of `$mod`.
        let m = keys::mod_key();
        let ctrl = keys::ctrl_key();
        // Chord hints must name the modifier. A few bindings are mouse or
        // key-range prose (`1-9`, `click`, `↑/↓`) and correctly have no
        // modifier to name.
        let chord_hints = crate::binds::key_hints()
            .iter()
            .filter(|msg| !msg.starts_with(['1', 'c', 'w', '←', '↵', '⇧']))
            .count();
        assert!(chord_hints > 0, "some hints are chords");
        for msg in &crate::binds::key_hints() {
            if msg.starts_with(['1', 'c', 'w', '←', '↵', '⇧', 'E', 'S']) {
                continue;
            }
            if msg.contains(ctrl) || msg.starts_with(keys::paste_key()) {
                continue;
            }
            assert!(msg.contains(m), "hint {msg:?} does not mention {m:?}");
        }
        let other = if cfg!(target_os = "macos") {
            "Alt"
        } else {
            "⌥"
        };
        for msg in &crate::binds::key_hints() {
            assert!(
                !msg.contains(other),
                "hint {msg:?} uses the other platform's modifier name"
            );
        }
    }

    #[test]
    fn key_hints_do_not_teach_dead_keys() {
        // Regressions guarded: the hints once advertised `⌥+c` ("new pane"),
        // which was never implemented, and once told users to "press c"/
        // "press ;" with no modifier at all, which just types the letter into
        // the focused pane.
        //
        // `⌥+c` is now a real binding (copy), so the check is no longer "this
        // one chord is forbidden" — it is the general property that made that
        // bug possible: every hint must name a chord the dispatcher really
        // handles. `binds.rs` owns that end to end (hints are generated from
        // `BINDS`, and `advertised_bindings_match_the_dispatcher` feeds every
        // entry through the real `handle_key`), so what is left to assert here
        // is that the default list is in fact the generated one and has not
        // been hand-edited back into a liability.
        let m = keys::mod_key();
        let ctrl = keys::ctrl_key();
        for msg in &crate::binds::key_hints() {
            assert!(
                !msg.to_lowercase().starts_with("press "),
                "hint {msg:?} omits the modifier"
            );
            assert!(
                msg.contains(m)
                    || msg.contains(ctrl)
                    || msg.starts_with(keys::paste_key())
                    || msg.contains("click"),
                "hint {msg:?} names no modifier and is not a mouse hint"
            );
        }
    }

    #[test]
    fn retired_mouse_keys_are_ignored_not_fatal() {
        // Old configs still on disk must keep loading: the keys are gone, so
        // they are simply not read rather than a parse error.
        let cfg = parse("mouse = false\nscroll_lines = 7\nstartup_panes = 2\n");
        assert_eq!(cfg.startup_panes, 2);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let cfg = parse("");
        assert_eq!(cfg.startup_panes, 1);
        // Chrome is fixed to the terminal's own colors: no config key can
        // change it.
        assert_eq!(cfg.palette(), Palette::TERMINAL);
    }

    #[test]
    fn retired_theme_keys_are_ignored_not_fatal() {
        // Old configs still on disk must keep loading: `[theme]` tables,
        // `theme = "..."` names, and legacy color keys are simply not read
        // rather than a parse error.
        let cfg = parse(
            "theme = \"nord\"\nbackground = 235\nfocus_color = 36\nskeleton_color = \"#333333\"\n",
        );
        assert_eq!(cfg.startup_panes, 1);
        assert_eq!(cfg.palette(), Palette::TERMINAL);
        let cfg = parse("[theme]\npreset = \"nord\"\naccent = \"#ff0000\"\n");
        assert_eq!(cfg.palette(), Palette::TERMINAL);
    }

    #[test]
    fn startup_panes_parses() {
        let cfg = parse("startup_panes = 2");
        assert_eq!(cfg.startup_panes, 2);
    }

    #[test]
    fn harness_dirs_parse_and_lookup() {
        let cfg = parse("harness_dirs = { jcode = \"~/git/gwae\", Muse = \"/tmp\" }\n");
        assert_eq!(
            cfg.harness_dirs.get("jcode").map(|s| s.as_str()),
            Some("~/git/gwae")
        );
        assert_eq!(cfg.dir_for_harness("jcode"), "~/git/gwae");
        assert_eq!(cfg.dir_for_harness("claude"), "");
        // dotted form and table form also parse via serde
        let cfg = parse("harness_dirs.jcode = \"~/a\"\n");
        assert_eq!(cfg.dir_for_harness("jcode"), "~/a");
        let cfg = parse("[harness_dirs]\njcode = \"~/b\"\n");
        assert_eq!(cfg.dir_for_harness("jcode"), "~/b");
        // args stripped
        let cfg = parse("harness_dirs = { jcode = \"~/x\" }\n");
        assert_eq!(cfg.dir_for_harness("jcode --resume"), "~/x");
    }

    #[test]
    fn harness_dir_writer_round_trips() {
        let cases = [
            ("", "jcode", "~/git/gwae"),
            ("harness_dirs = { jcode = \"~/a\" }\n", "claude", "~/b"),
            ("harness_dirs = { jcode = \"~/a\" }\n", "jcode", "~/new"),
            ("harness_dirs.jcode = \"~/old\"\n", "jcode", "~/new2"),
            ("[harness_dirs]\njcode = \"~/old\"\n", "jcode", "~/new3"),
            (
                "startup_panes = 1\n\n[theme]\npreset = \"nord\"\n",
                "jcode",
                "~/git/gwae",
            ),
        ];
        for (before, key, dir) in cases {
            let after = crate::agent::set_harness_dir_text(before, key, dir);
            let v: toml::Value = toml::from_str(&after)
                .unwrap_or_else(|e| panic!("broke toml {e:?} after={after:?} from {before:?}"));
            assert_eq!(
                v["harness_dirs"][key].as_str(),
                Some(dir),
                "after={after:?} before={before:?}"
            );
        }
    }
}
