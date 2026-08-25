---
task-type: implement
---

# R2f1b slice 4H-2 — the eight-arm executor multiplexer

## Description

The single risky edit of slice 4. Replace the bare
`let Some(first) = inflight.next().await else { break; }` in
`crates/bridge-workflow/src/executor.rs` — **this is issue #22** — with a `biased` select over the
eight arms 4D settled.

Every input this needs already exists and is tested in isolation: 4A's shared clock, 4B's activation,
4C's cleanup settlement, 4D's arbitration order, 4E's impossibility proofs, 4F's fixed-grace timer,
4G's warning cadence, 4H-1's widened visibility and decided boundaries, 4H-1b's minted proof. 4H-2
wires them; it invents nothing.

**Readiness still ships `Disarmed`, so production behaviour must be unchanged.** That is this
sub-slice's safety property and its primary test obligation: with no deadline armed, the loop must
behave **exactly** as the bare await does today.

Base: `origin/main` = `fd1f66f2` (R2f1b slice 4H-1b).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4H).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §4.3.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `crates/bridge-workflow/src/executor.rs` contains
  `let Some(first) = inflight.next().await else { break; };` inside the scheduling `loop`, immediately
  followed by a `now_or_never()` drain into `ready` and a `ready.sort_by(…node id…)`.
- `crates/bridge-workflow/src/scheduler_arbitration.rs` exports `SCHEDULER_ARM_PRIORITY_V1`'s eight
  arms via `SchedulerArmV1`, plus `arbitrate_scheduler_v1`.
- `bridge_core::mechanical_impossibility` items are reachable from `bridge-workflow`.
- `UnidentifiableCleanupOwnerProofV1` can be obtained by `bridge-workflow` only as a minted proof
  returned from `transfer_cleanup_deadline`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line. The #22 site has moved 4619 → 5233 → 5268 across this
programme; that drift is why the contract is the symbol.

### Verified anchors — operator-measured on this base

- The bare await is the loop's **only** wait point. Everything else in the body is synchronous
  processing of the drained batch.
- The existing body already implements arm 1's semantics: take the first completion, drain the rest
  with `now_or_never()`, sort by `NodeId`.
- `FuturesUnordered` is the `inflight` collection and must be kept.

## What this sub-slice does

**1 — Replace the bare await with a `biased` select. Keep `FuturesUnordered`.**

Arms, in the order 4D settled:

1. Drain immediately-ready node completions (preserving the `NodeId` ready-batch sort).
2. Durable trigger-barrier acknowledgements.
3. Workflow/external cancellation.
4. Fixed-grace expiry.
5. Absolute cutoff.
6. Mechanically proved impossibility.
7. Due no-progress snapshots.
8. Wait on node / activity / control / clock.

**2 — Do not fork 4D's single representation.**

4D's deliverable was that the priority order exists **once**. A `select!` hardcodes its arm order in
source, so integration threatens exactly that. **Add a test that asserts the select's arm order equals
`SCHEDULER_ARM_PRIORITY_V1`**, element by element, so a future edit to either one that is not
mirrored in the other fails. If you can drive the select from the constant directly, better still —
but do not claim you have if you have not.

**3 — Production parity while disarmed.**

With readiness `Disarmed`, no timer arms and no non-completion arm can fire. The loop must produce the
**same events in the same order** and terminate on the same condition as the bare await. Prove it with
a test over a representative multi-node workflow, comparing observed event sequences — not merely that
the workflow completed.

**4 — No busy-spin.**

A `biased` select whose early arms are perpetually not-ready must still **park**, not spin. Prove the
loop makes no progress and consumes no CPU when nothing is ready — for example by asserting a bounded
number of polls, or that the future is pending rather than returning `Poll::Ready`. A hot loop here
would be a production regression that no functional test would catch.

**5 — Termination is unchanged.**

When `inflight` empties, the loop breaks exactly as before. The select must not convert an exhausted
`FuturesUnordered` into a hang.

## Invariants — must not change

- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- No node future is dropped while it is the sole cleanup owner (4C's invariant 5).
- `FixedGraceInactive` still fires for `FanOutPolicyV1::FixedGrace` under `ManualOnlyR2f1a`.
- Event payloads, ordering guarantees, and the `NodeId` ready-batch sort are preserved.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report**.

**The refusal gate.** Re-assert, as 4B–4H-1b did, that no production caller can construct an automatic
attempt while readiness is `Disarmed`.

**Carried from 4B:** "fully refused" is an *admission-layer* property. Do not add a second production
entry point to `resolve_execution_policy_with_readiness_v1`.

**Carried from 4C, and now due:** `into_disposition`'s pending path freezes
`elapsed_after_cancellation_ms` at construction with no accessor — its contract is
**reconstruct-per-poll, not mutate-or-reuse**. Honour that when polling; do not cache a settlement
across polls.

## Out of scope

- Issue #22's *terminalization closure* — 4I. This sub-slice installs the multiplexer; 4I proves a
  nonterminating sibling no longer blocks terminalization.
- Arming — 4J. Nothing here flips readiness.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. **Arm-order parity**: the select's arm order equals `SCHEDULER_ARM_PRIORITY_V1`, element by element.
2. **Disarmed production parity**: a representative multi-node workflow yields the same event sequence
   and terminates on the same condition as the base.
3. **No busy-spin**: with nothing ready, the loop parks rather than spinning.
4. **Termination**: an exhausted `FuturesUnordered` breaks the loop, no hang.
5. **Ready-batch semantics**: multiple simultaneous completions are drained and `NodeId`-sorted.
6. The refusal gate.

## Size

**Cap: 400 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 280. The cap is a **stop boundary**. If the change cannot be made within it, **stop and
report** — this slice was split from a larger 4H precisely so it would fit.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify it applies
cleanly, reddens at least one named test (report the **actual** red population from a **full-suite**
run as a set difference against the candidate's own failures), and that the mutated tree still passes
`cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **swapping two adjacent arms in the select** — it should redden the arm-order parity test, which
is the guard protecting 4D's single representation through integration.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h2-handoff.md` covering: what changed, how
arm-order parity is enforced, the disarmed-parity evidence, the no-busy-spin evidence, the control
patch path and SHA-256, the actual red population, and the counted line total.

**`executor.rs` changes in this slice** — so instead of publishing its SHA-256 as an unchanged
invariant, publish the **base and candidate hashes with a one-line summary of exactly what changed
inside it**.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. Exclude diagnostic runs that failed for their own reasons, and name them.

**Note on the host suite:** nine `tests/smoke_cli.rs` / `tests/fallback_plan_cli.rs` failures are
environmental and intermittent on this lane, and `staged_candidate_` is excluded from the configured
verify by design. Report what you see; do not chase either into production changes.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- Production behaviour while `Disarmed` is unchanged, proven by event-sequence comparison.
- The loop parks rather than spins when nothing is ready.
- Counted added nonblank Rust lines ≤ 400.

## Files

- `crates/bridge-workflow/src/executor.rs` — the multiplexer.
- `crates/bridge-workflow/src/scheduler_arbitration.rs` — if the parity check needs an accessor.
- Test files under `crates/bridge-workflow/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.3 event loop and tie rules.

## Commit Message

Install the eight-arm biased select in the workflow executor (R2f1b slice 4H-2)
