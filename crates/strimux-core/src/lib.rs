//! The hot-swappable core. This cdylib is loaded at runtime by the HMR host
//! and swapped in-place on rebuild. Keep the session state in the HOST; this
//! crate only holds cheap scratch state that is fine to reset on reload.

use std::ffi::c_void;
use strimux_core_api::{Cell, Cmd, Frame, Key, KeyCode, SessionState, StrimuxCore};

/// Bump this label each time you change rendering/nav behavior so the status
/// line visibly reflects that a reload happened.
pub const LABEL: &str = "core v0";

struct Core {
    tick: u64,
}

impl StrimuxCore for Core {
    fn label(&self) -> &'static str {
        LABEL
    }

    fn handle_key(&mut self, state: &mut SessionState, key: Key) -> Cmd {
        use KeyCode::*;
        match key.code {
            Char('l') if !key.shift => {
                state.focus = (state.focus + 1).min(state.panes.len().saturating_sub(1));
                Cmd::Repaint
            }
            Char('h') if !key.shift => {
                state.focus = state.focus.saturating_sub(1);
                Cmd::Repaint
            }
            Char('q') => Cmd::Quit,
            // Dev convenience: trigger a reload straight from a key.
            Char('r') if key.alt => Cmd::Reload,
            _ => Cmd::None,
        }
    }

    fn render(&mut self, state: &mut SessionState, frame: &mut Frame) {
        self.tick += 1;
        state.frames += 1;
        let plain = Cell {
            ch: ' ',
            fg: 15,
            bg: 12,
            ..Cell::default()
        };
        let focus = Cell {
            ch: ' ',
            fg: 15,
            bg: 21,
            bold: true,
            ..Cell::default()
        };
        // One strip of PANE_COUNT equal columns.
        let n = state.panes.len().max(1);
        let w = frame.cols / n as u16;
        for (i, title) in state.panes.iter().enumerate() {
            let x0 = i as u16 * w;
            for x in x0..(x0 + w).min(frame.cols) {
                for y in 0..frame.rows {
                    let bg = if i == state.focus { focus } else { plain };
                    frame.put(x, y, bg);
                }
            }
            frame.text(
                x0 + 1,
                1,
                title,
                Cell {
                    ch: ' ',
                    fg: 15,
                    bg: 0,
                    ..Cell::default()
                },
            );
            if i == state.focus {
                frame.text(
                    x0 + 1,
                    2,
                    "<focus>",
                    Cell {
                        ch: ' ',
                        fg: 15,
                        bg: 21,
                        ..Cell::default()
                    },
                );
            }
        }
        // Status line at the bottom.
        let st = format!(
            " strimux-hmr | {} | focus {} | cols {} | frames {} | alt+r reload, q quit",
            self.label(),
            state.focus,
            frame.cols,
            state.frames,
        );
        frame.text(
            0,
            frame.rows - 1,
            &st,
            Cell {
                ch: ' ',
                fg: 15,
                bg: 8,
                ..Cell::default()
            },
        );
    }
}

/// Allocate the core and return it as a raw pointer. The trait object is
/// double-boxed so the fat (data+vtable) pointer is stored in stable memory;
/// the host recovers the outer `Box<Box<dyn StrimuxCore>>` from this thin
/// pointer and accesses the core by reference.
/// # Safety
///
/// Returns a raw pointer to a `Box<Box<dyn StrimuxCore>>` that the caller owns
/// and must reclaim with `Box::from_raw` (as `Box<Box<dyn StrimuxCore>>`).
#[no_mangle]
pub unsafe extern "C" fn strimux_core_create() -> *mut c_void {
    let core: Box<dyn StrimuxCore> = Box::new(Core { tick: 0 });
    let holder: Box<Box<dyn StrimuxCore>> = Box::new(core);
    Box::into_raw(holder) as *mut c_void
}
