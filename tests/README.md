# Cross-crate E2E tests

Scripted-terminal integration tests live here once the M0 PTY spike lands:
run the real binary against `strimux-testkit`'s fake terminal, spawn real
shells, and assert rendered frames.

- The layout invariants themselves are `proptest` properties inside
  `crates/strimux-layout/tests/invariants.rs` (no PTY needed).
- Real-shell/ConPTY tests run nightly via `.github/workflows/e2e.yml`.
