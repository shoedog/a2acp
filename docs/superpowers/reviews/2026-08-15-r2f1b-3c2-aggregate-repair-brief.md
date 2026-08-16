---
task-type: implement
---
# R2f1b 3c2 owner-authorized aggregate repair: captured-checkpoint recovery

## Description

Perform the one owner-authorized repair from the 3c2 aggregate dual-lens
round. The frozen input is exact commit
`50f3336e4260f9e2bc3b6894eae6c6921baf4241` (the accepted final head of
all eleven implementation rounds). One confirmed cross-module blocker;
nothing else changes. Production remains `LegacyV2` with the V3 route
unarmed.

Own `crates/bridge-core/src/remote_request_flight.rs` (the
`open_base`/`authorize_checkpoint` region and colocated tests), narrow
READ-ONLY inspection accessors on
`crates/bridge-core/src/namespace_transaction.rs` ONLY if the
inspection below cannot be built from existing surfaces (the Task C
narrow-accessor precedent: read-only, no mutation authority), and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.

The blocker (operator-confirmed at every link): the checkpoint-replace
transaction (`NamespaceTransactionV2::replace` on the ordinary
checkpoint — used by ordinary admission checkpoint advancement AND B2
orphan healing) has a durable `TransitionV2::Captured` window: the
replacement intent exists with its content commitment, the ordinary
checkpoint has been renamed to the intent's capture name, and the
successor is staged but unpublished. A crash there is legitimate and
the namespace recovery handles it — but `open_base` calls
`authorize_checkpoint` BEFORE `NamespaceTransactionV2::recover`, and
the absent-checkpoint branch refuses `Malformed("checkpoint is
absent")` with no transaction-awareness. The recovery that would repair
the state is unreachable; every reopen repeats the refusal; the journal
is permanently bricked.

Implement exactly one repair:

1. **Transaction-aware absent-checkpoint authorization.** When the
   ordinary checkpoint name is absent, perform a READ-ONLY transaction
   inspection before refusing: accept the state ONLY when there is
   exactly ONE transaction intent and it targets the checkpoint name,
   its capture child is present, the captured bytes validate as THIS
   attempt's checkpoint (the same identity-chain digest and attempt
   validation `validate_checkpoint` applies today — the predecessor
   capture is the identity source), and the staged successor (if
   present) matches the intent's content commitment; additionally
   validate every ordinary request row residue-tolerantly (the Task C
   property: no recovery mutation before full-row validation). Only
   when ALL of that holds, invoke
   `NamespaceTransactionV2::recover(...)` and then re-run the ordinary
   checkpoint authorization against the recovered namespace,
   preserving the existing strict post-recovery scan unchanged. Every
   other absent-checkpoint state — no intent, multiple intents, an
   intent targeting another name, a missing/foreign/corrupt capture
   (wrong attempt, wrong digest), a commitment mismatch — refuses
   byte-preserved exactly as today. The inspection is read-only until
   every validation passes; no new authority source is introduced.

Red regressions (integrated crash-cuts at the REAL call sites — the
existing namespace `TransitionV2` hook seams simulate the interruption
with the disk left in the captured state; the existing B2 seam that
injects only after the checkpoint adapter returns is NOT sufficient):

- interrupt the real ordinary-admission checkpoint advancement at
  `TransitionV2::Captured`, drop all handles, reopen: on the frozen
  input reopen refuses `Malformed("checkpoint is absent")` forever; on
  the repaired tree reopen succeeds, the checkpoint is advanced (or
  rolled back per the recovery's own rules), and a SECOND reopen also
  succeeds;
- the same crash-cut through the B2 orphan-heal replacement, asserting
  the healed orphan lands `PreSendFailure` and the journal reopens
  twice;
- a foreign/corrupt captured checkpoint (wrong attempt identity or
  digest) and a multiple-intent state must refuse byte-preserved with
  NO recovery mutation (assert the namespace is unchanged after the
  refusal).

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands
  and admissibility; the two crash-cut reds must fail behaviorally on
  the frozen input at the reopen assertion.
- The Task C validation-before-recovery property is preserved: no
  recovery mutation before full ordinary-row validation, in BOTH the
  ordinary and the absent-checkpoint paths.
- All existing Task A-G tests keep passing unchanged; the strict
  post-recovery scan, capacity checks, foreign-attempt and
  digest-mismatch refusals are byte-preserved.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction fs_custody`, plus `git diff --check` and
  `cargo fmt --all -- --check`; no new `rustfmt::skip`.
- Refresh the handoff: exact frozen input `50f3336e`, red evidence,
  honest churn accounting (additions plus deletions, post-format), and
  the statement that production V3 remains unarmed.
- Stop and report before exceeding **150 changed production lines or
  450 total changed lines** (churn convention, post-format) relative to
  `50f3336e`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- `crates/bridge-core/src/namespace_transaction.rs` (read-only
  inspection accessors only if strictly needed)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): recover a captured checkpoint before authorization refuses

## Round Contract

This dispatch is the single owner-authorized aggregate repair. One
bounded hard-read-only Sol/xhigh re-review of the fix region follows
separately; do not self-repair a rejection. Never restart from a fresh
artifact and never extend the cap.
