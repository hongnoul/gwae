# Examples

Copy-pasteable `gwae.toml` configs. Drop one at
`~/.config/gwae/gwae.toml` (or `$XDG_CONFIG_HOME/gwae/gwae.toml`) and
gwae live-reloads the config file while running. All keys are optional.
Chrome is retro (true-black panels, functional colors) unless overridden under `[theme]`.

| File | For |
|---|---|
| [`agent-fleet.toml`](agent-fleet.toml) | Running 4+ CLI agents in parallel with the minimap always on |
| [`minimal.toml`](minimal.toml) | The smallest useful config: default agent |
| [`wide-panes.toml`](wide-panes.toml) | Half-width columns for fewer, wider agents |

Verify any config parses with:

```sh
gwae doctor
```

## Application layouts

[`yazi/`](yazi/) contains an opt-in responsive Yazi plugin. Narrow gwae panes
show only the current directory, then reveal the preview and parent panels
as the pane widens. Standalone Yazi keeps its normal layout.
