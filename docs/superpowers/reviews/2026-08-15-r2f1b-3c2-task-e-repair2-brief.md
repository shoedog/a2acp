---
task-type: implement
---
# R2f1b 3c2 Task E owner-authorized second repair

## Description

Perform the one owner-authorized additional repair of the Task E artifact.
The frozen input is exact commit
`1f3c3a82cef043ce824959e4cbae8037347590fa`. Two confirmed blockers from
the counted closure review; nothing else changes. Production remains
`LegacyV2` with `resource_flight_route_v3 = None`; no bridge-core,
workflow, worktree, or `bin/` changes.

Own `crates/bridge-api/src/backend.rs`, focused colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement exactly two repairs:

1. **Absorbing `TimedOut` inside `finish()` (class-terminal).** The
   operator completion made `TimedOut` absorbing inside `settle_drop`,
   but the cell has a second terminal writer: ordinary
   `RequestScope::settle` calls `finish()`, which overwrites the state
   unconditionally on identity match. A successful V3 settlement whose
   synchronous result publisher stalls across the cleanup deadline lets
   concurrent checked cleanup expire to `TimedOut`, then `finish()`
   overwrites it with `Terminal` — erasing timeout debt. Repair: move
   the absorbing check INTO `finish()` under its own lock — when the
   current state is `TimedOut`, record the terminal result as evidence
   (`terminal`) WITHOUT changing state, and return `true` so the
   already-settled scope proceeds normally; the protective `Unknown`
   projection is retained and the cell stays non-reclaimable. This
   closes the whole overwrite family at the single remaining
   `Complete`-projecting writer rather than at one call site.
   `refuse()` overwriting `TimedOut` with `SettlementRefused` is
   accepted as-is (an equally protective non-upgrading state, both
   project `Unknown`) — do not widen scope there.
   Red (public path, closure-prescribed): drive a real request through
   the backend with a barrier-controlled result publisher; while the
   publisher stalls after the durable append, run a checked cleanup
   that expires to `TimedOut`; release the publisher. Assert on the
   CELL STATE (`TimedOut`, not `Terminal`) and non-reclaimability — the
   state assertion is the discriminator and must fail on the frozen
   input. Cover the late-`Complete` case; a refusal/non-complete edge
   may reuse existing coverage.
2. **Honest publication acknowledgement.** The backend records
   `acknowledged=true` as a literal for results settled through the old
   `DurableRemoteRequestFlightV3` adapter — `RequestScope::settle`'s
   `finish(..., true)` and both `settle_drop` success tails — but the
   old adapter's publisher is a void callback (default
   `NoopResourceFlightResultPublisher`) with no exact-echo
   acknowledgement surface, so a V3 `Complete` projects `Complete`
   without the matching publication acknowledgement the binding table
   requires. Repair: record `acknowledged=false` for every result
   derived from an old-adapter settlement (all three sites). A V3
   `Terminal(Complete, acknowledged=false)` then projects `Unknown`
   until Task F wires the Task D driver, whose publication demands the
   exact delivery-identity echo. Do NOT change the no-authority /
   pre-admission fast paths (`begin_cleanup`'s never-admitted
   `(Complete, true)` and `finish_pending` callers where the table's
   no-effect rows apply): those rows assert no provider effect existed,
   not a publication claim. Do not change the cell's projection table
   itself — the cell keeps honoring an acknowledged=true input; only
   the backend stops fabricating it.
   Red (closure-prescribed): a fully successful V3 request through the
   public path with the default no-op publisher, then checked cleanup —
   must return `Unknown`; on the frozen input it returns `Complete`.

Note the interaction: repair 2 alone would mask repair 1's projection
symptom (`Complete` becomes unreachable while acknowledgements are
`false`), which is why repair 1's red regression must discriminate on
the recorded cell state, not the projection alone.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility; both repairs need public-path tests that fail
  behaviorally on the frozen input, with the state-level discriminator
  for repair 1.
- The exact checked projection table is unchanged at the cell:
  `TimedOut` and `SettlementRefused` still project `Unknown`; the
  no-authority row still projects `Complete`; no new path may produce
  `Complete` from a timed-out or unacknowledged settlement.
- Deadline expiry still leaves zero live waiters and no blocking
  threads; the settled scope's normal completion path returns success
  when its durable settlement succeeded, even when the cell retains
  `TimedOut`.
- All existing bridge-api tests keep passing unchanged EXCEPT any that
  pinned the fabricated acknowledgement — migrate those with cause
  stated in the handoff; the old request adapter still compiles;
  production construction still assigns `resource_flight_route_v3 =
  None`.
- Run `cargo test -p bridge-api` (all harnesses) and
  `cargo test -p bridge-core --lib -- remote_request_flight`
  (unchanged), plus `git diff --check` and `cargo fmt --all -- --check`;
  no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `1f3c3a82`, red evidence,
  honest churn accounting (additions plus deletions, post-format), and
  the statement that Tasks F-G and production V3 remain unarmed.
- Stop and report before exceeding **120 changed production lines or
  400 total changed lines** (churn convention, post-format) relative to
  `1f3c3a82`.

## Files

- `crates/bridge-api/src/backend.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): absorbing TimedOut in finish and honest acknowledgement

## Round Contract

This dispatch is the single owner-authorized additional repair of the
Task E artifact. One hard-read-only Sol/xhigh closure review follows
separately; do not self-repair a rejection. Never restart from a fresh
artifact and never extend the cap.
