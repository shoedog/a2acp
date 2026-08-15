---
task-type: implement
---
# R2f1b 3c2 Task D targeted repair

## Description

Perform the one declared targeted repair of the Task D candidate. The
frozen input is exact commit `bd29eddf4759f210306d27e3e25c2dd782f86cc2`.
One confirmed blocker: when the arming append publishes the
`ProviderSendArmed` row but returns a non-`Complete` protective outcome
(effect-then-debt), the wrapper correctly never polls the provider future,
but it leaves the armed row un-terminalized — a later recovery then reports
`Unknown` with `accepted = true` even though the live path positively knew
zero polls occurred.

Own `crates/bridge-core/src/remote_request_flight.rs`, focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Nothing else changes.

Implement exactly one repair:

- On every non-`Complete` outcome from the arming append, before surfacing
  the error and without ever polling the inner future, attempt to settle
  the request's durable terminal as pre-send `Failed` with
  `accepted = false` through the normal CAS settlement (which tolerates the
  armed row's presence or absence). If that settlement succeeds, recovery
  and observation report `Failed, accepted = false`. Only if the terminal
  settlement itself fails does the conservative post-arm `Unknown,
  accepted = true` stand as the crash-equivalent fallback — document that
  residual in the handoff. No transition ordering, observation, or outbox
  behavior changes otherwise.

## Acceptance Criteria

- Begin with a focused red test that fails on the frozen input: inject the
  effect-then-debt outcome at the arming append (armed row durably
  published, protective return), assert the inner future was polled zero
  times, then reopen and require recovery to report `Failed` with
  `accepted = false` — the frozen input reports `Unknown, true`.
- A second case injects failure of the terminal settlement as well and
  pins the conservative `Unknown, accepted = true` fallback with zero
  polls.
- All existing Task A/B/C/D and legacy tests keep passing unchanged.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `bd29eddf`, red evidence, honest
  churn accounting, the documented conservative residual, and the statement
  that Tasks E-G and production V3 remain unarmed.
- Stop and report before exceeding **80 changed production lines or 250
  total changed lines** (churn convention, post-format) relative to
  `bd29eddf`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): settle unarmed failures before recovery can overstate them

## Round Contract

This dispatch is the single declared targeted repair of the Task D
artifact. One hard-read-only Sol/xhigh closure review follows separately;
do not self-repair a rejection. Never restart from a fresh artifact and
never extend the cap.
