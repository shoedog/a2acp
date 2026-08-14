---
task-type: implement
---
# R2f1b 3c2 Task A2: trusted route binding and sibling operation lease

## Description

Continue the approved Task A custody sequence on the exact closure-approved A1
candidate at `5cbeea1ed882afe448d3825984af9a3ed74bcb58`. Implement only A2 from
the binding custody redesign. This is a planned sequential cut on the preserved
artifact, not a fresh Task A restart and not an amendment of the approved A1
surface.

Own `crates/bridge-core/src/fs_custody.rs`, focused colocated tests, narrow
`crates/bridge-core/src/liveness.rs` visibility changes only if strictly
needed, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Keep every legacy mechanism signature used outside Task A and keep the
approved A1 surface (object/content identity split, `ChildNameV2`, reserved
names, intent schema, capture/restore classifiers) behaviorally unchanged. Do
not implement A3 capture settlement or crash recovery, A4 owned journal APIs
or candidate-method deletion, Task B, request execution, production V3 arming,
HTTP work, or migrations of the shared generation journal, worktree custody,
`local_file`, reapers, or recursive removal.

Add compile-correct route-authority foundations:

- `JournalRootBindingV2` supplied from outside the mutable journal root,
  binding by exact identity: trusted anchor, parent name and identity, root
  name and identity, and one sibling operation-lock object name plus its
  required `ObjectIdentityV2` (device, inode, mandatory birthtime). The lock
  object is below the trusted anchor but outside the replaceable root. Reading
  the binding from the root it authenticates is forbidden.
- `JournalRootCustodyV2::open(anchor_path, binding, label)` opens and verifies
  the anchor, parent, root, and the pre-existing sibling lock object without
  creating any of them. Any identity mismatch, wrong object type, missing
  birthtime, or missing object refuses typed before any effect.
- A dedicated owned operation guard (`JournalRootOperationV2` or
  compile-correct equivalent): `begin_operation` takes the in-process mutex,
  opens, verifies, and nonblocking-flocks the exact bound lock object, then
  re-proves anchor -> parent -> root while the flock is held, and only then
  returns the owned operation value. The guard's lock fd is private; it
  exposes no path projection and no raw `File`.
- Remove the candidate's free-standing revalidate-as-authority surface and the
  path-exposing journal lock result from the route-authority path. Later
  operations must not accept a bare revalidate or a pathname as authority.
  Raw writable-file/plain-replace/name-unlink APIs are A4's deletion scope and
  stay as they are.

Route or lock objects that cannot supply the required identity or primitive
(no birthtime, no no-follow/nonblocking open, no flock) must return typed
unsupported/refusal before mutation. There is no path-based, replacing-rename,
link/copy, exchange, or degraded device/inode-only fallback. Confirmed success
covers cooperating bridge participants inside the owner-private namespace;
noncooperating interference may produce protective refusal or retained
evidence, never success.

## Acceptance Criteria

- Begin with focused red tests. Record in the handoff the exact pre-change red
  commands and why each observation is admissible. A compile failure counts
  only when it is specifically the missing A2 API; zero selected tests does
  not.
- Parent replacement and root replacement each refuse across the full red
  schedule: substituted before lock acquisition, substituted while a
  contending holder still holds the flock (acquired only after the peer
  releases), and substituted immediately after flock acquisition; the held
  re-prove detects all of them and no operation value is returned.
- A wrong lock inode (same name, different object) refuses typed; a planted
  replacement lock is proved to be a distinct lock cell via exact
  `ObjectIdentityV2` mismatch, not via path comparison.
- Two lock cells straddling a root replacement can never both hold route
  authority: the test proves the stale cell's route re-prove fails while the
  bound cell's contention is real.
- Independent second-fd contention: a separately opened fd on the original
  bound lock inode proves a second nonblocking flock returns `EWOULDBLOCK`
  while the operation guard is held, and acquisition succeeds after release.
- A removed root is never recreated: `open` and `begin_operation` refuse
  typed; no `mkdir`/create call exists on that path.
- The binding is consumed as an externally supplied value; no code reads
  binding material from inside the authenticated root.
- Missing-birthtime and absent-primitive cases return typed
  unsupported/refusal before mutation, and the fallback mechanism is proved
  never called.
- No public surface of the new types exposes the lock fd, a lock path, or a
  raw writable `File`.
- Existing legacy `fs_custody` tests and the approved A1 `custody_v2` tests
  continue to compile and pass without changing their public behavior.
- Run focused A2 selectors, `cargo test -p bridge-core custody_v2`, and
  `cargo test -p bridge-core fs_custody`.
- Run `git diff --check` and `cargo fmt --all -- --check`.
- Refresh the handoff with exact frozen input, red evidence, changed paths,
  production/test line counts, focused totals, exclusions, and the explicit
  statement that A3-A4, Task B, and production V3 remain unarmed.
- Stop and report a split before exceeding 220 changed production lines or 500
  total changed lines relative to `5cbeea1e`. Do not solve a cap breach by
  deleting unrelated inherited tests or by adding a weaker fallback.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- focused tests colocated in `fs_custody.rs`
- `crates/bridge-core/src/liveness.rs` (narrow visibility only if needed)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`
- `docs/superpowers/plans/2026-08-12-r2f1b-3c2-salvage-redesign.md`
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): bind trusted journal route and operation lease

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator will
first classify it: only a closed, enumerable rejection may receive one
targeted repair on this same artifact followed by one closure review. An
open-class or repeating family parks A2. Never restart from a fresh artifact
and never silently extend the cap.
