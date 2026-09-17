//! Keyboard input decoding: chords to commands (verbatim move from `tui/mod.rs`).
//!
//! `handle_key` logic is untouched; the NAV rewrite lands here, not in the monolith.

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers, ModifierKeyCode};

use gwae_layout::{Action, Layout, PaneId, PaneStatus};

/// Ctrl+Shift+J/K step in plain panes, matching jcode's default
/// `keybindings.scroll_lines` (3). Keep independent of wheel tuning, and
/// leave agent panes to the harness's own configured speed at dispatch.
const KEYBOARD_SCROLL_LINES: i32 = 3;

pub(crate) fn is_alt_modifier(ev: &KeyEvent) -> bool {
    matches!(
        ev.code,
        KeyCode::Modifier(ModifierKeyCode::LeftAlt) | KeyCode::Modifier(ModifierKeyCode::RightAlt)
    )
}

pub(crate) fn physical_shift(ev: &KeyEvent) -> bool {
    if ev.modifiers.contains(KeyModifiers::SHIFT) {
        return true;
    }
    // With Kitty REPORT_ALTERNATE_KEYS shifted keys arrive as their shifted
    // codepoint with SHIFT cleared (e.g. Shift+h -> 'H'). Caps Lock alone
    // also yields uppercase but sets CAPS_LOCK state, so we must not confuse
    // the two: a Caps-generated 'H' must NOT be treated as an intentional Shift.
    if let KeyCode::Char(c) = ev.code {
        if c.is_ascii_uppercase() && !ev.state.contains(KeyEventState::CAPS_LOCK) {
            return true;
        }
    }
    false
}

pub(crate) fn logical_char(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// True when this event is the jcode-style transcript-scroll chord
/// (Ctrl+Shift+J/K) that gwae otherwise claims for its own incremental
/// history scroll (see `handle_key`).
///
/// The dispatch site uses this to prefer the harness: when the focused pane
/// is an agent pane, the chord is forwarded to the child untouched so an
/// inner jcode keeps its native scroll. Everywhere else gwae keeps the
/// chord, so plain shells scroll three lines per key press.
pub(crate) fn is_harness_scroll_chord(ev: &KeyEvent) -> bool {
    if ev.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    if !ev.modifiers.contains(KeyModifiers::CONTROL) || !physical_shift(ev) {
        return false;
    }
    matches!(logical_char(ev), Some('j') | Some('k'))
}

/// A decoded keyboard instruction.
///
/// `Cmd` is intentionally key-event shaped, not action shaped: some commands
/// must know *how* they were typed, not just what was typed. Destructive
/// verbs ignore auto-repeat (see `is_repeatable`), so a stuck key or a long
/// hold can never kill panes faster than the HUD can repaint them.
#[derive(Debug, PartialEq)]
pub(crate) enum Cmd {
    Act(Action),
    Scroll(i32),
    ScrollPane(i32),
    /// Move the focused pane's *vertical* scrollback by this many rows
    /// (positive = back into history). Reached from the keyboard
    /// (`⌥+↑/↓`, Ctrl+Shift+J/K) and from the wheel over a plain pane;
    /// a reporting child owns its own wheel instead.
    ScrollBack(i32),
    Input(Vec<u8>),
    /// Smart-jump: focus the next pane that needs the user (`⌥+g`). Resolved
    /// against the live layout in the main loop, not here.
    SmartJump,
    /// Open the spawn-directory picker (`⌥+d`): choose the directory new
    /// panes start in, for this session or written back to the config.
    DirPick,
    /// Toggle the centered cheat-sheet HUD (`⌥+/`), the same overlay shown
    /// once at startup. Any other key still dismisses it.
    ToggleHud,
    /// Toggle `keep_awake` (`⌥+w`): hold or release the macOS `caffeinate`
    /// assertion that keeps idle/display sleep from freezing the panes.
    /// Resolved in the main loop (which owns the guard and the config), not
    /// here.
    ToggleKeepAwake,
    Quit,
    None,
}

impl Cmd {
    /// Whether this command may fire on key auto-repeat.
    ///
    /// Destructive verbs (kill, quit) never repeat: holding
    /// ⌥+q must not kill panes faster than the HUD can repaint them, and
    /// a held quit chord must not confirm its own disclaimer.
    /// Everything else (focus moves, scrolls, plain input) repeats as before.
    pub(crate) fn is_repeatable(&self) -> bool {
        !matches!(
            self,
            Cmd::Act(Action::KillPane)
                | Cmd::Act(Action::ClosePane(_))
                | Cmd::Quit
                | Cmd::ToggleHud
        )
    }
}

/// Encode a key event that is not a gwae chord into PTY bytes.
///
/// Alt (Option) is forwarded as Meta: an `ESC` prefix before the base
/// sequence, matching what a pane sees when run natively outside gwae
/// (e.g. Alt/Option+Backspace becomes `ESC DEL` / `\x1b\x7f` which
/// readline interprets as `backward-kill-word`). Previously only `Char`
/// keys honored the Alt bit; `Backspace`/`Delete`/arrows etc. dropped it
/// and sent plain `DEL`, so word-delete never fired inside the mux.
pub(crate) fn key_bytes(ev: &KeyEvent) -> Vec<u8> {
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let sup = ev.modifiers.contains(KeyModifiers::SUPER);
    let shift = physical_shift(ev);
    // CSI-u branch must preserve modifiers (Ctrl+Shift vs Ctrl) for
    // inner kitty-aware apps (jcode: Ctrl+K/J = prompt, Ctrl+Shift+K/J
    // = line scroll). Legacy terminals encode both as the same 0x01 byte,
    // so intercept Ctrl+Shift before the legacy push and emit kitty CSI-u
    // (ESC [ <code> ; <mods> u) which crossterm decodes with CONTROL|SHIFT.
    if ctrl && shift {
        if let KeyCode::Char(c) = ev.code {
            if c.is_ascii_alphabetic() {
                let lc = c.to_ascii_lowercase();
                let mut bits: u8 = 0;
                if shift {
                    bits |= 1;
                }
                if alt {
                    bits |= 2;
                }
                if ctrl {
                    bits |= 4;
                }
                if sup {
                    bits |= 8;
                }
                if ev.modifiers.contains(KeyModifiers::HYPER) {
                    bits |= 16;
                }
                if ev.modifiers.contains(KeyModifiers::META) {
                    bits |= 32;
                }
                let mods = bits + 1; // kitty bias
                                     // Alt is encoded in mods for CSI-u; do not also prefix ESC.
                return format!("\x1b[{};{}u", lc as u32, mods).into_bytes();
            }
        }
    }
    let mut out = Vec::new();
    // Meta prefix: ESC before the base sequence. Ctrl combinations take
    // precedence for the base encoding but still keep the Meta ESC in front
    // (C-M-... = ESC + C-...), matching xterm's metaSendsEscape.
    let meta = alt && !matches!(ev.code, KeyCode::Esc);
    if meta {
        out.push(0x1b);
    }
    match ev.code {
        KeyCode::Char(c) => {
            if ctrl {
                let lc = c.to_ascii_lowercase();
                if lc.is_ascii_lowercase() {
                    out.push(lc as u8 - b'a' + 1);
                } else {
                    out.extend_from_slice(&[b'^', c as u8, b'\n']);
                }
            } else {
                let mut s = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut s).as_bytes());
            }
        }
        KeyCode::Enter => out.extend_from_slice(b"\r"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Tab => out.extend_from_slice(b"\t"),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        // Function keys use the standard xterm sequences so full-screen
        // children (nvim, htop, …) receive them. Before this they fell into
        // the catch-all below and produced zero bytes: silently swallowed.
        KeyCode::F(n) => {
            let seq: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => b"",
            };
            out.extend_from_slice(seq);
        }
        // Crossterm can deliver the legacy BackTab (Shift+Tab) code; forward it
        // with the same Meta prefix convention.
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        _ => {}
    }
    out
}

