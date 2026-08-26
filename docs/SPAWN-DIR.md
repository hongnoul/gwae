# Spawn directory (where `⌥+;` starts)

## Problem

Every pane inherits gwae's own working directory. Launching gwae from `~`
(the normal case: a terminal opens at `$HOME`) means every agent harness
starts at `~`, and the first thing the user types in each new pane is
`cd ~/git/thing`. With four panes per strip that is four `cd`s per strip,
and a harness that has already indexed the wrong tree.

## Shape of the fix

Three layers, cheapest first, each one able to stand alone:

1. **Config** — `agent_dir` in `gwae.toml` is the preconfigured directory
   for agent panes. `~` and `$VAR` expand. Empty (default) keeps the old
   behaviour of inheriting gwae's cwd, so nothing changes for anyone who
   does not set it.
2. **CLI** — `gwae run --dir <path>` (and `gwae --dir`) overrides it for one
   session. This is what a shell alias or a project-local script uses.
3. **Keybind** — `⌥+d` opens a directory picker: type to filter, `↵` to use
   it for the rest of the session, `s` to write it back to `gwae.toml`.
   Same interaction grammar as the `⌥+t` theme picker, so it costs the user
   no new muscle memory.

The picker's candidate list is discovered, not typed: the session dir, gwae's
cwd, `$HOME`, every pinned entry in `agent_dirs`, and every immediate child of
each root in `agent_dir_roots` (default `~/git`, `~/code`, `~/projects`,
`~/src`, `~/dev`, `~/Developer`) that is itself a directory. On this machine
that is the ~35 repos under `~/git` with zero configuration.

## Precedence

`⌥+d` session pick > `--dir` > `agent_dir` > gwae's own cwd.

One resolved value lives in the TUI loop (`spawn_dir`), and both spawn paths
(startup pane 1.1 and `sync_panes` for `⌥+;` / `⌥+:`) read it. Plain shell
panes get it too: a shell that opens next to an agent in a different tree
would be its own papercut.

## Why not per-pane state

A pane's cwd is the child process's business the moment it starts; gwae
cannot follow a `cd` without shell integration it does not require. So the
directory is a *spawn-time* input only, resolved once per pane, and never
re-read. That keeps the feature to one `CommandBuilder::cwd` call and no
lifecycle.

## Failure mode

A configured directory that does not exist must not stop a pane from opening.
`resolve` falls back to the inherited cwd and the TUI shows a transient note,
because a pane that fails to spawn looks like a gwae bug, while a pane in the
wrong directory is self-evident.
