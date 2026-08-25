---
task-type: implement
---

# R2f1b slice 4H-1 — discharge the wiring residuals (no executor change)

## Description

The decomposition declares one sub-slice 4H, "eight-arm executor multiplexer", at a 500-line cap. It
is being **split before dispatch, not after rejection**, into:

- **4H-1 (this task):** discharge every accumulated wiring obligation so the integration point is
  reachable and its inputs are decided. **`executor.rs` is untouched.**
- **4H-2 (next):** the single risky edit — replace the bare await with the `biased` select.

The reason is measured, not stylistic. Slice 4C was also a 500-cap slice with a rich obligation set;
it consumed all three loop attempts plus two decoupled fix turns. 4H arrives carrying **five**
residuals from 4C, 4D, 4E and 4G *on top of* its own integration work, which is more than its
projection assumed. Sizing a slice so its review loop can converge is a planning duty.

Base: `origin/main` = `7d2fb43b` (R2f1b slice 4G).

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` (§4, sub-slice 4H).

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails on the base tree:

- `crates/bridge-core/src/mechanical_impossibility.rs` declares `prove_mechanical_impossibility_v1`
  and its observation types as `pub(crate)`.
- `tests/trybuild/mechanical_impossibility_proof_is_sealed.rs` and its `.stderr` exist, and the
  `.stderr` contains **seven** `error[E0603]` entries plus `error[E0599]`, a plain
  `error: cannot construct … with struct literal syntax due to private fields`, and `error[E0277]`.
- `UnidentifiableCleanupOwnerProofV1` is a tuple struct with a private `ResourceFlightIdV1` field and
  no public constructor.
- `crates/bridge-workflow/src/scheduler_arbitration.rs` and
  `crates/bridge-core/src/no_progress_warning.rs` exist with the public items listed below.

**Do NOT stop for immaterial measurement differences** — line numbers, diff counts, formatting-only
deltas. Cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- Seven items in `mechanical_impossibility.rs` are `pub(crate)`: `ProducerResultObservationV1`,
  `ContainerSpawnSettlementV1`, `RouteStateV1`, `TerminalResultObservationV1`,
  `ProducerFinalRouteObservationV1`, `MechanicalImpossibilityObservationV1`,
  `prove_mechanical_impossibility_v1`.
- `UnidentifiableCleanupOwnerProofV1(ResourceFlightIdV1)` — the field is private; today only
  `bridge-core`'s own module tests construct it.
- `scheduler_arbitration` exports `arbitrate_scheduler_v1`, `SchedulerArmV1`,
  `SchedulerArbitrationReadinessV1`, `SchedulerTieFactsV1`, `SchedulerArbitrationV1`,
  `ReadyNodeCompletionV1`.
- `no_progress_warning` exports `no_progress_warning_ordinal_v1`, `NoProgressWarningV1`,
  `NoProgressWarningPollV1`, `NoProgressWarningEpochV1`, `NO_PROGRESS_WARNING_INTERVAL_MS = 1_800_000`.
- The executor's loop already drains a ready batch and sorts it by `NodeId` — arm 1's semantics are
  present at the site 4H-2 will replace.

## What this sub-slice does

**1 — Widen visibility so 4H-2 can wire (residual from 4E).**

Make the seven `pub(crate)` items in `mechanical_impossibility.rs` reachable from `bridge-workflow`.
Widen **only** what wiring requires; anything that can stay crate-private, should.

**2 — Regenerate the sealing fixture and prove the seal survived (residual from 4E — read carefully).**

Widening visibility **deletes seven `E0603` "is private" errors** from
`tests/trybuild/mechanical_impossibility_proof_is_sealed.stderr`, breaking the fixture. Regenerating
it with `TRYBUILD=overwrite` is expected.

**The trap:** those seven were never the point. Only three errors prove unconstructibility —
`E0599` (no `default`), the plain `error: cannot construct … due to private fields`, and `E0277`
(`From<bool>` unsatisfied). A regeneration that quietly drops them leaves a green test that proves
nothing.

**After regenerating, confirm all three are still present, and state so explicitly in the handoff.**
If any is gone, the seal has been weakened — rebuild the case so it asserts unconstructibility
directly, and report what changed.

**3 — Decide the post-cutoff completion drop (residual from 4D).**

When the cutoff is reached, `arbitrate_scheduler_v1` drops completions whose `ready_at_ms` is **after**
the cutoff, so those nodes are cancelled instead. That is defensible under "unfinished nodes are then
cancelled", but the task never said it and nothing tests it. **Decide it and test it.** Either
behaviour is acceptable; an untested silent drop is not. Record the decision and its reason in the
handoff.

**4 — Decide the delayed-first-poll ordinal skip (residual from 4G).**

A first poll at 65 minutes yields ordinal 2, and ordinal 1 never emits. **Decide and test.** Emitting
only the highest due ordinal is acceptable; emitting each skipped ordinal is acceptable; silently
skipping without a test is not. Record the decision and its reason.

**5 — A constrained constructor for `UnidentifiableCleanupOwnerProofV1` (residual from 4C).**

4H-2 must be able to construct the ownerless-`Unknown` observation from `bridge-workflow`. Add a
constructor that **requires evidence that ownership is genuinely unidentifiable** — not a public
tuple constructor, and not `pub` on the field. If it can be built from a caller's say-so, this
residual has been made worse, not discharged.

## Invariants — must not change

- **`crates/bridge-workflow/src/executor.rs` is byte-identical to the base.** Publish its SHA-256 on
  both trees in the handoff, as slice 4G did.
- No timer arms; no `select!`, sleep, spawn, token, or cancellation is added or altered.
- No behaviour change to any already-shipped decision **except** the two residuals this task
  explicitly asks you to decide (items 3 and 4). If you find yourself changing a third, stop and report.
- Readiness ships `Disarmed`; `AutomaticR2f1b` stays unreachable from production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched. If a change is
  genuinely unavoidable, **stop and report**.

**The refusal gate (decomposition §5).** Re-assert, as 4B–4G did, that no production caller can
construct an automatic attempt while readiness is `Disarmed`.

**Carried from 4B and still binding:** "fully refused" is an *admission-layer* property. Do not add a
second production entry point to `resolve_execution_policy_with_readiness_v1`.

## Out of scope

- The `biased` select and any executor edit — **4H-2**.
- Issue #22 closure — 4I. Arming — 4J.
- Revisiting the `MessageDelta`/`ThoughtDelta` progress classification: it is a recorded open
  judgement for the owner, not a defect to fix here.

## Required tests

Each must fail on the pre-change tree — verify that, do not assume it:

1. The widened items are reachable from `bridge-workflow` (a test in that crate that would not
   compile before).
2. The regenerated sealing fixture still fails on `E0599`, private-fields, and `E0277`.
3. The post-cutoff completion drop, whichever way you decide it, with the boundary asserted exactly.
4. The delayed-first-poll ordinal behaviour, whichever way you decide it.
5. `UnidentifiableCleanupOwnerProofV1` cannot be constructed without its required evidence — prefer a
   compile-fail case over a runtime assertion.
6. The refusal gate, as in 4B–4G.

## Size

**Cap: 300 counted lines** (added nonblank physical Rust lines after `cargo fmt`, docs excluded).
Projection: 200. The cap is a **stop boundary**, not a target. If the change cannot be made within it,
**stop and report** — this task exists precisely so that the next one has room.

## Frozen single-mutation control

Produce a patch reverting exactly one **production** change, record its SHA-256, and verify it applies
cleanly, reddens at least one named test (report the **actual** red population from a **full-suite**
run as a set difference against the candidate's own failures), and that the mutated tree still passes
`cargo clippy --all-targets --all-features --locked -- -D warnings`.

Prefer mutating the new **constrained constructor** so it accepts an unevidenced owner — that is the
guarantee most easily lost.

If the container cannot fetch crates, use the warm cache offline —
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true` with localhost excluded from the injected proxy, and an
explicit `RUSTDOC`.

