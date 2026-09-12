# Responsive Yazi panels

An opt-in layout for **Yazi 26.8.15 or newer** running inside gwae. It uses
the actual pane width, so changing column presets, fullscreen, and resizing
the terminal all update the layout automatically.

| Pane width (terminal columns) | Visible panels |
|---|---|
| Below 64 | Current directory only |
| 64–95 | Current directory + right preview |
| 96 and above | Parent + current directory + preview |

With a 215-column terminal, gwae's quarter/third/half presets give roughly
53/71/107 columns, revealing one stage at a time. These are column thresholds,
not preset names. On a smaller display, a half-width pane can remain compact.
The widest stage restores your configured Yazi ratio (normally `1:4:3`).
Standalone Yazi is unchanged, including in narrow terminals.

## Install

From the gwae repository root:

```sh
yazi_config="${YAZI_CONFIG_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/yazi}"
mkdir -p "$yazi_config/plugins/gwae-responsive.yazi"
cp examples/yazi/gwae-responsive.yazi/main.lua "$yazi_config/plugins/gwae-responsive.yazi/main.lua"
```

Add this to your Yazi `init.lua` in the same configuration directory, keeping
any existing initialization:

```lua
require("gwae-responsive"):setup()
```

Restart Yazi, or run `ya emit plugin gwae-responsive` from a shell spawned by
that Yazi instance to enable it without losing your place. There is no need
to rebuild or restart gwae. Gwae's default `content_width = 0` must remain in
effect so the child terminal follows the visible pane width.

Optional breakpoint overrides in `init.lua`:

```lua
require("gwae-responsive"):setup { preview_width = 80, parent_width = 120 }
```

This plugin owns the pane ratio while inside gwae. Do not combine it with
other automatic layout or pane-toggle plugins. It does not change your
`yazi.toml`, openers, keybindings, or directory navigation. Remove the setup
line and restart Yazi to revert.

## Validate

With Yazi installed on `PATH`, run from the repository root:

```sh
cargo test -p gwae --test yazi_e2e -- --ignored --nocapture
```

These optional tests run actual Yazi in PTYs and inside gwae. They assert
rendered parent/file/preview content through width cycling, fullscreen,
terminal resizing, breakpoint boundaries in both directions, repeated live
activation, and standalone use. They also compare filename readability
before/after activation, custom breakpoint/ratio behavior, ordinary directory
navigation, and rollback. See [the requirement-to-observation report](ACCEPTANCE.md).

The live-activation case first checks the unmodified three-panel layout,
then enables the plugin in the same Yazi process. To replay against your
installed gwae rather than the Cargo-built executable:

```sh
GWAE_E2E_BIN="$(command -v gwae)" cargo test -p gwae --test yazi_e2e -- --ignored --nocapture
```
