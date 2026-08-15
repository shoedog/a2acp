---
task-type: implement
---
# R2f1b 3c2 Task F targeted repair

## Description

Perform the one contracted targeted repair of the Task F artifact. The
frozen input is exact commit
`f17e2958b934534173f60f90e4fe71de070a338a`. Two confirmed blockers from
the implementation review plus one documentation correction; nothing
else changes. Production remains `LegacyV2` with the V3 route unarmed.
The F2 split (old adapter private and unreferenced, removal deferred to
F2) stands — do not attempt the removal here.

Own `crates/bridge-api/src/backend.rs`, the narrowly-annotated retained
adapter lines in `crates/bridge-core/src/process.rs` and
`crates/bridge-core/src/retained_resource_flight.rs`, focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement exactly these repairs:

1. **Acceptance-keyed pre-send disposition.** `begin_dispatch()` sets
   `dispatched=true` at dispatch AUTHORIZATION (journal intent +
   authorize), but arming is durable only at the send future's first
   poll. The drop and cancellation exits key their proposed disposition
   on `dispatched`, so an exit after authorization but before the first
   poll settles the UNARMED row as `Partial,false` (cancellation) or
   `Unknown,false` (drop) — contradicting the binding recovery table's
   pre-send `Failed, accepted=false`. Repair: choose the terminal
   disposition from the acceptance/arming marker, not `dispatched` —
   every unaccepted exit records `Failed,false`; accepted
   cancellation/drop keeps `Partial`/`Unknown` with `accepted=true`.
   Red: a deterministic V3 test that reaches dispatch authorization,
   then cancels (and a drop variant) before the send future is first
   polled; assert zero HTTP requests and the durable `Failed,false`
   result. On the frozen input the row records `Partial`/`Unknown`.
2. **Clippy gate for the F2 split.** The retained private adapter fails
   the workspace `-D warnings` gate as dead code (the operator
   host-enumerated population is the complete list; the supplied
   verifier reported seven `dead_code` errors across the process.rs
   adapter symbols and `attach_remote_request_owner`). Repair: the
   narrowest possible `#[allow(dead_code)]` annotations scoped to
   exactly the retained adapter items, each with a one-line comment
   naming F2 as the removal point. Do not blanket-allow at module or
   crate level; do not perform the F2 removal. Regression gate:
   workspace all-target all-feature Clippy with `-D warnings` exits 0.
3. **Handoff accounting correction.** The handoff states 367 production
   lines; addition-plus-deletion counting over the production hunks
   gives 371 (351 backend + 6 config + 8 process + 6 remote-request).
   Correct the number and state the counting convention.

Do not add the deferred API-level hardening from the review's first
SMELL (first-poll ordering at the real reqwest future;
rejected/mismatched-echo acknowledgement projecting `Unknown`) beyond
what repair 1's red tests require; that is closure/aggregate material.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands
  and admissibility; repair 1 needs tests that fail behaviorally on the
  frozen input (repair 2 is gated by Clippy itself).
- The recovery table is unchanged and now honored live: unaccepted
  exits are `Failed,false`; `ProviderSendArmed` remains
  `Unknown, accepted=true`; the exact-echo acknowledgement path is
  untouched.
- Zero-round / never-polled admission behavior from the base commit is
  preserved; cancellation between rounds still prevents the successor
  send.
- All existing bridge-api and bridge-core tests keep passing unchanged
  except any that pinned the two defective dispositions, migrated with
  cause.
- Run `cargo test -p bridge-api` (all harnesses),
  `cargo test -p bridge-core --lib -- remote_request_flight`, plus
  `git diff --check` and `cargo fmt --all -- --check`; no
  `rustfmt::skip`; workspace Clippy `-D warnings` green.
- Refresh the handoff: exact frozen input `f17e2958`, red evidence,
  honest churn accounting (additions plus deletions, post-format), the
  corrected Task F numbers, and the statement that F2, Task G, and
  production V3 remain unarmed.
- Stop and report before exceeding **100 changed production lines or
  300 total changed lines** (churn convention, post-format) relative to
  `f17e2958`.

## Files

- `crates/bridge-api/src/backend.rs`
- `crates/bridge-core/src/process.rs` (allow annotations on retained
  adapter items only)
- `crates/bridge-core/src/retained_resource_flight.rs` (same)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/plans/2026-08-12-r2f1b-3c2-salvage-redesign.md`
  (section F — binding, including the F2 split clause)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): acceptance-keyed pre-send disposition and scoped F2 allows

## Round Contract

This dispatch is the single contracted targeted repair of the Task F
artifact. One hard-read-only Sol/xhigh closure review follows
separately; do not self-repair a rejection. Never restart from a fresh
artifact and never extend the cap.