## Handoff

Write `docs/superpowers/reviews/2026-08-24-r2f1b-slice4h1-handoff.md` covering: what changed, **the
two residual decisions with their reasons**, **explicit confirmation that the three sealing errors
survived regeneration**, `executor.rs`'s SHA-256 on both trees, the control patch path and SHA-256,
the actual red population, and the counted line total against the cap.

**Report gate results truthfully.** If the configured test command is not green, say so and name the
failing test. If a fixture was hand-written rather than tool-generated, say so. Exclude diagnostic runs
that failed for their own reasons, and name them.

**Note on the host suite:** nine `tests/smoke_cli.rs` / `tests/fallback_plan_cli.rs` failures are
environmental and intermittent on this lane. Report them; do not chase them into production changes.

**Do not record your own head or tree sha.**

## Acceptance Criteria

- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`,
  `cargo build --locked`, and the configured test command are all green.
- Every test in "Required tests" exists and fails on the pre-change tree.
- The three sealing errors are confirmed present after regeneration.
- Both residual decisions are made, tested, and justified in the handoff.
- `executor.rs` is byte-identical to the base, proven by published SHA-256.
- Counted added nonblank Rust lines ≤ 300.

## Files

- `crates/bridge-core/src/mechanical_impossibility.rs`, `execution_policy.rs`,
  `no_progress_warning.rs`
- `crates/bridge-workflow/src/scheduler_arbitration.rs`
- `tests/trybuild/`, and test files under both crates.

## Spec Refs

- `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md` — plan of record.
- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md` — §4.3 event loop.

## Commit Message

Discharge the 4H wiring residuals without touching the executor (R2f1b slice 4H-1)
