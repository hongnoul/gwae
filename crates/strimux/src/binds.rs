//! The single source of truth for strimux keybindings.
//!
//! Before this module existed the same bindings were spelled out in four
//! places: the `handle_key` match in [`crate::tui`], the cheat-sheet HUD, the
//! default cowsay hints in [`crate::config`], and the README table. They drifted
//! — the HUD advertised a `c` binding that never existed and claimed `q` quits
//! when `⌥+q` kills a pane — because nothing forced them to agree.
//!
//! The fix is not to invent a second registry that the dispatcher then has to
//! obey; the *hard-coded dispatcher is still the authority*. This table
//! declares, for each user-visible binding, the exact key event it claims to
//! handle and the [`Cmd`] it claims to produce, and a test feeds every entry
//! through the real [`crate::tui::handle_key`] and asserts they match. A
//! binding that is renamed, removed, or re-bound in the dispatcher fails the
//! build; documentation surfaces then render from this table and cannot lie.

use crate::keys;
use strimux_layout::Action;

/// How a binding is typed. Only the variants that describe a single decodable
/// key event can be verified against the dispatcher; the rest exist so the
/// cheat-sheet can still mention them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// `$mod` + this character (Option/Alt held).
    Chord(char),
    /// `$mod` + Shift + this character.
    ShiftChord(char),
    /// `$mod` + Return, optionally with Shift. Spelled by the platform module
    /// so it reads `⌥+↵` on macOS and `Alt+Enter` elsewhere. Machine-checkable
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
    /// Open the theme picker.
    ThemePick,
    /// Toggle the cheat-sheet HUD.
    ToggleHud,
    /// Quit strimux.
    Quit,
    /// Scroll the row viewport by this many cells.
    Scroll(i32),
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
    /// Meta, if strimux decodes one for this binding (e.g. `©` for `⌥+g`).
    pub glyph: Option<char>,
    pub group: Group,
    /// Short label for the cheat-sheet grid.
    pub desc: &'static str,
    /// The binding as one line of natural language, used verbatim by the
    /// cowsay hints in empty placeholder boxes. Mandatory: every binding is
    /// bijective with exactly one hint, so adding a keybinding necessarily
    /// adds its cow hint and the helper can never fall behind the
    /// dispatcher. Phrased to read after [`Bind::label`], e.g.
    /// "⌥+s splits this column".
    pub hint: &'static str,
    pub effect: Effect,
}

impl Bind {
    /// How the binding is spelled for the user, platform-aware (`⌥+g` on
    /// macOS, `Alt+g` elsewhere).
    pub fn label(&self) -> String {
        match self.trigger {
            Trigger::Chord(c) => keys::chord(&c.to_string()),
            Trigger::ShiftChord(c) => keys::shift_chord(&c.to_string()),
            // The two Enter rows are `$mod` chords like everything else; the
            // label has to carry the modifier or the cow tells the user to
            // press a bare Return, which just goes to the focused pane.
            Trigger::EnterChord { shift: false } => keys::chord(keys::enter_key()),
            Trigger::EnterChord { shift: true } => keys::shift_chord(keys::enter_key()),
            Trigger::ModProse(s) => keys::chord(s),
            Trigger::Prose(s) => s.to_string(),
        }
    }
}

