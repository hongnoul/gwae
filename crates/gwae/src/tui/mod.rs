//! TUI: the M0 render/event loop (single process, one focused row).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::mpsc::channel;
use std::time::Instant;

use crossterm::cursor;
use crossterm::event::{
    self, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton, MouseEventKind, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as term_size, EnterAlternateScreen,
};
use gwae_layout::{Action, FollowScroll, Layout, PaneId, PaneStatus, Viewport};
use gwae_term::{Cell, Size as GridSize, TermGrid};

use crate::config::Config;
use crate::geometry::CellPixels;
use crate::select::{self, Selection};

mod app;

pub use app::run_tui;

mod chrome;
mod empty_art;
mod pickers;
mod render;

pub(crate) use chrome::{
    draw_center_hud, draw_toast_at, has_attention, hud_pane_at, paint_center_minimap,
    plan_center_minimap, BuildPill, HudFacts, HudPlan,
};
pub(crate) use pickers::{
    draw_dir_picker, draw_harness_picker, draw_quit_confirm, DirPicker, HarnessChoice,
    HarnessPicker,
};
#[cfg(test)]
pub(crate) use render::render_frame;
pub(crate) use render::{focused_pane_views_with_chrome, render_frame_with_images, PaneView};

mod config_io;
mod input;

pub(crate) use config_io::{perform_reload, write_harness_dir};
pub(crate) use input::{
    focused_pane, handle_key, is_alt_modifier, is_harness_scroll_chord, key_bytes,
    layout_pane_count, paste_note, picker_paste_query, picker_step, smart_jump_target, Cmd,
};

mod platform;
mod pty;
mod term;

pub(crate) use platform::{
    host_supports_kitty_graphics, is_ghostty, macos_option_held, native_modifier_poll_enabled,
};
pub use pty::PtyPane;
pub(crate) use pty::{
    adopt_pane, descendants, feed_pane_output, kill_pane_tree, nudge_repaint, pane_grid_sizes,
    spawn_pane, sync_panes, PaneMsg,
};
pub(crate) use term::{
    first_line, input_poll_interval, re_enter_terminal, refresh_size, restore_terminal,
    BINARY_SETTLE, CONFIG_POLL, NOTE_LINGER, SIZE_POLL,
};

mod diff;
mod mouse;

pub(crate) use diff::paint;
use mouse::{
    clamped_pane_point, is_horizontal_wheel, mouse_role, pane_at, sgr_mouse_report,
    wheel_alt_screen_keys, wheel_pan_delta, wheel_scroll_delta, MouseRole, WHEEL_SCROLL_LINES,
};

mod osc;
mod shell;
mod title;

use osc::{osc_status, scan_osc133};
use shell::agent_gateway_cmd;
pub use shell::shell_split;
use title::emit_title;

/// Rectangle in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) w: u16,
    pub(crate) h: u16,
}

#[cfg(test)]
mod tests {

    #[test]
    fn keep_awake_badge_shows_only_while_the_guard_is_active() {
        // The state signal is a text badge on the Option HUD, never a
        // palette change: toggling off must mean plain chrome everywhere.
        let off = crate::keepawake::Guard::acquire(false);
        assert!(!off.active(), "a disabled guard holds no assertion");
        assert!(
            !crate::keepawake::KEEP_AWAKE_BADGE.is_empty(),
            "the active badge must exist to stamp onto the HUD"
        );
    }
}
