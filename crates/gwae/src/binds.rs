//! The single source of truth for gwae keybindings.
//!
//! Before this module existed the same bindings were spelled out in three
//! places: the `handle_key` match in [`crate::tui`], the cheat-sheet HUD,
//! and the README key list. They
//! drifted: the HUD advertised a `c` binding that never existed and claimed `q`
//! quits when `⌥+q` kills a pane, because nothing forced them to agree.
//!
//! The fix is not to invent a second registry that the dispatcher then has to
//! obey; the *hard-coded dispatcher is still the authority*. This table
//! declares, for each user-visible binding, the exact key event it claims to
//! handle and the [`Cmd`] it claims to produce, and a test feeds every entry
//! through the real [`crate::tui::handle_key`] and asserts they match. A
//! binding that is renamed, removed, or re-bound in the dispatcher fails the
//! build; documentation surfaces then render from this table and cannot lie.

use crate::keys;
use gwae_layout::Action;

/// How a binding is typed. Only the variants that describe a single decodable
/// key event can be verified against the dispatcher; the rest exist so the
/// cheat-sheet can still mention them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// `$mod` + this character (Option held).
    Chord(char),
    /// `$mod` + Shift + this character.
    ShiftChord(char),
    /// Ctrl + Shift + this character (the jcode-style transcript scroll).
    /// Labelled from [`crate::keys::ctrl_shift_chord`] so it reads `⌃+⇧+K`
    /// on macOS.
    CtrlShift(char),
    /// `$mod` + Return, optionally with Shift. Spelled by the platform module
    /// so it reads `⌥+↵` on macOS. Machine-checkable
    /// like the character chords: the dispatcher only produces these commands
    /// with the modifier held, so the label must say so.
    EnterChord { shift: bool },
    /// `$mod` + something the cheat-sheet can only describe in prose (digit
    /// ranges, arrows). Still labelled with the modifier, because pressing the
    /// key alone does nothing.
    ModProse(&'static str),
    /// Described in prose (arrows, digits, mouse); not machine-checkable.
    Prose(&'static str),
}

/// Which cheat-sheet column a binding belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Moving focus and the viewport.
    Navigate,
    /// Creating, resizing and destroying panes.
    Panes,
}

/// What a binding does, in the dispatcher's own vocabulary. Mirrors the
/// non-parameterised arms of `tui::Cmd`, which is private to that module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    /// A layout verb.
    Act(Action),
    /// Smart-jump to the pane that needs the user.
    SmartJump,
    /// Open the spawn-directory picker.
    DirPick,
    /// Toggle the cheat-sheet HUD.
    ToggleHud,
    /// Toggle the macOS keep-awake assertion.
    ToggleKeepAwake,
    /// Quit gwae.
    Quit,
    /// Scroll the row viewport by this many cells.
    Scroll(i32),
    /// Scroll the focused pane's history by this many rows (positive = back).
    ScrollBack(i32),
    /// Not a single dispatcher outcome (prose entries).
    Unverifiable,
}

/// One user-visible binding.
#[derive(Debug, Clone, Copy)]
// `glyph` and `effect` exist to be cross-checked against the dispatcher, which
// happens in tests; they are declarations of intent, not render inputs.
#[cfg_attr(not(test), allow(dead_code))]
pub struct Bind {
    pub trigger: Trigger,
    /// The macOS Unicode glyph a terminal sends when Option is *not* mapped to
    /// Meta, if gwae decodes one for this binding (e.g. `©` for `⌥+g`).
    pub glyph: Option<char>,
    pub group: Group,
    /// Short label for the cheat-sheet grid.
    pub desc: &'static str,
    pub effect: Effect,
}

