# Contributing to gwae

Thanks for helping. This project is deliberately small in scope; the
governance rules below keep it that way.

## License / inbound = outbound

gwae is **MIT**. Your contributions are MIT too.

## Read-only constraint: niri

niri is **GPL-3.0**. gwae is MIT, so we **cannot read or port niri source**.
We reimplement niri's layout semantics from its public docs, wiki, and observed
behavior only. Do not copy niri code, and do not vendor code that was derived
from it. This keeps the codebase clean.

## Spec-first rule

Layout/behavior changes are spec-first:

1. State the intended behavior in the PR description before changing code; the
   README's layout model (fixed-width columns, viewport scroll-snap) is the
   normative contract.
2. Then the code PR adds/updates the relevant `proptest` invariants in
   `crates/gwae-layout/tests/invariants.rs`.

## What to work on

- Check the `good first issue` label and the non-goals list in the README
  before opening anything. The **daemon** is a non-goal; most multiplexer
  feature requests are politely declined.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs all three on macOS, Linux, and Windows. Keep it green.

gwae ships via the installer scripts (`install.sh` for macOS/Linux, `install.ps1` for Windows) and, on macOS, Homebrew (`brew install hongnoul/tap/gwae`).
