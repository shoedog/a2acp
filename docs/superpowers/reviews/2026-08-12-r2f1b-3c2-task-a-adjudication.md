# R2f1b 3c2 Task A implementation-review adjudication

Date: 2026-08-12

## Frozen inputs and cap

- Feature head at dispatch: `771c0fb8deca88fca06cac631208c9c83b87ea53`.
- Frozen code substrate: `S0 = 530992b7ff1e8e9151fb2a69e86f3ff71c44f905`;
  the two descendants changed only the lane handoff.
- Tier 3 `container_rw` implementor and hard-read-only reviewer:
  `gpt-5.6-sol`, effort `xhigh`.
- Declared cap: one implementation review, one targeted repair on the same
  artifact, and one closure review. The artifact also had a 450-production-line
  and 800-total-line ceiling.

Two pre-execution refusals consumed no attempt: the sandbox could not create the
quarantine clone outside its writable root, and language auto-detection refused
the repository's multiple markers. Approved host execution plus explicit
`--lang rust` resolved those preflight conditions. The admitted run was
`impl-63492-ma5xqzan`; its retained clone is
`/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan`.

## Round 1

The first committed artifact was
`40b9ac364495ee9ca90ac21660dc8e54e929427e`. Its configured hermetic verifier
passed format, warnings-denied Clippy, build, and tests. The independent review
returned `REJECT` with five closed WRONGs:

1. no descriptor-custodied no-replace publication primitive;
2. no expected parent/root identity at reopen;
3. expected target identity not carried to the replacing-rename boundary;
4. regular-child identity omitted birthtime; and
5. a substituted writerless FIFO could block the regular-child read.

Source adjudication confirmed all five on that artifact. They were finite,
local to Task A, and non-repeating, so the one declared targeted repair was
admissible.

## Targeted repair and closure review

The amended exact artifact is
`517703cbd2e469bf208f20a36248169536bca8b3`. Its worktree is clean. Raw Git
numstat is exactly **795 insertions / 5 deletions = 800 total changed lines**;
its handoff records exactly 450 production lines. The configured verifier again
passed all four commands. The bridge emitted terminal digest
`d858ece4fb24b320d186bad8389de891115cfcc8ac88e101410236620e449db2`.

The closure review returned `REJECT` at the cap with three proposed BLOCKER
WRONGs, one MAJOR SMELL, and one MINOR SMELL. Operator source adjudication is:

1. **WRONG confirmed — root-name check/use race.** `revalidate` proves the
   parent entry names the pinned root, then a separate `renameat` or other
   mutation operates through the root descriptor. A peer can rename/replace the
   root in that gap. Publication then mutates the detached old root and
   `settle_publication` can return `Durable`, even though the configured journal
   route no longer names that object. The existing regression replaces the root
   before the final check and does not exercise this gap.
2. **WRONG confirmed — child identity check/use race.** Both exact-child unlink
   and replacing rename open and verify the expected target, then mutate the
   target name through a later syscall. A peer can substitute B after the check;
   `unlinkat` deletes B or replacing `renameat` overwrites B and returns success.
3. **SMELL, not WRONG — returned append fd.** The proposed failure claimed a
   later name replacement redirects a write. It cannot: `openat` returns a file
   descriptor pinned to the already verified object, and
   `open_regular_child_for_append` itself performs no write. The revised design
   should still state the caller's operation-lock and same-object content/length
   obligations before exposing a raw writable `File`.
4. **SMELL retained — lock-test discrimination.** The test proves the retained
   guard's fd identity but does not independently contend on that original
   object to prove `flock` remains held.
5. **SMELL retained — visibility.** Making the guard's fd crate-visible exists
   only for that test and should be replaced by a narrow identity/duplication
   seam if the primitive survives redesign.

## Convergence decision

The cap is exhausted. Although the confirmed WRONG count fell from five to two,
both new findings are instances of one open class: an identity check followed
by a separate pathname-namespace mutation with no atomic compare-and-swap.
Adding another last recheck only creates another check/use gap. The same root
binding question reaches create, publish, replace, unlink, sync, and lock
acquisition, so a third local repair would be an undeclared reroll of the same
defect family.

Task A is therefore **PARKED FOR TARGETED FILESYSTEM-CUSTODY DESIGN**. The
candidate is preserved, not scrapped or integrated. A design correction must
bind all of the following before another implementation turn:

- whether the namespace adversary is limited to bridge participants holding one
  descriptor-root operation lock or includes arbitrary peer renames;
- whether exact-child retirement/replacement uses an exchange/quarantine
  protocol that preserves the displaced object and reports a protective outcome,
  or whether the contract is explicitly weakened to post-effect detection;
- the macOS and Linux primitives that implement the same fail-closed contract
  without a path fallback; and
- which operations may safely return pinned fds versus which must remain inside
  an owned custody transaction.

The repaired expected identities, birthtime strengthening, no-follow/nonblocking
opens, bounded enumeration, and opened-file lock helper are salvage candidates,
not accepted delivery. No feature integration, fold, full host acceptance gate,
push, CI, provider call, smoke, compatibility run, deployment, or operator
mutation followed. Production API V3 remains unarmed; Task B and 3d remain
blocked.

## Superseding design resolution

The capped Sol/xhigh plus Opus/xhigh custody round and operator adjudication are
now complete. The binding result is
[`2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`](2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md).
It selects an owner-private cooperating-participant contract, an externally
bound trusted anchor plus sibling operation-lock object, required birthtime,
no-replace capture with distinct replace/retire recovery namespaces, owned
stage/append sessions, and a protective result lattice in which only `Complete`
projects success.

The design park is therefore resolved, but implementation acceptance is not.
The exact retained candidate `517703cb` remains the salvage input and remains
non-integrable. Task A is split into A1-A4; Task B and 3d stay blocked until all
four cuts are individually green, reviewed, committed, and integrated.
