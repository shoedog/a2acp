---
task-type: implement
---
# R2f1b 3c2 Task A4: owned journal API and broken-method deletion

## Description

Complete Task A on the exact accepted A3 head
`6114596d58cce4ae3577afc6c015a212eb50c3c1`. Implement only A4 from the
binding custody redesign: wire the journal surface through the owned
operation value and delete the broken candidate methods. This is the final
planned sequential cut on the preserved artifact, not a restart and not a
Task B step.

Own `crates/bridge-core/src/fs_custody.rs`,
`crates/bridge-core/src/namespace_transaction.rs`, narrow
`crates/bridge-core/src/lib.rs` exports, candidate tests that must migrate or
be deleted with their APIs, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Keep the accepted A1-A3 behavior (identity split, reserved names, capture
classifiers, route binding, operation lease, transaction/recovery table,
commitment) unchanged except where this contract names the change. Do not
wire Task B request-journal consumers, request execution, production V3
arming, HTTP work, or migrations of the shared generation journal, worktree
custody, `local_file`, reapers, or recursive removal.

Implement:

- an owned journal API: stage, publish, append, replace, retire, read,
  enumerate, and sync operate only through a live
  `JournalRootOperationV2`. Staged writes and appends are owned sessions
  that keep the operation borrow and their verified descriptor until sync
  and settlement; no raw writable `File` escapes the surface. Appends verify
  the expected object identity and content position before writing and
  refuse on mismatch, partial write, or file/root sync failure with the
  protective lattice, never a flattened success.
- retained recovery debt is write-blocking: while a `Retained`,
  `ProtectiveDebt`, or unrecovered residue state exists for the root, every
  mutating entry point refuses with typed protective debt until bounded
  recovery reports the root clean; only `recover` may run.
- delete the broken candidate methods, per the binding adjudications: the
  candidate raw writable-file open surface, plain replacing-rename,
  name-based unlink, the free-standing lock API
  (`acquire_persistent_child_lock` and its path-exposing
  `PersistentLockGuard` result), and `revalidate`-as-authority on
  `JournalRootCustodyV1`. Delete `JournalRootCustodyV1` itself if nothing
  outside its own colocated tests still needs it (repository-wide search
  first; there were zero external callers at dispatch). Migrate or delete
  the colocated candidate tests with their APIs; lock-descriptor privacy is
  fully restored — no public path, fd, or `File` projection anywhere in the
  Task A surface.
- carry-forward rider (accepted A3 closure, DEFER): add the
  recovery-specific fail-first regression — a test-only transition between
  recovery's first target verification and `finish`, mutating the target
  in place there, requiring `Retained` with the predecessor capture
  preserved; degrading the recovery `finish` call's target expectation to
  `None` must make it red.

## Acceptance Criteria

- Begin with focused red tests. Record the exact pre-change red commands and
  why each observation is admissible. A compile failure counts only when it
  is specifically the missing A4 API; zero selected tests does not.
- A no-replace target appearing at the exact publication boundary refuses
  and classifies protectively; the owned append refuses on expected-object
  mismatch, wrong content position, partial write, and file/root sync
  refusal.
- Every mutator that loses the route at its actual final boundary refuses
  without namespace effect beyond the protective captures.
- With planted retained residue, every mutating entry point refuses until
  recovery clears it; `recover` remains callable and idempotent.
- The A3-closure recovery-recheck regression is present and red under the
  described degradation.
- Protective arms cannot flatten to success anywhere on the new surface
  (no `is_success`-style helper, no `Result<(), _>` wrapper over the
  lattice).
- After deletion, repository-wide search proves no Task A symbol has an
  unintended production caller and the deleted candidate APIs have zero
  remaining references; `cargo check` proves the workspace compiles without
  them.
- Existing legacy `fs_custody` mechanisms used outside Task A keep their
  signatures; accepted A1-A3 tests keep passing unchanged except those that
  exercised the deleted candidate APIs, which migrate to the owned surface
  or are deleted with cause recorded in the handoff.
- Run the focused selectors:
  `cargo test -p bridge-core --lib -- namespace_transaction custody_v2 fs_custody journal_route`.
- Run `git diff --check` and `cargo fmt --all -- --check`; no
  `rustfmt::skip` may be introduced anywhere, and caps are measured under
  normal formatting.
- Refresh the handoff with exact frozen input, red evidence, changed paths,
  production/test line counts, deletion inventory, focused totals,
  exclusions, and the explicit statement that Task B and production V3
  remain unarmed.
- Stop and report a split before exceeding 280 changed production lines or
  650 total changed lines relative to `6114596d`, measured post-format. Do
  not solve a cap breach by keeping a broken candidate API or weakening a
  protective arm.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/namespace_transaction.rs`
- `crates/bridge-core/src/lib.rs` (narrow exports only)
- colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; final sections are the A1-A3 implementer statements)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): wire owned journal surface and delete candidate APIs

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks A4. Never restart from a fresh artifact
and never silently extend the cap.
