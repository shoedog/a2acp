---
task-type: implement
---
# R2f1b 3c2 Task B1 targeted repair

## Description

Perform the one declared targeted repair of the Task B1 candidate. The
frozen input is exact commit `2815259d3a7a3b2869f0968c33cea010a4a1ede1`.
This is a bounded repair on the same artifact, not a restart; the delivered
journal grammar, admission flow, and the authorized B1/B2 split stay as
shipped except where the six confirmed findings below require change.

Own `crates/bridge-core/src/remote_request_flight.rs`, a narrow private
strict wire representation if needed (do NOT change the public
`AttemptIdentity` in `ids.rs` — other surfaces consume it), focused
colocated tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify Task A surfaces, implement B2 retirement, or add any
production caller.

Implement exactly six repairs:

1. **Unforgeable authority.** `RemoteRequestAuthorityV1` currently exposes a
   public field over a cloneable ID, so any consumer can construct or
   duplicate authority without admission. Make the field private with no
   `Clone`/`Copy`, no public constructor, and only borrowed identity access;
   admission remains the sole producer. Follow the 2c2
   `DeletionCapabilityV1` precedent.
2. **Strict nested decoding.** The checkpoint/child wires deny unknown
   fields only at the top level; the nested `AttemptIdentity` accepts
   unknown fields, so `attempt.extra=true` survives decode with an unchanged
   digest. Introduce a private strict wire struct for the attempt identity
   inside this module (exact fields, `deny_unknown_fields`) and convert at
   the boundary. Red: nested-unknown checkpoint and request-child fixtures
   refuse before any mint or mutation with every root byte preserved.
3. **Duplicate-mint refusal.** Admission never compares a freshly minted ID
   against the census, so a repeated mint publishes a second child and
   returns a second authority for the same external ID. Reject a duplicate
   immediately after mint, before staging, with `IdentityCollision`. Red:
   force the mint seam to repeat an ID; assert refusal, unchanged
   checkpoint, unchanged root, no authority.
4. **Over-cap classification.** `scan` enumerates `capacity + 1`, so a root
   with two or more entries past capacity fails inside Task A enumeration
   and surfaces as a generic Task A refusal instead of `Capacity`. Map that
   exact enumeration-limit error to `Capacity`. Red: capacity-plus-two
   census returns `Capacity` without mutation.
5. **Clippy gate.** Fix the `needless_borrow` at the `read_wire(&op, ...)`
   call; the workspace clippy command with `-D warnings` must pass.
6. **Cap compliance.** The candidate's compiled-production prefix measured
   505 added production lines against the 500-line B1 contract. After the
   repairs, recount honestly (production = compiled non-test lines; churn
   convention, post-format) and end at or under 500 production additions
   relative to `d8ec93ad` — the privatizations above may buy the room; if
   not, tighten without weakening a protective arm, and record the final
   split in the handoff.

Where cheap, strengthen the two review-noted coverage gaps: inject at least
one protective Task A outcome at a real call boundary (not only the
test-only consumption helper) and assert `next_ordinal` after a successful
admission. Do not expand scope beyond that.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. Repairs 2, 3, and 4 each need a test that fails on the
  frozen input; repair 1 needs the compile-level proof that external
  construction and duplication are impossible (a trybuild-style test is NOT
  required — an API-shape test module boundary proof plus the privatized
  type is acceptable; state the reasoning in the handoff).
- All existing B1, Task A, and legacy tests keep passing unchanged except
  tests that pinned the six defective behaviors.
- Run `cargo test -p bridge-core --lib -- remote_request_flight
  namespace_transaction custody_v2 fs_custody journal`,
  `cargo clippy --workspace --all-targets --all-features --locked -- -D
  warnings` (or the container's configured equivalent), `git diff --check`,
  and `cargo fmt --all -- --check`.
- Refresh the handoff: exact frozen input `2815259d`, red evidence, the
  recounted production/test split vs `d8ec93ad`, and the statement that B2
  retirement, Tasks C-G, and production V3 remain unarmed.
- Stop and report before exceeding **120 changed production lines or 300
  total changed lines** (churn convention, post-format) relative to
  `2815259d`.

## Files

- `crates/bridge-core/src/remote_request_flight.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): seal request authority and strict admission decoding

## Round Contract

This dispatch is the single declared targeted repair of the Task B1
artifact. One hard-read-only Sol/xhigh closure review follows separately; do
not self-repair a rejection. Never restart from a fresh artifact and never
extend the cap.
