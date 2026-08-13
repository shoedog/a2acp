---
task-type: design
---
# R2f1b 3c2 Task A namespace-custody redesign

## Description

This is a hard read-only design/spec round against the retained, rejected Task A
candidate in the current checkout. Do not edit, build, test, install, use
network access, invoke another provider, or start nested helpers.

- Program main and fold base: `42249b3d`.
- Frozen 3c2 code substrate: `530992b7ff1e8e9151fb2a69e86f3ff71c44f905`.
- Retained candidate under review: `517703cbd2e469bf208f20a36248169536bca8b3`.
- Candidate checkout: `/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan`.
- Candidate changes: `crates/bridge-core/src/fs_custody.rs`,
  `crates/bridge-core/src/liveness.rs`, and a handoff; raw diff is exactly 800
  changed lines, with 450 production lines.
- The candidate is preserved for salvage. It is not accepted, not integrated,
  and must not be restarted from scratch without a mechanism-level proof that
  it is unsalvageable.
- Production API V3 is still unarmed. Task B and slice 3d remain blocked.

## How Task A reached this design round

The declared implementation cap was one review, one targeted repair on the
same artifact, and one closure review. Round 1 found five closed WRONGs:

1. no descriptor-custodied no-replace publication primitive;
2. constructor accepted no expected parent/root identities;
3. replacing rename did not carry expected target identity to the boundary;
4. regular-child identity omitted birthtime; and
5. a substituted writerless FIFO could block a child read.

The targeted repair fixed those finite items. The closure review then exposed
two confirmed WRONGs belonging to one open class:

1. Root entry TOCTOU: `JournalRootCustodyV1::revalidate` proves the configured
   parent entry names the pinned root, then a later descriptor-relative
   namespace mutation can act on that now-detached directory after a peer
   renames/replaces the root. The operation may report success or `Durable`
   although the configured journal route no longer names the mutated object.
2. Exact-child TOCTOU: replacement and unlink open/verify expected child A,
   then a later pathname syscall can overwrite/delete substituted child B if a
   peer replaces the target entry in between.

The same root-name gap reaches create, publish, replace, unlink, directory sync,
enumeration, append-open, and persistent lock acquisition. Another final
identity recheck merely creates another gap. Under the convergence rule this is
an open-class design defect, so the cap parked the candidate instead of
authorizing a third repair.

One proposed closure WRONG was operator-downgraded to a SMELL: once `openat`
returns a verified regular-file descriptor, later name replacement cannot
redirect writes through that fd. The design must still specify the operation
lock and same-object content/length obligations before a raw writable `File`
may escape. Two other retained SMELLs are that the persistent-lock regression
does not independently contend on the original locked object, and the guard's
fd was widened to crate visibility only for that test.

## Current seams to inspect, not assume correct

In `crates/bridge-core/src/fs_custody.rs`, inspect at minimum:

- `PinnedDirectoryV1`, `RegularChildRefV1`, and `CustodyPublicationV1`;
- `JournalRootCustodyV1::{open,revalidate,open_regular_child,
  create_new_regular_child,publish_new_regular_child,
  open_regular_child_for_append,replace_regular_child,
  enumerate_child_names,unlink_regular_child,sync,
  acquire_persistent_child_lock}`;
- `publish_new_regular_child_impl`, `replace_regular_child_impl`,
  `settle_publication`, and exact-child identity helpers;
- the existing last-check `verify_then_remove` family, which is evidence of a
  weaker check/classify pattern, not an atomic namespace compare-and-swap; and
- the deterministic hooks/tests added around the candidate's root replacement,
  child substitution, publication, and lock behavior.

Inspect `crates/bridge-core/src/liveness.rs` for the opened-file persistent
lock helper and guard exposure. Trace every existing production caller of the
changed primitives and all call-site/test-double ripple that a redesign would
create. Tests, comments, types, and the candidate's green verifier are not
delivery proof.

## Design question

Design a cross-platform, fail-closed namespace custody layer that closes the
whole defect family before Task A is implemented again. Do not silently choose
a weaker threat model. Explicitly decide or surface as an owner decision:

1. Is the namespace adversary limited to cooperating bridge participants that
   must hold one persistent operation lock, or does the contract cover an
   arbitrary local peer capable of renaming/replacing entries without that
   lock? State which production requirement supports the choice.
2. For exact-child replacement and retirement, can Linux `renameat2` with
   `RENAME_EXCHANGE` and macOS `renameatx_np` with `RENAME_SWAP`, followed by
   identity verification in a unique custody/quarantine name, provide one
   common contract? Specify collision resistance, displaced-object custody,
   recovery after every crash cut, sync ordering, cleanup ownership, and what
   happens when the platform primitive is unavailable. No raw-path fallback.
3. Can root entry authority be protected by the same exchange/quarantine
   protocol, or must all journal operations occur under a durable cooperative
   lock whose acquisition itself is bound to the expected root object? Explain
   the linearization point and how root replacement before, during, and after
   acquisition is classified.
4. Which operations may return a pinned fd safely, and which namespace effects
   must remain inside an owned transaction object until settlement? Define
   types so callers cannot accidentally project an unverified effect as
   `Durable`/success.
5. Define the result vocabulary for confirmed no-effect, confirmed committed,
   retained/quarantined protective custody, and unknown/unsupported outcomes.
   Only a positive complete proof may project as destructive or durable
   success.

## Acceptance Criteria

Produce a compile-correct design and task specification, not code. Include:

- precise invariants and state machines for root binding, operation locking,
  exact-child exchange/quarantine, publication, sync, and crash recovery;
- syscall-level Linux/macOS behavior and cfg boundaries, including errno and
  unsupported-platform handling without a semantic fallback;
- exact proposed Rust types, method signatures, ownership/lifetimes, and result
  projections;
- a KEEP / REVISE / REPLACE map for the retained candidate, tied to exact
  symbols and existing tests;
- red-first deterministic adversarial schedules that inject root replacement
  or child substitution at the actual last pre-syscall boundary, plus
  independent lock contention and crash-cut recovery tests;
- production caller and test-double ripple, including any current mutator not
  listed above that falls under the same authority contract;
- a green-after-each-task sequence, with exact files/symbols, focused gates,
  full workspace gate and commit boundary, and a hard changed-line stop rule;
- whether Task A should split into A1 (platform namespace transaction
  primitives) and A2 (`JournalRootCustodyV1` policy/integration), or a different
  smallest independently reviewable split; and
- explicit owner decisions, rejected alternatives, residual risks, and any
  evidence that would collapse a proposed requirement.

The round cap is one independent Sol/xhigh executability lens plus one
independent Opus/xhigh custody lens, followed by one operator synthesis. No
implementation or repair retry is authorized in this round.
