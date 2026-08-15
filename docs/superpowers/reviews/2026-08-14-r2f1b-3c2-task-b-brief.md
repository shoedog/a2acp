---
task-type: implement
---
# R2f1b 3c2 Task B: request journal, atomic admission, and bounded retirement

## Description

Begin the request-flight sequence on the exact accepted Task A head
`d8ec93ad4a03a29d6da80c4fdf9fa818c8572459`. Implement only Task B from the
binding salvage plan: the request child/checkpoint grammar with atomic
admission and bounded retirement. This module must be unreachable outside
tests — no production caller, route, or V3 arming.

Own a new `crates/bridge-core/src/remote_request_flight.rs`, the narrow
`crates/bridge-core/src/lib.rs` export, focused colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify the accepted Task A surfaces (`fs_custody.rs`,
`namespace_transaction.rs`) except — if strictly necessary — narrow
`pub(crate)` visibility widening with zero behavior change; record any such
line in the handoff. Do not implement Task C recovery/lease/outbox, the
request driver, API/HTTP work, provider sends, or migrations of any shared
journal.

Implement, building strictly on the accepted Task A owned-journal and
transaction surfaces (`JournalRootBindingV2`/`JournalRootCustodyV2`/
`JournalRootOperationV2` journal mutators, `NamespaceTransactionV2`, and
their protective outcome lattices):

- the request child grammar: one request child is both the reservation and
  the journal — there is no separate durable reservation file and therefore
  no zero-row reservation state in this format. The initial complete row
  (authority, canonical `DedicatedRemoteRequestIdV1`, owner) is written to a
  private temporary child, synced, renamed no-replace to its final
  authority name, and followed by a root sync, all through the Task A owned
  surfaces;
- the checkpoint child: schema, exact attempt identity, `next_ordinal`, and
  a chain/identity digest; strict decoding with `deny_unknown_fields`;
  malformed, foreign-schema, or digest-mismatched checkpoints refuse
  protectively without mutation;
- admission, under one attempt-level mutex: (1) validate the checkpoint and
  a bounded child census; (2) refuse before ID mint or any mutation when
  4,096 active request children exist or the census plus this admission's
  footprint would exceed the bound; (3) allocate the ordinal by checked
  arithmetic from the checkpoint and the active maximum; (4) atomically
  publish the initial child; (5) atomically advance and sync
  `next_ordinal`; (6) return the non-cloneable request authority only after
  both publications. If step 5 fails, no authority returns; reopen advances
  the checkpoint from the validated child and closes it as a pre-send
  failure;
- bounded retirement: acknowledged terminal children are unlinked
  (identity-checked, via the Task A surfaces) and the root synced; a crash
  before unlink sees the acknowledgement and retires without republishing;
  a crash after unlink leaves no debt;
- protective consumption: only exact `Complete` outcomes from the Task A
  surfaces may advance the checkpoint or acknowledge retirement;
  `Retained`, `Unknown`, `Unsupported`, `ProtectiveDebt`, or any refusal
  blocks the attempt with a typed protective outcome — no flattening, no
  `is_success` helper;
- enumeration reads at most capacity plus one so a corrupt over-cap root
  refuses explicitly; a root containing old 3c2-era request-journal state
  (the pre-salvage `FileResourceFlightJournal` shapes) returns a typed
  `LegacyMigrationRequired`-style refusal without mutation.

## Acceptance Criteria

- Begin with focused red tests. Record the exact pre-change red commands
  and why each observation is admissible. A compile failure counts only
  when it is specifically the missing Task B API; zero selected tests does
  not.
- No zero-row reservation exists at any admission crash cut: interrupt at
  every boundary (temp write, temp sync, no-replace publication, root sync,
  checkpoint advance, checkpoint sync) and prove reopen either sees a
  complete authoritative child or clean recoverable residue — never an
  empty or partial authority.
- Capacity refuses before ID mint and before any mutation at the 4,096
  bound, with the exact positive boundary admitted.
- More than 4,096 sequential admit-acknowledge-retire cycles succeed
  (retirement frees capacity; use a reduced injected cap if the full count
  is impractical, and say so in the handoff).
- Corrupt, over-cap, foreign, and legacy-format censuses refuse
  protectively without mutation.
- The checkpoint-before-return and ack-before-unlink restart schedules
  self-heal: replaying admission after a step-5 crash closes the orphan
  child as a pre-send failure and advances the checkpoint; replaying
  retirement after an ack-persisted crash retires without republishing.
- A protective outcome injected from the Task A surface at any consumption
  point blocks the attempt and cannot advance the checkpoint or acknowledge
  retirement.
- The module has no production caller: repository-wide search shows only
  the `lib.rs` export and colocated tests reference it.
- Run the focused selector `cargo test -p bridge-core --lib --
  remote_request_flight` plus
  `cargo test -p bridge-core --lib -- namespace_transaction custody_v2 fs_custody journal`.
- Run `git diff --check` and `cargo fmt --all -- --check`; no
  `rustfmt::skip` anywhere; caps are measured under normal formatting.
- Refresh the handoff: exact frozen input `d8ec93ad`, red evidence, changed
  paths, production/test line counts, exclusions, and the explicit
  statement that Tasks C-G and production V3 remain unarmed.
- Line caps, churn convention — additions plus deletions both count,
  measured post-format: stop and report a split before exceeding **500
  changed production lines or 900 total changed lines** relative to
  `d8ec93ad`. If retirement cannot fit, land the journal grammar first and
  name the retirement remainder as B2 in the handoff instead of exceeding
  the cap.

## Files

- `crates/bridge-core/src/remote_request_flight.rs` (new)
- `crates/bridge-core/src/lib.rs` (narrow export only)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the Task A implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): add request journal admission and retirement

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks Task B. Never restart from a fresh
artifact and never silently extend the cap.
