---
task-type: code-review
---
# R2f1b 3c2 Task E final closure review

## Description

Perform the one owner-authorized final closure review of the complete
Task E line: exact diff `2697c438..a1f1f8de` in this checkout, where
`2697c438` is the accepted Task D head and `a1f1f8de` is the current
head. This is the closure of the owner-authorized additional repair
round; it is capped at one pass with no repair loop inside it.

The prior closure (of head `1f3c3a82`) adjudicated all four earlier
blockers FIXED, sustained the operator completion's test-seam, inlining,
and retry-gating rulings, and rejected on exactly two fresh WRONGs:

1. fabricated acknowledgement: `RequestScope::settle` and both
   `settle_drop` success tails recorded `acknowledged=true` as a
   literal although the old adapter's publisher is a void no-op with no
   exact-echo surface, so a V3 `Complete` projected `Complete` without
   the matching publication acknowledgement the binding table requires;
2. second terminal writer: `finish()` overwrote `TimedOut`
   unconditionally on identity match, so a normal successful settlement
   whose synchronous publisher stalled across the cleanup deadline
   erased timeout debt and made the cell reclaimable.

The new final commit `a1f1f8de` (production +10/−7, tests +133/−2,
handoff +60; 212 total) implements the prescribed repairs, red-first
(both public-path regressions failed behaviorally on `1f3c3a82`; its
advisory review returned APPROVE with two non-blocking test-hardening
DEFERs):

- `finish()` treats a current `TimedOut` as absorbing under its own
  lock: it records the terminal result as evidence, preserves the state
  and non-reclaimability, and returns success to the already-settled
  scope (the class-terminal fix at the single remaining
  `Complete`-projecting writer); `refuse()` is unchanged by design (an
  equally protective non-upgrading transition);
- all three old-adapter result tails record `acknowledged=false`; the
  no-authority `begin_cleanup` row, `finish_pending` callers, and the
  cell's exact projection table are unchanged, so a V3 `Complete`
  projects `Unknown` until Task F wires the exact-echo driver.

Adjudicate:

- the two prior-closure blockers as FIXED, PARTIAL, or OPEN against
  `a1f1f8de`, including the public-path barrier-publisher schedule
  (state-level discriminator) and the no-op-publisher `Unknown`
  regression;
- the DISCLOSED side effect that honest acknowledgement forced:
  `begin_admission()`'s reuse-reset now keys on terminal `Complete`
  alone rather than `Complete && acknowledged` (otherwise multi-round
  turns stopped before request B, since round-1 results are never
  acknowledged through the old adapter). The implementer claims the
  reset atomically marks admission started and cannot project
  `Complete` from the unacknowledged result; the operator's assessment
  is that the reset gates intra-turn reuse only, the cleanup projection
  still demands the acknowledgement for V3 `Complete`, and `TimedOut`
  cells never re-admit. Falsify if you can — this is the one
  functional change outside the two prescribed repairs;
- that the absorbing `finish()` does not break legitimate flows: the
  settled scope's success path, `Terminal` re-admission for later
  rounds, the recovery rows, and the four cleanup surfaces;
- the two migrated direct-cell assertions (`(Complete, true)` →
  `(Complete, false)` in the settle_drop evidence tails) as legitimate
  pins of the removed fabrication rather than concealment;
- the prior closure's SMELL (direct-cell tests did not bind production
  paths) — the repair added two public-path regressions; judge whether
  the remaining gap (a genuinely bound stale-cell recreation test)
  still defers to the aggregate ledger or hides a blocker;
- scope: across the final commit only the owned module and handoff
  changed; no production caller, provider integration, or V3 arming;
  `Cargo.lock` unchanged; no `rustfmt::skip`; production construction
  still assigns `resource_flight_route_v3 = None`.

All prior-line adjudications (the four earlier blockers, the operator
completion's absorbing `settle_drop`, diagnostic custody, drop transfer,
bounded observation, surface convergence, recreation fencing) were
sustained by the prior closure and are not reopened unless you find a
new constructible WRONG.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `a1f1f8de`, clean worktree, branch
  `implement/impl-20946-km7adik8`;
- in-container verify fully green on this run (fmt, clippy, build,
  test);
- operator host gates on exact `a1f1f8de` all exit 0: `git diff
  --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,086 passed / 0 failed / 13 ignored across 90 harnesses**, locked
  release build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the two prior blockers and the disclosed
  admission-reset relaxation, and confirm no regression in the
  previously sustained families.
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
