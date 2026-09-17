# Configuration

Written for you by `gwae setup` (the guided first-run setup; `gwae init`
is an alias). Everything below can equally be hand-edited, and
gwae live-reloads the config file while it runs.

Location: `$XDG_CONFIG_HOME/gwae/gwae.toml` (default
`~/.config/gwae/gwae.toml`). TOML (ADR-008). All keys optional; missing
keys fall back to the defaults below.

Example:

```toml
default_column_width = "half"     # or "quarter", "two-thirds", "full", or 80 (cells)
default_agent = ""                # normally empty: ⌥+; remembers your pick itself
agent_dir = "~/git/gwae"          # directory new panes start in ("" = gwae's cwd)
input_poll_ms = 1            # event-loop poll; 30ms backoff once the screen is quiet
keep_awake = true            # macOS only: hold idle/display sleep via caffeinate (false lets it sleep)

[minimap]
show = true
mode = "off"
max_width = 32
max_rows = 6
show_counts = true

[update]
check = true                 # daily "a new gwae is out" notice; false = silent
source = ""                  # "" detects; or pin: brew, install.sh, cargo,
                             # cargo-git, source, nix, system (brew is canonical)
```

## Responsive Yazi panels

Yazi owns its internal panel layout. The optional
[`gwae-responsive` plugin](../examples/yazi/) hides its parent and preview
panels below 64 columns, reveals the preview at 64, and restores the full
layout at 96. It follows pane resizes automatically and leaves standalone
Yazi unchanged. Panes wrap at the visible width, so Yazi sees the pane width.

## Staying up to date (`[update]`)

Homebrew is the canonical install: `brew install hongnoul/tap/gwae`, upgraded
with `brew upgrade gwae`. gwae upgrades **the way it was installed, or not at
all**: `gwae upgrade` prints the exact command for this machine and runs it
for routes it owns, and only *prints* the command for routes another package
manager owns (a checkout you built yourself, or a legacy Nix / distro install).

`check = true` asks GitHub once a day whether a newer release exists and shows a
one-line notice naming the exact command for your machine. The request is an
unauthenticated `HEAD` of the `releases/latest` redirect and carries nothing
about you, your machine, or your version. `GWAE_NO_UPDATE_CHECK=1` turns it off
from the environment, which beats this key.

`source` is only needed when detection is wrong or cannot tell — `~/.local/bin`
and `/usr/local/bin` are genuinely ambiguous. `gwae doctor` prints what it
decided and whether that came from your config, the installer's receipt, or the
binary's path. An unrecognized value is reported by `doctor` and ignored.
Full reasoning: [`UPDATES.md`](UPDATES.md).

## What `gwae setup` covers

`gwae setup` audits its stages (config file, agent, updates,
spawn dir, latency) and writes only gwae's own config file.

## Colors

gwae paints its own retro chrome: true-black panels with high-contrast
functional colors (cyan focus, blue running, amber idle, green done, red
failed, white text). The chrome reads the same whatever the host terminal
is themed as.

Hand-edit escape hatch, no picker: any key can be overridden under
`[theme]`, and a save repaints the running session.

```toml
[theme]
accent = "#ff00ff"  # RGB hex (with or without the #)
running = 12        # 256-color index
text = "default"    # the terminal's own color for this key
```

Keys: `base`, `surface`, `overlay`, `accent`, `text`, `label`, `running`,
`idle`, `done`, `failed`. Unset keys keep the retro default. A retired
`theme = "name"` preset string (or a `preset` key inside the table) parses
as "no overrides". Any other unknown key is a parse error, so a typo fails
loudly instead of painting a silently wrong chrome.

## Keeping the Mac awake (`keep_awake`)

gwae is a single process with no daemon: when macOS sleeps, every pane (and
every agent in it) freezes until wake. `keep_awake = true` holds a
`caffeinate` assertion for gwae's own lifetime, so idle and display sleep
never pause a session you walked away from. macOS-only; elsewhere the key
does nothing. Default `true`: agents keep working unless you opt out with
`keep_awake = false` or `⌥+w`. Set `GWAE_NO_KEEP_AWAKE=1` to force it off
(scripted setups, tests). Editing the key applies live to the running
session, with a one-line toast confirming the change.

`⌥+w` toggles it mid-session and writes the choice back to the config, so
the keypress survives a restart. While the assertion is held a small
`keep-awake` badge is stamped on the Option HUD frame —
hold Option to see it. The palette itself is never touched, so toggling
off restores the chrome exactly.

