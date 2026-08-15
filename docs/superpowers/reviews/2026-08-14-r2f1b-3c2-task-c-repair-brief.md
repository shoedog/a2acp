---
task-type: implement
---
# R2f1b 3c2 Task C targeted repair

## Description

Perform the one declared targeted repair of the Task C candidate. The
frozen input is exact commit `4db414f08b96541d8471707b4143903d7a4a75e6`.
This is a bounded repair on the same artifact; the delivered send-state
grammar, recovery table, outbox, and authority binding stay as shipped
except where the two confirmed findings below require change.

Own `crates/bridge-core/src/remote_request_flight.rs`, one narrow
operator-authorized addition to `crates/bridge-core/src/fs_custody.rs`
described in repair 2 (nothing else in Task A changes), focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement exactly two repairs plus the folded evidence items:

1. **Lease-aware capacity headroom (confirmed WRONG).** The permanent
   lease child is not counted in `ADMISSION_FOOTPRINT`, so at maximum
   occupancy admission publishes a child whose checkpoint replacement is
   then refused on headroom, stranding the attempt with protective debt it
   cannot self-heal. Repair: account for the lease across every admission
   and reopen headroom computation (effectively four entries of admission
   headroom), and make reopen able to heal the interrupted positive-edge
   state. Red: (a) construct the exact maximum-occupancy state and require
   the next admission to refuse before mint or any mutation; (b) interrupt
   a positive-edge admission before checkpoint advance and require reopen
   to heal it; the existing cap-edge test must go red before the repair
   and be migrated to the corrected edge.
2. **Lease before operation lock (confirmed WRONG).** `open_recovered`
   currently takes the Task A operation lock before trying the lifetime
   flock, so a second opener contending with a live instance's operation
   returns `TaskA(Unknown)` instead of the typed `AttemptLive`, and the
   declared lock order (lifetime lease first) is violated. Repair: the
   operator authorizes ONE narrow `fs_custody` addition — a
   `pub(crate)` route-proved accessor on `JournalRootCustodyV2` that
   opens an existing regular child by exact name (no-create, no-follow,
   identity-verifiable) and nonblocking-flocks it, without taking the
   operation lock; it grants no mutation authority and exposes no path.
   `open_recovered` uses it to acquire the lifetime lease FIRST; only
   after the lease is held does any Task A operation begin. A held lease
   elsewhere returns exact `AttemptLive` with zero mutation. Red: block an
   admission closure while it holds the Task A operation guard,
   concurrently `open_recovered` through a second custody handle, and
   require exact `AttemptLive`, unchanged bytes, lease release on drop,
   and subsequent successful open. Add a lock-order regression proving the
   lease flock precedes any operation acquisition (ordering token,
   B2-mutex-test style).
3. **Folded evidence items (review SMELL, low cost):** behavioral
   fail-first evidence for at least the recovery-mapping and
   acknowledgement branches via bounded mutation-style controls recorded
   in the handoff; exact (not either/or) assertions in the
   refusal-versus-mismatch acknowledgement regression; a `PreSendFailure`
   recovery case; and the named inventory of migrated test setups
   (`initialized`, `unchecked`, foreign-checkpoint) with causes.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. Repairs 1 and 2 each need tests that fail on the frozen
  input.
- The fs_custody addition is exactly one narrow accessor: no-create,
  no-follow, no path projection, no mutation authority, colocated tests
  for wrong-identity/type refusal and contention; everything else in Task
  A byte-unchanged.
- All existing Task A/B/C tests keep passing unchanged except the migrated
  cap-edge and setup tests, listed with cause.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `4db414f0`, red evidence, honest
  churn accounting, the fs_custody-addition disclosure, and the statement
  that Tasks D-G and production V3 remain unarmed.
- Stop and report before exceeding **180 changed production lines or 450
  total changed lines** (churn convention, post-format) relative to
  `4db414f0`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/fs_custody.rs` (the one authorized accessor only)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): take the attempt lease first and count it in headroom

## Round Contract

This dispatch is the single declared targeted repair of the Task C
artifact. One hard-read-only Sol/xhigh closure review follows separately;
do not self-repair a rejection. Never restart from a fresh artifact and
never extend the cap.
