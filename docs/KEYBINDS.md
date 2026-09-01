# Configurable keybindings — strategy

Status: design, not yet implemented. Written against `main` at the time the
deliberately shaped to *not* touch `handle_key`'s behavior.

## Where we are today

Three places know about keys, and one of them is the boss:

| File | Role today |
|---|---|
| `crates/gwae/src/tui.rs` `handle_key` (~line 2594) | **the authority**. A hand-written match: modifier tests, a US-layout Option-glyph fallback, then per-character arms producing a `Cmd`. |
| `crates/gwae/src/binds.rs` | a *declarative mirror* of the above. Each `Bind` carries the trigger, the macOS glyph, a group, a cheat-sheet `desc` and a one-line `hint`. |
| `crates/gwae/src/keys.rs` | platform naming only (`⌥` vs `Alt`), so labels read right on both. |

`binds.rs` is not a registry the dispatcher obeys; it is a claim the dispatcher
is tested against (`tui.rs::advertised_bindings_match_the_dispatcher`). The
cheat-sheet HUD, the cowsay hints in empty boxes, and (by another test) the
README table all render from `BINDS`, so documentation cannot drift from code.
That property is the most valuable thing in this area and **any keybinding
config that breaks it is a regression**, however configurable it is.

Two other facts constrain the design:

* **The Option-glyph fallback is per-binding, by hand.** `⌥+g` also matches `©`
  because someone wrote `glyph: Some('\u{a9}')`. A binding added without a glyph
  silently does nothing on terminals that don't send Option as Meta. This is
  already a latent bug generator; with user-chosen keys it becomes unusable,
  because nobody will look up that `⌥+w` is `∑`.
* **Unbound keys belong to the pane.** `handle_key` falls through to
  `Cmd::Input(key_bytes(ev))`. Chords gwae claims are chords the child cannot
  have — there is already a carve-out at tui.rs:2678 so `⌥+Shift+s` reaches
  jcode. "Give me my key back" is the single most likely reason a user wants
  this feature, and it must be a first-class outcome, not a side effect.

## What "configurable" has to mean here

1. **Rebind**: `⌥+w` should kill a pane if I say so.
   see above.
3. **Defaults still work with an empty config**, and an old config keeps
   behaving identically. No flag day.
4. **Docs stay generated**: the HUD, the cow, and `gwae doctor` describe *my*
   keymap, not the shipped one.
5. **Glyph fallback keeps working for keys I chose**, without me knowing what
   `∑` is.
6. **A broken `[keys]` table degrades, loudly.** Config already survives garbage
   by warning and falling back; the keyboard must not be the exception that
   bricks a session.

## The design

### 1. One normalization funnel: `KeyEvent -> Chord`

Introduce `keys::Chord { mods: Mods, key: Key }` and `Chord::from_event(&KeyEvent)`.
It absorbs the three normalizations that are today scattered through
`handle_key`:

* `physical_shift` (Kitty reports shifted keys as the shifted codepoint with
  SHIFT cleared; Caps Lock must not count) — tui.rs:87;
* `logical_char` case folding — tui.rs:103;
* **the US-layout Option-glyph table**, promoted from per-binding data to one
  `glyph -> (base_char, shift)` map: `˙ -> (h, false)`, `Ó -> (h, true)`, `ç ->
  (c, false)`, and so on for the whole layout.

That last move is the crux of the whole feature. Once the glyph map is a
*layout* fact rather than a *binding* fact, any key the user picks gets the
fallback for free, and the map is testable on its own: for every ASCII key `k`,
`Chord::from_event(glyph_event(option_glyph(k))) == Chord::mod_(k)`.

Two honest caveats to document rather than paper over:

* **Dead keys.** `⌥+e ⌥+i ⌥+n ⌥+u` (and backtick) emit nothing until the next
  keystroke on macOS. They are unusable without Option-as-Meta, so binding one
  earns a startup warning naming the terminal setting that fixes it.
* **Non-US layouts.** The table is US, as today. The glyph path only fires for
  glyphs whose base key is actually bound, so on other layouts an unbound glyph
  types through to the pane instead of being eaten — strictly better than the
  current unconditional match arms.

`Chord` also gets `FromStr` and `Display`, sharing `keys.rs`'s platform naming
so `Chord::mod_('h').to_string()` is `⌥+h` on macOS and `Alt+h` elsewhere. That
is what the HUD and the cow print.

### 2. Keymap as data

```rust
pub struct Keymap {
    order: Vec<(Chord, Command)>,     // declaration order, for display
    lookup: HashMap<Chord, Command>,  // dispatch
}
```

`handle_key` becomes: normalize to a `Chord`, look it up, and on a miss do
exactly what it does today (arrows/scrollback/pane input). The parameterized
and stateful commands (`JumpDigit`, `ScrollBack`, `Input`) stay in code; the
keymap covers the named verbs.

Lookup should be written as `lookup(&[Chord]) -> Match::{Exact, Prefix, None}`
even though v1 only ever passes one chord. A tmux-style prefix key
(`C-b` then a key) is the next request after this one, and shaping the lookup
for sequences now costs one enum and saves a redesign later. Do not implement
sequences yet.

### 3. The config surface

```toml
[keys]
"mod+w"       = "kill-pane"    # rebind
"mod+q"       = "none"         # unbind: ⌥+q now reaches the pane
"mod+shift+h" = "move-pane-left"
```

Decisions, each with its reason:

* **`mod` is the portable spelling** of Option/Alt, matching the `$mod` story in
  ARCHITECTURE.md. `alt+h` and `⌥+h` parse too, and all three normalize to one
  `Chord`, so a config is portable across machines.
