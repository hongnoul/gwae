//! TUI: the M0 render/event loop (single process, one focused row).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::mpsc::channel;
use std::time::Instant;

use crate::theme::Palette;
use crossterm::cursor;
use crossterm::event::{
    self, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    MouseButton, MouseEventKind,
    PushKeyboardEnhancementFlags,
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
mod pickers;
mod render;

pub(crate) use chrome::{
    draw_center_hud, draw_toast, draw_toast_at, has_attention, hud_pane_at,
    paint_center_minimap, plan_center_minimap, HudFacts, HudPlan,
};
pub(crate) use pickers::{draw_dir_picker, draw_quit_confirm, draw_theme_picker, DirPicker};
pub(crate) use render::{
    focused_pane_views, focused_pane_views_with_chrome, render_frame_with_images, PaneView,
};
#[cfg(test)]
pub(crate) use render::render_frame;

mod config_io;
mod input;

pub(crate) use config_io::{perform_reload, write_harness_dir, write_keep_awake};
pub(crate) use input::{
    focused_pane, handle_key, is_alt_modifier, is_harness_scroll_chord, key_bytes,
    layout_pane_count, picker_paste_query, paste_note,
    smart_jump_target, Cmd, JumpAccum,
};

mod platform;
mod pty;
mod term;

pub use pty::PtyPane;
pub(crate) use pty::{adopt_pane, descendants, feed_pane_output, kill_pane_tree, nudge_repaint, spawn_pane, sync_panes, PaneMsg, pane_grid_sizes};
pub(crate) use platform::{host_supports_kitty_graphics, is_ghostty, macos_option_held, native_modifier_poll_enabled};
pub(crate) use term::{first_line, input_poll_interval, re_enter_terminal, refresh_size, restore_terminal, BINARY_SETTLE, CONFIG_POLL, NOTE_LINGER, SIZE_POLL};

mod diff;
mod mouse;

pub(crate) use diff::paint;
use mouse::{
    MouseRole, clamped_pane_point, mouse_role, pane_at, sgr_mouse_report,
    wheel_alt_screen_keys, wheel_scroll_delta, WHEEL_SCROLL_LINES,
};

mod osc;
mod shell;
mod title;

pub use shell::shell_split;
use osc::scan_osc133;
use shell::agent_gateway_cmd;
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
    fn keep_awake_red_ring_layers_over_the_theme_and_releases() {
        // The ring is derived per frame, never stored: toggling off must
        // restore the theme exactly, and a theme edit mid-session must not
        // bake the red in.
        let nord = crate::theme::Palette::NORD;
        let off = crate::keepawake::Guard::acquire(false);
        assert_eq!(
            crate::keepawake::effective_palette(&nord, &off),
            nord,
            "inactive guard must leave the theme untouched"
        );
        // An active guard swaps only the accent; everything else survives.
        let fake_active = crate::keepawake::effective_palette_for_test(&nord, true);
        assert_eq!(
            fake_active.accent,
            crate::keepawake::ACTIVE_ACCENT,
            "active guard must paint the ring red"
        );
        let mut rest = fake_active;
        rest.accent = nord.accent;
        assert_eq!(rest, nord, "only the accent may change");
    }

}
