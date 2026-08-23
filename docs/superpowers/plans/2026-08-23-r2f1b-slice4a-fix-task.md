---
task-type: implement
---

# R2f1b slice 4A — review-fix round (clock forward on `ImplementAttemptTelemetry`)

## Description

Slice 4A's implement loop reached its attempt bound with `verify: PASS` and one remaining
review BLOCKER. This is a **targeted fix on the existing candidate**, not a re-implementation.
The clock-unification design is confirmed sound by both reviewers; one production wrapper was
left unconverted.

Base: `refs/s4/4a-candidate` = `8dfb899e` (the slice 4A candidate, already on this repo).
Everything else in that commit is accepted — **do not revisit, refactor, or re-litigate it.**

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md`.
Slice spec: `docs/superpowers/plans/2026-08-23-r2f1b-slice4a-task.md`.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `bin/a2a-bridge/src/main.rs` has `struct ImplementAttemptTelemetry` with a field
  `factory: bridge_core::attempt_activity::AttemptTelemetrySinkFactory`.
- `impl bridge_core::ports::RichEventSinkFactory for ImplementAttemptTelemetry` overrides **only**
  `make()` — i.e. it does not already define `monotonic_clock()`.
- `AttemptTelemetrySinkFactory::clock()` exists and returns `Arc<dyn MonotonicClock>`.
- `bridge_core::ports::RichEventSinkFactory::monotonic_clock` has a defaulted body returning `None`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- `ImplementAttemptTelemetry`'s `RichEventSinkFactory` impl defines only `make()`; the trait default
  therefore returns `None` for `monotonic_clock()`.
- `crates/bridge-workflow/src/executor.rs` builds its cleanup clock as
  `ctx.make_rich_sink.and_then(|f| f.monotonic_clock()).unwrap_or_else(|| Arc::new(SystemMonotonicClock::start()))`
  — so a `None` here means a **second epoch** for the cleanup tracker on the production `implement`
  review path.
- `AttemptTelemetrySinkFactory`'s own `RichEventSinkFactory` impl already forwards
  `monotonic_clock()` as `Some(self.clock())`.
- The two sibling wrappers `DetachedCompositeRichSinkFactory` and `RunWorkflowAttemptTiming`
  already forward and are test-covered.

## Required change

**1 — forward the clock (production).** Add to
`impl bridge_core::ports::RichEventSinkFactory for ImplementAttemptTelemetry`:

```rust
fn monotonic_clock(&self) -> Option<Arc<dyn bridge_core::attempt_activity::MonotonicClock>> {
    Some(self.factory.clock())
}
```

(Use whatever `Arc`/path spelling the surrounding file already uses; match local idiom.)

**2 — regression test.** Prove by **identity**, not by value, that this wrapper exposes the same
clock as its underlying factory: construct an `ImplementAttemptTelemetry`, then assert

```rust
Arc::ptr_eq(&telemetry.factory.clock(), &telemetry.monotonic_clock().unwrap())
```

The established pattern is the barrier-identity assertion in
`crates/bridge-coordinator/src/detached.rs` (`Arc::ptr_eq` over `Arc<dyn MonotonicClock>`) — follow it.
A test that merely asserts `monotonic_clock().is_some()` is **not acceptable**: it passes for a
freshly minted second clock, which is the exact defect. The test must fail on the base tree.

**3 — disclosure.** In `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-handoff.md`, add one line
naming `ImplementAttemptTelemetry` as a **converged** `RichEventSinkFactory` wrapper, alongside the
existing entries for the deliberate exclusions.

## Out of scope — do not touch

- Any other file, symbol, or behaviour from the base commit. No refactors, no drive-by cleanups.
- `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-control.patch` — the frozen mutation control.
  Its bytes and its SHA-256 must not change, and it must still apply to the fixed tree.
- `DirectAttemptBarrier` / `workflow_history.rs` (a pre-existing, untouched execution surface, already
  disclosed as out of scope by the reviewers).
- `run_agent_preflight_uncached` / `run_node` turn-local diagnostics (disclosed exclusions).
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, any manifest, and `Cargo.lock`.
- No timer is armed. `DeadlineActivationV2::AutomaticR2f1b` stays unconstructible from production.

## Size

**Cap: 40 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: ~25. The cap is a stop boundary, not a target — if the change cannot be made within it,
stop and report rather than growing it.

## Acceptance

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- The new regression test **fails on the base tree** (`8dfb899e`) and passes after the change.
- The frozen control patch still applies and still reddens its named test.
- No behaviour change outside the clock-identity forward.
