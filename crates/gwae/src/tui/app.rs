//! The event loop: setup, the main loop, teardown (verbatim move from `tui/mod.rs`).
//!
//! `run_tui` is intentionally still one function: this step only moves it.
//! Structuring the loop (`App`, handlers) is follow-up work, not this refactor.

use super::render::chrome_rows;
use super::*;
use std::time::Duration;

/// How long a pane without OSC 133 shell integration must stay silent before
/// the activity heuristic calls it idle ("wants attention") instead of
/// working. Long enough that a compiler pausing between crates doesn't
/// flicker, short enough that a finished agent surfaces quickly.
const QUIET_AFTER: Duration = Duration::from_secs(4);

/// How long after boot a sole agent pane's instant death falls back to the
/// gateway picker instead of quitting gwae. A broken remembered command dies
/// in well under a second; a real session the user chose to leave lasts
/// longer than this.
const STARTUP_FALLBACK_GRACE: Duration = Duration::from_secs(5);

/// Notice line for the `⌥+⇧+;` force-pick overlay, which always opens the
/// picker instead of taking the fast path.
///
/// Mirrors the [`crate::agent::plan`] arms so the overlay says the same thing
/// the `⌥+;` overlay would: a missing override is named, a resolvable
/// override is called out (it still wins for `⌥+;`, so a pick here only
/// steers this pane), and otherwise a stale memory is named. `None`
/// when there is nothing to explain, which is the common bypass case of a
/// healthy remembered pick.
fn force_pick_notice(want: &str, last: &str, ordered: &[crate::agent::Found]) -> Option<String> {
    let want = want.trim();
    let last = last.trim();
    if ordered.is_empty() {
        return Some("No agent harness found — type a command or take a shell".to_string());
    }
    if !want.is_empty() && !crate::agent::command_available(want) {
        return Some(format!("`{want}` is not installed"));
    }
    // A live override outranks memory in `plan`, so it is named first: with
    // both set, `⌥+;` spawns the override, not the stale pick.
    if !want.is_empty() && crate::agent::command_available(want) {
        return Some(format!("default_agent `{want}` still wins for ⌥+;"));
    }
    if !last.is_empty() && !ordered.iter().any(|f| f.cmd == last) {
        return Some(format!("remembered `{last}` is gone; pick another"));
    }
    None
}

/// Spawn the harness chosen in the `⌥+;` overlay: apply the spawn-agent verb,
/// mark the new pane as an agent pane running this exact command, remember
/// the pick in the state file, and confirm with a one-line note.
#[allow(clippy::too_many_arguments)]
fn spawn_picked_harness(
    cmd: &str,
    known: bool,
    layout: &mut Layout,
    panes: &mut HashMap<PaneId, PtyPane>,
    agent_panes: &mut HashSet<PaneId>,
    agent_cmds: &mut HashMap<PaneId, String>,
    tx: &std::sync::mpsc::Sender<PaneMsg>,
    geometry: (GridSize, CellPixels),
    cwd: Option<&std::path::Path>,
    cfg: &Config,
    state: &mut crate::agent::HarnessState,
    state_path: Option<&std::path::Path>,
    note: &mut Option<String>,
    note_until: &mut Option<Instant>,
) {
    let v = Viewport::new(geometry.0.cols);
    let f = FollowScroll::default();
    let _ = layout.apply(Action::SpawnAgent, v, f);
    if let Some(pid) = focused_pane(layout) {
        agent_panes.insert(pid);
        agent_cmds.insert(pid, cmd.to_string());
        // Fresh allocs default to `Plain`; an agent spawn is positive
        // evidence of work, so claim `Running` up front. The OSC protocol
        // or the quiet heuristic corrects it from here.
        if let Some(lp) = layout.panes.get_mut(&pid) {
            lp.status = PaneStatus::Running;
        }
    }
    if let Err(e) = sync_panes(
        layout,
        panes,
        tx,
        geometry,
        agent_panes,
        agent_cmds,
        cwd,
        cfg,
    ) {
        tracing::error!("sync panes: {e}");
    }
    state.record_pick(cmd, known);
    if let Some(path) = state_path {
        let _ = crate::agent::save_harness_state(path, state);
    }
    *note = Some(format!("agent: {cmd} — ⌥+; goes straight there"));
    *note_until = Some(Instant::now() + NOTE_LINGER);
}

