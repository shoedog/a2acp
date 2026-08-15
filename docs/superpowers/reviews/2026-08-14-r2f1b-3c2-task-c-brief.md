---
task-type: implement
---
# R2f1b 3c2 Task C: attempt lease, complete recovery, and outbox acknowledgement

## Description

Begin Task C on the exact accepted Task B head
`dbf514bd548f00ab4563d36ee48dcecf2cd343b8`. Implement the attempt lifetime
lease, the complete durable request-state recovery table, and the
idempotent publication outbox. The module stays unreachable outside tests —
no production caller, route, provider send, or V3 arming.

Own `crates/bridge-core/src/remote_request_flight.rs`, narrow
`crates/bridge-core/src/lib.rs` exports, focused colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify Task A surfaces, implement the Task D owned request driver or
observation, or touch API/HTTP code.

Implement, per the binding salvage design:

- **durable send-state rows:** extend the request-child grammar (strict
  decoding everywhere) with the send-state progression `Reserved ->
  IntentJournaled -> DispatchAuthorized -> ProviderSendArmed ->
  TerminalPendingPublication(result, prompt_may_have_been_accepted) ->
  PublicationAcknowledged(delivery_id)`, recorded through the Task A owned
  surfaces with only exact `Complete` advancing state. B1/B2's existing
  admission and pre-send-failure semantics remain; this progression also
  supplies the durable evidence that resolves B2's deliberately deferred
  below-checkpoint ambiguity (an admitted child's row now says which side
  of send it reached);
- **the attempt lifetime lease:** one production-capable constructor,
  `open_recovered(custody, attempt, capacity, publisher)`, which acquires
  an exclusive nonblocking lifetime lock on an already-existing child of
  the attempt root (create it at initialization, never on open), completes
  recovery and publication debt, and only then exposes admission. A second
  live opener returns a typed `AttemptLive` refusal without recovery or
  mutation. Admission never runs recovery. The existing `open`/
  `open_with_capacity` become test-only or delegate; state the choice.
  Lock order is fixed: lifetime lease, then the attempt admission mutex,
  then any per-request transition step, then the Task A operation lease;
  every lock is released before a publisher callback runs;
- **the complete recovery table:** on `open_recovered`, every durable
  prefix resolves exactly as designed — `Reserved`, `IntentJournaled`, and
  `DispatchAuthorized` recover as `Failed` with
  `prompt_may_have_been_accepted = false` (the first-poll fence in Task D
  will guarantee no provider code ran); `ProviderSendArmed` recovers as
  `Unknown` with `accepted = true`; `TerminalPendingPublication` replays
  the durable CAS winner idempotently; `PublicationAcknowledged` retires
  without republishing; an invalid order, identity, digest, or schema
  refuses the entire attempt with every byte preserved. Recovery never
  reconstructs or resends provider authority;
- **the idempotent publication outbox:** a
  `RemoteRequestResultPublisherV1` trait with `publish_idempotent`
  receiving the terminal publication (delivery identity binding the
  attempt, ordinal, and request id; result; accepted flag). There is no
  no-op implementation. The acknowledgement must echo the exact delivery
  identity; a mismatched or refused acknowledgement leaves the terminal
  outbox pending and blocks admission from the reopened attempt until it
  drains. The sink contract (durable dedup on delivery id) is documented
  as the consumer's obligation; recovery may call the publisher again
  after a crash — exactly-once holds at the sink, not the call count;
- **rider (accepted B1 closure, binding):** the request authority now
  binds attempt identity and ordinal privately alongside the request id;
  every publication/acknowledgement key uses the full binding so two
  attempts can never alias on a colliding request id.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. A compile failure counts only when it is specifically the
  missing Task C API; zero selected tests does not.
- Every durable prefix has a recovery regression with the exact prescribed
  result and accepted flag, each proving no provider resend, no duplicate
  publication, and byte-preservation on refusal paths.
- A second live opener (same process via a second custody handle, and the
  reopened-after-drop case) cannot recover, admit, or mutate; the lease
  releases on drop and a subsequent open succeeds.
- Recovery precedes admission structurally: no admission path is reachable
  on an instance whose recovery has not completed.
- Crash before the publisher call, sink-commit-before-ack, mismatched ack,
  and refused ack each have regressions: pending outbox blocks admission;
  replay after crash publishes idempotently by delivery identity.
- Old/corrupt/foreign roots refuse without mutation (extend the existing
  refusal matrix to the new rows).
- The authority binding rider has a regression: identical request ids in
  two attempt roots produce non-aliasing authorities and delivery
  identities.
- All existing B1/B2, Task A, and legacy tests keep passing unchanged
  except tests that must migrate to the new constructor; migrations are
  listed in the handoff with cause.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `dbf514bd`, red evidence, honest
  churn accounting (additions plus deletions, post-format; production
  deletions can never exceed the file's total deletions), and the
  statement that Tasks D-G and production V3 remain unarmed.
- Stop and report a split before exceeding **500 changed production lines
  or 900 total changed lines** (churn convention) relative to `dbf514bd`.
  If recovery and outbox cannot both fit, land recovery first and name the
  outbox remainder as C2 — never expose admission between them.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/lib.rs` (narrow exports only)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the B1/B2 implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): add attempt lease, recovery table, and outbox

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks Task C. Never restart from a fresh
artifact and never silently extend the cap.