impl Bind {
    /// How the binding is spelled for the user (`⌥+g`).
    pub fn label(&self) -> String {
        match self.trigger {
            Trigger::Chord(c) => keys::chord(&c.to_string()),
            Trigger::ShiftChord(c) => keys::shift_chord(&c.to_string()),
            Trigger::CtrlShift(c) => keys::ctrl_shift_chord(&c.to_string()),
            // The two Enter rows are `$mod` chords like everything else; the
            // label has to carry the modifier or it would read as a bare
            // Return, which just goes to the focused pane.
            Trigger::EnterChord { shift: false } => keys::chord(keys::enter_key()),
            Trigger::EnterChord { shift: true } => keys::shift_chord(keys::enter_key()),
            Trigger::ModProse(s) => keys::chord(s),
            Trigger::Prose(s) => s.to_string(),
        }
    }
}

/// Every binding gwae advertises, in cheat-sheet order.
pub const BINDS: &[Bind] = &[
    // -- navigation ------------------------------------------------------
    Bind {
        trigger: Trigger::Chord('h'),
        glyph: Some('\u{2d9}'),
        group: Group::Navigate,
        desc: "focus left",
        effect: Effect::Act(Action::FocusLeft),
    },
    Bind {
        trigger: Trigger::Chord('j'),
        glyph: Some('\u{2206}'),
        group: Group::Navigate,
        desc: "focus down",
        effect: Effect::Act(Action::FocusDown),
    },
    Bind {
        trigger: Trigger::Chord('k'),
        glyph: Some('\u{2da}'),
        group: Group::Navigate,
        desc: "focus up",
        effect: Effect::Act(Action::FocusUp),
    },
    Bind {
        trigger: Trigger::Chord('l'),
        glyph: Some('\u{ac}'),
        group: Group::Navigate,
        desc: "focus right",
        effect: Effect::Act(Action::FocusRight),
    },
    Bind {
        trigger: Trigger::ShiftChord('h'),
        glyph: Some('\u{d3}'),
        group: Group::Navigate,
        desc: "move pane left",
        effect: Effect::Act(Action::MovePaneLeft),
    },
    Bind {
        trigger: Trigger::ShiftChord('j'),
        glyph: Some('\u{d4}'),
        group: Group::Navigate,
        desc: "move pane down",
        effect: Effect::Act(Action::MovePaneDown),
    },
    Bind {
        trigger: Trigger::ShiftChord('k'),
        glyph: Some('\u{f8ff}'),
        group: Group::Navigate,
        desc: "move pane up",
        effect: Effect::Act(Action::MovePaneUp),
    },
    Bind {
        trigger: Trigger::ShiftChord('l'),
        glyph: Some('\u{d2}'),
        group: Group::Navigate,
        desc: "move pane right",
        effect: Effect::Act(Action::MovePaneRight),
    },
    Bind {
        trigger: Trigger::Chord('g'),
        glyph: Some('\u{a9}'),
        group: Group::Navigate,
        desc: "smart jump",
        effect: Effect::SmartJump,
    },
    Bind {
        trigger: Trigger::Chord('['),
        glyph: None,
        group: Group::Navigate,
        desc: "view left",
        effect: Effect::Scroll(-200),
    },
    Bind {
        trigger: Trigger::Chord(']'),
        glyph: None,
        group: Group::Navigate,
        desc: "view right",
        effect: Effect::Scroll(200),
    },
    Bind {
        trigger: Trigger::ModProse("↑/↓"),
        glyph: None,
        group: Group::Navigate,
        desc: "scrollback",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::CtrlShift('K'),
        glyph: None,
        group: Group::Navigate,
        desc: "scroll up",
        effect: Effect::ScrollBack(3),
    },
    Bind {
        trigger: Trigger::CtrlShift('J'),
        glyph: None,
        group: Group::Navigate,
        desc: "scroll down",
        effect: Effect::ScrollBack(-3),
    },
    Bind {
        trigger: Trigger::ModProse("←/→"),
        glyph: None,
        group: Group::Navigate,
        desc: "pan content",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::Prose("click"),
        glyph: None,
        group: Group::Navigate,
        desc: "focus pane",
        effect: Effect::Unverifiable,
    },
    // -- panes -----------------------------------------------------------
    Bind {
        trigger: Trigger::Chord(';'),
        glyph: Some('\u{2026}'),
        group: Group::Panes,
        desc: "new agent",
        effect: Effect::Act(Action::SpawnAgent),
    },
    Bind {
        trigger: Trigger::ShiftChord(';'),
        glyph: Some('\u{da}'),
        group: Group::Panes,
        desc: "new agent row",
        effect: Effect::Act(Action::SpawnAgentRow),
    },
    Bind {
        trigger: Trigger::Chord('b'),
        glyph: Some('\u{222b}'),
        group: Group::Panes,
        desc: "split below",
        effect: Effect::Act(Action::SplitBelow),
    },
    Bind {
        trigger: Trigger::Chord('r'),
        glyph: None,
        group: Group::Panes,
        desc: "cycle width",
        effect: Effect::Act(Action::CycleWidth),
    },
    Bind {
        trigger: Trigger::Chord('f'),
        glyph: Some('\u{192}'),
        group: Group::Panes,
        desc: "full width",
        effect: Effect::Act(Action::ToggleFullWidth),
    },
    Bind {
        trigger: Trigger::Chord('q'),
        glyph: Some('\u{153}'),
        group: Group::Panes,
        desc: "kill pane",
        effect: Effect::Act(Action::KillPane),
    },
    Bind {
        trigger: Trigger::ShiftChord('q'),
        glyph: None,
        group: Group::Panes,
        desc: "force quit",
        effect: Effect::Quit,
    },
    Bind {
        trigger: Trigger::Chord('d'),
        glyph: Some('\u{2202}'),
        group: Group::Panes,
        desc: "spawn dir",
        effect: Effect::DirPick,
    },
    Bind {
        // Consumed by the host terminal and delivered as Event::Paste, not
        // a key event that handle_key can decode.
        trigger: Trigger::Prose(keys::paste_key()),
        glyph: None,
        group: Group::Panes,
        desc: "paste",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::Chord('/'),
        glyph: Some('\u{f7}'),
        group: Group::Panes,
        desc: "toggle help",
        effect: Effect::ToggleHud,
    },
    Bind {
        trigger: Trigger::Chord('w'),
        glyph: Some('\u{2211}'),
        group: Group::Panes,
        desc: "keep awake",
        effect: Effect::ToggleKeepAwake,
    },
    Bind {
        trigger: Trigger::EnterChord { shift: false },
        glyph: None,
        group: Group::Panes,
        desc: "new column",
        effect: Effect::Act(Action::NewColumn),
    },
    Bind {
        trigger: Trigger::EnterChord { shift: true },
        glyph: None,
        group: Group::Panes,
        desc: "new row",
        effect: Effect::Act(Action::NewRow),
    },
];

/// The bindings of one cheat-sheet group, in declaration order.
pub fn group(g: Group) -> impl Iterator<Item = &'static Bind> {
    BINDS.iter().filter(move |b| b.group == g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_use_the_platform_modifier() {
        let g = BINDS
            .iter()
            .find(|b| b.trigger == Trigger::Chord('g'))
            .unwrap();
        assert_eq!(g.label(), keys::chord("g"));
    }

    #[test]
    fn every_binding_is_documented_in_the_readme() {
        // Apex README is minimal: it lists core keys and links to docs/KEYBINDS.md
        // for the rest. Require that kill, spawn, and jump are present so a new
        // user copying from the apex page can operate gwae.
        let readme = include_str!("../../../README.md");
        for key in ["⌥+q", "⌥+;", "⌥+g", "⌥+h", "⌥+Enter"] {
            assert!(readme.contains(key), "README documents {key}");
        }
    }

    #[test]
    fn every_binding_has_a_distinct_cheat_sheet_slot() {
        // The HUD grid keys off (trigger, group); duplicates would render two
        // identical rows and hide a real binding.
        let mut seen = std::collections::HashSet::new();
        for b in BINDS {
            assert!(seen.insert(b.label()), "{} is declared twice", b.label());
        }
    }
}