/// Run the interactive TUI.
pub fn run_tui(command: Option<String>, cfg: Config, cli_dir: Option<String>) -> Result<(), i32> {
    use std::io;
    // Config is re-resolved whenever the config file changes on disk (see
    // the reload check in the render loop). The palette follows `[theme]`
    // overrides through the same reload. Every chrome color painted below
    // reads from it.
    let mut cfg = cfg;
    let mut pal = cfg.palette();
    let mut stdout = io::stdout();
    // Boot instant for the handover clock freeze: the next image shifts
    // its quiet timers by the gap so panes do not flap on reload.
    #[cfg_attr(not(unix), allow(unused_variables))]
    let boot = Instant::now();
    // Arm signal handlers + panic hook before the first pane exists, and hold
    // a drop guard so every early return below still reaps. Quitting gwae is
    // documented as killing everything in the panes; this makes that true for
    // the abnormal exits too, not just ⌥+Shift+q.
    crate::reap::install();
    let _reap_guard = crate::reap::Guard;
    // Keep-awake always starts off: the machine sleeps as normal unless the
    // user presses ⌥+w in this session. The config file's `keep_awake` is
    // ignored at startup and the toggle is never persisted.
    cfg.keep_awake = false;
    let mut keep_awake = crate::keepawake::Guard::acquire(false);
    enable_raw_mode().map_err(|e| {
        eprintln!("raw mode: {e}");
        1
    })?;
    if let Err(e) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
        eprintln!("enter alt screen: {e}");
        let _ = disable_raw_mode();
        return Err(1);
    }
    // Turn off the host's automatic margin wrap (DECAWM) for our alt screen.
    // gwae positions every run absolutely, so wrapping is never wanted: its
    // only effect is that a run which overshoots the right margin (a glyph the
    // host renders wider than the emulator assumed) spills onto the next row
    // and smears that row's background across the screen. With DECAWM off the
    // overshoot is clamped at the margin and repaired on the next frame.
    let _ = stdout.write_all(b"\x1b[?7l");
    let _ = stdout.flush();
    // Request Kitty keyboard protocol *without* REPORT_ALL_KEYS.
    //
    // Ghostty + macOS Hangul (and other CJK IMEs) break when
    // REPORT_ALL_KEYS_AS_ESCAPE_CODES is active: every key is reported as a
    // CSI-u escape, so the OS IME composition never commits as normal UTF-8
    // — Korean appears broken/duplicated inside gwae while it works fine in
    // plain Ghostty. The bare-Alt HUD previously needed REPORT_ALL to see a
    // lone Option press, but `chord_alt_until` already covers that via the
    // repeated-chord fallback (and the bare-modifier path when the terminal
    // does send it). So request only the disambiguation/alt-shift fixes.
    //
    // Ghostty's Hangul IME still flickers even with the reduced flags
    // (DISAMBIGUATE+ALTERNATE+EVENT_TYPES) — Ghostty switches to CSI-u framing
    // for *all* input while the protocol is active, which breaks the macOS
    // IME preedit flush in the same way. So under Ghostty the default is now
    // to leave the protocol entirely alone; the host already disambiguates
    // without it. Users who prefer kitty semantics inside Ghostty can force it
    // with `GWAE_KITTY_KEYBOARD=1`.
    // Env overrides: GWAE_KITTY_KEYBOARD=0 disables, =1 forces enable.
    let kitty_env = std::env::var("GWAE_KITTY_KEYBOARD")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase());
    let kitty_keyboard = match kitty_env.as_deref() {
        Some("0") | Some("false") | Some("off") | Some("no") => {
            tracing::info!("kitty keyboard protocol disabled via GWAE_KITTY_KEYBOARD=0");
            false
        }
        Some("1") | Some("true") | Some("on") | Some("yes") => matches!(
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
                )
            ),
            Ok(())
        ),
        _ if is_ghostty() => {
            tracing::info!(
                "kitty keyboard protocol skipped under Ghostty for Hangul IME (set GWAE_KITTY_KEYBOARD=1 to force)"
            );
            false
        }
        _ => matches!(
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
                )
            ),
            Ok(())
        ),
    };
    if kitty_keyboard {
        tracing::info!("kitty keyboard protocol enabled (bare Alt hover)");
    }
    // Capture the mouse so clicks and drags land here: focus follows a click,
    // a drag selects text, and a child that asked for mouse reporting gets the
    // event forwarded verbatim. gwae itself does nothing with the wheel.
    // The host must frame native Cmd+V/Ctrl+Shift+V as a paste event even
    // when the focused child has not enabled the mode. Child mode changes
    // live in its emulator, not the host. Without this request, newlines
    // become Enter keys and pasted Option glyphs can trigger gwae commands.
    if let Err(e) = execute!(stdout, EnableBracketedPaste, EnableMouseCapture) {
        tracing::warn!("enable paste/mouse: {e}");
    }
    // Whether the *host* terminal understands the Kitty graphics protocol.
    // Gates APC passthrough: forwarding graphics sequences to a terminal that
    // does not parse them would print base64 garbage over the frame.
    let host_kitty_graphics = host_supports_kitty_graphics();
    if host_kitty_graphics {
        tracing::info!("host supports kitty graphics; pane image passthrough enabled");
    }
    let (cols, mut rows) = term_size().map_err(|e| {
        eprintln!("size: {e}");
        restore_terminal(&mut stdout, kitty_keyboard);
        1
    })?;
    let mut cols = cols.max(1);
    rows = rows.max(2);
    let mut cell_pixels = CellPixels::measure().unwrap_or_default();
    if std::env::var_os("GWAE_DEBUG_SIZE").is_some() {
        eprintln!("[gwae] initial terminal size -> {cols} cols x {rows} rows");
    }
    // Where every pane in this session starts. `--dir` beats `agent_dir`
    // beats gwae's inherited cwd; `⌥+d` rebinds it live for panes spawned
    // from then on (existing panes keep whatever they were born with, since
    // a process's cwd is not ours to change).
    let mut spawn_dir: Option<std::path::PathBuf> =
        crate::spawndir::resolve(cli_dir.as_deref(), &cfg.agent_dir);
    // A configured directory that does not exist is a typo worth surfacing:
    // the panes silently opening in `~` is exactly the confusion this
    // feature exists to remove.
    let bad_dir: Option<String> = {
        let raw = cli_dir
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(cfg.spawn_dir());
        let fell_back = spawn_dir == crate::spawndir::inherited();
        match (fell_back, raw.trim().is_empty()) {
            (true, false) => crate::spawndir::check(raw).err(),
            _ => None,
        }
    };
    // A hot reload hands the previous image's session over in a temp file
    // (see `crate::reload`). Taken before the default layout is built so the
    // adopted tree replaces it rather than racing it.
    let handover = crate::reload::Handover::take();
    let mut layout = match &handover {
        Some(h) => h.layout.clone(),
        None => Layout::new(cfg.startup_panes.max(1)),
    };
    // A ⌥+d choice made before the reload must not be forgotten by the new
    // image, so the handover's value wins over re-resolving the config.
    if let Some(h) = &handover {
        if h.spawn_dir.is_some() {
            spawn_dir = h.spawn_dir.clone();
        }
    }
    // Clock continuity: adopted panes start their `last_output` at
    // adoption (see `adopt_pane`), so quiet ages measure from the exec,
    // not from zero. The sub-second exec gap is negligible next to the
    // 4s quiet window; no shift needed.
    // Ask (at most once a day, on a background thread) whether a newer gwae
    // exists. Started here so the request overlaps pane spawn instead of
    // adding to startup; the answer lands in a slot the main loop reads, and
    // a session that ends first simply never sees it. Nothing is ever
    // installed by this: the notice names the command and stops.
    let update_slot = crate::update::spawn_check(cfg.update.check, cfg.update.source_detected());
    // Authoritative harness status from the jcode daemon (`clients:map`):
    // a finished agent TUI keeps emitting maintenance output (spinner
    // redraws, cross-session notification toasts), so the activity
    // heuristic alone paints `Running` forever. The daemon knows which
    // sessions are actually generating; the loop reconciles `Running`
    // tiles against this snapshot below. Same detached-thread-plus-slot
    // shape as the update check: the loop only ever locks briefly.
    let harness_slot = crate::harness_status::spawn_poll();
    let (tx, rx) = channel::<PaneMsg>();
    // Input forwarder: a dedicated thread blocks in `event::read()` and
    // forwards every host terminal event into the same channel the PTY
    // readers use. The main loop then has exactly one blocking wait
    // (`recv_timeout` below) that wakes instantly for a keystroke *or* a
    // pane byte. The old shape — `event::poll(input_poll_ms)` with pane
    // output drained between polls — taxed every echo with up to a full
    // poll tick of queue latency; measured against other multiplexers that
    // tick was the difference between ~2.5 ms and sub-millisecond echo.
    // The thread parks in kernel space, costs nothing while idle, and dies
    // with the process (reads fail once the terminal handle is gone).
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match event::read() {
                Ok(ev) => {
                    if tx.send(PaneMsg::Input(ev)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("input forwarder: event read: {e}");
                    // Don't spin on a persistently failing terminal.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        });
    }
    let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
    // What `⌥+;` remembers: the last pick spawns with no UI. Loaded once at
    // startup; the loop owns it from here and saves on every pick.
    let harness_state_path = crate::agent::harness_state_path();
    let mut harness_state = match &harness_state_path {
        Some(path) => crate::agent::load_seeded_harness_state(path, &cfg.default_agent),
        None => crate::agent::HarnessState::default(),
    };
    fn resolve_startup_cmd(
        default_agent: &str,
        state: &crate::agent::HarnessState,
    ) -> Option<String> {
        let ordered = state.clone().order(crate::agent::detect());
        match crate::agent::plan(default_agent, state, ordered) {
            crate::agent::Plan::Configured(cmd) | crate::agent::Plan::Auto(cmd) => Some(cmd),
            _ => None,
        }
    }
    let initial = command.clone().unwrap_or_default();
    let initial_sizes: HashMap<_, _> = pane_grid_sizes(&layout, GridSize { cols, rows }, &cfg)
        .into_iter()
        .collect();
    // Spawn every pane in the initial strip; the rest get the user's shell.
    // Sort by id: `panes` is a HashMap, and unsorted iteration made *which
    // pane runs the command* random (ids are allocated in column order, so id
    // order is column order).
    //
    // Pane 1.1 is the agent pane unless `run <cmd>` named something else.
    // gwae exists to drive agents, so opening on a bare shell asked every
    // user to type the harness name themselves on every launch. A resolved
    // harness (an override, a remembered pick, a lone install) spawns by
    // name, indistinguishable from launching it directly; otherwise pane 1.1
    // runs the gateway bootstrap, which prompts in-pane exactly once and
    // remembers the pick. An explicit `run` command still wins, since that
    // is the user being specific.
    let mut pane_ids: Vec<PaneId> = layout.panes.keys().copied().collect();
    pane_ids.sort_unstable();
    let mut first_is_agent = false;
    // A resolved startup harness spawns by name so pane 1.1 *is* the harness
    // from the first frame. Otherwise pane 1.1 runs the gateway bootstrap,
    // which prompts in-pane exactly once and remembers the pick.
    let mut first_agent_cmd: Option<String> = None;
    // Reload path: the panes already exist as live PTYs inherited across the
    // execve, so they are adopted rather than spawned. Their children never
    // learn that gwae's code was replaced underneath them.
    #[cfg(unix)]
    let reloaded_agents: HashSet<PaneId> = match &handover {
        Some(h) => {
            for hp in &h.panes {
                match adopt_pane(hp.id, hp.fd, hp.pid, hp.cols, hp.rows, tx.clone()) {
                    Ok(p) => {
                        panes.insert(hp.id, p);
                    }
                    // One unusable fd must not cost the whole session: the
                    // pane is dropped from the layout and the rest carry on.
                    Err(e) => {
                        tracing::error!("adopt pane {}: {e}", hp.id);
                    }
                }
            }
            h.panes
                .iter()
                .filter(|p| p.is_agent)
                .map(|p| p.id)
                .collect()
        }
        None => HashSet::new(),
    };
    #[cfg(not(unix))]
    let reloaded_agents: HashSet<PaneId> = HashSet::new();
    let reloading = handover.is_some();
    for (i, pid) in pane_ids.iter().enumerate() {
        // Already adopted above; spawning would start a second child on a
        // pane that already has a live one.
        if reloading {
            break;
        }
        let cmd = if i == 0 {
            if initial.trim().is_empty() {
                first_is_agent = true;
                match resolve_startup_cmd(&cfg.default_agent, &harness_state) {
                    Some(direct) => {
                        first_agent_cmd = Some(direct.clone());
                        direct
                    }
                    None => agent_gateway_cmd(),
                }
            } else {
                initial.clone()
            }
        } else {
            String::new()
        };
        let size = initial_sizes[pid];
        match spawn_pane(
            *pid,
            &cmd,
            size.cols,
            size.rows,
            tx.clone(),
            spawn_dir.as_deref(),
            cell_pixels,
        ) {
            Ok(p) => {
                panes.insert(*pid, p);
            }
            Err(e) => {
                eprintln!("spawn: {e}");
                restore_terminal(&mut stdout, kitty_keyboard);
                return Err(1);
            }
        }
    }
    let mut frame: Vec<Cell> = Vec::new();
    let mut last: Vec<Cell> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut host_images = crate::graphics_host::Host::default();
    let mut dirty = true;
    // Headless PTY tests must not inherit a real Option key held in another
    // terminal. Protocol key events still work when this explicit opt-out is
    // set; normal interactive sessions retain native polling by default.
    let native_modifiers =
        native_modifier_poll_enabled(std::env::var("GWAE_NO_NATIVE_MODIFIERS").ok().as_deref());
    let mut bare_alt_held = false;
    let mut chord_alt_until: Option<Instant> = None;
    let mut last_alt_held = false;
    // Startup-only cheat-sheet HUD: shown once at init, dismissed on first key.
    let mut hud_active: bool = true;
    // The ⌥-hold dashboard's geometry for the frame currently on screen, so a
    // click can be resolved against the tiles the user is actually looking at.
    // `None` whenever the panel is not up.
    let mut hud_plan: Option<HudPlan> = None;
    let mut last_has_attention = has_attention(&layout);
    // Pane ids created by the spawn-agent verb; these are (re)spawned running
    // the agent gateway instead of a plain shell. A respawn re-resolves, so
    // installing a harness mid-session is picked up without a restart.
    let mut agent_panes: HashSet<PaneId> = HashSet::new();
    // Panes with a resolved harness command: spawned running the harness
    // directly, with no gateway in between. Entries are dropped with the pane.
    let mut agent_cmds: HashMap<PaneId, String> = HashMap::new();
    // Pane 1.1 counts as an agent pane when it opened on the gateway, so a
    // respawn re-resolves rather than dropping the user into a bare shell.
    if first_is_agent {
        if let Some(pid) = pane_ids.first() {
            agent_panes.insert(*pid);
            if let Some(cmd) = first_agent_cmd {
                agent_cmds.insert(*pid, cmd);
            }
            // Fresh allocs are `Plain`; pane 1.1 spawning a harness is
            // positive evidence of work.
            if let Some(lp) = layout.panes.get_mut(pid) {
                lp.status = PaneStatus::Running;
            }
        }
    }
    // Panes that were agent panes before a reload stay marked as such.
    //
    // Narrower than it sounds, and worth stating precisely: a pane whose
    // process exits is *closed*, not respawned (see the `PaneMsg::Exited`
    // arm, which also drops the pane from this set), so this does not
    // resurrect a dead harness. What it preserves is the marking itself, so
    // the set stays consistent with the layout it describes across a reload
    // rather than silently emptying.
    agent_panes.extend(reloaded_agents.iter().copied());
    // An adopted pane's grid starts empty (contents are not carried across a
    // reload), so ask each child to redraw. Without this the screen stays
    // blank until the user types, which reads as a crash. The nudge is a
    // deliberate, observable size change, so it finishes in two phases: the
    // restore lands a moment later, from the loop.
    let mut repaint_restore = RepaintRestore::default();
    if reloading {
        repaint_restore = nudge_repaint(&mut panes);
    }
    // The title currently shown on the host terminal; we only write when it
    // changes so we don't spam the host with identical OSC sequences.
    let mut last_title: String = String::new();
    // Live config reload state: the file we watch, its last seen mtime, when
    // we last checked, and the transient note shown after a reload.
    let cfg_path = Config::default_path();
    let mut cfg_mtime = Config::mtime(&cfg_path);
    let mut reload_check = Instant::now();
    // Hot reload (dev): watch our own binary and swap into the new build
    // in place, keeping every pane. See `crate::reload`.
    #[cfg(unix)]
    let hot_reload = crate::reload::enabled();
    // Dev session marker, read once: the Option HUD stamps DEV on its bottom
    // frame row iff this is on, so the dev tab is visually distinct from the
    // stable tab in the next terminal over.
    let dev_mode = crate::reload::enabled();
    #[cfg(unix)]
    let exe_path = crate::reload::own_path().ok();
    #[cfg(unix)]
    let mut exe_mtime = exe_path.as_deref().and_then(crate::reload::binary_mtime);
    // A rebuild is not atomic: the linker truncates and writes, so the file
    // can be seen mid-write with a fresh mtime and a broken image. Waiting
    // for the mtime to stop moving costs one poll interval and avoids
    // exec'ing a half-written binary.
    #[cfg(unix)]
    let mut exe_changed_at: Option<Instant> = None;
    // Auto-rebuild watch (dev): when `GWAE_DEV_WATCH=1` the session builds
    // itself on source change, so `make dev` needs no manual step. The
    // live panes always show the last good image: a build runs off the
    // event loop with piped output, and only a clean, signed, loadable
    // binary feeds the exec path above. Failures are invisible except for
    // the dim HUD pill; detail waits in the `⌥+/` overlay.
    #[cfg(unix)]
    let auto_build = crate::reload::watch_enabled() && hot_reload;
    #[cfg(unix)]
    let mut src_mtime = auto_build.then(crate::reload::source_mtime).flatten();
    #[cfg(unix)]
    let mut src_changed_at: Option<Instant> = None;
    #[cfg(unix)]
    let mut build_child: Option<std::process::Child> = None;
    // Tail of the in-flight build's output, filled by drain threads so the
    // child never blocks writing into a pipe nobody reads. See the spawn
    // site: not draining deadlocks the build and holds cargo's target lock.
    #[cfg(unix)]
    let mut build_output: Option<std::sync::Arc<std::sync::Mutex<String>>> = None;
    #[cfg(unix)]
    let mut build_phase: Option<crate::reload::BuildPhase> = None;
    #[cfg(unix)]
    let mut build_before: Option<std::time::SystemTime> = None;
    #[cfg(unix)]
    let mut build_cooldown_until: Option<Instant> = None;
    #[cfg(unix)]
    let mut build_error: Option<String> = None;
    // Crash-loop guard: too many execs in a minute pins the last good
    // image and stops watching until a manual rebuild.
    #[cfg(unix)]
    let mut exec_attempts: Vec<Instant> = Vec::new();
    #[cfg(unix)]
    let mut watch_held: Option<String> = None;
    // Quiet-window wait start for the exec gate above: held across ticks
    // while panes are busy, cleared on fire.
    #[cfg(unix)]
    let mut exe_wait_start: Option<Instant> = None;
    let mut size_check = Instant::now();
    // Last time *anything* happened (a keystroke or a byte from any pane).
    // Drives the adaptive input poll below: tight while in use, relaxed once
    // the whole screen has gone quiet.
    let mut last_activity = Instant::now();
    // A bad `agent_dir` announces itself on the first frame; the panes have
    // already opened in the inherited cwd by then, so this explains what the
    // user is looking at rather than blocking anything.
    let mut reload_note: Option<String> =
        bad_dir.map(|e| format!("agent_dir: {e}; panes opened in gwae's cwd"));
    let mut reload_note_until: Option<Instant> = reload_note
        .as_ref()
        .map(|_| Instant::now() + NOTE_LINGER * 2);
    // Where the note is drawn: `None` = bottom-left of the screen,
    // `Some(rect)` = bottom-left of that pane (drag-copy notes).
    let mut reload_note_anchor: Option<Rect> = None;
    // Whether the update notice still has to be shown. Latched false after
    // one showing so a user who dismissed it is not told again this session.
    let mut update_note_pending = true;
    // Spawn-directory picker (⌥+d): the candidate list, the typed filter, and
    // the highlighted row. Built when the picker opens rather than at startup
    // so a repo cloned mid-session shows up without a restart.
    let mut dir_pick: Option<DirPicker> = None;
    // Harness picker (`⌥+;` with more than one answer, `⌥+⇧+;` always): the
    // state-ordered candidates, the typed filter, and the highlighted row.
    // Built when the chord fires rather than at startup so a harness
    // installed mid-session shows up without a restart. The `⌥+;` fast paths
    // (an override, a remembered pick, a lone install) never open it; the
    // `⌥+⇧+;` force-pick always does.
    let mut harness_pick: Option<HarnessPicker> = None;
    // Force-quit confirmation (⌥+Shift+q): true while the centered disclaimer
    // is up. Quitting kills every pane's process, so the chord arms this
    // overlay and a second deliberate keystroke commits.
    let mut quit_confirm = false;
    // Pane selection: highlight while dragging, copy on release.
    let mut selection: Option<Selection<PaneId>> = None;
    // Layout changed and PTYs must be reconciled (spawn missing panes, kill
    // removed ones). Deferred until *after* the next frame is flushed: a
    // pane spawn (⌥+Enter) paints its empty frame first and forks the shell
    // after, so the keypress-to-frame latency never includes openpty+fork
    // (~3-5 ms). The spawned child's first output wakes the loop again via
    // the channel and fills the frame in.
    let mut needs_sync = false;
    // Host terminal events forwarded by the input thread, not yet handled.
    // Lives outside the loop because a `break 'drain` (e.g. per-kill frames
    // under a held ⌥+q) must keep the remaining queued events for the next
    // iteration instead of dropping them.
    let mut pending_input: std::collections::VecDeque<Event> = std::collections::VecDeque::new();

    'main: loop {
        // The loop's single blocking wait: sleep until *any* event source
        // fires — a keystroke or mouse report (via the input forwarder
        // thread) or pane output (via the PTY reader threads) — or until the
        // timer tick expires for the periodic work below (size re-check,
        // note expiry, status flips, bare-Option HUD detection). A dirty
        // frame or leftover input skips the block entirely so rendering is
        // never delayed. `input_poll_interval` keeps its adaptive cadence,
        // but it now only paces timers: input latency no longer depends on
        // it, because an event on the channel ends the wait immediately.
        let mut next_msg = if dirty || !pending_input.is_empty() {
            rx.try_recv().ok()
        } else {
            let mut tick = input_poll_interval(cfg.input_poll_ms, last_activity.elapsed());
            // A pending repaint restore is a deadline: sleeping past it would
            // leave every adopted pane one row taller than it should be for a
            // whole idle tick (up to 30ms), which the user would see as the
            // frame not quite fitting.
            if repaint_restore.is_pending() {
                let until = repaint_restore
                    .due_at()
                    .saturating_duration_since(Instant::now());
                tick = tick.min(until.max(std::time::Duration::from_millis(1)));
            }
            rx.recv_timeout(tick).ok()
        };
        while let Some(msg) = next_msg.take() {
            // Any traffic (input, output, or an exit) is activity: keep the
            // timer tick tight so a burst of output is drained and drawn at
            // full rate rather than at the idle backoff.
            last_activity = Instant::now();
            match msg {
                PaneMsg::Input(ev) => {
                    pending_input.push_back(ev);
                }
                PaneMsg::Output(pid, bytes) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.grid.set_cell_size(cell_pixels.width, cell_pixels.height);
                        feed_pane_output(
                            p,
                            &bytes,
                            host_kitty_graphics,
                            cfg.image_pane == crate::config::ImagePane::Auto,
                        );
                        p.last_output = Instant::now();
                        // Explicit OSC 133 status beats the activity
                        // heuristic from the first marker onward, for any
                        // pane (a shell-integrated plain shell is
                        // trustworthy). `Running` is the exception: it is an
                        // agent claim, so a non-agent pane's `133;C` (an
                        // editor, a pager, any foreground command in a plain
                        // shell) reads as neutral, not "working". The
                        // heuristic below it is agent-only too: a plain
                        // shell's output claims nothing.
                        // The scan folds markers over the pane's current
                        // status, so a `Failed` verdict survives the prompt
                        // marker that follows it (same read or a later one)
                        // and clears only on the next command start.
                        let cur = layout
                            .panes
                            .get(&pid)
                            .map(|lp| lp.status)
                            .unwrap_or(PaneStatus::Plain);
                        if let Some(st) = scan_osc133(cur, &bytes) {
                            p.saw_osc133 = true;
                            if let Some(lp) = layout.panes.get_mut(&pid) {
                                lp.status = osc_status(st, agent_panes.contains(&pid));
                            }
                        } else if !p.saw_osc133 && agent_panes.contains(&pid) {
                            // Fresh output from a protocol-less *agent* pane:
                            // working.
                            if let Some(lp) = layout.panes.get_mut(&pid) {
                                lp.status = PaneStatus::Running;
                            }
                        }
                        dirty = true;
                    }
                }
                PaneMsg::Exited(pid) => {
                    // A pane whose process exited closes naturally, exactly
                    // like Alt+q: remove it from the layout, compact columns
                    // (fill left first), and reap the PTY. When the last pane
                    // exits there is nothing left to show, so gwae quits.
                    if let Some(p) = panes.get_mut(&pid) {
                        p.alive = false;
                    }
                    let total = layout_pane_count(&layout);
                    let in_layout = layout.locate_pane(pid).is_some();
                    // Startup fallback: a directly-resolved harness (a
                    // remembered pick or a lone install) that dies straight
                    // away would otherwise take the whole session with it —
                    // gwae quits when the last pane exits, so a stale
                    // `harness.json` pointing at a broken command produced a
                    // flash of a frame and an exit 0 with no explanation.
                    // Instead, the sole pane respawns as the gateway picker,
                    // names the command that died, and the poisoned memory is
                    // dropped so the next bare launch does not repeat it.
                    // Only within a short grace window: a harness the user
                    // has been working in for minutes exiting is a normal
                    // quit, not a broken command. An explicit `gwae run cmd`
                    // never lands here (it is not an agent pane), so a fast
                    // one-shot command still exits cleanly by design.
                    if in_layout && total <= 1 && boot.elapsed() < STARTUP_FALLBACK_GRACE {
                        if let Some(died) = agent_cmds.remove(&pid) {
                            if harness_state.last == died {
                                harness_state.last.clear();
                                harness_state.mru.retain(|c| c != &died);
                                if let Some(path) = harness_state_path.as_deref() {
                                    let _ = crate::agent::save_harness_state(path, &harness_state);
                                }
                            }
                            panes.remove(&pid);
                            agent_panes.insert(pid);
                            needs_sync = true;
                            reload_note =
                                Some(format!("`{died}` exited immediately; pick an agent"));
                            reload_note_anchor = None;
                            reload_note_until = Some(Instant::now() + NOTE_LINGER * 2);
                            dirty = true;
                            next_msg = rx.try_recv().ok();
                            continue;
                        }
                    }
                    if in_layout && total <= 1 {
                        break 'main;
                    }
                    if in_layout {
                        let v = Viewport::new(cols);
                        let f = FollowScroll::default();
                        let _ = layout.apply(Action::ClosePane(pid), v, f);
                        agent_panes.remove(&pid);
                        agent_cmds.remove(&pid);
                        needs_sync = true;
                    } else {
                        // Already removed from the layout (explicit kill);
                        // just drop the dead PTY handle.
                        agent_cmds.remove(&pid);
                        panes.remove(&pid);
                    }
                    dirty = true;
                }
            }
            // Drain whatever else is queued without blocking, then move on
            // to handle input and paint one frame for the whole batch.
            next_msg = rx.try_recv().ok();
        }

        // Second half of the reload repaint nudge: put the real sizes back
        // once the child has had time to notice it grew. Doing this here
        // rather than sleeping in the adopt path keeps the loop responsive
        // while the panes are momentarily one row taller.
        if repaint_restore.maybe_apply(&mut panes) {
            dirty = true;
        }

        // Keep the frame sized to the live terminal, even when a resize event
        // is dropped or coalesced. Re-measuring here guarantees the panes stay
        // full-bleed to the actual right margin.
        if size_check.elapsed() >= SIZE_POLL {
            size_check = Instant::now();
            // Font/DPI changes can alter pixels without changing rows/cols.
            // Keep the last valid metrics across a transient ioctl failure.
            if let Some(measured) = CellPixels::measure() {
                cell_pixels = measured;
            }
            if refresh_size(&mut cols, &mut rows) {
                layout.clamp_scrolls(Viewport::new(cols));
                dirty = true;
            }
        }

        // Activity heuristic for *agent* panes that never speak OSC 133
        // (agent TUIs without shell integration): output within the window
        // means "working", silence past it flips to "wants attention" so the
        // minimap and smart-jump still triage them. Panes with real shell
        // integration are owned by the explicit protocol above and skipped
        // here; plain (non-agent) panes are skipped entirely — their output
        // claims nothing and they stay `Plain`.
        //
        // Panes the daemon already covers skip the heuristic: otherwise a
        // quiet-but-busy pane would flap every frame (heuristic writes
        // `Idle`, daemon writes `Running` back), repainting the frame for
        // no visible change. The daemon verdict below is the persistent
        // answer for those panes.
        let now = Instant::now();
        let daemon_covers: std::collections::HashSet<PaneId> = match harness_slot.lock() {
            Ok(slot)
                if !crate::harness_status::snapshot_stale(&slot, now)
                    && !slot.clients.is_empty() =>
            {
                panes
                    .iter()
                    .filter_map(|(pid, p)| {
                        crate::harness_status::status_for_title(p.grid.title(), &slot).map(|_| *pid)
                    })
                    .collect()
            }
            _ => std::collections::HashSet::new(),
        };
        for (pid, p) in panes.iter() {
            if p.saw_osc133 || !agent_panes.contains(pid) || daemon_covers.contains(pid) {
                continue;
            }
            let quiet = now.duration_since(p.last_output) >= QUIET_AFTER;
            let want = if quiet {
                PaneStatus::Idle
            } else {
                PaneStatus::Running
            };
            if let Some(lp) = layout.panes.get_mut(pid) {
                if lp.status != want {
                    lp.status = want;
                    dirty = true;
                }
            }
        }

        // Authoritative override from the jcode daemon, in both directions:
        // a finished agent TUI keeps emitting maintenance output (periodic
        // redraws, ambient notification toasts), so the heuristic above can
        // hold `Running` forever on a done client — demote that tile to
        // `Idle` (done, waiting on the user), or to `Failed` when the daemon
        // says the turn broke. Conversely a generating agent
        // in a quiet stretch (a long tool call with no redraw) goes silent
        // past the quiet window — hold that tile at `Running` instead of
        // flapping to attention mid-turn. A `Failed` tile is included only
        // so a daemon-busy session (a retry started) un-fails to `Running`;
        // a failed shell verdict on a settled session stands (`reconcile`
        // returns `None` for it). Unknown panes (no title match, no daemon,
        // stale snapshot) keep the heuristic.
        //
        // The mapping is the pane's terminal title, which embeds the
        // session short name (`jcode Iwazaru`, `Release planning (fox)`).
        // Fresh panes with no title yet simply miss and retry next frame.
        // Staleness reads the snapshot's own refresh clock (stamped by the
        // poller on every success), so a healthy poller never ages out no
        // matter how long the session has lived.
        if let Ok(slot) = harness_slot.lock() {
            if !crate::harness_status::snapshot_stale(&slot, now) && !slot.clients.is_empty() {
                // `slot` is borrowed; collect first so the layout and
                // panes borrows below do not fight it.
                let candidates: Vec<(PaneId, PaneStatus, String)> = layout
                    .panes
                    .iter()
                    .filter(|(_, lp)| {
                        matches!(
                            lp.status,
                            PaneStatus::Running | PaneStatus::Idle | PaneStatus::Failed
                        )
                    })
                    .filter_map(|(pid, lp)| {
                        panes
                            .get(pid)
                            .map(|p| (*pid, lp.status, p.grid.title().to_string()))
                    })
                    .collect();
                for (pid, current, title) in candidates {
                    let st = crate::harness_status::status_for_title(&title, &slot);
                    if let Some(want) = crate::harness_status::reconcile(current, st) {
                        if let Some(lp) = layout.panes.get_mut(&pid) {
                            lp.status = want;
                            dirty = true;
                        }
                    }
                }
            }
        }

        // Live config reload. Editing the config file repaints the running
        // session, so a config edit is a save away rather than a restart:
        // restarting would kill every pane, which is exactly what someone
        // running long-lived agents cannot afford.
        //
        // Polled by mtime rather than a filesystem watcher: one `stat` at the
        // rate below is negligible next to the render loop, and it avoids a
        // dependency plus the cross-platform watcher differences on the three
        // OSes gwae supports.
        if reload_check.elapsed() >= CONFIG_POLL {
            reload_check = Instant::now();
            let now = Config::mtime(&cfg_path);
            if now != cfg_mtime {
                cfg_mtime = now;
                match Config::load_checked(&cfg_path) {
                    Ok(new) => {
                        // Keep the panes and their harnesses exactly as they
                        // are; only adopt what is re-read every frame.
                        // The keep-awake guard follows silently: no
                        // bottom-left toast, the HUD keep-awake badge carries it.
                        // The palette is re-read too, so a `[theme]` edit
                        // repaints the chrome on the next frame.
                        cfg.adopt_appearance(new);
                        pal = cfg.palette();
                        keep_awake.refresh(cfg.keep_awake);
                        reload_note_anchor = None;
                        reload_note = Some("config reloaded".to_string());
                        dirty = true;
                    }
                    Err(e) => {
                        // Keep the running config: a half-written file (the
                        // editor saved mid-keystroke) must not blow away the
                        // running session.
                        reload_note_anchor = None;
                        reload_note = Some(format!("config error: {}", first_line(&e)));
                        dirty = true;
                    }
                }
                reload_note_until = Some(Instant::now() + NOTE_LINGER);
            }
        }
        // Auto-rebuild (dev): sources changed -> build off the event
        // loop with piped output. Only a clean, loadable binary reaches
        // the exec path below; failures stay invisible on the last good
        // image with the error held for the `⌥+/` overlay.
        #[cfg(unix)]
        if auto_build && watch_held.is_none() {
            // Poll an in-flight build without blocking the frame.
            if let Some(child) = build_child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // The drain threads own the pipes, so the tail is
                        // already collected: read it rather than reading the
                        // child, which would block on a pipe nobody is
                        // writing to.
                        build_child = None;
                        let stderr_tail = build_output
                            .take()
                            .map(|log| crate::reload::build_log_tail(&log, 40))
                            .unwrap_or_default();
                        let outcome = crate::reload::build_outcome(
                            status.success(),
                            &stderr_tail,
                            exe_path.as_deref().unwrap_or(std::path::Path::new("")),
                            build_before,
                        );
                        build_phase = None;
                        match outcome {
                            crate::reload::BuildOutcome::Ready => {
                                build_error = None;
                                // Codesign the fresh image the way
                                // `make dev-build` does, best-effort.
                                if let Some(exe) = exe_path.as_deref() {
                                    let _ = std::process::Command::new("codesign")
                                        .args(["-f", "-s", "-"])
                                        .arg(exe)
                                        .output();
                                }
                                // Fall through to the exec path below:
                                // the binary mtime moved, so it fires.
                            }
                            crate::reload::BuildOutcome::Failed(msg) => {
                                if msg.is_empty() {
                                    // Cached no-op build: nothing to do.
                                } else {
                                    build_error = Some(msg);
                                }
                                // Back off before the next attempt so a
                                // save storm does not fork builds forever.
                                build_cooldown_until =
                                    Some(Instant::now() + std::time::Duration::from_secs(5));
                            }
                        }
                        dirty = true;
                    }
                    Ok(None) => {
                        // Still building: keep the pill up, keep painting.
                    }
                    Err(_) => {
                        build_child = None;
                        build_output = None;
                        build_phase = None;
                    }
                }
            } else {
                // No build running: watch sources with a settle window so
                // a save burst fires one build, not one per keystroke.
                let now_src = crate::reload::source_mtime();
                if now_src != src_mtime {
                    src_mtime = now_src;
                    src_changed_at = Some(Instant::now());
                } else if src_changed_at
                    .map(|t| t.elapsed() >= std::time::Duration::from_millis(500))
                    .unwrap_or(false)
                    && !build_cooldown_until
                        .map(|t| Instant::now() < t)
                        .unwrap_or(false)
                {
                    src_changed_at = None;
                    // Snapshot the binary mtime so the outcome check can
                    // tell a real rebuild from a cached no-op.
                    build_before = exe_path.as_deref().and_then(crate::reload::binary_mtime);
                    // Spawn detached from the panes: piped output, never
                    // touching a PTY. The agent session driving the
                    // feature reads its own log; this session stays
                    // pristine on the last good image.
                    let mut cmd = std::process::Command::new("cargo");
                    cmd.args(["build", "--offline"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    // Run from the workspace root so relative manifests
                    // resolve. Best-effort: failure to spawn just
                    // retries next poll.
                    if let Some(exe) = exe_path.as_deref() {
                        if let Some(root) = exe
                            .parent()
                            .and_then(|p| p.parent())
                            .and_then(|p| p.parent())
                        {
                            cmd.current_dir(root);
                        }
                    }
                    match cmd.spawn() {
                        Ok(mut child) => {
                            // Drain both pipes continuously. Without this the
                            // build deadlocks once its output fills the pipe
                            // buffer, and takes cargo's target lock with it;
                            // see `reload::drain_build_output`.
                            build_output = Some(crate::reload::drain_build_output(&mut child));
                            build_child = Some(child);
                            build_phase = Some(crate::reload::BuildPhase::Building);
                            build_error = None;
                            dirty = true;
                        }
                        Err(_) => {
                            build_cooldown_until =
                                Some(Instant::now() + std::time::Duration::from_secs(5));
                        }
                    }
                }
            }
        }
        // Hot reload: the binary on disk changed, so replace this process
        // with the new build and carry every pane across (see
        // `crate::reload`). Dev-gated: the failure mode of a subtly wrong
        // reload is orphaned agent processes, not a bad frame.
        #[cfg(unix)]
        if hot_reload {
            if let Some(exe) = exe_path.as_deref() {
                let now = crate::reload::binary_mtime(exe);
                if now != exe_mtime {
                    // Changed: note when, and wait for it to settle.
                    exe_mtime = now;
                    exe_changed_at = Some(Instant::now());
                } else if exe_changed_at
                    .map(|t| t.elapsed() >= BINARY_SETTLE)
                    .unwrap_or(false)
                {
                    exe_changed_at = None;
                    // Crash-loop guard: too many execs in a minute pins
                    // the last good image and holds the watch.
                    exec_attempts.retain(|t| t.elapsed() < std::time::Duration::from_secs(60));
                    if exec_attempts.len() >= 3 {
                        watch_held =
                            Some("reload held: 3 execs in 60s — rebuild manually".to_string());
                        tracing::error!("hot reload held: crash-loop guard fired");
                        dirty = true;
                    } else {
                        // Prefer a quiet window so output does not tear
                        // mid-frame: hold the exec until panes go quiet
                        // (300ms) or 2s pass, whichever comes first.
                        let quiet =
                            last_activity.elapsed() >= std::time::Duration::from_millis(300);
                        let waited_long_enough =
                            exe_wait_start.get_or_insert(Instant::now()).elapsed()
                                >= std::time::Duration::from_secs(2);
                        if !quiet && !waited_long_enough {
                            // Not quiet yet: re-arm the settle and try
                            // again next tick. The wait clock keeps
                            // running so a busy session still reloads
                            // after 2s.
                            exe_changed_at = Some(Instant::now());
                        } else {
                            exe_wait_start = None;
                            exec_attempts.push(Instant::now());
                            // Leave the terminal exactly as a normal exit would: the
                            // tty is kernel state and survives the exec, so a new
                            // image would otherwise inherit raw mode and the alt
                            // screen and paint into a screen it never entered.
                            tracing::info!("hot reload: binary changed, execing new image");
                            let _ = stdout.write_all(&host_images.clear());
                            restore_terminal(&mut stdout, kitty_keyboard);
                            // Drop the sleep assertion before the exec: the new image
                            // acquires its own guard at startup, and the old
                            // `caffeinate` (bound with `-w` to this pid) exits with us.
                            keep_awake.release();
                            match perform_reload(
                                &layout,
                                &panes,
                                &agent_panes,
                                spawn_dir.as_deref(),
                                boot,
                            ) {
                                // `Ok` is uninhabited: the process is gone.
                                Ok(never) => match never {},
                                Err(e) => {
                                    // The exec failed, so this image is still running
                                    // and still owns every pane. Put the screen back
                                    // and carry on rather than dying with them.
                                    tracing::error!("hot reload failed: {e}");
                                    // Re-acquire the sleep assertion we released
                                    // before the exec: this image is still the
                                    // session, so it keeps the promise the config
                                    // makes.
                                    keep_awake.refresh(cfg.keep_awake);
                                    if let Err(e) = re_enter_terminal(&mut stdout) {
                                        tracing::error!("restore after failed reload: {e}");
                                        break 'main;
                                    }
                                    last.clear();
                                    reload_note_anchor = None;
                                    reload_note =
                                        Some(format!("hot reload failed: {}", first_line(&e)));
                                    reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                    dirty = true;
                                }
                            }
                        } // end quiet-gate else: exec path above only runs when quiet or timed out
                    }
                }
            }
        }

        // The background update check, if it found something. Taken (not
        // read) so the notice is shown exactly once per session, and only
        // when the screen is not already saying something else: an upgrade
        // hint is the least urgent thing gwae ever has to say, so it yields
        // to a config error or a paste note rather than stomping it.
        if update_note_pending && reload_note.is_none() {
            if let Some(text) = update_slot.lock().ok().and_then(|mut g| g.take()) {
                update_note_pending = false;
                reload_note_anchor = None;
                reload_note = Some(text);
                // Longer than a reload note: this one asks the reader to
                // remember a command, not just to notice that a color
                // changed.
                reload_note_until = Some(Instant::now() + NOTE_LINGER * 3);
                dirty = true;
            }
        }
        // Expire the reload note so it does not sit on screen forever.
        if let Some(t) = reload_note_until {
            if Instant::now() >= t {
                reload_note = None;
                reload_note_until = None;
                reload_note_anchor = None;
                dirty = true;
            }
        }

        // Handle the host terminal events forwarded by the input thread.
        // Batch semantics are unchanged from the old zero-timeout drain:
        // Korean (Hangul) composition and fast typing queue multiple UTF-8
        // KeyEvents per frame; handling them all before rendering avoids a
        // render+wait of latency per syllable. `break 'drain` (per-kill
        // frames) leaves the remainder queued for the next iteration.
        if !pending_input.is_empty() {
            last_activity = Instant::now();
            'drain: loop {
                let ev: std::io::Result<Event> = match pending_input.pop_front() {
                    Some(ev) => Ok(ev),
                    None => break 'drain,
                };
                match ev {
                    Ok(Event::Paste(text)) => {
                        // Native paste is data, never a sequence of keybinds.
                        // In particular, a pasted newline cannot confirm quit
                        // or accept a picker, and Option glyphs stay text.
                        selection.take_if(|s| !s.dragging);
                        hud_active = false;
                        dirty = true;
                        if quit_confirm {
                            quit_confirm = false;
                            continue;
                        }
                        if let Some(pick) = dir_pick.as_mut() {
                            let query = picker_paste_query(&text);
                            if !query.is_empty() {
                                pick.query.push_str(&query);
                                pick.sel = 0;
                            }
                            continue;
                        }
                        if let Some(pick) = harness_pick.as_mut() {
                            let query = picker_paste_query(&text);
                            if !query.is_empty() {
                                pick.query.push_str(&query);
                                pick.sel = 0;
                            }
                            continue;
                        }
                        let anchor = focused_pane_views_with_chrome(
                            &layout,
                            cols,
                            rows,
                            0,
                            true,
                            chrome_rows(&cfg),
                        )
                        .iter()
                        .find(|v| focused_pane(&layout).is_some_and(|pid| v.pid == pid))
                        .map(|v| v.rect);
                        if let Some(p) = focused_pane(&layout).and_then(|pid| panes.get_mut(&pid)) {
                            // Re-frame for this child's current DECSET 2004
                            // state. Agent panes receive the same text event,
                            // not a clipboard chord (which could paste twice).
                            let bracketed = p.grid.wants_bracketed_paste();
                            let bytes = select::paste_bytes(&text, bracketed);
                            if !bytes.is_empty() {
                                p.grid.scroll_to_bottom();
                                let result =
                                    bytes.chunks(select::PASTE_CHUNK).try_for_each(|chunk| {
                                        p.writer.write_all(chunk).and_then(|()| p.writer.flush())
                                    });
                                reload_note = Some(match result {
                                    Ok(()) => paste_note(&text, bracketed),
                                    Err(e) => {
                                        tracing::warn!("native paste: {e}");
                                        format!("paste failed: {e}")
                                    }
                                });
                                reload_note_anchor = anchor;
                                reload_note_until = Some(Instant::now() + NOTE_LINGER);
                            }
                        }
                    }
                    Ok(Event::Key(ke))
                        if matches!(ke.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        // The HUD persists until the next key press; ⌥+/ toggles
                        // it explicitly, so remember whether it was up before we
                        // dismiss it here.
                        let hud_was_active = hud_active;
                        // Typing dismisses a finished selection, the way it does in
                        // any editor: the highlight is a transient artifact of the
                        // drag, and leaving it inverted over live pane output would
                        // read as corruption. A drag still in flight is left alone.
                        if selection.take_if(|s| !s.dragging).is_some() {
                            dirty = true;
                        }
                        if hud_active {
                            hud_active = false;
                            dirty = true;
                        }
                        // Bare Alt hold: track before handle_key so chords don't double-count.
                        let bare_alt = is_alt_modifier(&ke);
                        if bare_alt {
                            if !bare_alt_held {
                                bare_alt_held = true;
                                dirty = true;
                            }
                        } else {
                            // Fallback for terminals that don't send bare Alt press/release
                            // (no Kitty keyboard protocol): any Alt chord counts as "held"
                            // for a short window so the centered HUD/minimap still reveals on press-and-hold
                            // via repeated chords (Option+hjkl etc.) or a single chord.
                            let alt_chord = ke.modifiers.contains(KeyModifiers::ALT)
                                || matches!(
                                    ke.code,
                                    KeyCode::Char('\u{2d9}')
                                        | KeyCode::Char('\u{2206}')
                                        | KeyCode::Char('\u{2da}')
                                        | KeyCode::Char('\u{ac}')
                                        | KeyCode::Char('\u{2026}')
                                        | KeyCode::Char('\u{153}')
                                        | KeyCode::Char('\u{a9}')
                                        | KeyCode::Char('\u{d3}')
                                        | KeyCode::Char('\u{d4}')
                                        | KeyCode::Char('\u{f8ff}')
                                        | KeyCode::Char('\u{d2}')
                                        | KeyCode::Char('\u{192}')
                                        | KeyCode::Char('\u{2202}')
                                        | KeyCode::Char('\u{2211}')
                                        | KeyCode::Char('\u{222b}')
                                        | KeyCode::Char('\u{f7}')
                                        | KeyCode::Char('\u{bf}')
                                        | KeyCode::Char('\u{da}')
                                );
                            if alt_chord {
                                // Short window so tapping Option+h for navigation
                                // doesn't linger 600ms after release. Hold stays
                                // visible via repeats (each chord refreshes) but
                                // vanishes ~180ms after the last chord.
                                chord_alt_until = Some(Instant::now() + Duration::from_millis(180));
                                dirty = true;
                            }
                        }
                        // While the force-quit disclaimer is up it owns the
                        // keyboard: nothing may reach a pane, because the very
                        // next keystroke may destroy every pane. Only the same
                        // chord again (or Enter) commits; anything else cancels,
                        // so a stray key can never quit by accident.
                        if quit_confirm {
                            // A held chord repeats; the disclaimer must only
                            // commit on a second deliberate press, never on
                            // the auto-repeat of the chord that armed it.
                            if ke.kind == KeyEventKind::Repeat {
                                continue;
                            }
                            let confirmed = matches!(handle_key(&ke), Some(Cmd::Quit))
                                || matches!(ke.code, KeyCode::Enter);
                            if confirmed {
                                break 'main;
                            }
                            quit_confirm = false;
                            dirty = true;
                            continue;
                        }
                        // While the directory picker is open it owns the
                        // keyboard: every printable key types into the filter, so
                        // nothing may reach a pane. Arrows move, ⌃/⌥+j/k moves
                        // too (bare j/k must keep filtering paths), ⏎ takes it
                        // for the session, `⌥+s` writes it to the config file,
                        // esc cancels. Bare `s` cannot save, because `s` is a
                        // filter character like any other.
                        if let Some(pick) = dir_pick.as_mut() {
                            let alt = ke.modifiers.contains(KeyModifiers::ALT);
                            let mut chosen: Option<(std::path::PathBuf, bool)> = None;
                            let mut close = false;
                            if let Some(d) = picker_step(&ke) {
                                pick.step(d);
                            } else {
                                match ke.code {
                                    KeyCode::Esc => close = true,
                                    KeyCode::Backspace => {
                                        pick.query.pop();
                                        pick.sel = 0;
                                    }
                                    KeyCode::Enter => {
                                        if let Some(c) = pick.current() {
                                            chosen = Some((c.path, false));
                                        }
                                        close = true;
                                    }
                                    KeyCode::Char('s') if alt => {
                                        if let Some(c) = pick.current() {
                                            chosen = Some((c.path, true));
                                        }
                                        close = true;
                                    }
                                    // ß is what macOS sends for ⌥+s when Option is not
                                    // mapped to Meta, the same fallback the rest of
                                    // the chords carry.
                                    KeyCode::Char('\u{df}') => {
                                        if let Some(c) = pick.current() {
                                            chosen = Some((c.path, true));
                                        }
                                        close = true;
                                    }
                                    KeyCode::Char(c)
                                        if !alt
                                            && !ke.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        pick.query.push(c);
                                        pick.sel = 0;
                                    }
                                    _ => {}
                                }
                            }
                            // The picker's harness decides which config key `save` writes.
                            // Read it before we clear `dir_pick`.
                            let harness_for_save = pick.harness_label.clone();
                            if close {
                                dir_pick = None;
                            }
                            if let Some((path, save)) = chosen {
                                spawn_dir = Some(path.clone());
                                let shown = crate::spawndir::tilde(&path);
                                reload_note_anchor = None;
                                reload_note = Some(if save {
                                    match write_harness_dir(&cfg_path, &harness_for_save, &shown) {
                                        Ok(()) => {
                                            if harness_for_save.is_empty() {
                                                format!("spawn dir: {shown} (saved to config)")
                                            } else {
                                                format!(
                                                    "spawn dir [{harness_for_save}]: {shown} (saved to config)"
                                                )
                                            }
                                        }
                                        Err(e) => format!(
                                            "spawn dir: {shown} (this session; save error: {e})"
                                        ),
                                    }
                                } else {
                                    format!("spawn dir: {shown} — new panes start here")
                                });
                                reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                // A config write bumps the mtime; adopt it now so
                                // the reload watcher does not report our own save
                                // back to us as an external edit.
                                cfg_mtime = Config::mtime(&cfg_path);
                            }
                            dirty = true;
                            continue;
                        }
                        // While the harness picker is open it owns the
                        // keyboard, like the directory picker: printable keys
                        // filter, arrows and ⌃/⌥+j/k move, ⏎ spawns, esc
                        // cancels. A pick is remembered in the state file, so
                        // there is no save key; the shell row spawns a plain
                        // pane.
                        if let Some(pick) = harness_pick.as_mut() {
                            let mut chosen: Option<HarnessChoice> = None;
                            let mut close = false;
                            if let Some(d) = picker_step(&ke) {
                                pick.step(d);
                            } else {
                                match ke.code {
                                    KeyCode::Esc => close = true,
                                    KeyCode::Backspace => {
                                        pick.query.pop();
                                        pick.sel = 0;
                                    }
                                    KeyCode::Enter => {
                                        // Never on auto-repeat: the first Enter
                                        // closes the overlay and remembers the
                                        // pick, so a repeat would fall through to
                                        // a fresh ⌥+; and fast-spawn a second
                                        // pane from the memory just written.
                                        if ke.kind != KeyEventKind::Repeat {
                                            chosen = pick.current();
                                            close = true;
                                        }
                                    }
                                    KeyCode::Char(c)
                                        if !ke.modifiers.contains(KeyModifiers::ALT)
                                            && !ke.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        pick.query.push(c);
                                        pick.sel = 0;
                                    }
                                    _ => {}
                                }
                            }
                            if close {
                                harness_pick = None;
                            }
                            if let Some(choice) = chosen {
                                match choice {
                                    HarnessChoice::Shell => {
                                        let v = Viewport::new(cols);
                                        let f = FollowScroll::default();
                                        let _ = layout.apply(Action::SpawnAgent, v, f);
                                        if let Err(e) = sync_panes(
                                            &mut layout,
                                            &mut panes,
                                            &tx,
                                            (GridSize { cols, rows }, cell_pixels),
                                            &agent_panes,
                                            &agent_cmds,
                                            spawn_dir.as_deref(),
                                            &cfg,
                                        ) {
                                            tracing::error!("sync panes: {e}");
                                        }
                                    }
                                    HarnessChoice::Listed(f) => {
                                        spawn_picked_harness(
                                            &f.cmd,
                                            true,
                                            &mut layout,
                                            &mut panes,
                                            &mut agent_panes,
                                            &mut agent_cmds,
                                            &tx,
                                            (GridSize { cols, rows }, cell_pixels),
                                            spawn_dir.as_deref(),
                                            &cfg,
                                            &mut harness_state,
                                            harness_state_path.as_deref(),
                                            &mut reload_note,
                                            &mut reload_note_until,
                                        );
                                    }
                                    HarnessChoice::Typed(cmd) => {
                                        spawn_picked_harness(
                                            &cmd,
                                            false,
                                            &mut layout,
                                            &mut panes,
                                            &mut agent_panes,
                                            &mut agent_cmds,
                                            &tx,
                                            (GridSize { cols, rows }, cell_pixels),
                                            spawn_dir.as_deref(),
                                            &cfg,
                                            &mut harness_state,
                                            harness_state_path.as_deref(),
                                            &mut reload_note,
                                            &mut reload_note_until,
                                        );
                                    }
                                }
                            }
                            dirty = true;
                            continue;
                        }
                        // Harness-first scroll: an agent harness (jcode) owns
                        // Ctrl+Shift+J/K natively, so when it is focused the
                        // chord is forwarded to the child untouched instead of
                        // scrolling gwae's history. Plain shells keep the gwae
                        // three-line scroll. `key_bytes` emits kitty CSI-u for the
                        // chord so a kitty-aware child decodes CONTROL|SHIFT.
                        if is_harness_scroll_chord(&ke)
                            && focused_pane(&layout).is_some_and(|pid| agent_panes.contains(&pid))
                        {
                            if let Some(pid) = focused_pane(&layout) {
                                if let Some(p) = panes.get_mut(&pid) {
                                    if p.grid.scroll_to_bottom() {
                                        dirty = true;
                                    }
                                    let _ = p.writer.write_all(&key_bytes(&ke));
                                    let _ = p.writer.flush();
                                }
                            }
                            continue;
                        }
                        if let Some(cmd) = handle_key(&ke) {
                            // Destructive commands never fire on auto-repeat:
                            // holding ⌥+q must not kill panes faster than the
                            // HUD can repaint them, and a held quit or toggle
                            // must not confirm or flicker itself. The repeat
                            // still refreshes the hold window above, so the
                            // dashboard stays up while the key is down — it
                            // just stops acting on it.
                            if ke.kind == KeyEventKind::Repeat && !cmd.is_repeatable() {
                                continue;
                            }
                            match cmd {
                                // Arm the disclaimer rather than exiting: the
                                // second press (handled above) is the one that
                                // actually kills every pane.
                                Cmd::Quit => {
                                    quit_confirm = true;
                                    dirty = true;
                                }
                                Cmd::ToggleHud => {
                                    hud_active = !hud_was_active;
                                    dirty = true;
                                }
                                Cmd::ToggleKeepAwake => {
                                    // Visible toggle: the HUD badge carries the
                                    // state while an overlay is up, but the
                                    // toggle itself dismisses the HUD, so a
                                    // bottom-left note confirms the flip.
                                    if !cfg!(target_os = "macos") {
                                        reload_note_anchor = None;
                                        reload_note = Some("keep-awake is macOS-only".to_string());
                                        reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                    } else {
                                        // Session-only: never persisted, and
                                        // always off again at next launch.
                                        cfg.keep_awake = !cfg.keep_awake;
                                        keep_awake.refresh(cfg.keep_awake);
                                        reload_note_anchor = None;
                                        reload_note = Some(if cfg.keep_awake {
                                            if keep_awake.active() {
                                                "keep-awake on for this session (off again at next launch)".to_string()
                                            } else {
                                                format!(
                                                    "keep-awake on (this session), assertion not held ({})",
                                                    crate::keepawake::availability_note()
                                                )
                                            }
                                        } else {
                                            "keep-awake off".to_string()
                                        });
                                        reload_note_until = Some(Instant::now() + NOTE_LINGER);
                                    }
                                    dirty = true;
                                }
                                Cmd::DirPick => {
                                    // Rebuilt on every open: repos are cloned and
                                    // deleted while gwae runs, and the scan is a
                                    // handful of readdirs. Harness label sourced
                                    // from the live resolution (override, memory,
                                    // or lone install), so the title names the
                                    // agent `⌥+;` would actually spawn.
                                    let harness_label = {
                                        let ordered =
                                            harness_state.clone().order(crate::agent::detect());
                                        match crate::agent::plan(
                                            &cfg.default_agent,
                                            &harness_state,
                                            ordered,
                                        ) {
                                            crate::agent::Plan::Configured(cmd)
                                            | crate::agent::Plan::Auto(cmd) => {
                                                crate::tui::shell_split(&cmd)
                                                    .first()
                                                    .cloned()
                                                    .unwrap_or_default()
                                            }
                                            _ => String::new(),
                                        }
                                    };
                                    let all = crate::spawndir::candidates(
                                        spawn_dir.as_deref(),
                                        &cfg.agent_dir,
                                        &[],
                                        &[],
                                    );
                                    dir_pick = Some(DirPicker {
                                        all,
                                        query: String::new(),
                                        sel: 0,
                                        harness_label,
                                    });
                                    dirty = true;
                                }
                                Cmd::ForcePick => {
                                    // `⌥+⇧+;`: the overlay opens unconditionally,
                                    // ignoring the fast paths `⌥+;` takes. The
                                    // notice mirrors the `⌥+;` overlay's own
                                    // wording; the silent case is a healthy
                                    // bypass of a remembered pick or lone
                                    // install.
                                    let ordered =
                                        harness_state.clone().order(crate::agent::detect());
                                    harness_pick = Some(HarnessPicker {
                                        notice: force_pick_notice(
                                            &cfg.default_agent,
                                            &harness_state.last,
                                            &ordered,
                                        ),
                                        all: ordered,
                                        query: String::new(),
                                        sel: 0,
                                    });
                                    dirty = true;
                                }
                                Cmd::ScrollBack(d) => {
                                    if let Some(pid) = focused_pane(&layout) {
                                        if let Some(p) = panes.get_mut(&pid) {
                                            // A full-screen app (vim, less) owns
                                            // its own scrolling and has no
                                            // scrollback of ours to move, so send
                                            // it the arrow keys it expects instead.
                                            if p.grid.alternate_screen() {
                                                let key: &[u8] =
                                                    if d > 0 { b"\x1b[A" } else { b"\x1b[B" };
                                                for _ in 0..d.abs().min(20) {
                                                    let _ = p.writer.write_all(key);
                                                }
                                                let _ = p.writer.flush();
                                            } else if p.grid.scroll_by(d) {
                                                dirty = true;
                                            }
                                        }
                                    }
                                }
                                Cmd::Act(a) => {
                                    let v = Viewport::new(cols);
                                    let f = FollowScroll::default();
                                    // Closing the last pane leaves nothing to show,
                                    // so gwae exits instead of resurrecting a
                                    // fresh default layout.
                                    if a == Action::KillPane && layout_pane_count(&layout) <= 1 {
                                        break 'main;
                                    }
                                    // A spawn-agent press resolves the harness
                                    // first: an override, a remembered pick, or
                                    // a lone install spawns directly with no
                                    // UI, while anything else opens the
                                    // overlay and spawns on confirm.
                                    if a == Action::SpawnAgent {
                                        let ordered =
                                            harness_state.clone().order(crate::agent::detect());
                                        match crate::agent::plan(
                                            &cfg.default_agent,
                                            &harness_state,
                                            ordered,
                                        ) {
                                            crate::agent::Plan::Configured(cmd)
                                            | crate::agent::Plan::Auto(cmd) => {
                                                let _ = layout.apply(a, v, f);
                                                if let Some(pid) = focused_pane(&layout) {
                                                    agent_panes.insert(pid);
                                                    agent_cmds.insert(pid, cmd);
                                                    if let Some(lp) = layout.panes.get_mut(&pid) {
                                                        lp.status = PaneStatus::Running;
                                                    }
                                                }
                                                needs_sync = true;
                                            }
                                            crate::agent::Plan::Missing { want, found } => {
                                                harness_pick = Some(HarnessPicker {
                                                    all: found,
                                                    query: String::new(),
                                                    sel: 0,
                                                    notice: Some(format!(
                                                        "`{want}` is not installed"
                                                    )),
                                                });
                                            }
                                            crate::agent::Plan::Choose(found) => {
                                                let notice =
                                                    if !harness_state.last.trim().is_empty() {
                                                        Some(format!(
                                                            "remembered `{}` is gone; pick another",
                                                            harness_state.last.trim()
                                                        ))
                                                    } else {
                                                        None
                                                    };
                                                harness_pick = Some(HarnessPicker {
                                                    all: found,
                                                    query: String::new(),
                                                    sel: 0,
                                                    notice,
                                                });
                                            }
                                            crate::agent::Plan::NoneInstalled { .. } => {
                                                harness_pick = Some(HarnessPicker {
                                                    all: Vec::new(),
                                                    query: String::new(),
                                                    sel: 0,
                                                    notice: Some(
                                                        "No agent harness found — type a command or take a shell"
                                                            .to_string(),
                                                    ),
                                                });
                                            }
                                        }
                                        dirty = true;
                                        continue;
                                    }
                                    let is_swap = matches!(
                                        a,
                                        Action::MovePaneLeft
                                            | Action::MovePaneRight
                                            | Action::MovePaneUp
                                            | Action::MovePaneDown
                                    );
                                    let before = is_swap.then(|| layout.clone());
                                    let _ = layout.apply(a, v, f);
                                    // Every real pane swap redeals the
                                    // background pals: the empty boxes reload
                                    // to a fresh cast instead of sitting
                                    // frozen on the launch shuffle. Focus,
                                    // widths, and no-op moves (a swap pressed
                                    // at the edge) leave the lineup alone.
                                    if is_swap
                                        && before
                                            .is_some_and(|b| layout.pane_order() != b.pane_order())
                                    {
                                        super::empty_art::reshuffle();
                                    }
                                    needs_sync = true;
                                    dirty = true;
                                    // A kill must paint before the next key is
                                    // read: holding ⌥+q repeats faster than
                                    // frames, so without this every queued
                                    // repeat lands in the same drain batch. The
                                    // layout would lose several panes while the
                                    // HUD still showed the first frame, and the
                                    // user — watching a stale dashboard — keeps
                                    // holding into working panes. Yield to the
                                    // render loop instead, so each kill gets its
                                    // own frame and the dashboard tracks it.
                                    if a == Action::KillPane {
                                        break 'drain;
                                    }
                                }
                                Cmd::Input(bytes) => {
                                    if let Some(pid) = focused_pane(&layout) {
                                        if let Some(p) = panes.get_mut(&pid) {
                                            // Typing means you want the prompt:
                                            // snap a scrolled-back pane to live.
                                            if p.grid.scroll_to_bottom() {
                                                dirty = true;
                                            }
                                            let _ = p.writer.write_all(&bytes);
                                            let _ = p.writer.flush();
                                        }
                                    }
                                }
                                Cmd::SmartJump => {
                                    // Jump to the next pane that needs attention
                                    // (failed > waiting > done), if any.
                                    if let Some(target) = smart_jump_target(&layout) {
                                        let v = Viewport::new(cols);
                                        let f = FollowScroll::default();
                                        let _ = layout.apply(Action::FocusPane(target), v, f);
                                        dirty = true;
                                    }
                                }
                                Cmd::None => {}
                            }
                        }
                    }
                    Ok(Event::Key(ke)) if ke.kind == KeyEventKind::Release => {
                        if is_alt_modifier(&ke) && bare_alt_held {
                            bare_alt_held = false;
                            // Bare release means the physical key is up — drop the
                            // fallback window too so a preceding Alt+hjkl chord
                            // doesn't linger after the hold is gone.
                            chord_alt_until = None;
                            dirty = true;
                        } else if ke.modifiers.contains(KeyModifiers::ALT) || is_alt_modifier(&ke) {
                            // Keep chord hold alive until its timeout expires.
                        }
                    }
                    Ok(Event::Mouse(me)) => {
                        // The ⌥-hold dashboard is a control surface, not a
                        // picture: while it is up, a click on a tile focuses that
                        // pane and a click anywhere else on the panel is
                        // swallowed, so the box never leaks a selection drag into
                        // the pane it is covering.
                        if let Some(plan) = &hud_plan {
                            let r = plan.rect;
                            let on_panel = me.column >= r.x
                                && me.row >= r.y
                                && me.column < r.x.saturating_add(r.w)
                                && me.row < r.y.saturating_add(r.h);
                            if on_panel {
                                if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) {
                                    if let Some(pid) = hud_pane_at(plan, me.column, me.row) {
                                        if focused_pane(&layout) != Some(pid) {
                                            let v = Viewport::new(cols);
                                            let f = FollowScroll::default();
                                            let _ = layout.apply(Action::FocusPane(pid), v, f);
                                            dirty = true;
                                        }
                                    }
                                }
                                continue;
                            }
                        }
                        let chrome = chrome_rows(&cfg);
                        let views = focused_pane_views_with_chrome(
                            &layout,
                            cols,
                            rows,
                            0,
                            true, chrome,
                        );
                        // A drag that wanders outside the pane (or off-screen)
                        // must still extend and finish the selection, exactly as
                        // it does in a browser or a native terminal. So resolve
                        // the target against the *owning* pane of a live drag
                        // first, clamping the point to that pane's rect, and only
                        // fall back to "whatever pane is under the cursor" when no
                        // drag is in flight.
                        let hit = selection
                            .filter(|s| s.dragging)
                            .and_then(|s| clamped_pane_point(&views, s.pane, me.column, me.row))
                            .or_else(|| pane_at(&views, me.column, me.row));
                        let mut handled = false;
                        // Peek slivers are hints, not interactive text: a click
                        // should focus the neighbour (like clicking a tab edge),
                        // never start a selection or forward mouse to the child.
                        let hit_is_peek = hit
                            .and_then(|(pid, _, _)| {
                                views.iter().find(|v| v.pid == pid).map(|v| v.peek)
                            })
                            .unwrap_or(false);
                        if hit_is_peek {
                            if matches!(me.kind, MouseEventKind::Down(MouseButton::Left)) {
                                if let Some((pid, _, _)) = hit {
                                    if focused_pane(&layout) != Some(pid) {
                                        let v = Viewport::new(cols);
                                        let f = FollowScroll::default();
                                        let _ = layout.apply(Action::FocusPane(pid), v, f);
                                        dirty = true;
                                    }
                                }
                            }
                            // Never start a drag, never forward to child.
                            handled = true;
                        } else if let Some((pid, gx, gy)) = hit {
                            let child_wants_mouse = panes
                                .get(&pid)
                                .map(|p| p.grid.wants_mouse())
                                .unwrap_or(false);
                            // Once gwae owns a drag, releasing Shift before the
                            // mouse must not strand it or leak its tail to a child.
                            let continuing_selection = selection
                                .is_some_and(|s| s.dragging && s.pane == pid)
                                && matches!(
                                    me.kind,
                                    MouseEventKind::Drag(MouseButton::Left)
                                        | MouseEventKind::Up(MouseButton::Left)
                                );
                            let role = if continuing_selection {
                                MouseRole::Select
                            } else {
                                mouse_role(me.kind, me.modifiers, child_wants_mouse)
                            };
                            match role {
                                MouseRole::Wheel => {
                                    // The wheel scrolls the pane under the
                                    // cursor, not just the focused one: with
                                    // several panes on screen the cursor is the
                                    // only sane target, the way a browser or a
                                    // native terminal behaves.
                                    if let Some(p) = panes.get_mut(&pid) {
                                        if p.grid.alternate_screen() {
                                            let key = wheel_alt_screen_keys(me.kind);
                                            for _ in 0..WHEEL_SCROLL_LINES {
                                                let _ = p.writer.write_all(key);
                                            }
                                            let _ = p.writer.flush();
                                        } else if p.grid.scroll_by(wheel_scroll_delta(me.kind)) {
                                            dirty = true;
                                        }
                                    }
                                    handled = true;
                                }
                                MouseRole::Select => {
                                    let point = select::Point::new(gx, gy);
                                    match me.kind {
                                        MouseEventKind::Down(MouseButton::Left) => {
                                            // Press arms a selection but shows nothing
                                            // yet: a plain click must clear the old
                                            // highlight, not paint a one-cell one.
                                            if selection.is_some() {
                                                dirty = true;
                                            }
                                            selection = Some(Selection {
                                                pane: pid,
                                                anchor: point,
                                                cursor: point,
                                                dragging: true,
                                            });
                                            // Clicking a pane still focuses it.
                                            if focused_pane(&layout) != Some(pid) {
                                                let v = Viewport::new(cols);
                                                let f = FollowScroll::default();
                                                let _ = layout.apply(Action::FocusPane(pid), v, f);
                                                dirty = true;
                                            }
                                        }
                                        MouseEventKind::Drag(MouseButton::Left) => {
                                            if let Some(s) = selection.as_mut() {
                                                if s.dragging && s.cursor != point {
                                                    s.cursor = point;
                                                    dirty = true;
                                                }
                                            }
                                        }
                                        MouseEventKind::Up(MouseButton::Left) => {
                                            let Some(s) = selection.as_mut().filter(|s| s.dragging)
                                            else {
                                                // No live drag: never recopy a stale selection.
                                                continue;
                                            };
                                            s.cursor = point;
                                            s.dragging = false;
                                            // A press+release without movement is a
                                            // plain click, not a selection: drop it so
                                            // no stray highlight lingers and nothing is copied.
                                            let done = selection.filter(|s| !s.is_empty());
                                            match done {
                                                Some(s) => {
                                                    let text = panes
                                                        .get(&s.pane)
                                                        .map(|p| select::selected_text(&p.grid, &s))
                                                        .unwrap_or_default();
                                                    let outcome = select::copy_to_clipboard(
                                                        &text,
                                                        &mut stdout,
                                                    );
                                                    reload_note_anchor = views
                                                        .iter()
                                                        .find(|v| v.pid == s.pane)
                                                        .map(|v| v.rect);
                                                    reload_note = Some(outcome.note(&text));
                                                    reload_note_until =
                                                        Some(Instant::now() + NOTE_LINGER);
                                                }
                                                None => selection = None,
                                            }
                                            dirty = true;
                                        }
                                        _ => {}
                                    }
                                    handled = true;
                                }
                                MouseRole::Forward | MouseRole::Local => {}
                            }
                        }
                        if let Some((pid, gx, gy)) = (!handled)
                            .then(|| pane_at(&views, me.column, me.row))
                            .flatten()
                        {
                            if matches!(me.kind, MouseEventKind::Down(MouseButton::Left))
                                && focused_pane(&layout) != Some(pid)
                            {
                                let v = Viewport::new(cols);
                                let f = FollowScroll::default();
                                let _ = layout.apply(Action::FocusPane(pid), v, f);
                                dirty = true;
                            }
                            if let Some(p) = panes.get_mut(&pid) {
                                // A child that asked for mouse reporting owns the
                                // event, translated into its own grid coordinates,
                                // so vim/less/jcode behave exactly as they would
                                // natively. Horizontal flicks never scroll here
                                // (gwae pans nothing); they forward only when
                                // the child reports mouse. A finished
                                // drag-selection never does.
                                if p.grid.wants_mouse() {
                                    if let Some(bytes) = sgr_mouse_report(&me, gx, gy) {
                                        let _ = p.writer.write_all(&bytes);
                                        let _ = p.writer.flush();
                                    }
                                }
                            }
                        }
                    }
                    Ok(Event::Resize(c, r)) => {
                        cols = c.max(1);
                        rows = r.max(2);
                        if let Some(measured) = CellPixels::measure() {
                            cell_pixels = measured;
                        }
                        // A wider terminal shrinks the strip relative to the
                        // viewport; drop any now-invalid scroll immediately so the
                        // strip snaps back to full bleed on the next paint.
                        layout.clamp_scrolls(Viewport::new(cols));
                        dirty = true;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("event read: {e}");
                    }
                }
                // Drain loop polls zero-timeout at top; break when empty.
                // Continue in handlers now targets 'drain, batching writes.
            }
        }

        // Size all panes, including hidden columns/strips, from logical
        // geometry. Deferring hidden panes until focus reveals them would
        // provoke a SIGWINCH redraw and falsely turn an idle `!` into `»`.
        for (pid, size) in pane_grid_sizes(&layout, GridSize { cols, rows }, &cfg) {
            if let Some(p) = panes.get_mut(&pid) {
                if p.grid.size() != size {
                    p.grid.resize(size);
                    dirty = true;
                }
                p.grid.set_cell_size(cell_pixels.width, cell_pixels.height);
                // A pane mid-repaint-nudge is deliberately one row taller;
                // "correcting" it here before the child is scheduled would
                // collapse the nudge into an unobservable no-op and leave an
                // adopted pane blank. The restore reasserts the true size.
                if repaint_restore.covers(pid) {
                    continue;
                }
                let pty_size = cell_pixels.pty_size(size.cols, size.rows);
                if p.pty_size != pty_size {
                    match p.master.resize(pty_size) {
                        Ok(()) => p.pty_size = pty_size,
                        Err(e) => tracing::warn!(pid, "resize pane: {e}"),
                    }
                    dirty = true;
                }
            }
        }

        // Treat gwae as an invisible layer for the host title bar: mirror the
        // focused pane's inner title (set via OSC 0/2 by e.g. jcode) out to the
        // host terminal, so switching panes updates the outer window/status bar
        // to the pane you're actually looking at instead of "gwae". Fall back
        // to a plain gwae label when the focused pane has set no title.
        let effective = focused_pane(&layout)
            .and_then(|pid| panes.get(&pid))
            .map(|p| p.grid.title())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| "gwae".to_string());
        if effective != last_title {
            last_title = effective.clone();
            if let Err(e) = emit_title(&mut stdout, &effective) {
                tracing::warn!("set title: {e}");
            }
        }

        // Track attention & alt flip for repaint, plus HUD (persist-until-key).
        let now_for_hud = Instant::now();
        if let Some(t) = chord_alt_until {
            if now_for_hud >= t {
                chord_alt_until = None;
            }
        }
        let chord_alt_held = chord_alt_until.is_some();
        // Bare Option polling: on macOS most terminals never emit a bare Alt
        // KeyEvent, so `bare_alt_held` alone cannot reveal the HUD. Poll the
        // system modifier flags directly; this works without REPORT_ALL and
        // without breaking Hangul IME. Keep the chord timer path as well — it
        // is still the portable/Linux fallback and the way to keep the HUD up
        // briefly after a chord on terminals that never send release events.
        let effective_alt_held =
            bare_alt_held || chord_alt_held || (native_modifiers && macos_option_held());
        let cur_has_attention = has_attention(&layout);
        if cur_has_attention != last_has_attention || effective_alt_held != last_alt_held {
            dirty = true;
            last_has_attention = cur_has_attention;
            last_alt_held = effective_alt_held;
        }
        let show_hud = hud_active;
        let show_center_minimap = effective_alt_held && !hud_active && cfg.minimap.show;
        // Everything the overlay knows beyond the layout: what each pane is
        // and how long it has been silent.
        // Built only when the panel is actually up, so a normal frame pays
        // nothing for it.
        let hud_facts = if show_center_minimap && !show_hud {
            HudFacts {
                keep_awake: keep_awake.active(),
                dev: dev_mode,
                build: {
                    #[cfg(unix)]
                    {
                        if let Some(p) = build_phase {
                            Some(match p {
                                crate::reload::BuildPhase::Building => {
                                    crate::tui::BuildPill::Building
                                }
                                crate::reload::BuildPhase::Linking => {
                                    crate::tui::BuildPill::Linking
                                }
                                crate::reload::BuildPhase::Signing => {
                                    crate::tui::BuildPill::Signing
                                }
                            })
                        } else if build_error.as_deref().is_some_and(|s| !s.is_empty()) {
                            Some(crate::tui::BuildPill::Failed)
                        } else {
                            None
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                held: {
                    #[cfg(unix)]
                    {
                        watch_held.clone()
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                ..HudFacts::default()
            }
        } else {
            HudFacts::default()
        };
        // Geometry is planned once: the panel and click-to-focus all read
        // the same plan, so a click can never land on a tile the paint put
        // somewhere else.
        hud_plan = (show_center_minimap && !show_hud)
            .then(|| plan_center_minimap(cols, rows, &layout, &cfg.minimap))
            .flatten();
        if host_kitty_graphics && host_images.refresh_due() {
            dirty = true;
        }
        if dirty {
            host_images.begin();
            render_frame_with_images(
                &mut frame,
                &layout,
                &mut panes,
                cols,
                rows,
                0,
                &pal,
                &cfg.minimap,
                selection.as_ref(),
                host_kitty_graphics.then_some(&mut host_images),
            );
            host_images.finish();
            if show_hud {
                draw_center_hud(&mut frame, cols, rows, &pal, keep_awake.active(), dev_mode);
            }
            if show_center_minimap && !show_hud {
                if let Some(plan) = &hud_plan {
                    paint_center_minimap(&mut frame, cols, rows, &layout, plan, &pal, &hud_facts);
                }
            }
            if let Some(pick) = &dir_pick {
                draw_dir_picker(&mut frame, cols, rows, pick, &pal);
            }
            if let Some(pick) = &harness_pick {
                draw_harness_picker(&mut frame, cols, rows, pick, &pal);
            }
            if let Some(note) = &reload_note {
                let ok = !note.contains("error") && !note.starts_with("paste failed:");
                draw_toast_at(&mut frame, cols, rows, note, &pal, ok, reload_note_anchor);
            }
            // Topmost: the destructive confirmation must never be obscured by
            // chrome that happens to be showing when the chord is pressed.
            if quit_confirm {
                draw_quit_confirm(&mut frame, cols, rows, layout_pane_count(&layout), &pal);
            }
            buf.clear();
            paint(&mut buf, &frame, &last, cols, rows);
            if !buf.is_empty() || !host_images.pending.is_empty() {
                // Synchronized update (ESC[?2026h/l): the host terminal holds
                // the screen and applies the whole frame atomically, so a
                // repaint can never be displayed half-drawn (visible shearing
                // when a vsync lands mid-write). Terminals that don't support
                // it ignore the markers.
                let _ = stdout.write_all(b"\x1b[?2026h");
                let _ = stdout.write_all(&host_images.pending);
                let _ = stdout.write_all(&buf);
                let _ = stdout.write_all(b"\x1b[?2026l");
                let _ = stdout.flush();
                last = frame.clone();
            }
            dirty = false;
        }

        // Reconcile PTYs with the layout *after* the frame is on screen:
        // the openpty+fork cost lands behind the paint, not in front of it.
        // The new child's first output re-wakes the loop and paints again.
        if needs_sync {
            needs_sync = false;
            if let Err(e) = sync_panes(
                &mut layout,
                &mut panes,
                &tx,
                (GridSize { cols, rows }, cell_pixels),
                &agent_panes,
                &agent_cmds,
                spawn_dir.as_deref(),
                &cfg,
            ) {
                tracing::error!("sync panes: {e}");
            }
        }
    }

    // Teardown: kill all panes, leave raw mode & alternate screen.
    for p in panes.values_mut() {
        kill_pane_tree(&mut p.child);
    }
    // Anything registered but no longer in `panes` (a pane dropped from the
    // map without going through `kill_pane_tree`) is caught here, so the
    // process leaves nothing behind.
    crate::reap::reap_all();
    let _ = stdout.write_all(&host_images.clear());
    restore_terminal(&mut stdout, kitty_keyboard);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::super::render::focused_pane_views;
    use super::*;
    #[cfg(unix)]
    use crate::theme::Palette;

    /// End-to-end acceptance for the quarter-pane overflow fix: four real PTY
    /// children, one per quarter column, rendered by `render_frame` at 342 cols
    /// (the reported failure width, not divisible by 4). Every screen cell up to
    /// and including the rightmost column must show the pane that owns it, with
    /// pane content never spilling past a column boundary or the screen edge.
    // Spawns a real `sh` in a PTY: unix-only (Windows has no sh, and
    // the spawn hangs the suite rather than failing fast).
    #[cfg(unix)]
    #[test]
    fn four_quarter_panes_render_to_screen_edge_e2e() {
        use gwae_layout::{Preset, Width};
        let cols: u16 = 342;
        let rows: u16 = 8;
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        let fills = ['A', 'B', 'C', 'D'];
        let mut pids = Vec::new();
        for _ in fills {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
            pids.push(p);
        }
        let ranges = layout
            .column_x_ranges(row, cols)
            .expect("ranges for the four-quarter row");
        assert_eq!(ranges.last().unwrap().1, cols as u32);

        // Spawn one real child per pane, each filling a line with its letter.
        let (tx, rx) = channel::<PaneMsg>();
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        for (i, pid) in pids.iter().enumerate() {
            let w = (ranges[i].1 - ranges[i].0) as u16;
            let cmd = format!(
                "sh -c \"for i in $(seq 1 {w}); do printf '%s' {}; done; echo\"",
                fills[i]
            );
            let pane = spawn_pane(*pid, &cmd, w, rows, tx.clone(), None, CellPixels::default())
                .expect("spawn pane");
            panes.insert(*pid, pane);
        }
        // Feed PTY output until every pane's first row is fully painted.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let done = pids.iter().enumerate().all(|(i, pid)| {
                let w = (ranges[i].1 - ranges[i].0) as u16;
                panes
                    .get(pid)
                    .map(|p| p.grid.cell(w.saturating_sub(1), 0).ch == fills[i])
                    .unwrap_or(false)
            });
            if done {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(PaneMsg::Output(pid, bytes)) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.grid.feed(&bytes);
                    }
                }
                Ok(PaneMsg::Exited(_)) | Ok(PaneMsg::Input(_)) => {}
                Err(_) => {}
            }
        }

        let mut out = Vec::new();
        render_frame(
            &mut out,
            &layout,
            &mut panes,
            cols,
            rows,
            0,
            &Palette::default(),
            &crate::config::Minimap::default(),
            None,
        );
        // The first content row shows each pane's letter across its column's
        // interior: the frame owns the boundary cells, content never bleeds past
        // them, and the rightmost column reaches the screen edge.
        let row1 = cols as usize; // screen row 1: the first content row
        for (i, (s, e)) in ranges.iter().enumerate() {
            for x in (*s + 1)..(*e - 1) {
                assert_eq!(
                    out[row1 + x as usize].ch,
                    fills[i],
                    "screen x={x} must show pane {} content",
                    fills[i]
                );
            }
        }
        assert_eq!(
            out[cols as usize - 1].ch,
            '╮',
            "the rightmost column's frame reaches the screen edge"
        );
        assert_eq!(
            out[row1 + cols as usize - 2].ch,
            'D',
            "pane D's content runs up to its frame"
        );
        for p in panes.values_mut() {
            kill_pane_tree(&mut p.child);
        }
    }

    /// Live-PTy proof that pane 4's content overflows at its logical width
    /// rather than wrapping early: a widened last column keeps a 39-cell grid
    /// (80 cols, half width, 1-cell left frame inset), so a 39-char line fills
    /// exactly one emulator row instead of wrapping at the 38-cell visible
    /// rect the old clamped sizing produced.
    // Spawns a real `sh` in a PTY: unix-only (Windows has no sh, and
    // the spawn hangs the suite rather than failing fast).
    #[cfg(unix)]
    #[test]
    fn widened_last_pane_wraps_at_logical_width_e2e() {
        use gwae_layout::{Action, FollowScroll, Preset, Viewport, Width};
        let cols: u16 = 80;
        let rows: u16 = 10;
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        let mut pids = Vec::new();
        for _ in 0..4 {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
            pids.push(p);
        }
        // Widen pane 4 to half (quarter -> third -> half).
        layout.focus.column = 3;
        let vp = Viewport::new(cols);
        for _ in 0..2 {
            let _ = layout.apply(Action::CycleWidth, vp, FollowScroll::default());
        }
        let views = focused_pane_views(&layout, cols, rows, 0, true);
        let v = views.iter().find(|v| v.col == 3).unwrap();
        assert_eq!(v.grid_cols, 39, "pane 4 keeps its logical grid width");

        // Print exactly 39 chars with no trailing newline, then check the
        // emulator wrapped (or not) at the live grid width.
        let (tx, rx) = channel::<PaneMsg>();
        let cmd = format!("sh -c \"printf '%s' {}\"", "D".repeat(v.grid_cols as usize));
        let pane = spawn_pane(
            pids[3],
            &cmd,
            v.grid_cols,
            rows,
            tx.clone(),
            None,
            CellPixels::default(),
        )
        .expect("spawn pane");
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        panes.insert(pids[3], pane);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let done = panes
                .get(&pids[3])
                .map(|p| p.grid.cell(v.grid_cols.saturating_sub(1), 0).ch == 'D')
                .unwrap_or(false);
            if done {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(PaneMsg::Output(pid, bytes)) => {
                    if let Some(p) = panes.get_mut(&pid) {
                        p.grid.feed(&bytes);
                    }
                }
                Ok(PaneMsg::Exited(_)) | Ok(PaneMsg::Input(_)) => {}
                Err(_) => {}
            }
        }
        let p = panes.get(&pids[3]).expect("pane 4 live");
        assert_eq!(
            p.grid.cell(0, 0).ch,
            'D',
            "line starts at the first grid cell"
        );
        assert_eq!(
            p.grid.cell(v.grid_cols.saturating_sub(1), 0).ch,
            'D',
            "39-char line fills the logical row without wrapping early"
        );
        assert_eq!(
            p.grid.cell(0, 1).ch,
            ' ',
            "nothing spills to row 1: no early wrap at the visible width"
        );
        for p in panes.values_mut() {
            kill_pane_tree(&mut p.child);
        }
    }

    /// Acceptance for scroll-state paint stability (the user-visible bug: "grids
    /// are painted slightly differently across different scroll states despite
    /// identical panes"). Eight identical quarter columns of real PTY panes at
    /// 342 cols (not divisible by 4, the width where absolute-rounded boundaries
    /// wobble by one cell between stops). Walking focus across the whole strip in
    /// the skeleton renderer, the x-positions of the vertical frame edges painted
    /// on a mid-strip row must be identical in every frame.
    // Spawns a real `sh` in a PTY: unix-only (Windows has no sh, and
    // the spawn hangs the suite rather than failing fast).
    #[cfg(unix)]
    #[test]
    fn identical_grids_paint_identically_across_scroll_states_e2e() {
        use gwae_layout::{Action, FollowScroll, Preset, Viewport, Width};
        let cols: u16 = 342;
        let rows: u16 = 8;
        let n = 8usize;
        let mut layout = Layout::new(1);
        if let Some(r) = layout.row_mut(layout.focus.row) {
            r.columns.clear();
        }
        let row = layout.focus.row;
        let mut pids = Vec::new();
        for _ in 0..n {
            let p = layout.alloc_pane();
            layout.add_column(row, Width::Preset(Preset::Quarter), vec![p]);
            pids.push(p);
        }
        // Real PTY children (sleeping shells: content is irrelevant, the frame
        // geometry is what must not wobble).
        let (tx, _rx) = channel::<PaneMsg>();
        let mut panes: HashMap<PaneId, PtyPane> = HashMap::new();
        for pid in &pids {
            let pane = spawn_pane(
                *pid,
                "sleep 30",
                80,
                rows,
                tx.clone(),
                None,
                CellPixels::default(),
            )
            .expect("spawn pane");
            panes.insert(*pid, pane);
        }

        let vp = Viewport::new(cols);
        let f = FollowScroll::default();
        let mut out = Vec::new();
        // Frames are thin box-drawing glyphs; the vertical edges (`│`) crossing a
        // mid-strip row mark every column boundary on screen.
        let mut boundary_sets: Vec<(i32, Vec<u16>)> = Vec::new();
        let mut paint = |layout: &Layout, panes: &mut HashMap<PaneId, PtyPane>| -> Vec<u16> {
            render_frame(
                &mut out,
                layout,
                panes,
                cols,
                rows,
                0,
                &Palette::default(),
                &crate::config::Minimap::default(),
                None,
            );
            let y = 2usize;
            (0..cols)
                .filter(|x| out[y * cols as usize + *x as usize].ch == '│')
                .collect()
        };
        // Walk focus right across the whole strip, painting at every stop, then
        // back left (reverse stops can differ from forward ones).
        let mut boundary_scroll = 0;
        boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
        for _ in 0..n - 1 {
            boundary_scroll = layout.apply(Action::FocusRight, vp, f).unwrap();
            boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
        }
        for _ in 0..n - 1 {
            boundary_scroll = layout.apply(Action::FocusLeft, vp, f).unwrap();
            boundary_sets.push((boundary_scroll, paint(&layout, &mut panes)));
        }
        // Every painted frame shows the same vertical-edge skeleton: the grid
        // never shifts by a cell between scroll states.
        let first = &boundary_sets[0].1;
        assert!(
            !first.is_empty(),
            "skeleton frame painted no vertical edges"
        );
        for (scroll, set) in &boundary_sets {
            assert_eq!(
                set, first,
                "grid boundaries moved at scroll={scroll}: {set:?} != {first:?}"
            );
        }
        // Distinct scroll stops were actually exercised (not one static frame).
        let stops: std::collections::HashSet<i32> = boundary_sets.iter().map(|(s, _)| *s).collect();
        assert!(
            stops.len() >= 4,
            "expected several scroll stops, got {stops:?}"
        );
        for p in panes.values_mut() {
            kill_pane_tree(&mut p.child);
        }
    }

    fn notice_found(cmds: &[&str]) -> Vec<crate::agent::Found> {
        cmds.iter()
            .map(|c| crate::agent::Found {
                cmd: c.to_string(),
                label: c.to_string(),
                path: std::path::PathBuf::from("/bin").join(c),
            })
            .collect()
    }

    #[test]
    fn the_force_pick_notice_bypasses_every_fast_path_silently_when_healthy() {
        // The force-pick promise: a healthy remembered pick (or a lone
        // install) still opens the picker rather than spawning. `plan` would
        // resolve both of these to Configured/Auto, which is exactly what the
        // force-pick chord must ignore.
        let ordered = notice_found(&["claude"]);
        assert_eq!(force_pick_notice("", "claude", &ordered), None);
        // No memory, no override, several harnesses: the plain choose case
        // carries no notice either.
        let ordered = notice_found(&["claude", "aider"]);
        assert_eq!(force_pick_notice("", "", &ordered), None);
    }

    #[test]
    fn the_force_pick_notice_explains_overrides_stale_memory_and_empty_scans() {
        // A missing override is named, mirroring the Missing plan arm.
        let ordered = notice_found(&["claude"]);
        assert_eq!(
            force_pick_notice("jcode-not-real", "", &ordered),
            Some("`jcode-not-real` is not installed".to_string())
        );
        // A stale remembered pick is named, mirroring the Choose notice.
        assert_eq!(
            force_pick_notice("", "gone-xyz", &ordered),
            Some("remembered `gone-xyz` is gone; pick another".to_string())
        );
        // A live override is called out: it still wins for ⌥+;, so the pick
        // here only steers this pane. It outranks memory, so with a stale
        // pick alongside, the override is what the notice names.
        // (`sh` stands in for an installed override; Windows has no sh, so
        // these two arms are unix-only.)
        #[cfg(unix)]
        {
            assert_eq!(
                force_pick_notice("sh", "claude", &ordered),
                Some("default_agent `sh` still wins for ⌥+;".to_string())
            );
            assert_eq!(
                force_pick_notice("sh", "gone-xyz", &ordered),
                Some("default_agent `sh` still wins for ⌥+;".to_string())
            );
        }
        // Nothing installed at all: same line as the NoneInstalled overlay.
        assert_eq!(
            force_pick_notice("", "", &[]),
            Some("No agent harness found — type a command or take a shell".to_string())
        );
    }
}
