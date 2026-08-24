---
task-type: implement
---

# R2f1b slice 4D — scheduler arbitration kernel

## Description

The fourth sub-slice of R2f1b slice 4. It settles the **eight-arm priority order and its tie rules**
as a pure, synchronous decision, so that when 4H replaces the executor's bare await with a `biased`
select, the ordering is already fixed, exhaustively tested, and has exactly one representation.

**This sub-slice arms no timer, adds no cancellation path, and changes no scheduling behaviour.**
`crates/bridge-workflow/src/executor.rs` is untouched — including the bare
`let Some(first) = inflight.next().await` that is issue #22. Readiness stays `Disarmed` and
`AutomaticR2f1b` remains unreachable from production.

Base: `origin/main` = `712fec68` (R2f1b slice 4C).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4D).
Scope document: `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` §4.3.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `crates/bridge-workflow/src/executor.rs` still contains a bare
  `let Some(first) = inflight.next().await` with no deadline arm.
- `crates/bridge-workflow/src/cancellation_settlement.rs` exists and exports
  `WorkflowNodeCancellationSettlementV1`, `PreservationRequiredV1`, `PreservationTypedV1`.
- `bridge_core::execution_policy` exports `settle_node_cleanup_v2` and `NodeCleanupV2`.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line. The scope document's line numbers are known stale: the #22 site
has moved 4619 → 5233 → 5268 across this programme. That drift is exactly why symbols are the contract.

### Verified anchors — operator-measured on this base

- The #22 bare await is present and unguarded.
- 4C's settlement seam is in place and is **deliberately not wired** into the live cleanup path.
- No arm, priority, or arbitration enum exists for this loop today. `PolicyTriggerBarrierResultV1`
  (`fanout.rs`) is a barrier result, not a priority order, and is not the thing to extend.
- `policy_trigger_id` is already threaded through the executor, coordinator, history and store — the
  vocabulary for naming a trigger exists; the arbitration that selects one does not.

## What this sub-slice does

**1 — The eight arms, in one ordered representation.**

Scope document §4.3, in priority order:

1. Drain immediately-ready node completions, preserving R2f1a's ready-batch sort by `NodeId`.
2. Durable trigger-barrier acknowledgements.
3. Workflow/external cancellation.
4. Fixed-grace expiry.
5. Absolute cutoff.
6. Mechanically proved impossibility.
7. Due no-progress snapshots.
8. Wait on node / activity / control / clock.

**2 — One pure arbitration function.**

Given which signals are ready plus the facts the tie rules need, return the winning arm. It must be
synchronous and clock-free: it takes readiness facts, it does not read a clock, spawn, sleep, select,
or cancel.

**3 — The tie rules, exactly as stated.**

- A completion ready **at** the cutoff **wins for that node**; unfinished nodes are then cancelled.
  "At" is inclusive — the boundary case is the rule, not an edge of it.
- A warning **loses to both** completion and cutoff.

**4 — One representation, and a table-driven test that is data.**

Adopted from `gpt-5.6-sol` and binding: *the priority order is represented once in executable code and
once in a table-driven test — there must not be separate production and test priority
implementations.* The test table is **data**: rows of (signals ready, tie facts) → expected arm. It
must not re-derive, sort, or otherwise compute the ordering; a test that reimplements the order would
pass against a wrongly-ordered production and is worthless.

The table must be **exhaustive over the arms**: every one of the eight is the winner in at least one
row, and a row with all eight ready selects arm 1.

## Invariants — must not change

- `crates/bridge-workflow/src/executor.rs` is **untouched**. The bare await stays; 4H replaces it.
- No timer arms; no `select!`, sleep, spawn, token, or cancellation is added or altered.
- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- `FixedGraceInactive` still fires for `FanOutPolicyV1::FixedGrace` under `Production`.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change here
  is genuinely unavoidable, **stop and report** rather than deciding it silently.

**The refusal gate (decomposition §5).** Re-assert, as 4B and 4C did, that no production caller can
construct an automatic attempt while readiness is `Disarmed`.

**Carried from 4B and still binding:** "fully refused" is an *admission-layer* property. Do not add a
second production entry point to `resolve_execution_policy_with_readiness_v1`.

## Out of scope

- Wiring the kernel into the executor — 4H. Build the seam and test it in isolation, exactly as 4A–4C
  were sequenced.
- The closed impossibility list — 4E. Arm 6 is a **readiness input** here, not a proof obligation.
- Fixed-grace timer mechanics — 4F. Progress-epoch arithmetic — 4G. Issue #22 closure — 4I.
- Arming — 4J.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. The exhaustive table: every arm wins in at least one row; all-ready selects arm 1.
2. Completion ready **at** the cutoff wins for that node — the inclusive boundary, asserted at exactly
   equal values, not merely below.
3. After that completion wins, unfinished nodes are cancelled.
4. A warning loses to a completion.
5. A warning loses to a cutoff.
6. Ready-batch completions preserve the `NodeId` sort.
7. The refusal gate, as in 4B and 4C.

## Size

**Cap: 450 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 300. The cap is a **stop boundary**, not a target. If the change cannot be made within it,
**stop and report** rather than growing it.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify:

- it applies cleanly to the candidate tree;
- it reddens at least one named test — report the **actual** red population from a **full-suite** run,
  computed as the set difference against the candidate's own pre-existing failures;
- the mutated tree still passes `cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer **swapping two adjacent arms** in the priority order: it should redden several table rows at
once, which is a stronger control, and it directly attacks this slice's whole deliverable.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy. Report
doc-test launch failures separately; they are environmental, not part of the red population.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4d-handoff.md` covering: what changed, the
control patch path and SHA-256, the actual red population, the deliberate exclusions, and the counted
line total against the cap.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test — a handoff that claims green over a red run is itself a defect, and it cost slice 4C a
review round. If a fixture or expectation was hand-written rather than tool-generated, say that too.

**Do not record your own head or tree sha** — that binding is the operator's evidence commit.

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- The priority order has exactly one executable representation; the test table computes no ordering.
- `executor.rs` is byte-identical to the base.
- Counted added nonblank Rust lines ≤ 450.

## Files

- `crates/bridge-workflow/src/` — the arm enum and arbitration function (a new module is fine).
- Test files under `crates/bridge-workflow/tests/`.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.3 event loop and tie rules.

## Commit Message

Settle the eight-arm scheduler arbitration order (R2f1b slice 4D)
