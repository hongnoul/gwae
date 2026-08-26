# Staying current

How an already-installed gwae learns that a new version exists, and how it
moves to one. Implemented in [`crates/gwae/src/update.rs`](../crates/gwae/src/update.rs);
this file is the decision and its reasoning (ADR-016).

## The rule

**gwae updates itself the way it was installed, or not at all.**

There is no in-place self-replacing binary, and there is never an upgrade the
user did not ask for. Both halves matter for different reasons.

*No self-replacement*, because most of the ways gwae is on a machine are owned
by something else. Homebrew tracks which files belong to a formula; `cargo
install` owns `~/.cargo/bin`; a Nix store path is read-only by construction;
AUR and distro packages have a manifest that a hand-written file silently
falsifies. A multiplexer that `write`s over its own inode in any of those
prefixes leaves the package manager describing a machine that no longer
exists, and the user finds out the next time they run an unrelated upgrade.

*No unattended upgrade*, because gwae is the process hosting your agent
sessions. Swapping the binary underneath a running one is at best a no-op
(the running image is already mapped) and at worst a mid-session surprise
with new keybindings. The check is allowed to be automatic. The action is not.

## The three questions, kept apart

Each is a pure function over facts, so every branch is testable without owning
five differently-installed machines.

### 1. Where did this binary come from?

`update::detect(&Facts) -> Source`, in order of authority:

| Rank | Evidence | Why it ranks here |
|---|---|---|
| 1 | `[update] source` in the config, or `GWAE_UPDATE_SOURCE` | The user telling us directly. Nothing outranks that. |
| 2 | The install receipt `install.sh` wrote | A fact recorded by the thing that did the install, not an inference. Only honoured while the binary still sits in the directory the receipt names. |
| 3 | The path of the running binary | The only fallible step, so it goes last. |

Path heuristics, after resolving symlinks (Homebrew links `bin/gwae` into the
Cellar, and the unresolved path hides the one marker that identifies it):

- `/nix/store/...` → `nix`. Checked first: a store path can *contain* any other marker.
- `**/.cargo/bin/**` → `cargo`, refined to `cargo-git` or `source` by the entry in `~/.cargo/.crates.toml`, which records whether the install came from the registry, a `--git` URL, or a `--path`.
- `/opt/homebrew/**`, `**/Cellar/**`, `**/linuxbrew/**` → `brew`.
- `**/target/{release,debug}/**` → `source` (someone running their own checkout).
- `/usr/bin`, `/bin`, `/usr/local/bin` → `system`, i.e. **someone else's file**.
- Anything else → `unknown`, which is admitted rather than guessed at.

`/usr/local/bin` deserves its own note: it is Homebrew on Intel macOS, hand-built
software on Linux, and some distro packages. It is only read as Homebrew when a
brew prefix says so; otherwise the safe reading is "owned by something else", so
gwae explains instead of acting. This ambiguity is exactly why the receipt exists.

### 2. What would upgrading take?

`update::plan(Source, exe) -> Plan`.

| Source | Route | gwae runs it? |
|---|---|---|
| `install.sh` | Re-run the installer with `GWAE_INSTALL_DIR` pinned to the current directory | yes |
| `brew` | `brew upgrade gwae` | yes |
| `cargo` | `cargo install gwae --locked --force` | yes |
| `cargo-git` | `cargo install --git .../gwae gwae --locked --force` | yes |
| `source` | `git pull && make install` | **no**, printed |
| `nix` | `nix flake update` / `nix profile upgrade gwae` | **no**, printed |
| `system` | your package manager, e.g. `paru -Syu gwae-bin` | **no**, printed |
| `windows` | download the zip from the latest release | **no**, printed |
| `unknown` | refuse and name the config key that fixes it | **no** |

The four `no` rows are the point of the whole design. `Plan::commands()`
returns an empty vector for them, so "will this run something?" is one
`is_empty()` at the call site rather than a match that has to be kept in sync
with a growing enum.

