# TUI Split Plan — `tui.rs` (10,727 lines)

Status: proposal. Complements `docs/NAV-REWRITE-PLAN.md` (which rewrites decode logic; this plan only moves code).

## 0. Correction to the premise

* There is no composer in `tui.rs` (grep: zero hits). `ARCHITECTURE.md:22` saying the bin owns "composer" is stale. The only text inputs are the `DirPicker` filter line and paste notes. Do not create a `composer` module.
* `cfg(target_os = "macos")` appears exactly twice (`macos_option_held`, L505/520). The real macOS scatter is implicit: US Option-glyph arms in `handle_key` (L3936, 211 lines), `is_ghostty` / `host_supports_kitty_graphics` (L455/472), Kitty-keyboard Ghostty carve-out (L4354-4376), `keepawake` guard, `CellPixels` metrics. The plan centralizes these without claiming dozens of cfg branches.
* True size drivers: `run_tui` L4318 = 1,602 lines (one fn, ~30 locals); `mod tests` L6104 = 4,191 lines; `render_frame_with_images` L1254 = 418 lines; `paint_center_minimap` L2528 = 234 lines; `handle_key` L3936 = 211 lines; `draw_dir_picker` L2826 = 180 lines.

## 1. Target layout (all private except today's pub surface)

Convert `src/tui.rs` → `src/tui/mod.rs` + submodules. Keep these paths stable via re-export so `agent.rs`, `config.rs`, `reap.rs`, `main.rs` do not change:

* `crate::tui::run_tui`, `crate::tui::shell_split`, `crate::tui::descendants` (pub(crate)), `crate::tui::{PtyPane, PaneIo, PaneProc}`

New files (leaf-first order, §2):

| # | File | Moves (line) | Size |
|---|------|--------------|------|
| 1 | `tui/shell.rs` | `shell_split` (797, 36L), `agent_gateway_cmd` (557) | ~50L |
| 2 | `tui/title.rs` | `sanitize_title` (528), `emit_title` (546) + tests L6753-6772 | ~60L |
| 3 | `tui/osc.rs` | `scan_osc133` (411, 44L) + status tests ~L7721 | ~60L |
| 4 | `tui/mouse.rs` | `MouseRole` (43), `mouse_role` (55), `is_wheel` (95), `wheel_*` (118/129), `sgr_mouse_report` (6065), `pane_at` (6025), `clamped_pane_point` (6047) + tests L6968-7249 | ~250L |
| 5 | `tui/diff.rs` | `paint` (3625, 70L), `crossterm_color` (3591), `host_width_agrees` (3695) + paint tests L6799-6967 | ~350L |
| 6 | `tui/pty.rs` | `PaneIo` (217), `PaneProc` (268), `PtyPane` (311), `feed_pane_output` (333), `spawn_pane` (712), `adopt_pane` (833), `kill_pane_tree` (676), `descendants` (624), `nudge_repaint` (917), `sync_panes` (4177), `pane_grid_sizes` (4147) + pty test L6109 | ~500L |
| 7 | `tui/platform.rs` | `macos_option_held` ×2 (505/520), `native_modifier_poll_enabled` (483), `is_ghostty` (472), `host_supports_kitty_graphics` (455) | ~80L. Only file allowed `cfg(target_os = "macos")` + `#[link(CoreGraphics)]`. Everything else calls `platform::option_held()` / `platform::is_ghostty()`. |
| 8 | `tui/term.rs` | raw/alt-screen enter (4337-4353), kitty-keyboard setup (4354-4400), `restore_terminal` (970), `re_enter_terminal` (986), `refresh_size` (4233), `input_poll_interval` + consts (4257-4307) + poll tests L6493-6545 | ~250L |
| 9 | `tui/input.rs` | `Cmd` (3710), `JumpAccum` (3768), `key_bytes` (3831), `handle_key` (3936), `is_alt_modifier` (153), `physical_shift` (160), `logical_char` (176), `is_harness_scroll_chord` (191), `smart_jump_target` (5959), `picker_paste_query`/`paste_note` (5920/5937) + key tests L6156-6752 | ~600L |
| 10 | `tui/render.rs` | `PaneView` (997), `focused_pane_views*` (1013/1057), `pane_window` (1193), `render_frame` (1224), `render_frame_with_images` (1254), `FrameCanvas` (1718-1867), `draw_focus_frame` (1867), `column_grid_sizes` (1028) | ~900L |
| 11 | `tui/chrome.rs` | HUD facts/plan (2096-2512), `paint_center_minimap` (2528), `draw_center_hud` (3223), `draw_edge_ticks` (3367), `draw_minimap` (3459), `draw_big_label`/`draw_art`/`draw_placeholder_contents`, `status_glyph_for`, `hud_hint`, toast (3173-3223) | ~1,400L |
| 12 | `tui/pickers.rs` | `DirPicker` (2762-3005), `draw_theme_picker` (3006), `draw_quit_confirm` (3085) | ~400L |
| 13 | `tui/config_io.rs` | `write_agent_dir` (571), `write_keep_awake` (585), `write_harness_dir` (599), `perform_reload` (927) | ~150L |
| 14 | `tui/app.rs` | `run_tui` body split: `App` struct holding the ~30 locals (cols/rows, panes, layout, hud/picker/quit/selection state, mtime watches, notes). Event fns: `on_pane_msg`, `on_resize`, `on_config_tick`, `on_reload_tick`, `on_paste`, `on_key`, `on_mouse`, `render`. `mod.rs::run_tui` becomes setup → `App::run()` → teardown. No logic change. | 1,602L → struct + ~10 fns |

