---
task-type: implement
---
# R2f1b 3c2 Task B2 targeted repair

## Description

Perform the one declared targeted repair of the Task B2 candidate. The
frozen input is exact commit `6115c93e78dd1bd35b0fcd56139e52f23d1dc5df`.
This is a bounded repair on the same artifact; the delivered retirement
grammar, sequential throughput, and riders stay as shipped except where the
confirmed findings below require change.

Own `crates/bridge-core/src/remote_request_flight.rs`, focused colocated
tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify Task A surfaces — the operator explicitly adjudicated the
mid-retire permanent-`Retained` semantics as the accepted, owner-ledgered
Task A residual; do not "fix" it.

Implement exactly four repairs:

1. **Heal only the proven orphan (confirmed WRONG).** Reopen currently
   rewrites every active child as a pre-send failure. Repair: heal exactly
   one child, the one whose ordinal equals `checkpoint.next_ordinal` — the
   only child that provably never returned authority (the
   checkpoint-advance-crash schedule). Active children at ordinals below
   `next_ordinal` were admitted with authority returned; their send state
   is ambiguous until Task C's durable send rows exist, so reopen leaves
   them untouched and active. A census with gaps, duplicates, or ordinals
   at or beyond `next_ordinal` other than the unique orphan refuses
   protectively without mutation. Red: (a) successful admission, crash,
   reopen — the issued child remains byte-identical and active, and the
   checkpoint is unchanged; (b) the step-5 orphan still heals as before;
   (c) gapped and multiple-ahead censuses refuse with preserved root
   bytes.
2. **Authorize before recovering (confirmed WRONG).** `open` runs Task A
   recovery before the request grammar or attempt identity is validated.
   Repair: first read the checkpoint child non-mutatingly and validate
   schema, digest, and attempt identity; `ForeignAttempt` or `Malformed`
   refuses with the root byte-identical, before any recovery or other
   mutation. Only a checkpoint proven to belong to this attempt admits the
   recovery pass. A root with no checkpoint at all remains whatever the
   shipped semantics prescribe (state it in the handoff). Red: a
   foreign-attempt root containing an interrupted Task A transaction
   refuses with byte-identical root bytes.
3. **Mid-retire crash surface (coverage only).** Add regressions proving
   B2 reopen at the Task A mid-retire protective cuts (post-unlink,
   post-zero-link) surfaces a typed protective refusal without mutation,
   authority, or panic. The permanent protective retention itself is the
   accepted Task A semantics — pin it, do not change it, and reference the
   owner-ledgered residue-disposition item in the handoff.
4. **Real-adapter fault injection (review SMELL-1, repeat class).** The
   fault boundary currently returns pre-mapped results without calling the
   production adapters, so adapter regressions stay green. Replace it with
   raw Task A outcome/fault injection consumed through the production
   mapping paths, covering publish and actual removal cuts. Also add the
   owner-validation oversized (`WIRE_CAP + 1`) and control-character
   branch tests with executed red evidence (SMELL-2).

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. Repairs 1 and 2 each need at least one test that fails on
  the frozen input.
- All existing B1/B2, Task A, and legacy tests keep passing unchanged
  except tests that pinned the two defective behaviors.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `git diff --check`, and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `6115c93e`, red evidence, honest
  churn accounting versus both `6115c93e` and `6033fd34`, the no-checkpoint
  semantics statement, the ledger reference for the mid-retire residual,
  and the statement that Tasks C-G and production V3 remain unarmed.
- Stop and report before exceeding **150 changed production lines or 400
  total changed lines** (churn convention, post-format) relative to
  `6115c93e`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): scope reopen healing and authorize before recovery

## Round Contract

This dispatch is the single declared targeted repair of the Task B2
artifact. One hard-read-only Sol/xhigh closure review follows separately;
do not self-repair a rejection. Never restart from a fresh artifact and
never extend the cap.
