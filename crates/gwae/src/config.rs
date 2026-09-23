//! TOML configuration (ADR-008).
//!
//! Loaded from `$XDG_CONFIG_HOME/gwae/gwae.toml` (or
//! `$HOME/.config/gwae/gwae.toml`). The schema is intentionally small in
//! M0 and grows with the layout. `docs/CONFIG.md` is generated from the doc
//! comments here.

#[cfg(test)]
use crate::keys;
use crate::theme::{Palette, ThemeConfig};
use gwae_layout::Width;
use serde::de::{self, Visitor};
use serde::Deserialize;
use std::fmt;
use std::path::PathBuf;

fn default_input_poll_ms() -> u64 {
    1
}

/// The user's home directory, portably.
///
/// `HOME` on unix; on Windows `HOME` is usually unset and `USERPROFILE` is
/// the real answer. One helper so no call site encodes the difference.
pub fn home_dir() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(h));
    }
    #[cfg(windows)]
    if let Some(h) = std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(h));
    }
    None
}

fn default_keep_awake() -> bool {
    false
}

fn no_agents() -> Vec<String> {
    Vec::new()
}

/// Image panes: `auto` promotes a pane to an image viewer once sustained
/// native image commits prove it is one (e.g. tdf); `off` never promotes,
/// keeping the classic text-grid path with inline tiles for minimalists.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImagePane {
    /// Promote image-heavy panes automatically (default).
    #[default]
    Auto,
    /// Never promote; panes stay text grids with inline image tiles.
    Off,
}

