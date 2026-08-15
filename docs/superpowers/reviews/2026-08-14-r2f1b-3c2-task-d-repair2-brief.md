---
task-type: implement
---
# R2f1b 3c2 Task D owner-authorized second repair

## Description

Perform the one owner-authorized additional repair of the Task D artifact.
The frozen input is exact commit `08aa553173eb5bd05bfbda4547ec49d0cf482656`.
Two confirmed concurrency blockers from the closure review; nothing else
changes.

Own `crates/bridge-core/src/remote_request_flight.rs`, focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

Implement exactly two repairs:

1. **Linear send permit (duplicate-wrapper privilege misuse).**
   `arm_provider_send` takes `&self` with no request-wide one-shot, so a
   second wrapper whose arm fails (`InvalidStateTransition` — the row is
   already armed) enters the zero-poll privileged branch and can durably
   publish `Failed, accepted=false` over a genuinely sent request. Repair:
   a request-wide, irreversible send-wrapper claim — only the first wrapper
   to claim may arm and may ever use the `failed_arm` privilege; a later
   wrapper destroys its own inner future unpolled and returns a typed
   refusal with zero row effect. Red: poll wrapper A through an
   accepted/`Pending` inner future, then poll wrapper B — B must refuse
   without publishing anything, and recovery must remain
   `Unknown, accepted=true`; on the frozen input B durably publishes
   `Failed,false`.
2. **Joinable publication flight (false-success settlement race).** A
   settler that loses the `publication_claimed` race currently returns
   `Ok(outcome)` while the winner's publication may still refuse — false
   success with durable debt remaining; a stale `publication_complete`
   precheck can also let a late claimant republish after retirement.
   Repair: replace the boolean claim with a joinable publication flight —
   every concurrent settler receives the SAME completed publication result
   (success or the winner's refusal); completion is rechecked after any
   claim acquisition so a post-retirement claimant neither republishes nor
   errors. Follow the codebase's established one-flight join precedent
   (observed-release joins in the container/reaper work). Red: (a) a
   barrier-controlled refusing publisher — no racer may return `Ok`; both
   settlers surface the refusal and the row stays pending; (b) a success
   race — no republish, no error after retirement, both settlers observe
   the same completed result.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility; both repairs need tests that fail on the frozen input.
- The permit is irreversible and request-scoped: drop of a losing wrapper
  or of the winning wrapper's future does not release it; the winner's
  normal settlement paths are unchanged.
- All existing Task A/B/C/D and legacy tests keep passing unchanged except
  any that pinned the two defective behaviors, migrated with cause.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `08aa5531`, red evidence, honest
  churn accounting, and the statement that Tasks E-G and production V3
  remain unarmed.
- Stop and report before exceeding **150 changed production lines or 400
  total changed lines** (churn convention, post-format) relative to
  `08aa5531`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): one send permit per request and joinable publication

## Round Contract

This dispatch is the single owner-authorized additional repair of the Task
D artifact. One hard-read-only Sol/xhigh closure review follows separately;
do not self-repair a rejection. Never restart from a fresh artifact and
never extend the cap.
