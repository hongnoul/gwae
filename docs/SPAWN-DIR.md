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

The picker's candidate list is discovered, not typed.

## Per-harness default

`⌥+;` does not always mean the same harness: `default_agent` is the user's
preferred one (`jcode`, `claude`, `codex`, …). A single global `agent_dir`
makes `⌥+;` open the wrong repo when the preferred harness lives elsewhere.

`harness_dirs` is a per-harness fallback table that is not tied to any
particular harness name:

```toml
# generic fallback (old key, still works)
agent_dir = "~/git/gwae"

# per-harness: key = harness command as in default_agent
harness_dirs = { jcode = "~/git/gwae", Muse = "~/src/foo" }
# or dotted / table forms:
# harness_dirs.jcode = "~/git/gwae"
# [harness_dirs]
# jcode = "~/git/gwae"
```

In the `⌥+d` picker the title shows the harness (`spawn dir [jcode]:`),
and `⌥+s` writes to `harness_dirs.<harness>` when a harness is set
(otherwise it writes `agent_dir` as before). Any harness is a valid key, so
the UI is general to whichever harness the user configured as preferred.

## Discovery must not guess at names

The first cut scanned a list of likely parents (`~/git`, `~/code`, `~/src`,
...). That works on the machine it was written on and finds *nothing*
anywhere else: people keep work in `~/Documents/clients`, `~/w`, `/srv`, or
whatever their employer mandates. A name list is a guess about a stranger's
filesystem, and it is usually wrong.

So discovery keys off things that mean the same thing on every machine:

* **Project markers** — a bounded breadth-first walk of `$HOME` (or
  `agent_dir_roots`) collecting any directory holding `.git`, `.hg`, `.svn`,
  `.jj`, `.gwae`, or `.projectile`. The walk stops descending at a project,
  skips hidden and dependency directories (`node_modules`, `target`,
  `vendor`, `Library`, ...), and is capped at depth 4 / 4000 directories so
  it cannot hang on a network mount. Measured at ~35 repos in about 2ms on a
  normal `$HOME`.
* **zoxide** — when installed, `zoxide query --list` is the highest-signal
  source there is: the directories this person actually visits, including
  ones outside `$HOME` that no scan would reach. Absent zoxide contributes
  nothing and is not an error.

Both are free of assumptions about layout, so `⌥+d` is useful on a machine
gwae has never seen, with no configuration. `agent_dir_roots` remains for
people who want to narrow or widen the search (`["~/work", "/srv"]`).

## Precedence

`⌥+d` session pick > `--dir` > `harness_dirs[preferred_harness]` > `agent_dir` > gwae's own cwd.

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
