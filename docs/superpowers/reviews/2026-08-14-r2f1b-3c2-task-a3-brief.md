---
task-type: implement
---
# R2f1b 3c2 Task A3: capture settlement and bounded crash recovery

## Description

Continue the approved Task A custody sequence on the exact accepted A2
candidate at `3890fa6c295abcf92055940816c162c781d824bf`. Implement only A3
from the binding custody redesign. This is a planned sequential cut on the
preserved artifact, not a restart and not an amendment of the accepted A1/A2
surfaces.

Own preferably a new `crates/bridge-core/src/namespace_transaction.rs`,
narrow `crates/bridge-core/src/fs_custody.rs` mechanism/exports changes,
focused tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Keep every legacy mechanism signature used outside Task A and keep the
accepted A1/A2 surfaces (identity split, `ChildNameV2`, reserved names,
intent schema, capture/restore classifiers, `JournalRootBindingV2`,
`JournalRootCustodyV2`, the operation guard) behaviorally compatible. Do not
wire request-journal callers, A4 owned-API rewiring or candidate-method
deletion, Task B, request execution, production V3 arming, HTTP work, or
migrations of the shared generation journal, worktree custody, `local_file`,
reapers, or recursive removal.

Implement, under the held `JournalRootOperationV2` lease:

- complete replace and retire transactions built on the A1 no-replace capture
  mechanism, using the distinct reversible reserved namespaces for
  transaction intent, staging, replacement capture (`swap`), and retirement
  capture (`del`);
- an immutable, synced intent barrier per transaction recording operation
  kind, target name, expected predecessor identity, staged identity/content
  snapshot, and the deterministic reserved names; intent and stage are synced
  before capture, and the root is synced after every namespace transition;
- the replace policy: create/sync stage and intent; no-replace capture
  `target -> swap` as the linearization point; reopen and classify `swap`; if
  it is not expected A, restore it no-replace only when the target is free,
  otherwise retain it and never publish the stage; when it is A, publish
  `stage -> target` no-replace, sync and verify target identity/content, then
  verify and retire exactly A from `swap`, prove the retained fd has zero
  links, sync, remove intent, sync, and re-prove the route before returning
  success;
- the retire policy in the distinct `del` namespace: after an authorized
  capture of exact A, recovery rolls forward to complete the retirement;
- distinct rollback versus roll-forward crash recovery driven by the
  immutable intent: crashed replacement after capture but before publication
  rolls A back; crashed retirement after capture rolls forward; recovery is
  bounded, idempotent, runs under the operation lease before the operation
  value is returned, and repeated recovery converges;
- typed recovery tickets and the full protective outcome lattice: only a
  proved `Complete` arm may project success; a no-effect claim requires
  positive proof that the authoritative target returned to its starting state
  or that a no-effect syscall precondition refused; a known target commit
  with unretired predecessor/intent residue is retained, not complete; no
  `is_success` helper or `Result<(), _)>` wrapper may flatten protective
  arms;
- malformed, duplicated, foreign, over-cap, or identity-ambiguous residue is
  preserved and blocks the transaction surface with typed protective debt;
  `Drop` performs no namespace cleanup — it may warn and leave durable,
  bounded recovery debt only;
- there is no path-based, replacing-rename, link/copy, exchange, or
  unchecked-unlink operation anywhere in the new surface; every unlink is
  identity-checked. Required-identity absence and missing primitives return
  typed unsupported before mutation with no fallback.

Carry-forward riders from the accepted A2 review (binding; colocated with the
A2 tests): add deterministic regressions for anchor replacement before and
after flock acquisition; a channel-ordered two-thread test proving same-cell
operations queue on the in-process mutex rather than returning spurious
contention; the constructor refusal when root and lock share a name; and a
`cfg(not(unix))` refusal test of the V2 route surface. If these riders would
breach the cap, stop and report the split rather than dropping them silently.

## Acceptance Criteria

- Begin with focused red tests. Record in the handoff the exact pre-change
  red commands and why each observation is admissible. A compile failure
  counts only when it is specifically the missing A3 API; zero selected tests
  does not.
- A deterministic substitution at the actual capture syscall boundary is
  captured and classified before any replacement is visible; the unexpected
  object is retained or exactly restored per the A1 policy and never
  published over.
- Target takeover between capture and publication refuses publication and
  classifies protectively; reserved-name substitution immediately before
  cleanup refuses the cleanup with protective debt rather than acting on the
  substitute.
- Every crash cut across stage creation, intent write/sync, capture, publish,
  target sync, retire, zero-link proof, intent removal, and final sync has a
  recovery test; the load-bearing pair holds: crashed replacement after
  capture restores A while crashed retirement after capture completes A's
  authorized retirement.
- Repeated recovery over the same residue is idempotent and converges;
  malformed, duplicate, foreign, and over-cap residue is preserved and blocks
  with typed debt.
- No unlink in the new surface acts without an identity check; grep-level
  proof that no `is_success`-style flattening of the outcome lattice exists.
- The four A2 rider regressions are present and red on a tree lacking their
  mechanism (mutation-style argument in the handoff is acceptable where a
  physical pre-change red is impossible).
- Existing legacy `fs_custody`, A1 `custody_v2`, and A2
  `journal_route_custody_v2` tests continue to compile and pass unchanged.
- Run the focused A3 selectors, `cargo test -p bridge-core
  namespace_transaction` (or the chosen module selector),
  `cargo test -p bridge-core custody_v2`, and
  `cargo test -p bridge-core fs_custody`.
- Run `git diff --check` and `cargo fmt --all -- --check`.
- Refresh the handoff with exact frozen input, red evidence, changed paths,
  production/test line counts, focused totals, exclusions, and the explicit
  statement that A4, Task B, and production V3 remain unarmed.
- Stop and report a split before exceeding 320 changed production lines or
  700 total changed lines relative to `3890fa6c`. Do not solve a cap breach
  by deleting unrelated inherited tests or by adding a weaker fallback.

## Files

- `crates/bridge-core/src/namespace_transaction.rs` (new, preferred) or
  clearly bounded additions in `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/lib.rs` (narrow export only if needed)
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1/A2 implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): add capture settlement and crash recovery

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks A3. Never restart from a fresh artifact
and never silently extend the cap.