Re-running `install.sh` *is* the upgrade for the script route, deliberately:
download, checksum verification, and atomic install already live there and
having a second implementation inside the binary would mean two places where
checksum verification can be forgotten.

### 3. Is there anything to upgrade to?

`update::latest_version()` does a `HEAD` of
`https://github.com/hongnoul/gwae/releases/latest` and reads the tag out of the
redirect target.

Not `api.github.com`: the API allows 60 unauthenticated requests per hour **per
IP**, which is a budget shared by everyone behind one office NAT. Being
silently rate-limited into "no updates, ever" is the worst failure this feature
could have, and the redirect has no such limit. It is the same endpoint
`install.sh` already uses, for the same reason.

What that request carries: nothing. No auth, no query string, no body, no
version, no machine identifier, no user agent beyond curl's default. The
response is a URL. gwae cannot count its users this way, and that is fine.

## Cadence and consent

- **At most one check a day** (`CHECK_INTERVAL`), cached in
  `$XDG_STATE_HOME/gwae/update.toml`. A machine that opens forty sessions a day
  makes one request.
- **On a background thread**, started before pane spawn so the request overlaps
  startup instead of adding to it. A session that ends before the answer
  arrives simply never sees it. A failed check is not news and is never
  reported: telling someone their network is flaky is not gwae's job.
- **The result is one line**, shown once per session in the existing toast, and
  only when nothing more important is on screen. It always ends in the exact
  command for *this* machine:

  ```
  gwae 1.0.2 is out (you have 1.0.1) · run: brew upgrade gwae
  ```

  "An update is available" with no route is a notification that makes the
  reader do research we already did.
- **Two off switches**: `check = false` in the config, and
  `GWAE_NO_UPDATE_CHECK=1` in the environment. The env var wins, so a CI runner
  or a shared machine can be made quiet without editing a file it may not own.

## The install receipt

`scripts/install.sh` writes `$XDG_STATE_HOME/gwae/install.toml`:

```toml
source = "install.sh"
dir = "/home/u/.local/bin"
version = "1.0.1"
```

State, not config: it is machine-written bookkeeping, so it stays out of
`~/.config/gwae`, which is a directory the user is invited to hand-edit.
Deleting it is safe and costs a fallback to path detection. It is ignored
outright when the running binary is not in the directory it names, since at
that point it describes a different file.

## `gwae upgrade`

```
gwae upgrade           # detect, check, show the command, ask, run it
gwae upgrade --check   # detect, check, show the command, stop
gwae upgrade -y        # skip the confirmation (dotfile bootstraps)
gwae update            # alias, because that is what half of people type first
```

Every path prints the binary path, the detected source, *how that source was
decided* (config / receipt / path), and the route, before it goes near the
network. The route prints even when you are already up to date: "how would this
machine upgrade?" is worth being able to answer before the day it matters.

A non-terminal stdin declines the confirmation, so a piped `gwae upgrade`
reports its plan and stops rather than acting on a consent nobody gave.

`gwae doctor` shows the same information on one line, which is where most
people will actually see it.

## Config

```toml
[update]
check = true      # daily check + one-line notice; false to go quiet
source = ""       # "" detects; or pin: install.sh, brew, cargo, cargo-git,
                  # source, nix, system, windows
```

An unrecognized `source` is reported by `gwae doctor` and otherwise ignored, and
detection runs instead. Silently obeying a misspelling would mean running the
wrong package manager's command, which is the one failure mode this whole
feature exists to avoid.

## Packagers

If you vendor gwae somewhere the heuristics cannot see, set
`GWAE_UPDATE_SOURCE=system` (or ship a config with `[update] source`) so gwae
tells your users to use *your* package manager and never offers to overwrite
your file. `GWAE_NO_UPDATE_CHECK=1` additionally silences the network check,
which is the right default for a distro build.
