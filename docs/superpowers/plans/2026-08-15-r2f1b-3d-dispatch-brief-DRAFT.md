# R2f1b 3d dispatch brief — DRAFT (2026-08-15)

Status: DRAFT. Dispatch blocked on (1) the ledger-discharge slice landing,
(2) owner glance at the B21 resolution below. Source of authority:
`docs/superpowers/plans/2026-08-09-r2f1b-slice3-brief.md` §3d (landed
`c0d43429`); anchors re-measured on live main `6ad88565` (the slice-3 brief's
anchors predate 3a–3c2 and have shifted).

## B21 resolution (required before implementation; R3-7)

**Question.** `PreparationFlightStateV1` landed (A2) with
`Open {} / BarrierSynced {} / Transferred { reason } / Failed { cause }` and no
success-settlement state. Is `Transferred` the success terminal, or does the
wire type need amendment?

**Resolution: amend the wire — add `Settled {}` as the success-settlement
terminal.** Mechanism reasoning:

- §2.5 of the focused-boundary design fixes `Transferred`'s meaning: bound
  expiry before the prepared barrier "transfers the exact preparation guard to
  the recovery flight rather than dropping it." `Transferred` is the
  finite-ownership escape to RECOVERY, and its `reason` is a redacted
  diagnostic string.
- Overloading `Transferred` as also-success would make consumers (sweeps,
  recovery, settlement) distinguish "recovery owns the guard" from
  "preparation succeeded" by free-text reason — exactly the string-keyed
  dispositioning this lane spent 3c2 eliminating (exact-disposition rule).
  Any closure lens would call it WRONG.
- `BarrierSynced` is mid-flight progress (§2.5 steps 6–7: barrier published
  and synced, reopen-verify and effect admission still ahead), not settlement
  — which is why the slice-3 brief says no success-settlement state exists.
- Cost is at its floor NOW: the type is inactive — zero production writers
  (3d adds the first), zero readers, zero persisted instances, so the
  amendment is a pure additive variant with **goldens + serialization tests +
  exhaustive-match updates in the same change** (B21's amendment protocol;
  A2's goldens are the deliberate tripwire). After 3d ships a writer the same
  amendment becomes a migration.
- Naming: `Settled {}` (serde `"settled"`) follows the lane's settlement
  vocabulary (`UnusedSettled`, settle-dispatch).

## Scope (from the slice-3 brief §3d, unchanged)

(a) Claimed, non-cancellable materialization flight — first production writer
of `PreparationFlightStateV1`; runner retains map/provider/custodian `Arc`s
across caller-future drop; phase-distinguished cancellation tests (M13):
before-claim / after-claim-before-add / mid-add / after-add-before-evidence /
terminal-publication-failure, each with its expected durable state.
(b) Finite ownership (M3): `nonreturning_custody_sync_transfers_pre_effect_owner`
under manual `PreparationClockV1`; ZERO production timers (slice 4 arms).
(c) Candidate settlement (owner ruling): recovery-side `UnusedSettled`
producer; implementer designs the async/trait recovery seam (B18 — sweep is
sync, registration probe is async+private `host_git::registration_absent`),
boot-caller wiring, tri-state refusal (present / absent / cannot-prove →
refuse), as a design note the review checks. Refusing lock window across
proof→transition→unlink (B19; both-order contention tests; does NOT activate
the parked blocking-acquisition policy). Descriptor-safe removal (B20:
same-object descriptor-relative transition-then-unlink, no-follow,
parent-synced, crash-ordering + replacement/symlink negatives).
(d) The 2b2 marker population: marker-removal authority keyed on
state-agnostic exact-absence proof serves BOTH populations; NO table edge.

**Red-first battery (mandated):**
`unused_candidate_settles_only_after_exact_absence` (present-target refuses;
registered-but-absent refuses; both-absent settles, marker only);
dropped-configure-future per phase; the finite-ownership row; contention both
orders; replacement/symlink negatives.

## Anchors (measured on `6ad88565`)

- `crates/bridge-core/src/preparation_flight.rs:115` state enum, `:127` clock
- `crates/bridge-worktree/src/backend.rs:2327` `materialize_under_custody`
  (recovery-side transition doc at `:8271`)
- `crates/bridge-worktree/src/custody.rs:132` `UnusedSettled {}`; frozen
  transition table in the same module
- `crates/bridge-worktree/src/sweep.rs` both arms
- `crates/bridge-worktree/src/host_git.rs:114` `registration_absent`
  (async+private — the B18 seam), `:44` `cleanup_failed_add` (V3-forbidden)

## Dispatch shape (proposed)

- Estimate ~2,500 lines — the largest sub-slice. Propose THREE sequential
  tasks to keep each review round convergent (3c2 lesson):
  T1 = (B21 amendment) + (a) flight writer + M13 phase tests;
  T2 = (b) finite ownership + clock seams;
  T3 = (c)+(d) candidate settlement + lock window + descriptor-safe removal +
  marker authority.
  Each task: terra/xhigh via bridge, one counted Sol closure, cap one round +
  one targeted repair; STOP at 1.5× estimate (R3-6).
- Base: main (VERIFY local main == origin/main before dispatch — stale-ref
  gotcha 2026-08-15).
- Exit gate for the slice = slice-3 brief §6 rows that 3d owns:
  `unused_candidate_settles_only_after_exact_absence` green; both marker
  populations served; preparation-finiteness under manual clocks.

## T1 execution log (2026-08-16)

- T1 dispatched (impl-91867-53y2udsc, base `c37338dd`); 3-attempt bound
  reached with candidate `545103a4` (+890/−24: preparation_flight +20,
  worktree backend +821, handoff doc). Final verify PASS all four stages.
- Attempt-2 blockers (clippy red; durable `Failed` missing on Open-publication
  failure; collapsed red-first evidence) FIXED by attempt 3 — verified at
  source (the `Err(_) => publish_failed_after_initial_open_failure()` branch;
  per-test red-first entries in the handoff).
- Part 1 operator-inspected: `Settled {}` amendment exactly per B21 — doc
  comment carries the success-vs-transfer rationale; wire golden
  `{"state":"settled"}`; deny-unknown-fields negative; exhaustive-match
  comments updated.
- Final internal review REJECT, ONE blocker — CONFIRMED at source: no
  production caller-departure observation between durable `Open` and the add
  (only the cfg(test) `after_open_for_test` injected refusal, backend.rs
  ~:2714); the phase-2 test's real `configure.abort()` is causally inert —
  the asserted `Failed` comes from `hooks.fail_after_open` (the handoff's own
  red-first entry documents the caller-owned-runner mutation, not a
  drop-observation red). False-positive test + unimplemented contract.
- TARGETED REPAIR dispatched on frozen `545103a4` (host branch
  `feat/r2f1b-3d-t1`): R1 one-sample caller-departure check at the
  add-admission boundary (departed → typed Failed, zero add; else committed,
  phase-3 unchanged; no timers/watchers); R2 honest phase-2 test red-first on
  `545103a4`; R3 `fail_after_open` hygiene. Caps 120/300, single file.
  Counted Sol closure follows the repair.
