---
task-type: implement
---
# R2f1b 3c2 Task D: owned request driver and bounded observation

## Description

Begin Task D on the exact accepted Task C head
`832221c905e3e32d541d311931a177637a2d0f28`. Implement the owned per-request
driver over the Task B/C journal: durable transition methods, the
first-poll admission token, durable-CAS-winner settlement, bounded async
observation, and refusal debt. The module stays unreachable outside tests —
no production caller, provider send, API/HTTP work, or V3 arming. Do not
remove or modify the old shared-flight request adapter elsewhere in the
tree (Task F owns that), and any change you believe necessary to shared
process/container semantics is a STOP for ownership adjudication, not an
edit.

Own `crates/bridge-core/src/remote_request_flight.rs`, narrow
`crates/bridge-core/src/lib.rs` exports, focused colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement:

- **the owned request value:** admission returns a non-cloneable owned
  request (extending the existing sealed authority) whose durable
  transition methods walk the Task C send-state rows in order —
  `journal_intent` (to `IntentJournaled`), `authorize_dispatch` (to
  `DispatchAuthorized`), then the arming step below — each recorded
  through the Task A surfaces with only exact `Complete` advancing, each
  refusing out-of-order transitions with a typed error and no namespace
  effect;
- **the first-poll admission token:** an arming wrapper that takes the
  provider-send future and returns a wrapped future which durably appends
  `ProviderSendArmed` immediately before the inner future's FIRST poll —
  structurally, no poll of the inner future may be possible before that
  row is durable; if the durable append fails, the inner future is never
  polled and the request settles pre-send `Failed` with
  `accepted = false`. The wrapper is provider-agnostic (generic over the
  future); no actual provider code enters this task;
- **durable-CAS-winner settlement:** exactly one terminal row wins per
  request (`TerminalPendingPublication` with the result and the accepted
  flag); every later settlement attempt — from the driver, drop, or
  recovery — returns the durable winner instead of rewriting it;
  settlement drives the Task C outbox (publish, exact acknowledgement,
  retirement) with every lock released before the publisher callback;
- **bounded async observation:** a `tokio::sync::watch`-based observer for
  the request's terminal outcome; deadline-bound waits leave zero live
  waiters and no blocking or OS thread after timeout; observation never
  holds any journal or admission lock;
- **refusal debt:** when settlement or publication is refused, dropping
  the owned request must not erase the debt — the request row stays
  pending-publication, the attempt's admission stays blocked per Task C,
  and reopen recovers it; drop itself performs no namespace mutation
  beyond what a normal settlement attempt is allowed.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. A compile failure counts only when it is specifically the
  missing Task D API; zero selected tests does not.
- Peer admission cannot affect a live request: two admitted requests
  transition and settle independently; a stale or dropped peer's control
  cannot signal, settle, or clear the other (extend the sealed-authority
  binding tests).
- Pre-poll recovery: crash cuts after `journal_intent` and
  `authorize_dispatch` but before arming recover `Failed` with
  `accepted = false`; a red test proves the inner future was never polled
  (poll-counting future).
- Post-arm recovery: a crash after the armed row recovers `Unknown` with
  `accepted = true`; the arming order is proven by a future whose first
  poll asserts the durable row already exists.
- Failed arming append: the inner future is never polled and the request
  settles pre-send `Failed` (inject via the existing Task A boundaries).
- Timeout observation leaves zero live waiters (assert observer/waiter
  count or channel receiver count after deadline).
- Settlement returns the durable winner under racing settlements (two
  tasks settle concurrently; exactly one terminal row; both observe the
  same winner).
- Drop with a refused publication retains the debt: reopen sees
  pending-publication and admission stays blocked until the outbox drains.
- All existing Task A/B/C and legacy tests keep passing unchanged.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `832221c9`, red evidence, honest
  churn accounting (additions plus deletions, post-format), and the
  statement that Tasks E-G and production V3 remain unarmed.
- Stop and report a split before exceeding **450 changed production lines
  or 850 total changed lines** (churn convention) relative to `832221c9`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/lib.rs` (narrow exports only)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the Task B/C implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): add owned request driver with first-poll arming

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks Task D. Never restart from a fresh
artifact and never silently extend the cap.
