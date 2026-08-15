---
task-type: code-review
---
# R2f1b 3c2 Task D closure review

## Description

Perform the one declared hard-read-only closure review of the complete Task
D line: exact diff `832221c9..08aa5531` in this checkout, where `832221c9`
is the accepted Task C head and `08aa5531` is the current head. Review the
full diff and the driver/wrapper/settlement/observation surfaces in
context. Do not edit, build, test, invoke another provider, or access the
network. This review round is capped at one; no repair loop is authorized
inside it.

The line contains three commits:

1. `bd29eddf` — the Task D implementation (428 production / 735 total
   churn, in caps): the owned request driver (`RemoteRequestDriverV1`,
   `OwnedRemoteRequestV1`) with ordered durable transitions
   (`journal_intent`, `authorize_dispatch`), the provider-agnostic arming
   wrapper (`ArmedProviderSendV1`) appending `ProviderSendArmed` durably
   before the inner future's first poll, durable-CAS-winner settlement
   driving the Task C outbox with locks released before publisher
   callbacks, `tokio::sync::watch` observation with deadline-bound waits
   and zero residual waiters, and drop-safe refusal debt. Its advisory
   review found one blocker.
2. `a072aacb` — the one declared targeted repair (7 production / 200 total
   churn): on every non-`Complete` arming outcome, the wrapper — without
   polling the inner future — settles the durable terminal as pre-send
   `Failed` with `accepted = false`; only if that settlement also fails
   does the conservative post-arm `Unknown, accepted = true` stand
   (documented residual). Its advisory review confirmed the fix but found
   the supporting CAS widening allowed ANY unaccepted settlement to
   consume an armed row.
3. `08aa5531` — a disclosed operator completion (+71/−5), red-first (the
   stale-flag regression published `accepted = false` over a durably armed
   row on the pre-change head): the armed-row allowance is now a private
   `failed_arm` privilege held only by the arming wrapper's zero-poll
   failure branch; public `settle`, drop, and every journal/recovery path
   refuse `InvalidStateTransition` when unaccepted settlement meets an
   armed row.

Operator adjudications you must independently judge:

1. The conservative residual: if the wrapper's pre-send `Failed`
   settlement itself fails after an effect-then-debt arming append, the
   armed row stands un-terminalized and recovery reports
   `Unknown, accepted = true` — crash-equivalent conservatism in the safe
   direction (never false not-sent). Judge acceptability.
2. The zero-poll privilege scoping: the wrapper positively owns the inner
   future and has not polled it when the privilege is exercised; judge
   that no other reachable path can obtain the privilege and that drop
   cannot race its own arming (single ownership).

Required judgments:

- First-poll fence: no reachable schedule polls the inner future before
  the armed row is durable; a failed durable append destroys the inner
  future unpolled.
- Recovery truthfulness: pre-poll cuts recover `Failed, false`; post-arm
  crashes recover `Unknown, true`; the effect-then-debt live path settles
  `Failed, false` when it can and falls back conservatively when it
  cannot; no path can durably misreport an accepted send as unaccepted.
- CAS-winner discipline under racing settlements, drop, and recovery;
  exactly one terminal row; all observers converge on the winner.
- Observation: deadline expiry leaves zero live waiters, no blocking or OS
  threads, and no held journal/admission lock during waits.
- Refusal debt: refused publication survives drop; reopen sees
  pending-publication; admission stays blocked until the outbox drains.
- Peer isolation: two live requests cannot signal, settle, or clear each
  other; stale authorities refuse.
- Scope: only the owned module, narrow exports, and the handoff changed;
  no production caller, provider integration, process/container semantic
  change, or V3 arming; `Cargo.lock` unchanged; no `rustfmt::skip`.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `08aa5531`, clean worktree, branch `implement/impl-2561-z43q222o`;
- all three in-container verifies on this line were fully green;
- full `bridge-core --lib` at head: **656 passed / 0 failed**;
- operator host gates on exact `08aa5531` all exit 0: `git diff --check`,
  formatter, locked all-target/all-feature workspace check and Clippy with
  `-D warnings`, full locked all-feature workspace test **4,070 passed /
  0 failed / 13 ignored across 90 harnesses**, locked release build,
  `cargo deny check`, and repository hygiene 40 tracked / 8 configs.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must name
  a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the round-1 blocker and the repair-round blocker
  as FIXED, PARTIAL, or OPEN against the shipped line.
- Judge the two operator adjudications.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/lib.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the Task D implementer statements)
- repository `AGENTS.md`
