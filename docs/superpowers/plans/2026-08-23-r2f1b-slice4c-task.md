---
task-type: implement
---

# R2f1b slice 4C — preservation-first cancellation and bounded cleanup transfer

## Description

The third sub-slice of R2f1b slice 4. It settles **preservation and exact ownership before any
cancellation path exists**, so that when 4D–4H build arbitration and the executor multiplexer, the
question "who owns this cleanup, and what happens at the deadline" is already answered and tested in
isolation.

**This sub-slice adds no cancellation path, arms no timer, and changes no scheduling behaviour.**
`crates/bridge-workflow/src/executor.rs`'s event loop is untouched. Readiness stays `Disarmed` and
`AutomaticR2f1b` stays unreachable from production.

Base: `origin/main` = `1b328196` (R2f1b slice 4B).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4C).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §5.5 steps 2 and 6–8,
and invariant 5.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `bridge_core::execution_policy::NodeCleanupV2` exists with the variants `Pending`, `Complete`,
  `Partial`, `Failed`, `NotNeeded`, `Unknown`, carrying the payloads described below.
- `bridge_core::resource_flight::RecoveryOwnerV1` exists with fields `attempt_id`,
  `resource_flight_id`, `reason`.
- `CLEANUP_TAIL_MS` is 60_000 and `REPORTING_TAIL_MS` is 10_000 in `execution_policy.rs`.
- `WorkflowCleanupTracker` in `crates/bridge-workflow/src/executor.rs` holds an
  `Arc<dyn MonotonicClock>` and records interval pairs as `(u64, u64)` millisecond offsets.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line. The scope document's line numbers are known stale.

### Verified anchors — operator-measured on this base

- `NodeCleanupV2::Partial` already carries a **required** `recovery_owner: RecoveryOwnerV1`, and
  `Unknown` carries an **optional** one. The settlement vocabulary this sub-slice needs already exists;
  it is not yet produced by any decision function.
- `NodeCleanupV2` is consumed by `workflow_history.rs`, `task_store.rs`, and `bridge-store/src/sqlite.rs`
  — it is already persisted, so its **encoding must not change**.
- `CLEANUP_TAIL_MS = 60_000`, `REPORTING_TAIL_MS = 10_000`, `DEFAULT_WORK_CUTOFF_MS = 7_200_000`.
- 4A's `R2F1B_CONTROL_ACTION_INTERNAL_TIMEOUT_MS = 30_000` and
  `R2F1B_CANCELLATION_INTERNAL_GRACE_MS = 5_000` are already asserted strictly below their observable
  bounds.
- The executor's `WorkflowCleanupTracker` produces `NodeCleanupV1 { disposition, duration_ms }`, the
  **V1** shape — nothing produces `NodeCleanupV2` today.
- `ResourceFlightStateV1` and `PreparationFlightStateV1` both exist with `Transferred`/`Settled`
  terminal states.

## What this sub-slice does

**1 — The cleanup settlement decision, as one pure function.**

Given a node's observed cleanup state, the elapsed offset from the cancellation anchor, and the
cleanup deadline, produce a `NodeCleanupV2`. The rules, from scope document §5.5:

- Settled before the deadline → `Complete` (or `Failed` with its cause; `NotNeeded` when nothing was
  materialized).
- **Not settled at the deadline → never a silent drop.** It becomes `Partial` or `Unknown`, and the
  exact owner **transfers** to a `RecoveryOwnerV1`. `Partial` requires one; `Unknown` records one
  whenever an owner is identifiable.
- The deadline anchors to the cancellation event but is **capped** by `work_cutoff + CLEANUP_TAIL_MS`.
  Prove the cap binds with a case where the anchor-relative deadline would otherwise exceed it.

The function must be synchronous and clock-free — it takes elapsed milliseconds, it does not read a
clock. Construction and evaluation must not spawn, sleep, select, cancel, or arm a timer.

**2 — Preservation precedes disposition (§5.5 step 2).**

Every materialized worktree's preservation must be completed or durably typed **before** any node
disposition is emitted — including for already-completed nodes whose checkout disposition was held for
the global outcome. Express this as an ordering that is enforced by construction, not by comment: a
disposition must be unobtainable until preservation for that node has been typed. A test must show
that attempting to obtain a disposition first fails or is unrepresentable.

**3 — The sole-owner guard (invariant 5).**

A node's cleanup guard must **never** be dropped while it is the sole cleanup owner. Represent
ownership so that dropping without either settling or transferring is **detectable**, and prove it: a
test must observe the violation when a guard is dropped un-settled and un-transferred, and observe no
violation when it is settled or transferred. Transfer must name the exact `RecoveryOwnerV1`, never a
reconstructed or best-effort owner.

