---
task-type: implement
---
# R2f1b 3c2 Task E targeted repair

## Description

Perform the one contracted targeted repair of the Task E artifact. The
frozen input is exact commit `05e9517e0cb566633ce04675430a29e474a7e3b1`.
Three confirmed blockers from the implementation review; nothing else
changes. Production remains `LegacyV2` with `resource_flight_route_v3 =
None`; no bridge-core, workflow, worktree, or `bin/` changes.

Own `crates/bridge-api/src/backend.rs`, focused colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement exactly three repairs:

1. **Clippy gate (two sites, fully enumerated).** The workspace has
   exactly two Clippy defects under `-D warnings`, both in this crate:
   `large_enum_variant` on `PreparedRequest` (the `Ready` variant carries
   a large `RequestScope` payload) and `manual_inspect` on the admission
   `map_err` that refuses the cleanup cell before re-returning the error.
   Repair: box or restructure the large variant payload; use
   `inspect_err`. Zero behavior change; existing tests are the gate.
2. **Diagnostic custody through expiry (destructive take).** In
   `observe()`, the acceptance-aware settlement diagnostic is removed
   with `diagnostic.take()` BEFORE the deadline check, and
   `request_flight_failure` runs under `timeout_at` with its result
   discarded. With a V3 request accepted and a drop-settlement refusal
   stored, an already-expired deadline (or a slow/rejecting observer)
   destroys the `prompt_may_have_been_accepted=true` evidence: the
   observation returns `Unknown` with no retained or emitted diagnostic.
   Repair: do not consume the diagnostic until recording is confirmed —
   record first, then clear only on confirmed success; on expiry,
   timeout, or observer rejection the diagnostic stays intact in the
   cell so a later checked cleanup can still see it. Red: (a) accepted
   refusal + expired deadline — the diagnostic must survive observation
   and remain visible; (b) rejecting/timing-out observer — same; on the
   frozen input both destroy it.
3. **Timeout-then-drop custody bypass.** Once observation sets
   `TimedOut`, `settle_drop` returns early without accepting the
   transfer; the moved flight is destroyed and its bridge-core
   destructor settles while ignoring the result, after which
   `RequestScope::drop` proceeds to `clear_exact`. A late refused
   settlement never deposits its error, lifecycle, or acceptance bit in
   the cell — this violates the binding drop-custody requirement that
   the scope never clears the slot after ignoring a result. Repair:
   accept the exact late transfer even in `TimedOut`; perform at most
   the initial local settlement (no after-deadline retry); record its
   result (success or refusal, with lifecycle/acceptance) in the cell
   while RETAINING the protective `Unknown` projection — a timed-out
   cleanup never upgrades to `Complete`. Red: deterministic
   timeout-then-drop schedules for both the success and the refusal
   settlement outcomes — on the frozen input the cell records nothing.

Do not add the deferred test-strength work from the review's SMELL
(public-path barrier tests, bound stale cell); that is closure/aggregate
material, not part of this repair.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility; repairs 2 and 3 need tests that fail on the frozen
  input (repair 1 is gated by Clippy itself).
- The exact checked projection table is unchanged: `TimedOut` and
  `SettlementRefused` still project `Unknown`; no new path may produce
  `Complete`.
- Deadline expiry still leaves zero live waiters and no blocking
  threads.
- All existing bridge-api tests keep passing unchanged (Wiremock suites
  included); the old request adapter still compiles; production
  construction still assigns `resource_flight_route_v3 = None`.
- Run `cargo test -p bridge-api` (all harnesses) and
  `cargo test -p bridge-core --lib -- remote_request_flight` (unchanged),
  plus `git diff --check` and `cargo fmt --all -- --check`; no
  `rustfmt::skip`.
- Refresh the handoff: exact frozen input `05e9517e`, red evidence,
  honest churn accounting (additions plus deletions, post-format), and
  the statement that Tasks F-G and production V3 remain unarmed.
- Stop and report before exceeding **150 changed production lines or 400
  total changed lines** (churn convention, post-format) relative to
  `05e9517e`.

## Files

- `crates/bridge-api/src/backend.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): retain cleanup diagnostics and accept timed-out custody

## Round Contract

This dispatch is the single contracted targeted repair of the Task E
artifact. One hard-read-only Sol/xhigh closure review follows separately;
do not self-repair a rejection. Never restart from a fresh artifact and
never extend the cap.