/// The resolved view of the config file, with defaults filled in.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default width of a newly created column (default: Half).
    pub default_column_width: Width,
    /// Explicit harness override for `⌥+;` (spawn-agent). Empty (the default)
    /// means "remember the pick": the first press offers what is installed and
    /// remembers the choice in the state file, so later presses go straight
    /// there. `⌥+⇧+;` always opens the picker instead and ignores this.
    /// Set this only to pin a harness in dotfiles or scripts; a value
    /// that is not installed falls back to the picker rather than a dead pane.
    pub default_agent: String,
    /// The directory new panes (agent and shell) start in. Empty (the
    /// default) inherits gwae's own working directory, which is whatever
    /// your terminal opened at. `~` and `$VAR` expand, so
    /// `agent_dir = "~/git"` works as written. A path that does not exist is
    /// ignored with a warning rather than breaking pane spawn.
    pub agent_dir: String,
    /// Retired: extra agent names once came from an `agents` list. They now
    /// come from the picker itself: any typed command that resolves is
    /// remembered in the state file. The key still parses so old configs load.
    #[serde(rename = "agents", default = "no_agents", skip_serializing)]
    #[allow(dead_code)]
    pub agents_retired: Vec<String>,
    /// Number of equal-width panes on screen at first launch. Default: 1 (a
    /// single quarter-width pane; the skeleton's placeholder boxes show the
    /// rest of the container).
    pub startup_panes: usize,
    /// Chrome color overrides, layered key by key on the enforced retro
    /// default (true-black panels, high-contrast functional colors). Every
    /// key accepts a 256-color index (`12`), a hex RGB string (`"#00ffff"`),
    /// or `"default"` for the terminal's own color. Unset keys keep retro.
    /// No presets, no picker: hand-edit only, applied live on save.
    pub theme: ThemeConfig,
    /// Image-pane promotion: `auto` (default) or `off`. Unknown values fail
    /// the config check the same way other mistyped keys do.
    pub image_pane: ImagePane,
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
    /// display sleep never pause the panes. macOS-only by construction.
    /// Session-only: keep-awake always starts off at launch and only the
    /// `⌥+w` toggle turns it on; the file value is not read at startup and
    /// the toggle is not saved back.
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
            default_agent: String::new(),
            agent_dir: String::new(),
            agents_retired: Vec::new(),
            startup_panes: 1,
            theme: ThemeConfig::default(),
            image_pane: ImagePane::default(),
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
        home_dir()
            .unwrap_or_default()
            .join(".config/gwae/gwae.toml")
    }

    /// The chrome palette: the enforced retro default, plus whatever the
    /// user overrode under `[theme]`. There is no preset and no `theme`
    /// name key; the default is always [`Palette::RETRO`].
    pub fn palette(&self) -> Palette {
        self.theme.resolve()
    }

    /// Adopt the appearance settings from `new`, keeping everything that
    /// cannot safely change while the TUI is running.
    ///
    /// Live reload only re-reads the file; it does not re-run startup. So
    /// settings that were *consumed once* at launch are deliberately kept:
    /// `startup_panes` (the panes already exist). `default_agent` is adopted,
    /// not kept: the `⌥+;` overlay and the fast paths read it on every press,
    /// so an edited override applies to the next agent pane regardless (the
    /// gateway bootstrap reads the file itself in the new pane, so it follows
    /// too). Everything that is read afresh
    /// every frame - minimap, scroll behavior - is adopted.
    pub fn adopt_appearance(&mut self, new: Config) {
        let Config {
            startup_panes,
            agent_dir,
            keep_awake,
            ..
        } = self.clone();
        *self = Config {
            startup_panes,
            // Kept: the running session may have overridden it via `--dir`
            // or `⌥+d`, and a config edit must not yank panes back to the
            // file's value.
            agent_dir,
            // Kept: keep-awake is session-only. It starts off every launch
            // and only ⌥+w flips it, so a config edit must not turn it on.
            keep_awake,
            ..new
        };
        // `startup_panes` stays pinned because it was consumed once at launch.
    }

    /// Directory new panes start in: `agent_dir`, or "" when unset.
    pub fn spawn_dir(&self) -> &str {
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
    /// No minimap/status chrome at all. Hold ⌥ to see the centered
    /// dashboard, or `⌥+/` for the cheat-sheet.
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
    /// cannot tell. `brew` is the primary route, `install.sh` the supported
    /// fallback; the rest are legacy
    /// routes detection still understands. One of `brew`, `install.sh`,
    /// `cargo`, `cargo-git`, `source`, `nix`, `system`. Empty (the default) means
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
    fn cheat_sheet_labels_name_the_platform_modifier() {
        // The cheat-sheet is the only keybinding doc most users ever read,
        // so its labels must speak the Mac keyboard's vocabulary: `⌥`, never
        // `Alt`. gwae is macOS-only. The two ⌃+⇧ scroll rows and the host's
        // native paste are exceptions: they name their own modifier.
        let m = keys::mod_key();
        let ctrl = keys::ctrl_key();
        assert_eq!(m, "⌥");
        let mut chords = 0;
        for b in crate::binds::BINDS {
            let label = b.label();
            assert!(
                !label.contains("Alt"),
                "label {label:?} uses the wrong platform's modifier name"
            );
            match b.trigger {
                crate::binds::Trigger::Chord(_)
                | crate::binds::Trigger::ShiftChord(_)
                | crate::binds::Trigger::EnterChord { .. }
                | crate::binds::Trigger::ModProse(_) => {
                    chords += 1;
                    assert!(label.contains(m), "label {label:?} does not mention {m:?}");
                }
                crate::binds::Trigger::CtrlShift(_) => {
                    chords += 1;
                    assert!(
                        label.contains(ctrl),
                        "label {label:?} does not mention {ctrl:?}"
                    );
                }
                crate::binds::Trigger::Prose(_) => {}
            }
        }
        assert!(chords > 0, "some labels are chords");
    }

    #[test]
    fn cheat_sheet_does_not_teach_dead_keys() {
        // Every machine-checkable cheat-sheet row must name a chord the
        // dispatcher really handles (`advertised_bindings_match_the_dispatcher`
        // feeds each entry through the real `handle_key`). What is left to
        // assert here is the shape of the prose rows: they must not read as
        // bare key presses that would just type into the focused pane.
        let m = keys::mod_key();
        let ctrl = keys::ctrl_key();
        for b in crate::binds::BINDS {
            let label = b.label();
            assert!(
                !label.to_lowercase().starts_with("press "),
                "label {label:?} omits the modifier"
            );
            match b.trigger {
                crate::binds::Trigger::Prose(_) => {
                    assert!(
                        label == keys::paste_key() || label == "click",
                        "prose label {label:?} names no modifier and is not a known exception"
                    );
                }
                _ => {
                    assert!(
                        label.contains(m) || label.contains(ctrl),
                        "label {label:?} names no modifier"
                    );
                }
            }
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
        // Chrome is the enforced retro default unless overridden.
        assert_eq!(cfg.palette(), Palette::RETRO);
    }

    #[test]
    fn theme_overrides_layer_on_retro() {
        let cfg = parse("[theme]\naccent = \"#ff00ff\"\n");
        assert_eq!(cfg.palette().accent, CColor::Rgb(0xff, 0x00, 0xff));
        assert_eq!(cfg.palette().base, Palette::RETRO.base);
    }

    #[test]
    fn retired_theme_keys_are_ignored_not_fatal() {
        // Old configs still on disk must keep loading: bare `theme = "..."`
        // names, `[theme] preset`, and legacy color keys are simply not read
        // as overrides rather than a parse error.
        let cfg = parse(
            "theme = \"nord\"\nbackground = 235\nfocus_color = 36\nskeleton_color = \"#333333\"\n",
        );
        assert_eq!(cfg.startup_panes, 1);
        assert_eq!(cfg.palette(), Palette::RETRO);
        let cfg = parse("[theme]\naccent = \"#ff0000\"\n");
        assert_eq!(cfg.palette().accent, CColor::Rgb(0xff, 0, 0));
    }

    #[test]
    fn keep_awake_defaults_to_off_but_parses_true() {
        // Fresh runs let the Mac sleep as normal; opting in is explicit
        // (`keep_awake = true` or `⌥+w`). Both the omitted and the absent
        // paths must agree, or init silently re-enables the assertion.
        assert!(!parse("").keep_awake);
        assert!(!Config::default().keep_awake);
        assert!(!default_keep_awake());
        assert!(parse("keep_awake = true").keep_awake);
        assert!(!parse("keep_awake = false").keep_awake);
    }

    #[test]
    fn startup_panes_parses() {
        let cfg = parse("startup_panes = 2");
        assert_eq!(cfg.startup_panes, 2);
    }

    #[test]
    fn image_pane_defaults_to_auto_parses_off_and_rejects_unknown() {
        assert_eq!(parse("").image_pane, ImagePane::Auto);
        assert_eq!(parse("image_pane = \"auto\"").image_pane, ImagePane::Auto);
        assert_eq!(parse("image_pane = \"off\"").image_pane, ImagePane::Off);
        assert!(toml::from_str::<Config>("image_pane = \"tiles\"").is_err());
    }

    #[test]
    fn retired_spawndir_keys_are_ignored_not_fatal() {
        // Old configs may name per-harness dirs or picker roots; those keys
        // are gone, so they are simply not read rather than a parse error.
        let cfg = parse("harness_dirs = { jcode = \"~/git/gwae\" }\nagent_dirs = [\"~/notes\"]\nagent_dir_roots = [\"~/work\"]\nagent_dir = \"~/git\"\n");
        assert_eq!(cfg.spawn_dir(), "~/git");
    }

    #[test]
    fn retired_agents_key_is_ignored_not_fatal() {
        // The `agents` list is gone (typed picks are remembered in state),
        // so old configs naming it must still load.
        let cfg = parse("agents = [\"zz\"]\ndefault_agent = \"claude\"\n");
        assert_eq!(cfg.default_agent, "claude");
    }

    #[test]
    fn live_reload_adopts_the_override_but_keeps_session_state() {
        // `default_agent` is read on every ⌥+; press, so an edit applies to
        // the next spawn. `startup_panes` was consumed at launch and
        // `agent_dir` may hold a live ⌥+d pick: both survive the reload.
        let mut live = parse("startup_panes = 3\nagent_dir = \"~/live\"\n");
        live.adopt_appearance(parse(
            "default_agent = \"claude\"\nstartup_panes = 9\nagent_dir = \"~/file\"\n",
        ));
        assert_eq!(live.default_agent, "claude");
        assert_eq!(live.startup_panes, 3);
        assert_eq!(live.agent_dir, "~/live");
    }
}