**4 — Bounded reasons stay bounded.**

Transfer reasons go through the existing bounded types. A reason that exceeds its bound must be
rejected or truncated by the existing constructor, never silently stored — assert the existing
behaviour rather than adding a second bounding path.

## Invariants — must not change

- No cancellation path is added. No `select!`, sleep, spawn, timer, or token is added or altered.
- `crates/bridge-workflow/src/executor.rs`'s event loop is untouched.
- `NodeCleanupV2`'s serialized encoding is unchanged — it is already persisted through
  `bridge-store/src/sqlite.rs`. Prove this against **literal bytes** for at least `Complete`, `Partial`,
  and `Unknown`.
- Readiness ships `Disarmed`; `AutomaticR2f1b` remains unreachable from production.
- `FixedGraceInactive` still fires for `FanOutPolicyV1::FixedGrace` under `Production`.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched.

**The refusal gate (decomposition §5).** Re-assert, as 4B did, that no production caller can construct
an automatic attempt while readiness is `Disarmed`. This is checked every sub-slice, not once.

**Carried from 4B, and load-bearing here:** "fully refused" is an *admission-layer* property.
`resolve_execution_policy_with_readiness_v1` is `pub` behind `#[doc(hidden)]`. Do not add a second
production entry point to it; admission must remain its only one.

## Out of scope

- Any cancellation trigger, arbitration, or priority ordering — 4D.
- Constructive impossibility proofs — 4E. Fixed grace — 4F. Progress epochs — 4G.
- The executor multiplexer — 4H. Issue #22 closure — 4I. Arming — 4J.
- Wiring the settlement decision into the live cleanup path. Build the seam and test it in isolation;
  integration lands with the multiplexer, exactly as 4A–4G are sequenced.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. Settled-before-deadline yields `Complete` / `Failed` / `NotNeeded` for the corresponding states.
2. Unsettled at the deadline yields `Partial` with an exact `RecoveryOwnerV1` — never a drop.
3. `Unknown` records a recovery owner when one is identifiable, and is representable without one when
   it is not.
4. The `work_cutoff + CLEANUP_TAIL_MS` cap binds: a case where the anchor-relative deadline would
   exceed the cap settles at the cap.
5. Preservation-before-disposition: obtaining a disposition before preservation is typed fails or is
   unrepresentable.
6. The sole-owner guard: dropping un-settled and un-transferred is detected; settling or transferring
   produces no violation.
7. Encoding stability: `Complete`, `Partial`, and `Unknown` encode to the exact bytes they encode on
   the base tree, asserted against literals.
8. The refusal gate, as in 4B.

## Size

**Cap: 500 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 350. This is the largest sub-slice in the decomposition; the cap is a **stop boundary**,
not a target. If the change cannot be made within it, **stop and report** rather than growing it.
Unused capacity from another sub-slice cannot be transferred here.

## Frozen single-mutation control

Produce a patch that reverts exactly one **production** change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  not a filtered one, computed as the set difference against the candidate's own pre-existing failures;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.
  A control that fails on `dead_code` before reaching its red tests proves nothing.

If the container cannot fetch crates, use the warm dependency cache offline — 4B measured its control
with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` and localhost excluded from the injected proxy. Report
doc-test launch failures separately; they are environmental, not part of the red population.

Prefer mutating the deadline comparison or the transfer arm — a control that reddens both the
transfer test and the cap test is stronger, not weaker.

## Handoff

Write `docs/superpowers/reviews/2026-08-23-r2f1b-slice4c-handoff.md` covering: what changed, the
control patch path and SHA-256, the actual red population, the encoding-stability evidence, the
deliberate exclusions, and the counted line total against the cap. **Do not record your own head or
tree sha** — that binding is the operator's evidence commit.

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- No cleanup owner can be dropped silently; every unsettled owner transfers to an exact
  `RecoveryOwnerV1`.
- `NodeCleanupV2` encodes identically to the base tree.
- Counted added nonblank Rust lines ≤ 500.

## Files

- `crates/bridge-core/src/execution_policy.rs` — the settlement decision and its inputs.
- `crates/bridge-core/src/resource_flight.rs` — recovery-owner construction, if it needs widening.
- `crates/bridge-workflow/src/` — the preservation ordering and sole-owner guard seams.
- Test files under `crates/bridge-core/tests/` and `crates/bridge-workflow/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §5.5 cancellation → terminal flow,
  invariant 5, §5.7 crash matrix rows for cleanup-pending and owner transfer.

## Commit Message

Settle preservation and exact cleanup ownership (R2f1b slice 4C)
