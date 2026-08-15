---
task-type: code-review
---
# R2f1b 3c2 Task E closure review

## Description

Perform the one counted closure review of the complete Task E line: exact
diff `2697c438..1f3c3a82` in this checkout, where `2697c438` is the
accepted Task D head and `1f3c3a82` is the current head. This is the
closure declared by the Task E round contract; it is capped at one pass
with no repair loop inside it.

The line has three commits:

1. `05e9517e` — the base implement: the `ApiRequestCleanupCustodianV1`
   cell keyed by the backend-global turn authority, drop custody transfer,
   bounded watch-based observation, the binding exact checked-cleanup
   projection table, and all four checked/observed cleanup surfaces.
   Its advisory review REJECTed on three WRONGs: (1) two Clippy defects
   (`large_enum_variant` on `PreparedRequest`, `manual_inspect` on the
   admission `map_err`); (2) `observe()` destructively consumed the
   acceptance-aware settlement diagnostic before the deadline check and
   discarded the recording result; (3) a scope dropping after cleanup
   timeout bypassed the custodian: `settle_drop` early-returned in
   `TimedOut`, the moved flight died in the bridge-core destructor with
   its result ignored, then drop cleared the slot. It also raised one
   SMELL-DEFER on the fail-first strength of the new-behavior tests.
2. `6b9788a6` — the contracted targeted repair (production 93 changed
   lines, total 338): boxed scope + `inspect_err`; clone-record-then-clear
   diagnostic custody (expiry/rejection/timeout retain it); a timed-out
   cell accepts the late drop transfer, settles at most once, records
   success or refusal, and retains a refused flight
   (`retained_late_flight`). Its advisory review REJECTed on exactly one
   fresh WRONG: the post-settlement branch keyed on a PRE-settlement
   `timed_out` snapshot, so a settlement beginning before but completing
   after expiry routed around the absorbing `TimedOut` — a crossing
   success overwrote it with `Terminal` and projected `Complete`
   (reclaimable), and a crossing refusal overwrote the state while the
   dropped flight's destructor performed a prohibited ignored retry.
3. `1f3c3a82` — the disclosed operator completion (production +35/−13 of
   which +15 are `#[cfg(test)]`-gated, tests +153, handoff +54): the
   post-settlement branch now runs under a single lock acquisition keyed
   on the CURRENT cell state; `TimedOut` is absorbing (crossing success
   records `terminal` evidence without changing state; crossing refusal
   stores the acceptance-aware diagnostic and retains the flight); a
   `#[cfg(test)]`-only ordering gate between the snapshot and the durable
   settlement makes both crossing schedules deterministic. Red-first:
   both crossing regressions failed behaviorally on `6b9788a6` at their
   `TimedOut` assertions before the fix.

Adjudicate:

- the three base-round WRONGs and the repair-round crossing WRONG as
  FIXED, PARTIAL, or OPEN against `1f3c3a82`;
- the operator completion at source — it is operator-authored and you are
  licensed to reject it. Disclosed operator concerns for contest: (a) the
  `#[cfg(test)]` gate is a test seam in a production file (it compiles
  out of production builds; the same discipline as the fs_custody
  ordering tokens) — judge whether it can perturb production behavior;
  (b) the completion inlines the former `finish()`/`refuse()` semantics
  into the single-lock tail — judge parity, noting the notify cadence is
  now unconditional at the tail (a superset of the old notify points);
  (c) the crossing-success evidence records `acknowledged=true` exactly
  as the pre-existing `finish(identity, disposition, true)` call did —
  judge whether that acknowledgement claim is sound for the projection
  table; (d) the stale `timed_out` snapshot still gates only the
  pre-deadline retry — the operator's reasoning is that `TimedOut` can
  only exist after the deadline, so a crossing schedule can never admit
  an after-deadline retry; falsify if you can;
- the base-round SMELL-DEFER (direct-cell rather than public-path tests;
  the bind/publication window not exercised through production
  admission; the recreation extension not binding the stale cell) —
  judge whether it hides a blocker; otherwise it goes to the aggregate
  ledger;
- the binding exact projection table and Task E acceptance criteria:
  active Legacy never `Complete`; bind/publication-window cleanup
  projects `Unknown`; terminal-refusal debt survives session-slot
  removal; drop retains acceptance-aware persistence diagnostics; proven
  completed work does not taint later independent cleanup; stale
  authority for a forgotten session cannot signal, settle, or clean a
  recreated one; deadline expiry leaves zero live waiters and no
  blocking threads;
- scope: across the whole line only `crates/bridge-api/src/backend.rs`
  and the implementer handoff changed; the old request adapter still
  compiles; production construction still assigns
  `resource_flight_route_v3 = None`; `Cargo.lock` unchanged; no
  `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `1f3c3a82`, clean worktree, branch `implement/impl-59023-lbpwrndo`;
- the repair's in-container verify was fully green (fmt, clippy, build,
  test); the base run's in-container whole-bin test failure was
  classified as the ledgered hermetic flock-EBADF class (instance 7),
  with a host control on exact `05e9517e` of 1,086 passed / 0 failed;
- operator host gates on exact `1f3c3a82` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy
  with `-D warnings`, full locked all-feature workspace test **4,084
  passed / 0 failed / 13 ignored across 90 harnesses**, locked release
  build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the four prior blockers and the operator
  completion, and confirm no regression in the previously sustained Task
  A-D families reachable from this crate.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-api/src/backend.rs`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`
