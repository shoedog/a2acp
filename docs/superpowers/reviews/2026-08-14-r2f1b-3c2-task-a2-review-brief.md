---
task-type: code-review
---
# R2f1b 3c2 Task A2 implementation review

## Description

Perform the one declared hard-read-only independent implementation review of
exact commit `3890fa6c295abcf92055940816c162c781d824bf`, whose exact parent is
the closure-approved A1 candidate
`5cbeea1ed882afe448d3825984af9a3ed74bcb58`. Review the complete
`5cbeea1e..3890fa6c` diff, the full changed files, and the bounded new public
surface in context. Do not edit, build, test, invoke another provider, or
access the network.

This is Task A2 of the approved custody redesign: trusted route binding and
sibling operation lease. The binding contract is:

- `JournalRootBindingV2` is supplied from outside the mutable journal root and
  binds the trusted anchor, parent, root, and one sibling operation-lock
  object by exact required identity (device, inode, mandatory birthtime). The
  lock object is below the trusted anchor but outside the replaceable root.
  Reading binding material from the root it authenticates is forbidden.
- `JournalRootCustodyV2::open(anchor_path, binding, label)` opens and verifies
  the anchor, parent, root, and pre-existing sibling lock without creating
  anything.
- `begin_operation` takes the in-process mutex, opens/verifies/nonblocking-
  flocks the exact bound lock object, then re-proves anchor -> parent -> root
  while the flock is held, and only then returns the owned operation value.
  The guard's lock fd is private with no path projection and no raw `File`
  escape.
- Route or lock objects that cannot supply the required identity or primitive
  return typed unsupported/refusal before mutation; there is no path-based,
  replacing-rename, link/copy, exchange, or degraded device/inode-only
  fallback.
- A removed root is never recreated.
- The approved A1 surface and every legacy mechanism signature used outside
  Task A remain behaviorally unchanged. A3 settlement/recovery, A4 owned
  journal APIs and candidate-method deletion, Task B, request execution, and
  production V3 arming are out of scope.

Required red schedule the tests must genuinely discriminate: parent and root
replacement before lock acquisition, under contention until the peer releases,
and immediately after flock acquisition; wrong lock inode (same name,
different object) and wrong object type; two lock cells straddling a root
replacement can never both hold route authority (stale re-prove fails while
bound-cell contention is real); independent second-fd contention on the
original bound inode; removed root never recreated; unsupported primitive and
missing birthtime fail closed with the fallback proven never called.

Operator-disclosed concerns you must explicitly judge (do not assume either
way):

1. The A2 design bullet says to remove the candidate's free-standing
   revalidate-as-authority and path-exposing journal lock result. The commit
   adds the V2 authority surface, which uses neither, but deletes nothing:
   `JournalRootCustodyV1::revalidate` and
   `JournalRootCustodyV1::acquire_persistent_child_lock` (which returns a
   `PersistentLockGuard` carrying a joined pathname) remain present with zero
   callers outside colocated candidate tests. A4's contract separately owns
   "owned journal API and deletion of broken candidate methods" including
   free-standing lock APIs and lock-fd privacy. Judge whether deferring the
   deletion to A4 is sound sequencing or a blocking A2 gap.
2. Lock contention surfaces as `FsCustodyError::Io` with
   `ErrorKind::WouldBlock` rather than a dedicated typed contention variant.
   Judge whether any caller could confuse protective contention with a real
   I/O failure in a way that produces a wrong result inside this slice's
   scope.
3. The production `begin_operation` delegates to a private
   `begin_operation_with(label, flock, after_lock)` seam used by tests to
   inject substitution at the exact post-flock boundary and an unsupported
   flock error. Judge that the production path is byte-equivalent to the seam
   path with the no-op hook and real flock, and that the seam cannot be
   reached from production.
4. `prove_route` re-proves the retained anchor/parent/root descriptors and
   then freshly re-walks anchor -> parent -> root by no-follow opens starting
   from the anchor's canonical path, verifying each hop against the binding.
   Authority terminates at the trusted anchor by design. Judge whether any
   substitution the design claims to exclude survives this re-walk, and
   whether the pre-flock lock-object verification on the opened descriptor
   plus object-level flock excludes a substituted lock from ever holding
   authority.
5. The one non-custody change makes `bridge_core::liveness::flock_nb`
   `pub(crate)`. Judge blast radius.

Adjudicate WRONG findings before SMELL findings under the repository severity
discipline: every WRONG must name a constructible input/state, the incorrect
result, realistic reachability, and a bounded fix. This review round is the
declared single independent review; no repair loop is authorized inside it.

Supplied exact-head evidence is corroboration only. You are explicitly
licensed to falsify or reject every supplied result:

- exact parent/head: `5cbeea1e` / `3890fa6c`; clean committed worktree on
  branch `implement/impl-66546-s8d4i725`;
- changed paths: `crates/bridge-core/src/fs_custody.rs` (+456), one visibility
  line in `crates/bridge-core/src/liveness.rs`, and the implementer handoff
  (+25); 214 changed production lines and 483 total, within the declared
  220 production / 500 total caps;
- in-container hermetic verify: fmt, clippy `-D warnings`, and build exit 0;
  focused `journal_route_custody_v2` 5/0, `custody_v2` 16/0, `fs_custody`
  82/0; the aggregate in-container workspace run passed all 1,084 bridge-core
  tests and then failed the whole-bin `a2a-bridge` harness at
  `api_entry_resolves_and_serves_through_registry`
  (`api.prompt.error_body_read`), a surface this diff does not touch;
- operator host gates on the exact candidate `3890fa6c` all exit 0:
  `git diff --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,004 passed / 0 failed / 13 ignored across 90 harnesses** (the ignored
  set is the repository's declared authenticated/live-provider population),
  locked release build, `cargo deny check`, and repository hygiene with 40
  tracked artifacts / 8 validated example configs — so the in-container
  whole-bin red is environment-classified, not candidate-attributed;
- the implementer notes the two dated design/plan reference paths are absent
  from the frozen tree (they live on the planning branch); the dispatch brief
  carried the full contract inline.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding.
- Explicitly judge the five operator-disclosed concerns.
- Verify the red schedule is genuinely discriminating (each test fails on a
  tree without the checked mechanism), scope containment, cap compliance, and
  that no production caller, persistence encoding, route, or V3 activation was
  introduced.
- Distinguish source correctness from environment-classified verify evidence.
- Give 0-100 confidence and name evidence that would raise, lower, or collapse
  the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal lines.

## Files

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-core/src/liveness.rs`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md` (in
  this checkout; its final section is the A2 implementer statement)
- repository `AGENTS.md`