/// Map a key event to a command. Returns None when it is a pass-through.
pub(crate) fn handle_key(ev: &KeyEvent) -> Option<Cmd> {
    // Bare modifier press/release (Alt, Shift, Ctrl, Super, etc.) must never
    // become pane input. With the Kitty keyboard protocol a lone Option press
    // arrives as `Modifier(LeftAlt)` with the Alt bit set; the previous
    // fallthrough turned that into `key_bytes() == ESC` and cleared the
    // focused pane's line editor (e.g. jcode). Treat every pure modifier key
    // as a no-op — Alt-hold tracking is handled in run_tui, not here.
    if matches!(ev.code, KeyCode::Modifier(_)) {
        return Some(Cmd::None);
    }
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = physical_shift(ev);
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    use KeyCode::*;
    // macOS Option+letter fallback: terminals that don't translate Option to
    // Meta send these Unicode glyphs instead (US layout: h->˙ j->∆ k->˚ l->¬).
    // Remap them to focus navigation so Option+hjkl works with zero config.
    // Only fires when the char arrives as plain input (never when Option-as-Alt
    // is set, which delivers ESC+h instead), so the two paths can't collide.
    if !alt && !ctrl {
        match ev.code {
            // Option+Shift+hjkl move the pane (niri-style), US layout glyphs.
            Char('\u{d3}') => return Some(Cmd::Act(Action::MovePaneLeft)), // Ó (Option+Shift+h)
            Char('\u{d4}') => return Some(Cmd::Act(Action::MovePaneDown)), // Ô (Option+Shift+j)
            Char('\u{f8ff}') => return Some(Cmd::Act(Action::MovePaneUp)), //  (Option+Shift+k)
            Char('\u{d2}') => return Some(Cmd::Act(Action::MovePaneRight)), // Ò (Option+Shift+l)
            // ¿ (Option+Shift+/), i.e. Option+? — same toggle as Option+/.
            Char('\u{bf}') => return Some(Cmd::ToggleHud),
            // Ú (Option+Shift+;) always opens the harness picker on a new strip.
            Char('\u{da}') => return Some(Cmd::Act(Action::SpawnAgentRow)),
            _ => {}
        }
    }
    if !alt && !ctrl && !shift {
        match ev.code {
            Char('\u{2026}') => return Some(Cmd::Act(Action::SpawnAgent)), // … (Option+;)
            Char('\u{2d9}') => return Some(Cmd::Act(Action::FocusLeft)),   // ˙ (Option+h)
            Char('\u{2206}') => return Some(Cmd::Act(Action::FocusDown)),  // ∆ (Option+j)
            Char('\u{2da}') => return Some(Cmd::Act(Action::FocusUp)),     // ˚ (Option+k)
            Char('\u{ac}') => return Some(Cmd::Act(Action::FocusRight)),   // ¬ (Option+l)

            Char('\u{153}') => return Some(Cmd::Act(Action::KillPane)), // œ (Option+q)
            Char('\u{a9}') => return Some(Cmd::SmartJump),              // © (Option+g)
            Char('\u{192}') => return Some(Cmd::Act(Action::ToggleFullWidth)), // ƒ (Option+f)
            Char('\u{2202}') => return Some(Cmd::DirPick),              // ∂ (Option+d)
            Char('\u{2211}') => return Some(Cmd::ToggleKeepAwake),      // ∑ (Option+w)
            Char('\u{222b}') => return Some(Cmd::Act(Action::SplitBelow)), // ∫ (Option+b)
            Char('\u{f7}') => return Some(Cmd::ToggleHud),              // ÷ (Option+/)
            _ => {}
        }
    }
    if !alt {
        // No gwae chord starts with Escape, so a bare Esc belongs to the
        // focused pane (vim/nvim insert-mode exit, picker cancel, …).
        // The generic fallthrough at the end of this arm forwards it as
        // 0x1b via key_bytes.
        // Ctrl+Shift+J / Ctrl+Shift+K scroll the focused pane's history three
        // lines per press, like jcode's default. Plain Ctrl+J / Ctrl+K
        // belong to the pane (jcode uses them for prompt jump), so only the
        // shifted form is claimed here. `physical_shift` covers Kitty's
        // shifted-codepoint form (Ctrl+Shift+J arriving as Char('J')).
        // This sits above the generic fallthrough so the chord never types
        // into the child.
        //
        // Harness exception lives at the dispatch site, not here: when the
        // focused pane is an agent pane, the event loop forwards this chord
        // to the child untouched (see `is_harness_scroll_chord`), so an
        // inner jcode keeps its own native transcript scroll. `handle_key`
        // stays focus-blind on purpose, which is what keeps the
        // `advertised_bindings_match_the_dispatcher` cross-check honest.
        if ctrl && shift {
            match logical_char(ev) {
                Some('k') => return Some(Cmd::ScrollBack(KEYBOARD_SCROLL_LINES)),
                Some('j') => return Some(Cmd::ScrollBack(-KEYBOARD_SCROLL_LINES)),
                _ => {}
            }
        }
        return Some(Cmd::Input(key_bytes(ev)));
    }
    // Alt chords (work when the terminal sends Option as Meta).
    if let Some(c) = logical_char(ev) {
        if c == 'h' || c == 'j' || c == 'k' || c == 'l' {
            return Some(if shift {
                match c {
                    'h' => Cmd::Act(Action::MovePaneLeft),
                    'j' => Cmd::Act(Action::MovePaneDown),
                    'k' => Cmd::Act(Action::MovePaneUp),
                    'l' => Cmd::Act(Action::MovePaneRight),
                    _ => unreachable!(),
                }
            } else {
                match c {
                    'h' => Cmd::Act(Action::FocusLeft),
                    'j' => Cmd::Act(Action::FocusDown),
                    'k' => Cmd::Act(Action::FocusUp),
                    'l' => Cmd::Act(Action::FocusRight),
                    _ => unreachable!(),
                }
            });
        }
        // ⌥+Shift+q and ⌥+Shift+? are the only shifted chords outside hjkl.
        // Everything else below is an *unshifted* chord: `logical_char` folds
        // case, so without this guard ⌥+Shift+s would be indistinguishable from
        // ⌥+s and gwae would split the column instead of forwarding the key.
        // That silently ate chords the focused pane owns (jcode binds
        // ⌥+Shift+s to copy), so shifted variants fall through to the pane.
        if shift && !matches!(c, 'q' | '/' | '?' | ';' | ':') {
            // Forward the shifted codepoint. Some terminals report Shift as a
            // modifier bit alongside the *unshifted* char; `key_bytes` encodes
            // `ev.code` verbatim and has no shift handling for `Char`, so the
            // pane would receive ESC+'s' and see a plain ⌥+s. Re-apply the
            // shift here so the pane sees ESC+'S' either way.
            let mut ev = *ev;
            if let KeyCode::Char(raw) = ev.code {
                ev.code = KeyCode::Char(raw.to_ascii_uppercase());
            }
            return Some(Cmd::Input(key_bytes(&ev)));
        }
        let act = match c {
            // Some terminals deliver ⌥+Shift+; as a bare ':' with no shift
            // bit; the shifted codepoint itself is the signal.
            ';' | ':' => Some(if shift || matches!(ev.code, Char(':')) {
                Action::SpawnAgentRow
            } else {
                Action::SpawnAgent
            }),
            // ⌥+b splits *below*. It used to be ⌥+s, which collided with
            // jcode's typing-scroll-lock toggle: gwae ate the chord and the
            // agent pane never saw it. `b` is free on both sides.
            'b' => Some(Action::SplitBelow),
            'r' => Some(Action::CycleWidth),
            'f' => Some(Action::ToggleFullWidth),
            'q' => {
                if shift {
                    return Some(Cmd::Quit);
                } else {
                    Some(Action::KillPane)
                }
            }
            'g' => return Some(Cmd::SmartJump),
            'd' => return Some(Cmd::DirPick),
            'w' => return Some(Cmd::ToggleKeepAwake),
            '/' | '?' => return Some(Cmd::ToggleHud),
            _ => None,
        };
        if let Some(a) = act {
            return Some(Cmd::Act(a));
        }
        if matches!(c, '[' | ']') {
            return Some(if c == '[' {
                Cmd::Scroll(-200)
            } else {
                Cmd::Scroll(200)
            });
        }
    }
    // Up/Down move the focused pane's scrollback: the wheel joins them as
    // a per-notch line step (see the mouse arm), and Ctrl+Shift+J/K match
    // jcode's default three-line transcript scroll. Shift (and
    // PageUp/PageDown) move by a screenful-ish jump rather than a small step.
    if matches!(ev.code, Up | Down | PageUp | PageDown) {
        let step = if shift || matches!(ev.code, PageUp | PageDown) {
            20
        } else {
            3
        };
        return Some(match ev.code {
            Up | PageUp => Cmd::ScrollBack(step),
            _ => Cmd::ScrollBack(-step),
        });
    }
    // Shift+arrow and plain arrow scroll the pane content.
    if matches!(ev.code, Left | Right) {
        if shift {
            return Some(match ev.code {
                Left => Cmd::ScrollPane(-16),
                Right => Cmd::ScrollPane(16),
                _ => unreachable!(),
            });
        }
        return Some(match ev.code {
            Left => Cmd::ScrollPane(-1),
            Right => Cmd::ScrollPane(1),
            _ => unreachable!(),
        });
    }
    if ev.code == Enter {
        return Some(Cmd::Act(if shift {
            Action::NewRow
        } else {
            Action::NewColumn
        }));
    }
    // Alt+digit/punct not listed above: check the original code directly
    // since those don't need case folding.
    match ev.code {
        Char('[') => return Some(Cmd::Scroll(-200)),
        Char(']') => return Some(Cmd::Scroll(200)),
        _ => {}
    }
    Some(Cmd::Input(key_bytes(ev)))
}