## 2. Sequencing (each step = one PR, green `cargo test` + `cargo clippy`)

1. **Mech move**: `git mv src/tui.rs src/tui/mod.rs`, add `mod shell; pub use shell::shell_split;` etc. No code moves yet. Proves re-export shim.
2. **Steps 1–5** (leaves, zero loop coupling): shell → title → osc → mouse → diff. Move function + its tests together. Each is a pure cut-paste; `mod.rs` re-exports for the loop.
3. **Steps 6–8** (pty → platform → term): pty first (loop only touches it via narrow calls), then platform (replace direct `macos_option_held()` call sites with `platform::`), then term.
4. **Steps 9–10** (input → render): input move is mechanical; **do not change `handle_key` logic here** — NAV rewrite lands after, in `tui/input.rs`, not the monolith. Render next (needs `PaneView` + `FrameCanvas` only).
5. **Steps 11–13** (chrome → pickers → config_io): straight extractions from render helpers.
6. **Step 14 last** (`app.rs`): only after all leaves are out does `run_tui` shrink enough to split into `App`. This is the sole step that touches loop control flow, and it must be behavior-identical (diff `GWAE_LOG=debug` traces before/after on a scripted session).

## 3. Coordination with NAV-REWRITE-PLAN

* NAV §3.1–3.3 (`keys::Chord`, `GLYPH_MAP`, `Keymap`) targets exactly `tui/input.rs` post-move. Until this split lands, NAV edits hit L3936 in the monolith and conflict with every macOS fix.
* Rule until split done: NAV work adds types to `keys.rs` + `binds.rs` only; no `handle_key` logic edits. After step 9, `handle_key → lookup` rewrite is a single-file change with the moved key tests as the gate.
* `binds.rs` coupling is doc-only today (`handle_key` mentioned in comments; `Effect` mirrors private `Cmd`). When `Cmd` moves to `tui/input.rs`, make it `pub(crate)` and add `Effect::from_cmd` mapping + keep the existing cross-check test — do not duplicate the enum.

## 4. Test strategy

* No new harness. Move each `mod tests` block with its code (paste + key + paint + mouse tests are already per-domain). Leave only loop-level e2e (`content_scroll_*`, `four_quarter_*`, `widened_last_*`, `identical_grids_*` at L10295-10727) in `mod.rs` until `app.rs` exists, then move to `app.rs` tests.
* Gate per step: `cargo test --package gwae`, plus `cargo test --package gwae-layout` (untouched). No behavior-change PR may also refactor.
* Regression net for step 14: existing `crates/gwae/tests/*_e2e.rs` (picker, resize, scrollback, teardown) already drive the real binary over a PTY — they cover the loop without unit churn.

## 5. Risks / non-goals

* Risk: `render_frame_with_images` (418L) + graphics host coupling tempts a graphics refactor. Non-goal: leave `graphics_*.rs` alone; pass `&mut Host` through.
* Risk: `PtyPane` fields are `pub` and touched by the loop. Keep them `pub` during the split; tighten visibility only after `app.rs`.
* Non-goal: no async rewrite, no `binds.rs` merge, no HUD redesign, no keymap behavior change.

## 6. Done when

* `src/tui/mod.rs` < 150 lines (modules + re-exports + `run_tui` thin wrapper).
* `grep -rn 'cfg(target_os' src/tui/` hits only `platform.rs`.
* Next macOS input fix touches `tui/input.rs` + `tui/platform.rs` only, with unit tests adjacent, zero render/pty diff.