* **Merge, don't replace.** User entries overlay the defaults. Replacement would
  mean every gwae release that adds a verb is invisible to anyone with a
  `[keys]` table. `keys_clear = true` is the escape hatch for people who want a
  blank slate; it is one line and it stops the "I want *only* my binds" issue.
* **`"none"` unbinds.** Requirement 2, spelled the obvious way.
* **Unknown command names warn and are skipped**, matching how an unknown theme
  behaves (`Config::palette_checked`). `gwae doctor` reports them by name with
  the list of valid verbs, because a warning on stderr under a TUI is a warning
  nobody reads.
* **One file.** `gwae.toml`, hand-edit surface, not a `gwae init` question:
  CONFIG.md already splits "what setup asks" from "what you edit", and keys are
  firmly the latter.
* **Live reload: yes, but validated.** Keys are read every keystroke, so
  `adopt_appearance` can adopt them like colors. A `[keys]` table that fails to
  parse must leave the *running* keymap alone and toast the error — the existing
  reload-error toast path already does this for the whole file.

### 4. Metadata moves from the binding to the command

This is the part that keeps the anti-drift property alive. Today `Bind` carries
`desc`/`hint`/`group` because binding and command are 1:1 forever. Once keys are
configurable, "kills the focused pane" is a fact about `KillPane`, not about
`⌥+q`. So:

```rust
struct CommandInfo { cmd: Command, group: Group, desc: &'static str, hint: &'static str }
```

The cow hint becomes `format!("{} {}", chord, info.hint)` over the *resolved*
keymap. `cowsay_hints()` keeps its signature but takes a `&Keymap`. The
bijectivity test moves from "one hint per binding" to "one hint per command",
which is stronger: a new verb cannot ship without a description, and a rebound
verb explains itself with the user's own key.

The README test narrows to the default keymap, which is all it ever meant.

### 5. Modal keys stay fixed, on purpose

The theme picker, the spawn-dir picker, the quit confirmation and the paste
confirmation read keys directly in `run_tui` (arrows, `hjkl`, Enter, Esc). They
stay hard-coded in v1, because they are transient overlays that print their own
legend on screen and their keys are not contested with panes. The one
inconsistency worth fixing is the dir picker's save key (`⌥+s`, tui.rs:3447):
it should render and match whatever chord `split-below` — no, whatever chord
the *save* verb — resolves to, or it lies the moment someone rebinds. Cheapest
correct move: give it its own named command in the keymap and look it up.

## Phasing

| Phase | Scope | User-visible | Est. |
|---|---|---|---|
| **P0** | `Chord` + glyph table + `handle_key` walks a `Keymap` built from the existing defaults | none (pure refactor) | ~1 day |
| **P1** | `[keys]` parsing, merge, `"none"`, live reload | rebind + unbind | ~half day |
| **P2** | metadata moves to commands; HUD/cow/README render from resolved keymap | correct docs after rebinding | ~half day |
| **P3** | `gwae keys` subcommand (print effective keymap, `--check`), doctor integration, CONFIG.md table | discoverability | ~half day |
| later | chord sequences / prefix key; per-mode keymaps; `--keys` profile switching | — | — |

P0 lands alone and is worth landing even if the rest is never built: it deletes
the per-binding glyph field, which is a live bug source.

**Ordering note.** P0 rewrites `handle_key`, which the in-flight copy/paste work
also edits. Land that first, or P0 will be a merge conflict wearing a refactor's
clothes.

## Risks and rejected alternatives

* **Rejected: a separate `keys.toml`.** Two files to find, two mtimes to watch,
  no benefit at this size.
* **Rejected: replace-not-merge semantics.** See above; it makes upgrades
  invisible.
* **Rejected: making modal/picker keys configurable in v1.** Multiplies the
  surface for a case nobody has asked for, and those keys are self-documenting.
* **Rejected: keeping the glyph table per binding.** It is the current design and
  it cannot survive user-chosen keys.
* **Perf**: one `HashMap` probe per keystroke, against a PTY write. Immaterial.
* **Footgun — a user unbinds everything.** Do not paternalize (unbinding is the
  point), but make recovery obvious: `gwae keys` prints the effective map from
  outside the TUI, and the startup HUD is what it is because `toggle-hud` is
  still bound in the defaults the user merged onto.
* **Footgun — a user binds a chord panes want.** Warn at load for the known-hot
  ones, and note in CONFIG.md that unbinding is how you give a key back.

## Test plan

The existing tests are the specification; they get generalized, not replaced.

1. `Chord::from_event` round-trip over the whole glyph table (property test).
2. Caps-Lock-vs-Shift and Kitty-shifted-codepoint cases, ported from
   `caps_lock_does_not_trigger_shift_chords` and its neighbours.
3. `advertised_bindings_match_the_dispatcher`, re-aimed: for **any** keymap,
   every entry dispatches to its command via both the Meta path and the glyph
   path.
4. Config: `[keys]` merge, `"none"` unbind reaches the pane as `Cmd::Input`,
   unknown verb warns and keeps the default, malformed table leaves the running
   keymap intact.
5. e2e (`tests/keys_e2e.rs`, modeled on `picker_e2e.rs`): real binary, real PTY,
   a config that rebinds kill-pane to `⌥+w` and unbinds `⌥+q`; assert `⌥+w`
   kills and `⌥+q` arrives at the child.
6. Docs: cow hints and the HUD render the *rebound* chord, asserted against a
   non-default keymap.