/// Step a picker list (`⌥+d` spawn-dir, `⌥+;` harness) with vim keys.
///
/// Bare `j`/`k` must keep typing into the filter (paths contain both), so
/// navigation takes the modified forms: `⌃+j/k`, `⌥+j/k`, `⌃+n/p`, plus the
/// macOS Option-glyphs (`∆`/`˚`) sent when Option is not Meta. Arrows keep
/// working. Returns the signed step or `None` when the event is not nav.
pub(crate) fn picker_step(ev: &KeyEvent) -> Option<i32> {
    use KeyCode::*;
    match ev.code {
        Up => Some(-1),
        Down => Some(1),
        // macOS no-Meta path: Option+j/k arrive as ∆/˚ with no modifiers.
        // They would otherwise be typed into the filter as literal glyphs.
        Char('\u{2206}') => Some(1), // ∆ (Option+j)
        Char('\u{2da}') => Some(-1), // ˚ (Option+k)
        Char(c) => {
            let lc = c.to_ascii_lowercase();
            let alt = ev.modifiers.contains(KeyModifiers::ALT);
            let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
            if !alt && !ctrl {
                return None;
            }
            match lc {
                'j' => Some(1),
                'k' => Some(-1),
                'n' if ctrl || alt => Some(1),
                'p' if ctrl || alt => Some(-1),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Trim clipboard text for the spawn-dir filter: directory paths are
/// single-line, so keep the first line and strip newlines. Empty in, empty
/// out: the caller decides whether to touch the filter.
pub(crate) fn picker_paste_query(text: &str) -> String {
    // Native terminals often send CR-only lines, whereas clipboard helpers
    // usually return LF or CRLF. str::lines() does not split a lone CR.
    text.trim()
        .split(['\r', '\n'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// Native paste is the single route for both delivery and confirmation.
///
/// A multi-line paste is the case that used to run each line as its own
/// command, so gwae says what it delivered. When the child never asked for
/// bracketed paste (`bracketed` false) those newlines genuinely are Returns —
/// nothing can prevent that, it is what the program asked for — so the toast
/// says so rather than letting the user infer safety from silence.
pub(crate) fn paste_note(text: &str, bracketed: bool) -> String {
    // Native terminals often send CR-only text. Count the same logical
    // lines for CR, LF, and CRLF, including preserved blank lines.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.lines().count().max(1);
    let summary = if lines == 1 {
        "pasted 1 line".to_string()
    } else {
        format!("pasted {lines} lines")
    };
    if !bracketed && normalized.contains('\n') {
        format!("{summary} · no bracket, newlines run")
    } else {
        summary
    }
}

/// Pick the pane a smart-jump (`⌥+g`) should land on: the next pane, in
/// layout order starting just past the focused one and wrapping, whose status
/// needs the user. Priority: Failed beats Idle (attention) beats Done;
/// Running panes are never targets (they're fine on their own). Returns None
/// when every other pane is happily working.
pub(crate) fn smart_jump_target(layout: &Layout) -> Option<PaneId> {
    let focused = focused_pane(layout);
    // Flatten the grid in reading order: strips top-down, columns
    // left-to-right, stacks top-down.
    let order: Vec<PaneId> = layout
        .rows
        .iter()
        .flat_map(|r| r.columns.iter())
        .flat_map(|c| c.panes.iter().copied())
        .collect();
    let start = focused
        .and_then(|f| order.iter().position(|p| *p == f))
        .map(|i| i + 1)
        .unwrap_or(0);
    let rank = |s: PaneStatus| match s {
        PaneStatus::Failed => Some(0u8),
        PaneStatus::Idle => Some(1),
        PaneStatus::Done => Some(2),
        PaneStatus::Running => None,
    };
    let mut best: Option<(u8, usize, PaneId)> = None;
    for (i, pid) in order
        .iter()
        .enumerate()
        .cycle()
        .skip(start)
        .take(order.len())
    {
        if Some(*pid) == focused {
            continue;
        }
        let Some(st) = layout.panes.get(pid).map(|p| p.status) else {
            continue;
        };
        let Some(r) = rank(st) else { continue };
        // Distance from the focused pane, wrapping: nearer wins within a rank.
        let dist = (i + order.len() - start) % order.len();
        if best.map(|(br, bd, _)| (r, dist) < (br, bd)).unwrap_or(true) {
            best = Some((r, dist, *pid));
        }
    }
    best.map(|(_, _, pid)| pid)
}

/// Total number of panes currently present in the layout.
pub(crate) fn layout_pane_count(layout: &Layout) -> usize {
    layout
        .rows
        .iter()
        .flat_map(|r| r.columns.iter())
        .map(|c| c.panes.len())
        .sum()
}

/// The currently focused pane id.
pub(crate) fn focused_pane(layout: &Layout) -> Option<PaneId> {
    layout
        .focused_row()
        .and_then(|r| r.columns.get(layout.focus.column))
        .and_then(|c| c.panes.get(layout.focus.pane))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use gwae_layout::{Layout, PaneId};

    #[test]
    fn handle_key_option_semicolon_spawns_agent() {
        // macOS sends U+2026 (…) for Option+; when it doesn't translate to Meta.
        let ev = KeyEvent::new(KeyCode::Char('\u{2026}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgent)));
        // Terminals that deliver Option as Meta send ESC+; -> Alt+;.
        let ev = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgent)));
    }

    #[test]
    fn handle_key_option_shift_semicolon_opens_the_row_picker() {
        // macOS glyph fallback: Option+Shift+; is Ú.
        let ev = KeyEvent::new(KeyCode::Char('\u{da}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
        // Option-as-Meta: ESC+':' arrives as Alt+':' (shifted codepoint, and
        // some terminals also set the Shift bit).
        let ev = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
        let ev = KeyEvent::new(KeyCode::Char(';'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SpawnAgentRow)));
    }

    #[test]
    fn option_a_and_option_x_belong_to_the_pane_again() {
        // Both bindings were removed: gwae must not swallow them, so the
        // focused pane sees the chord (jcode and vim bind ⌥+a / ⌥+x).
        for c in ['a', 'x'] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(key_bytes(&ev))),
                "⌥+{c} should be forwarded, not claimed"
            );
        }
    }

    #[test]
    fn alt_up_down_is_the_keyboard_route_into_scrollback() {
        // `⌥+↑/↓` is the original keyboard route into scrollback;
        // Ctrl+Shift+J/K and the wheel join it (see below), so this asserts
        // the long-standing chords keep working rather than being the only
        // way back into history.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(3)),
            "no way back into scrollback"
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(-3))
        );
        // Shift and PageUp/PageDown take a bigger bite.
        assert_eq!(
            handle_key(&KeyEvent::new(
                KeyCode::Up,
                KeyModifiers::ALT | KeyModifiers::SHIFT
            )),
            Some(Cmd::ScrollBack(20))
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(20))
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::PageDown, KeyModifiers::ALT)),
            Some(Cmd::ScrollBack(-20))
        );
        // Without Alt these are ordinary keys the program inside the pane
        // owns: stealing a bare Up arrow would break every shell's history.
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
        assert!(matches!(
            handle_key(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(Cmd::Input(_))
        ));
        // And the horizontal pan it sits next to still works.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            Some(Cmd::ScrollPane(-1))
        );
    }

    #[test]
    fn harness_scroll_chord_matches_both_shift_forms_but_nothing_else() {
        // The dispatch-site carve-out must fire for exactly the chord the
        // multiplexer otherwise claims: Ctrl+Shift+J/K in both the explicit
        // (CONTROL|SHIFT + lowercase) and Kitty (CONTROL + uppercase) forms.
        let shift_ctrl = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        for ev in [
            KeyEvent::new(KeyCode::Char('j'), shift_ctrl),
            KeyEvent::new(KeyCode::Char('k'), shift_ctrl),
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::CONTROL),
        ] {
            assert!(is_harness_scroll_chord(&ev), "{ev:?} is the harness chord");
        }
        // Plain prompt-jump chords, Alt focus moves, and Caps Lock impostors
        // are not the scroll chord and must never trigger the carve-out.
        let caps = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('K'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        for ev in [
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Char('x'), shift_ctrl),
            caps,
        ] {
            assert!(
                !is_harness_scroll_chord(&ev),
                "{ev:?} must not count as the harness chord"
            );
        }
        // ...and every positive case is one `handle_key` would otherwise
        // claim as multiplexer scroll, which is the conflict being resolved.
        for ev in [
            KeyEvent::new(KeyCode::Char('j'), shift_ctrl),
            KeyEvent::new(KeyCode::Char('k'), shift_ctrl),
            KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('K'), KeyModifiers::CONTROL),
        ] {
            assert!(
                matches!(handle_key(&ev), Some(Cmd::ScrollBack(_))),
                "{ev:?} must be a multiplexer scroll without the carve-out"
            );
        }
        // The forwarded bytes must preserve the chord: a kitty-aware harness
        // decodes CSI-u, so Ctrl+Shift+K must not collapse to a bare 0x0b
        // (plain Ctrl+K), which is jcode's prompt jump, not its scroll.
        // Both shift forms forward identically: the explicit CONTROL|SHIFT
        // lowercase event and Kitty's CONTROL + uppercase codepoint.
        let k = KeyEvent::new(KeyCode::Char('k'), shift_ctrl);
        let j = KeyEvent::new(KeyCode::Char('j'), shift_ctrl);
        let kitty_k = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::CONTROL);
        let kitty_j = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL);
        assert_eq!(key_bytes(&k), b"\x1b[107;6u".to_vec());
        assert_eq!(key_bytes(&j), b"\x1b[106;6u".to_vec());
        assert_eq!(key_bytes(&kitty_k), b"\x1b[107;6u".to_vec());
        assert_eq!(key_bytes(&kitty_j), b"\x1b[106;6u".to_vec());
    }

    #[test]
    fn plain_ctrl_jk_always_reaches_the_pane_as_prompt_jump_bytes() {
        // The harness contract: plain Ctrl+J / Ctrl+K carry no Shift
        // information in any terminal encoding, so they can never be a
        // scroll chord. gwae must forward them as the legacy control byte
        // (0x0A / 0x0B) in every pane, and must never claim them for its
        // own scrollback or any layout action. This is the regression test
        // the Sept-4 scroll claim lacked: it pinned the shifted chord but
        // left the unshifted one to drift.
        for (c, byte) in [('j', 0x0au8), ('k', 0x0bu8)] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(vec![byte])),
                "plain Ctrl+{c} must forward 0x{byte:02X}, jcode's prompt jump"
            );
            // The chord is shift-free by construction: the harness predicate
            // must reject it, so the agent-pane fast-path can never divert
            // it away from the child either.
            assert!(
                !is_harness_scroll_chord(&ev),
                "plain Ctrl+{c} must not count as the harness scroll chord"
            );
            // Caps Lock must not promote it either: Ctrl+CapsLock+K is the
            // same legacy byte, not a scroll.
            let caps = KeyEvent::new_with_kind_and_state(
                KeyCode::Char(c.to_ascii_uppercase()),
                KeyModifiers::CONTROL,
                KeyEventKind::Press,
                KeyEventState::CAPS_LOCK,
            );
            assert_eq!(
                handle_key(&caps),
                Some(Cmd::Input(vec![byte])),
                "Ctrl+CapsLock+{c} must forward 0x{byte:02X} like plain Ctrl+{c}"
            );
            assert!(
                !is_harness_scroll_chord(&caps),
                "Ctrl+CapsLock+{c} must not count as the harness scroll chord"
            );
        }
    }

    #[test]
    fn ctrl_shift_jk_scroll_scrollback_three_lines_like_jcode() {
        // jcode's default: Ctrl+Shift+K scrolls up three lines, Ctrl+Shift+J
        // scrolls down three lines. Plain Ctrl+J / Ctrl+K stay with the pane
        // (jcode uses them for prompt jump), so only the shifted form is
        // claimed here.
        let shift_ctrl = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('k'), shift_ctrl)),
            Some(Cmd::ScrollBack(3)),
            "Ctrl+Shift+K should scroll back three lines"
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('j'), shift_ctrl)),
            Some(Cmd::ScrollBack(-3)),
            "Ctrl+Shift+J should scroll forward three lines"
        );
        // Kitty's shifted-codepoint form (Shift reported as uppercase with
        // the SHIFT bit cleared) counts too: `physical_shift` sees the 'K'.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('K'), KeyModifiers::CONTROL)),
            Some(Cmd::ScrollBack(3)),
            "Kitty-form Ctrl+Shift+K should scroll back three lines"
        );
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('J'), KeyModifiers::CONTROL)),
            Some(Cmd::ScrollBack(-3)),
            "Kitty-form Ctrl+Shift+J should scroll forward three lines"
        );
        // Holding the chord keeps the same stride for every repeat event.
        for (c, delta) in [('k', 3), ('j', -3)] {
            let ev = KeyEvent::new_with_kind(KeyCode::Char(c), shift_ctrl, KeyEventKind::Repeat);
            assert_eq!(handle_key(&ev), Some(Cmd::ScrollBack(delta)));
        }
        // Unshifted Ctrl+J / Ctrl+K reach the child untouched.
        for c in ['j', 'k'] {
            assert!(
                matches!(
                    handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
                    Some(Cmd::Input(_))
                ),
                "plain Ctrl+{c} belongs to the pane, not to scrollback"
            );
        }
        // Alt must not hijack the chord either: ⌥+J/K are focus moves.
        assert_eq!(
            handle_key(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
            Some(Cmd::Act(Action::FocusDown))
        );
        // Caps Lock is not Shift: Ctrl+CapsLock+K must reach the pane as a
        // control byte, not scroll. (physical_shift excludes CAPS_LOCK.)
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('K'),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert!(
            matches!(handle_key(&ev), Some(Cmd::Input(_))),
            "Ctrl+CapsLock+K belongs to the pane, not to scrollback"
        );
    }

    #[test]
    fn handle_key_alt_q_kills_pane() {
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::KillPane)));
        // macOS Option+q -> œ (U+0153) on the no-Meta path.
        let ev = KeyEvent::new(KeyCode::Char('\u{153}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::KillPane)));
    }

    #[test]
    fn destructive_verbs_never_fire_on_auto_repeat() {
        // Holding ⌥+q repeats faster than frames: without this, every queued
        // repeat lands in one drain batch and the HUD shows a stale frame
        // while several panes die underneath it.
        for cmd in [
            Cmd::Act(Action::KillPane),
            Cmd::Act(Action::ClosePane(1)),
            Cmd::Quit,
            Cmd::ToggleHud,
        ] {
            assert!(!cmd.is_repeatable(), "{cmd:?} must not repeat");
        }
        // Focus moves, scrolls, and pane input repeat as before.
        for cmd in [
            Cmd::Act(Action::FocusLeft),
            Cmd::Act(Action::SpawnAgent),
            Cmd::Scroll(200),
            Cmd::ScrollBack(3),
            Cmd::SmartJump,
            Cmd::Input(vec![b'x']),
            Cmd::None,
        ] {
            assert!(cmd.is_repeatable(), "{cmd:?} should still repeat");
        }
    }

    #[test]
    fn handle_key_option_shift_hjkl_moves_pane() {
        // Terminals delivering Option as Meta: Alt+Shift+hjkl.
        for (c, act) in [
            ('h', Action::MovePaneLeft),
            ('j', Action::MovePaneDown),
            ('k', Action::MovePaneUp),
            ('l', Action::MovePaneRight),
        ] {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
            // Uppercase variant, with or without an explicit SHIFT bit.
            let up = c.to_ascii_uppercase();
            let ev = KeyEvent::new(KeyCode::Char(up), KeyModifiers::ALT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
        }
        // macOS no-Meta path: Option+Shift+hjkl arrive as Ó Ô  Ò.
        for (g, act) in [
            ('\u{d3}', Action::MovePaneLeft),
            ('\u{d4}', Action::MovePaneDown),
            ('\u{f8ff}', Action::MovePaneUp),
            ('\u{d2}', Action::MovePaneRight),
        ] {
            let ev = KeyEvent::new(KeyCode::Char(g), KeyModifiers::SHIFT);
            assert_eq!(handle_key(&ev), Some(Cmd::Act(act)));
        }
    }

    #[test]
    fn caps_lock_does_not_trigger_shift_chords() {
        // Caps+h sends an uppercase 'H' with CAPS_LOCK state (state is
        // produced by the kitty bit-64 alternate). It must NOT become
        // MovePaneLeft (which requires physical Shift).
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('H'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::FocusLeft)));
        // Same with Alt+Shift: Caps should not fake Shift+hjkl either.
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('H'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::FocusLeft)));
    }

    #[test]
    fn kitty_shifted_key_with_shift_cleared_is_still_a_move() {
        // With REPORT_ALTERNATE_KEYS the shift is consumed: 'H' arrives with no
        // SHIFT modifier but without CAPS_LOCK, so physical_shift sees it as Shift.
        let ev = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::MovePaneLeft)));
        let ev = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::MovePaneUp)));
    }

    #[test]
    fn alt_shift_letter_is_forwarded_not_swallowed_as_the_unshifted_chord() {
        // Regression: ⌥+Shift+b used to case-fold to 'b' and split the column,
        // stealing a chord the focused pane owns (jcode copies with ⌥+Shift+s).
        // Both encodings a terminal may send must reach the pane as ESC+'S'.
        for ev in [
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::ALT | KeyModifiers::SHIFT),
            // Kitty REPORT_ALTERNATE_KEYS consumes the shift bit.
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::ALT),
            // Terminals that keep the unshifted codepoint plus a SHIFT bit.
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        ] {
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(b"\x1bB".to_vec())),
                "{ev:?} should be forwarded to the pane"
            );
        }
        // The unshifted chord still splits (⌥+b since the ⌥+s rebind).
        let ev = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SplitBelow)));
        // Caps Lock is not Shift: ⌥+CapsLock+b must still split.
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('B'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Act(Action::SplitBelow)));
        // The two intentional shifted chords keep working.
        let ev = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert_eq!(handle_key(&ev), Some(Cmd::Quit));
    }

    #[test]
    fn shift_and_caps_typed_text_passes_through_as_shifted() {
        // Plain Shift+a -> 'A' is pane input, not a gwae chord.
        let ev = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
        // Shift+1 -> '!' via the shifted codepoint path.
        let ev = KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"!".to_vec())));
        // Caps+a also produces 'A' (caps state) but should still type 'A' when
        // not an Alt chord, just like Shift. Focus test is plain key:
        let ev = KeyEvent::new_with_kind_and_state(
            KeyCode::Char('A'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::CAPS_LOCK,
        );
        assert_eq!(handle_key(&ev), Some(Cmd::Input(b"A".to_vec())));
    }

    #[test]
    fn hangul_syllables_are_forwarded_as_utf8_not_swallowed() {
        // Regression: Korean IME composition must reach the pane verbatim as UTF-8.
        // Every Hangul syllable/jamo appears as KeyCode::Char(c) with no modifiers
        // (or only SHIFT for doubled consonants) and must become Cmd::Input with
        // the 3-byte UTF-8 encoding, not a gwae chord or None.
        for ch in [
            '\u{ac00}', // 가
            '\u{b098}', // 나
            '\u{b2e4}', // 다
            '\u{3142}', // ㅂ (jamo)
            '\u{3147}', // ㅇ
            '\u{ce58}', // 치
        ] {
            let ev = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
            let mut want = [0u8; 4];
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(ch.encode_utf8(&mut want).as_bytes().to_vec())),
                "hangul {ch} U+{:04X} should be forwarded as UTF-8",
                ch as u32
            );
            // key_bytes must preserve the same UTF-8 even for Shift+Hangul (doubled
            // consonant path) and must not misroute through the Alt/shift-chord check.
            let ev = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::SHIFT);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(ch.encode_utf8(&mut want).as_bytes().to_vec())),
                "shifted hangul {ch} should still be UTF-8, not a move-pane chord"
            );
        }
        // Raw key_bytes for a burst (fast typing) joins correctly: the drain loop
        // idea is that multiple such KeyEvents between polls are all flushed before
        // the next paint. Exercise the burst at the handler level.
        let burst = ['\u{ac00}', '\u{b098}', '\u{b2e4}']; // 가나다
        let mut bytes = Vec::new();
        for &ch in &burst {
            let ev = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
            match handle_key(&ev).unwrap() {
                Cmd::Input(b) => bytes.extend(b),
                other => panic!("unexpected {other:?} for {ch}"),
            }
        }
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "\u{ac00}\u{b098}\u{b2e4}"
        );
    }

    #[test]
    fn bare_modifier_never_sends_input_to_pane() {
        // With the Kitty keyboard protocol a lone Option press arrives as
        // `Modifier(LeftAlt)` with the Alt bit set. The old fallthrough
        // treated it as Meta+<nothing> -> a bare ESC, which clears jcode's
        // line editor (e.g. pressing Option to poll the HUD erased the
        // input line). Every pure modifier press/release must be a no-op.
        for code in [
            KeyCode::Modifier(ModifierKeyCode::LeftAlt),
            KeyCode::Modifier(ModifierKeyCode::RightAlt),
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyCode::Modifier(ModifierKeyCode::RightShift),
            KeyCode::Modifier(ModifierKeyCode::LeftControl),
            KeyCode::Modifier(ModifierKeyCode::RightControl),
            KeyCode::Modifier(ModifierKeyCode::LeftSuper),
            KeyCode::Modifier(ModifierKeyCode::RightSuper),
        ] {
            let mods = if matches!(
                code,
                KeyCode::Modifier(ModifierKeyCode::LeftAlt)
                    | KeyCode::Modifier(ModifierKeyCode::RightAlt)
            ) {
                KeyModifiers::ALT
            } else {
                KeyModifiers::NONE
            };
            let ev = KeyEvent::new(code, mods);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::None),
                "bare modifier {code:?} must not become pane input"
            );
        }
    }

    #[test]
    fn smart_jump_prefers_failed_then_attention() {
        let mut layout = Layout::default(); // 4 panes, focus on pane 0
        let ids: Vec<PaneId> = layout.rows[0]
            .columns
            .iter()
            .flat_map(|c| c.panes.clone())
            .collect();
        // All running: nothing needs the user.
        assert_eq!(smart_jump_target(&layout), None);
        // Pane 3 done, pane 2 idle, pane 1 failed: failed wins outright.
        layout.panes.get_mut(&ids[3]).unwrap().status = PaneStatus::Done;
        assert_eq!(smart_jump_target(&layout), Some(ids[3]));
        layout.panes.get_mut(&ids[2]).unwrap().status = PaneStatus::Idle;
        assert_eq!(
            smart_jump_target(&layout),
            Some(ids[2]),
            "attention beats done"
        );
        layout.panes.get_mut(&ids[1]).unwrap().status = PaneStatus::Failed;
        assert_eq!(smart_jump_target(&layout), Some(ids[1]), "failed beats all");
        // The focused pane is never a target even when it failed.
        layout.panes.get_mut(&ids[0]).unwrap().status = PaneStatus::Failed;
        assert_eq!(smart_jump_target(&layout), Some(ids[1]));
    }

    /// The registry in [`crate::binds`] is only a single source of truth if
    /// the dispatcher agrees with it. Feed every machine-checkable entry
    /// through the real `handle_key` (both the Meta path and, where one
    /// exists, the macOS glyph fallback) and require the advertised effect.
    #[test]
    fn advertised_bindings_match_the_dispatcher() {
        use crate::binds::{Effect, Trigger, BINDS};
        let expect = |e: Effect| -> Option<Cmd> {
            Some(match e {
                Effect::Act(a) => Cmd::Act(a),
                Effect::SmartJump => Cmd::SmartJump,
                Effect::DirPick => Cmd::DirPick,
                Effect::ToggleHud => Cmd::ToggleHud,
                Effect::ToggleKeepAwake => Cmd::ToggleKeepAwake,
                Effect::Quit => Cmd::Quit,
                Effect::Scroll(n) => Cmd::Scroll(n),
                Effect::ScrollBack(n) => Cmd::ScrollBack(n),
                Effect::Unverifiable => return None,
            })
        };
        for b in BINDS {
            if expect(b.effect).is_none() {
                continue;
            }
            let want = expect(b.effect);
            match b.trigger {
                Trigger::Chord(c) => {
                    let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
                    assert_eq!(
                        handle_key(&ev),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                }
                Trigger::ShiftChord(c) => {
                    let ev =
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT);
                    assert_eq!(
                        handle_key(&ev),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                }
                Trigger::CtrlShift(c) => {
                    // Ctrl+Shift arrives either as the lowercase char with
                    // CONTROL|SHIFT or, under Kitty's alternate-keys flag, as
                    // the uppercase char with CONTROL alone. Both must reach
                    // the advertised scroll.
                    for ev in [
                        KeyEvent::new(
                            KeyCode::Char(c.to_ascii_lowercase()),
                            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                        ),
                        KeyEvent::new(KeyCode::Char(c.to_ascii_uppercase()), KeyModifiers::CONTROL),
                    ] {
                        assert_eq!(
                            handle_key(&ev),
                            want,
                            "{} ({}) must dispatch as advertised",
                            b.label(),
                            b.desc
                        );
                    }
                }
                Trigger::EnterChord { shift } => {
                    let mut mods = KeyModifiers::ALT;
                    if shift {
                        mods |= KeyModifiers::SHIFT;
                    }
                    assert_eq!(
                        handle_key(&KeyEvent::new(KeyCode::Enter, mods)),
                        want,
                        "{} ({}) must dispatch as advertised",
                        b.label(),
                        b.desc
                    );
                    // A *bare* Return (no modifier) belongs to the focused
                    // pane. The cheat-sheet used to label this row `↵`, which
                    // told users to press a key that only types a newline.
                    assert!(
                        matches!(
                            handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                            Some(Cmd::Input(_))
                        ),
                        "bare Return must reach the pane, not the layout"
                    );
                }
                Trigger::ModProse(_) | Trigger::Prose(_) => continue,
            }
            // The macOS glyph fallback must reach the same command, so the
            // cheat-sheet is honest on terminals without "Option as Meta".
            if let Some(g) = b.glyph {
                let mods = if matches!(b.trigger, Trigger::ShiftChord(_)) {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                assert_eq!(
                    handle_key(&KeyEvent::new(KeyCode::Char(g), mods)),
                    expect(b.effect),
                    "glyph {g:?} fallback for {} must match",
                    b.label()
                );
            }
        }
    }

    #[test]
    fn keep_awake_toggle_reaches_the_main_loop_on_both_paths() {
        // Option-as-Meta path and the macOS glyph path must both decode as
        // the toggle: the cross-check test covers the BINDS entry, but this
        // pins the exact events so a dispatcher refactor cannot silently turn
        // ⌥+w into pane input.
        let meta = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT);
        assert_eq!(handle_key(&meta), Some(Cmd::ToggleKeepAwake));
        let glyph = KeyEvent::new(KeyCode::Char('\u{2211}'), KeyModifiers::NONE);
        assert_eq!(handle_key(&glyph), Some(Cmd::ToggleKeepAwake));
        // ...while a bare `w` still types into the pane.
        let bare = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE);
        assert!(
            matches!(handle_key(&bare), Some(Cmd::Input(_))),
            "bare w must reach the pane"
        );
    }

    #[test]
    fn option_v_is_unbound_and_belongs_to_the_child() {
        // Only the host's native paste event triggers gwae paste handling.
        // Unbound chords and literal Unicode retain ordinary pane semantics.
        let meta = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
        assert_eq!(handle_key(&meta), Some(Cmd::Input(b"\x1bv".to_vec())));
        let glyph = KeyEvent::new(KeyCode::Char('\u{221a}'), KeyModifiers::NONE);
        assert_eq!(
            handle_key(&glyph),
            Some(Cmd::Input("√".as_bytes().to_vec()))
        );
        // ...while a bare `v` still types into the pane.
        let bare = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert!(
            matches!(handle_key(&bare), Some(Cmd::Input(_))),
            "bare v must reach the pane"
        );
        assert_eq!(key_bytes(&meta), b"\x1bv".to_vec());
    }

    #[test]
    fn paste_note_confirms_single_line_and_warns_without_bracket() {
        // The single native route reports completion and whether the child
        // buffers newlines. Host CR-only text must not undercount the paste.
        assert_eq!(paste_note("ls -la", true), "pasted 1 line");
        assert_eq!(paste_note("a\nb\nc", true), "pasted 3 lines");
        assert_eq!(paste_note("a\r\rb\r", true), "pasted 3 lines");
        assert_eq!(paste_note("a\r\n\r\nb\r\n", true), "pasted 3 lines");
        assert!(paste_note("a\rb\rc", false).contains("newlines run"));
        assert!(paste_note("a\nb\nc", false).contains("newlines run"));
        assert!(paste_note("ls\r", false).contains("newlines run"));
        assert_eq!(paste_note("ls", false), "pasted 1 line");
    }

    #[test]
    fn only_native_paste_is_advertised_in_the_cheat_sheet() {
        let rows: Vec<_> = crate::binds::group(crate::binds::Group::Panes)
            .filter(|b| b.desc == "paste")
            .collect();
        assert_eq!(rows.len(), 1, "exactly one paste action is advertised");
        assert_eq!(rows[0].label(), crate::keys::paste_key());
        assert!(crate::binds::BINDS
            .iter()
            .all(|b| { b.trigger != crate::binds::Trigger::Chord('v') && b.glyph != Some('√') }));
    }

    #[test]
    fn picker_step_takes_modified_jk_and_arrows_but_not_bare_letters() {
        // Arrows always move; modified j/k/n/p move; bare letters must type
        // into the filter (paths contain j and k).
        assert_eq!(
            picker_step(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(-1)
        );
        assert_eq!(
            picker_step(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            Some(1)
        );
        for (mods, key, want) in [
            (KeyModifiers::CONTROL, 'j', 1),
            (KeyModifiers::CONTROL, 'k', -1),
            (KeyModifiers::CONTROL, 'n', 1),
            (KeyModifiers::CONTROL, 'p', -1),
            (KeyModifiers::ALT, 'j', 1),
            (KeyModifiers::ALT, 'k', -1),
        ] {
            assert_eq!(
                picker_step(&KeyEvent::new(KeyCode::Char(key), mods)),
                Some(want),
                "ctrl/alt {key:?} must step {want}"
            );
        }
        // macOS no-Meta glyphs arrive bare and must not type ∆/˚ into the
        // filter.
        assert_eq!(
            picker_step(&KeyEvent::new(
                KeyCode::Char('\u{2206}'),
                KeyModifiers::NONE
            )),
            Some(1)
        );
        assert_eq!(
            picker_step(&KeyEvent::new(KeyCode::Char('\u{2da}'), KeyModifiers::NONE)),
            Some(-1)
        );
        for c in ['j', 'k', 'n', 'p', 's', 'x'] {
            assert_eq!(
                picker_step(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
                None,
                "bare {c} must type, not move"
            );
        }
        assert_eq!(
            picker_step(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT)),
            None,
            "the dir-picker save chord is not nav"
        );
    }

    #[test]
    fn picker_paste_query_keeps_a_single_clean_path() {
        // Pasted `~/` paths land in the filter; multi-line payloads and
        // carriage returns are trimmed to the first line.
        assert_eq!(picker_paste_query("  ~/git/gwae  "), "~/git/gwae");
        assert_eq!(picker_paste_query("~/a\n~/b\n"), "~/a");
        assert_eq!(picker_paste_query("a\rb\n"), "a");
        assert_eq!(picker_paste_query("~/a\r\n~/b\r\n"), "~/a");
        assert!(picker_paste_query("  \n  ").is_empty());
    }

    #[test]
    fn alt_digits_are_forwarded_to_the_pane() {
        // Column jump is gone: Option+digits belong to the child (readline
        // word ops, vim counts) and must arrive as Meta ESC+digit.
        for d in 0..=9u32 {
            let c = char::from_digit(d, 10).unwrap();
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
            assert_eq!(
                handle_key(&ev),
                Some(Cmd::Input(key_bytes(&ev))),
                "digit {d} should be forwarded, not claimed"
            );
        }
    }

    #[test]
    fn alt_slash_toggles_the_cheat_sheet_hud() {
        // Option-as-Meta path and the macOS glyph path both map to ToggleHud.
        let ev = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::ALT);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('\u{f7}'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        // Option+Shift+/ (Option+?) is the same toggle, both paths.
        let ev = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('\u{bf}'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::ToggleHud)));
        let ev = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert!(matches!(handle_key(&ev), Some(Cmd::Input(_))));
        // Plain `/` stays pane input.
        let ev = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(matches!(handle_key(&ev), Some(Cmd::Input(_))));
    }

    #[test]
    fn bare_escape_reaches_the_pane() {
        // No gwae chord starts with Escape: a bare Esc must forward as 0x1b
        // so vim/nvim can leave insert mode (previously swallowed as None).
        let ev = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(handle_key(&ev), Some(Cmd::Input(vec![0x1b])));
    }

    #[test]
    fn function_keys_encode_xterm_sequences() {
        // F-keys previously fell into the catch-all and produced zero bytes.
        for (n, want) in [
            (1u8, b"\x1bOP".as_slice()),
            (2, b"\x1bOQ"),
            (3, b"\x1bOR"),
            (4, b"\x1bOS"),
            (5, b"\x1b[15~"),
            (6, b"\x1b[17~"),
            (7, b"\x1b[18~"),
            (8, b"\x1b[19~"),
            (9, b"\x1b[20~"),
            (10, b"\x1b[21~"),
            (11, b"\x1b[23~"),
            (12, b"\x1b[24~"),
        ] {
            let ev = KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE);
            assert_eq!(handle_key(&ev), Some(Cmd::Input(want.to_vec())), "F{n}");
        }
    }
}
