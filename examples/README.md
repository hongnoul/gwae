# Examples

Copy-pasteable `gwae.toml` configs. Drop one at
`~/.config/gwae/gwae.toml` (or `$XDG_CONFIG_HOME/gwae/gwae.toml`) and
gwae live-reloads appearance keys while running. All keys are optional;
see [docs/CONFIG.md](../docs/CONFIG.md) for every key and default.

| File | For |
|---|---|
| [`agent-fleet.toml`](agent-fleet.toml) | Running 4+ CLI agents in parallel with the minimap always on |
| [`minimal.toml`](minimal.toml) | The smallest useful config: theme + default agent |
| [`wide-panes.toml`](wide-panes.toml) | Half-width columns for fewer, wider agents |
| [`terminal-theme.toml`](terminal-theme.toml) | Inherit your terminal's own ANSI palette |

Verify any config parses with:

```sh
gwae doctor
```