/// Every binding strimux advertises, in cheat-sheet order.
pub const BINDS: &[Bind] = &[
    // -- navigation ------------------------------------------------------
    Bind {
        trigger: Trigger::Chord('h'),
        hint: "moves focus left a column",
        glyph: Some('\u{2d9}'),
        group: Group::Navigate,
        desc: "focus left",
        effect: Effect::Act(Action::FocusLeft),
    },
    Bind {
        trigger: Trigger::Chord('j'),
        hint: "moves focus down a pane",
        glyph: Some('\u{2206}'),
        group: Group::Navigate,
        desc: "focus down",
        effect: Effect::Act(Action::FocusDown),
    },
    Bind {
        trigger: Trigger::Chord('k'),
        hint: "moves focus up a pane",
        glyph: Some('\u{2da}'),
        group: Group::Navigate,
        desc: "focus up",
        effect: Effect::Act(Action::FocusUp),
    },
    Bind {
        trigger: Trigger::Chord('l'),
        hint: "moves focus right a column",
        glyph: Some('\u{ac}'),
        group: Group::Navigate,
        desc: "focus right",
        effect: Effect::Act(Action::FocusRight),
    },
    Bind {
        trigger: Trigger::ShiftChord('h'),
        hint: "carries this pane left",
        glyph: Some('\u{d3}'),
        group: Group::Navigate,
        desc: "move pane left",
        effect: Effect::Act(Action::MovePaneLeft),
    },
    Bind {
        trigger: Trigger::ShiftChord('j'),
        hint: "carries this pane down the stack",
        glyph: Some('\u{d4}'),
        group: Group::Navigate,
        desc: "move pane down",
        effect: Effect::Act(Action::MovePaneDown),
    },
    Bind {
        trigger: Trigger::ShiftChord('k'),
        hint: "carries this pane up the stack",
        glyph: Some('\u{f8ff}'),
        group: Group::Navigate,
        desc: "move pane up",
        effect: Effect::Act(Action::MovePaneUp),
    },
    Bind {
        trigger: Trigger::ShiftChord('l'),
        hint: "carries this pane right",
        glyph: Some('\u{d2}'),
        group: Group::Navigate,
        desc: "move pane right",
        effect: Effect::Act(Action::MovePaneRight),
    },
    Bind {
        trigger: Trigger::Chord('g'),
        hint: "jumps to the pane that needs you",
        glyph: Some('\u{a9}'),
        group: Group::Navigate,
        desc: "smart jump",
        effect: Effect::SmartJump,
    },
    Bind {
        trigger: Trigger::Chord('['),
        hint: "scrolls the strip left",
        glyph: None,
        group: Group::Navigate,
        desc: "view left",
        effect: Effect::Scroll(-200),
    },
    Bind {
        trigger: Trigger::Chord(']'),
        hint: "scrolls the strip right",
        glyph: None,
        group: Group::Navigate,
        desc: "view right",
        effect: Effect::Scroll(200),
    },
    Bind {
        trigger: Trigger::ModProse("1-9"),
        hint: "jumps straight to a column",
        glyph: None,
        group: Group::Navigate,
        desc: "jump column",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::ModProse("←/→"),
        hint: "pans wide content sideways",
        glyph: None,
        group: Group::Navigate,
        desc: "pan content",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::Prose("click"),
        hint: "focuses the pane you click",
        glyph: None,
        group: Group::Navigate,
        desc: "focus pane",
        effect: Effect::Unverifiable,
    },
    Bind {
        trigger: Trigger::Prose("wheel"),
        hint: "scrolls this pane's history",
        glyph: None,
        group: Group::Navigate,
        desc: "scrollback",
        effect: Effect::Unverifiable,
    },
    // -- panes -----------------------------------------------------------
    Bind {
        trigger: Trigger::Chord('a'),
        hint: "opens a column here",
        glyph: None,
        group: Group::Panes,
        desc: "new column",
        effect: Effect::Act(Action::NewColumn),
    },
    Bind {
        trigger: Trigger::Chord(';'),
        hint: "spawns an agent",
        glyph: Some('\u{2026}'),
        group: Group::Panes,
        desc: "new agent",
        effect: Effect::Act(Action::SpawnAgent),
    },
    Bind {
        trigger: Trigger::Chord('s'),
        hint: "splits this column",
        glyph: None,
        group: Group::Panes,
        desc: "split below",
        effect: Effect::Act(Action::SplitBelow),
    },
    Bind {
        trigger: Trigger::Chord('r'),
        hint: "cycles this column's width",
        glyph: None,
        group: Group::Panes,
        desc: "cycle width",
        effect: Effect::Act(Action::CycleWidth),
    },
    Bind {
        trigger: Trigger::Chord('f'),
        hint: "toggles full width",
        glyph: Some('\u{192}'),
        group: Group::Panes,
        desc: "full width",
        effect: Effect::Act(Action::ToggleFullWidth),
    },
    Bind {
        trigger: Trigger::Chord('x'),
        hint: "kills the focused pane",
        glyph: None,
        group: Group::Panes,
        desc: "kill pane",
        effect: Effect::Act(Action::KillPane),
    },
    Bind {
        trigger: Trigger::Chord('q'),
        hint: "kills the focused pane too",
        glyph: Some('\u{153}'),
        group: Group::Panes,
        desc: "kill pane",
        effect: Effect::Act(Action::KillPane),
    },
    Bind {
        trigger: Trigger::ShiftChord('q'),
        hint: "force-quits after a confirmation",
        glyph: None,
        group: Group::Panes,
        desc: "force quit",
        effect: Effect::Quit,
    },
    Bind {
        trigger: Trigger::Chord('t'),
        hint: "previews themes",
        glyph: Some('\u{2020}'),
        group: Group::Panes,
        desc: "theme picker",
        effect: Effect::ThemePick,
    },
    Bind {
        trigger: Trigger::Chord('/'),
        hint: "toggles this cheat-sheet",
        glyph: Some('\u{f7}'),
        group: Group::Panes,
        desc: "toggle help",
        effect: Effect::ToggleHud,
    },
    Bind {
        trigger: Trigger::EnterChord { shift: false },
        hint: "opens a column here as well",
        glyph: None,
        group: Group::Panes,
        desc: "new column",
        effect: Effect::Act(Action::NewColumn),
    },
    Bind {
        trigger: Trigger::EnterChord { shift: true },
        hint: "starts a new strip below",
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

/// Every binding as one line of natural language, in declaration order, with
/// the cheat-sheet toggle hoisted to the front.
///
/// This is exactly `BINDS.len()` strings: the mapping is bijective by
/// construction, because [`Bind::hint`] is a required field. Adding a
/// keybinding therefore adds its cow hint automatically, and there is no way
/// to ship a binding the cow does not know how to explain.
///
/// Index `0` is special: [`crate::cowsay::message_for`] pins it to the first
/// empty box on screen, so the one hint guaranteed to be read is the one that
/// opens the full cheat-sheet. Everything else is a bonus the user discovers
/// while glancing around the skeleton.
pub fn cowsay_hints() -> Vec<String> {
    let render = |b: &Bind| format!("{} {}", b.label(), b.hint);
    let pinned = BINDS
        .iter()
        .find(|b| b.effect == Effect::ToggleHud)
        .expect("a binding opens the cheat-sheet");
    std::iter::once(render(pinned))
        .chain(
            BINDS
                .iter()
                .filter(|b| b.effect != Effect::ToggleHud)
                .map(render),
        )
        .collect()
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
        // The README keybinding table is a doc surface like any other; a
        // binding that is not listed there is undiscoverable.
        let readme = include_str!("../../../README.md");
        for b in BINDS {
            let key = match b.trigger {
                Trigger::Chord(c) => c.to_string(),
                Trigger::ShiftChord(c) => c.to_string(),
                Trigger::EnterChord { .. } => "Enter".to_string(),
                Trigger::ModProse(_) | Trigger::Prose(_) => continue,
            };
            let mac = format!("⌥+{key}");
            assert!(readme.contains(&mac), "README documents {mac} ({})", b.desc);
        }
    }

    #[test]
    fn hints_are_bijective_with_bindings() {
        // The property this module exists to guarantee: exactly one hint per
        // binding, no duplicates, none empty. A new binding cannot compile
        // without a hint (the field is required), and this catches the other
        // failure mode: copy-pasting an existing hint onto a new key.
        let hints = cowsay_hints();
        assert_eq!(
            hints.len(),
            BINDS.len(),
            "one hint per binding, got {hints:?}"
        );
        let mut seen = std::collections::HashSet::new();
        for b in BINDS {
            assert!(!b.hint.is_empty(), "{} has an empty hint", b.label());
            assert!(
                !b.hint.ends_with('.'),
                "{}: hints are phrases, not sentences: {:?}",
                b.label(),
                b.hint
            );
            assert!(
                seen.insert(b.hint),
                "{} reuses the hint {:?}; every binding needs its own",
                b.label(),
                b.hint
            );
        }
        // Each rendered hint starts with some binding's label, so the cow
        // always tells the user which keys to press.
        for h in &hints {
            assert!(
                BINDS.iter().any(|b| h.starts_with(&b.label())),
                "hint {h:?} should lead with a key label"
            );
        }
        // The pinned slot must be the cheat-sheet toggle: it is the only hint
        // guaranteed a visible box, so it has to be the one that opens the
        // full list.
        let toggle = BINDS
            .iter()
            .find(|b| b.effect == Effect::ToggleHud)
            .unwrap();
        assert_eq!(
            hints[0],
            format!("{} {}", toggle.label(), toggle.hint),
            "the first empty box must advertise the cheat-sheet"
        );
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