Honest limit: this does **not** defeat lid-close sleep. A closed lid still
sleeps the machine unless it is in clamshell mode (power + external display
+ external input) or sleep is disabled outright (`sudo pmset disablesleep
1`). What it buys is the common case: lid open, display asleep, agents
still running in the morning. `gwae doctor` reports the effective state.

Everything else here is hand-edit only, deliberately:

* `input_poll_ms` defaults to `1`, so there is nothing to set up: settings that only
  you can change (kitty, macOS) are reported once on the summary screen.
* `[minimap]` geometry is a niche taste; a
  setup flow long enough to cover it is one nobody finishes.
* `btm` is not a config key at all - it is an action on the machine, so it is
  never written to this file. On macOS a yes installs Homebrew first if it is
  missing. Set `GWAE_NO_INSTALL=1` to turn the offer off entirely.

## Live reload

Saving the config file reloads the **running** session: there is no restart,
so every pane and every agent keeps going. The file is polled for changes a
few times a second, and a one-line toast confirms the reload along the bottom
of the screen.

`startup_panes` is consumed once at launch (the panes already exist), so
changing it still needs a restart. `default_agent` is an explicit override
read fresh each time `⌥+;` fires, so editing it applies to the *next* agent
pane without a restart; panes already running a harness keep running it. The
remembered pick lives in the state file, not the config, so picking a
harness never rewrites your config. That file is
`$XDG_STATE_HOME/gwae/harness.json` (`~/.local/state/gwae/harness.json` by
default): `last` is the pick `⌥+;` spawns with no UI, `mru` ranks the picker,
and `custom` remembers typed commands detection could never guess. Deleting
it costs one extra pick; nothing in it is hand-edited. Everything read every frame - `[minimap]`,
`[theme]`, scroll behavior - takes effect immediately. `keep_awake` also applies live:
flipping it starts or drops the `caffeinate` assertion at once, with the
change named in the toast.

A config that fails to parse mid-edit (an editor saving between keystrokes)
leaves the running settings alone and reports the error, rather than dropping
you back to defaults.

## Checking your config

A config file that fails to parse is **ignored entirely** (gwae falls back to
defaults rather than refusing to launch). Retired preset names (`theme =
"nord"`, `[theme] preset`) parse as "no overrides". A broken file is easy
to miss, so `doctor` reports it:

```sh
gwae doctor
```

```
gwae doctor:
  config: /home/you/.config/gwae/gwae.toml
  config file: parses [ok]
  agent: claude [ok]
  updates: brew (detected from path) · checks daily · latest is 1.0.1 · `gwae upgrade` -> brew upgrade gwae [ok]
  spawn dir: /home/you/git/foo [ok]
  keep-awake: on; caffeinate holds idle/display sleep while gwae runs (a closed lid still sleeps outside clamshell mode)
  onboarding: done [ok]
  latency: all layers tuned [ok]
  keyboard: kitty; Meta path expected [ok]
  focus: daemon loaded, socket live [ok]
  bindings: kitty; no per-terminal snippet installed yet
  layout smoke: columns 4 -> 5 on default row [ok]
```

Every line is produced by its owning setup stage, so doctor can never
disagree with the flow that acts on it. See `gwae setup --print` for the
stage list.

## Unified setup (`gwae setup`)

One flow owns every machine concern: harness, latency,
keyboard path, kitty focus repair, per-terminal snippets, companions, and
the update route. Each concern is an independent stage; adding or removing
one is one file plus one registry line, and doctor renders each stage's
own verdict.

```sh
gwae setup              # apply safe fixes, report the rest
gwae setup --check      # audit only; nonzero exit when anything needs work
gwae setup --yes        # apply without prompting (scripts, dotfiles)
gwae setup --only focus # run one stage by id
gwae setup --print      # show every stage's planned steps
```

Only gwae's own config is ever written silently. kitty.conf and
LaunchAgents print their diff first; macOS globals are printed as exact
commands and never applied. `GWAE_NO_INSTALL=1` disables all writes.

A config file that is not being applied at all points at the syntax error:

```
  config file: INVALID, so it is being ignored entirely: TOML parse error at line 2, column 6
```

## Keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_column_width` | width | `"quarter"` | Width of newly created columns. A preset name (`"quarter"`, `"third"`, `"half"`, `"two-thirds"`, `"three-quarters"`, `"full"`; separators and case are ignored, and `"1/2"` style also works), a bare integer for fixed cells (`80`), or the table forms `{ preset = "half" }` / `{ cells = 80 }`. |
| `default_agent` | string | `""` (unset) | Explicit harness override for `⌥+;` and the **first pane** at startup. Normally left empty: the first press offers what is installed (a lone install launches itself with no UI) and remembers your pick in the state file, so every later press goes straight there. Set it only to pin a harness in dotfiles or scripts; a value that is not on `PATH` falls back to the picker rather than a dead pane. `gwae run <cmd>` overrides the first pane. See `gwae agent --print`. |
| `startup_panes` | integer | `1` | Number of equal-width quarter panes on screen at first launch. Each pane keeps a fixed `1/4` share of the viewport regardless of this count, so a value below `4` leaves the right side of the screen empty (shown as skeleton placeholder boxes). The default `1` opens a single terminal in the leftmost quarter. |
| `input_poll_ms` | integer | `1` | Milliseconds the event loop waits for a keystroke before checking PTY output and repainting. gwae sits on the keystroke round trip twice (your key in, the program's echo out), so this costs roughly double. The loop backs off to 30ms once the screen has been quiet for 750ms, so an idle session stays cheap. Run `gwae setup --only latency` to check this and the macOS/terminal settings around it. Valid range 1..50. See `docs/LATENCY.md`. |
| `keep_awake` | bool | `true` | macOS-only: hold a `caffeinate` assertion (idle/display sleep) while gwae runs, so agents keep working with the display asleep. On unless you write `keep_awake = false` or toggle it off with `⌥+w`; never asked by setup. Does **not** defeat lid-close sleep outside clamshell mode (power + external display + input) or `sudo pmset disablesleep 1`. Applies live on save. `GWAE_NO_KEEP_AWAKE=1` forces it off. |
| `minimap.show` | bool | `true` | Draw the minimap dashboard in the bottom-right corner. It appears once there is more than one pane (or more than one strip). Rows of the map are strips; each tile is a pane, its width proportional to the column's real width share. Tiles are tinted by status - blue `»` working, amber `!` wants attention, green `✓` done, red `✗` failed (non-zero exit) - the focused pane's tile keeps its status tint with accent lines above and below (an overline + underline ring), the focused strip gets a `❯` gutter chevron, and each tile's first cell shows its column digit. Status comes from OSC 133 shell integration when the pane emits it, else from an output-activity heuristic (silent for a few seconds → wants attention). |
| `minimap.mode` | string | `"off"` | Chrome presentation: `off` (no persistent row; `⌥`/Alt reveals centered HUD + minimap), `overlay` (bottom-right corner), `edge_ticks` (frame ticks). Legacy `reserved` / `reserved_quasimode` parse as `off` (no bottom row). |
| `minimap.max_width` | integer | `32` | Width of the minimap. A hard *cap* for both the corner `overlay` and the centered panel revealed by `⌥`/Alt: the centered panel sizes each tile to its content (status glyph + column address) and never pads out to this number. Lower it to shrink the panel; it never exceeds ⅔ of the screen. |
| `minimap.max_rows` | integer | `6` | Maximum number of strips (map rows) shown. Used for `overlay` and the centered minimap while holding `⌥`/Alt. Strips past the cut are counted on the panel (`⋯ +3 strips`) rather than silently dropped. |
| `minimap.show_counts` | bool | `true` | Summary tallies, e.g. `5 »2 !1 ✓1 ✗1` (zero counts skipped), above the map. |

### The centered dashboard (hold `⌥`/Alt)

The corner `overlay` answers "where am I". The centered panel answers the
questions you actually hold the modifier to ask:

* **Which one is it.** Each tile shows the status glyph plus the column
  address (`»2`, `!3`): position says which column, color and glyph say how
  it is doing. Tiles are spatial only, no titles, no ages.
* **Where should I look.** Panes that want attention carry their status
  color and glyph (`!` idle, `✗` failed); `⌥+g` jumps to the most urgent
  one (failed, then idle, then done).
* **What is on screen.** A rule under a strip marks the columns currently in
  the viewport - the one thing an infinite strip cannot show by itself.
* Strips share one scale, so a 2-column strip reads shorter than a 6-column
  one.
* **Clicking a tile focuses that pane.** The session dims behind the panel.

Tiles degrade gracefully as they narrow: the status glyph always survives,
then the column digit. With a single pane there
is nothing to triage, so the hold paints no dashboard; key help lives only in
the `⌥+/` cheat-sheet.

Generated from the config structs' doc comments; keep this file in sync when the
schema changes.
### Retro chrome readability

HUD panels are true black with white text. Minimap tiles carry muted status
tints with black/white contrast ink, and the focused tile keeps its status
tint with accent lines above and below (an overline + underline ring), so
focus never hides what the pane is doing and never depends on color alone.
Status glyphs (`» ! ✓ ✗`) distinguish state by shape as well as hue. This
only covers gwae chrome, not programs inside panes. While
the keep-awake assertion is held a small `keep-awake` badge
is stamped on the Option HUD frame instead.
