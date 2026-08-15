---
task-type: implement
---
# R2f1b 3c2 Task B2: acknowledged retirement and reopen self-healing

## Description

Complete Task B on the exact accepted B1 head
`6033fd34fccb2fb8fbbb45585df5472eb95331df`. Implement the B2 remainder named
by the authorized B1 split: acknowledged retirement, reopen self-healing,
and the sequential-throughput proof, plus the two carry-forward riders from
the accepted B1 closure. The module stays unreachable outside tests — no
production caller, route, or V3 arming.

Own `crates/bridge-core/src/remote_request_flight.rs`, focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify Task A surfaces, `ids.rs`, or `lib.rs` beyond what already
exists; do not implement Task C's lifetime lease, recovery table for
send-state rows, or the publication outbox — B2 owns only the journal-level
retirement/reopen mechanics.

Implement:

- **terminal acknowledgement grammar:** extend the request-child wire with
  the minimal state needed to mark a child's terminal result acknowledged
  (strict decoding preserved end-to-end; unknown fields refuse at every
  nesting level). The full send-state machine (`IntentJournaled` through
  `TerminalPendingPublication`) is Task C/D scope — B2 records only the
  journal-level acknowledged marker via the Task A owned surfaces (append
  or replace through `NamespaceTransactionV2`), consuming only exact
  `Complete` outcomes;
- **acknowledged retirement:** an acknowledged child retires by
  identity-checked removal through the Task A surfaces plus root sync;
  retirement frees capacity; only exact `Complete` acknowledges the
  retirement; every protective or refused outcome blocks with a typed
  refusal and no flattening;
- **reopen self-healing:** `open` (or a dedicated reopen pass under the
  operation lease) heals the two prescribed restart schedules — (a) a
  crash after initial-child publication but before checkpoint advance:
  reopen validates the orphan child, advances the checkpoint from it, and
  closes it as a pre-send failure; (b) a crash after acknowledgement but
  before unlink: reopen sees the acknowledged marker and retires without
  republishing; a crash after unlink leaves no debt. Reopen never mints,
  never re-issues authority, and refuses protectively on any ambiguous
  residue;
- **sequential throughput:** more than capacity-many sequential
  admit-acknowledge-retire cycles succeed on one root (use a reduced
  injected capacity if the full 4,096 is impractical and say so in the
  handoff);
- **rider 1 (B1 closure SMELL-1):** real fault seams at the Task A call
  boundaries so `Refused`, `Retained`, `Unsupported`, and I/O-`Unknown`
  outcomes are each injected at least once at a real `stage`/`publish`/
  append/replace/removal boundary (not only the test-only mapper), plus
  child schema/digest/name corruption refusal cases — each proving no
  authority, no checkpoint advance, and preserved root bytes;
- **rider 2 (B1 closure SMELL-2):** owner validation — a shared check
  refusing an empty or malformed `ResourceFlightOwnerV1` before mint and
  during census decoding, with red tests proving zero mint and preserved
  root bytes.

Recorded for the handoff, not for implementation: the attempt-bound
authority identity (B1 closure SMELL-3) is Task C scope, to land before any
consumer integration.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. A compile failure counts only when it is specifically the
  missing B2 API; zero selected tests does not.
- Both restart schedules self-heal exactly as prescribed, with every crash
  cut between acknowledgement, unlink, and root sync pinned (complete child
  or classified recoverable state — never a duplicate authority, duplicate
  publication, or silent loss).
- Retirement frees capacity: the sequential-throughput test proves more
  than capacity-many cycles on one root with the checkpoint monotonic
  throughout.
- Reopen after the step-5 orphan schedule closes the orphan as a pre-send
  failure and advances the checkpoint; replaying reopen is idempotent.
- Both riders land with their red evidence.
- All existing B1, Task A, and legacy tests keep passing unchanged.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `6033fd34`, red evidence, the
  production/test split versus both `6033fd34` and `d8ec93ad` (honest churn
  accounting — additions plus deletions; production deletions can never
  exceed the file's total deletions), and the statement that Tasks C-G and
  production V3 remain unarmed.
- Stop and report a split before exceeding **350 changed production lines
  or 700 total changed lines** (churn convention, post-format) relative to
  `6033fd34`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the B1 implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): retire acknowledged requests and heal reopen

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks B2. Never restart from a fresh
artifact and never silently extend the cap.
