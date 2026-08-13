# R2f1b 3c2 Task A namespace-custody design adjudication

Date: 2026-08-12

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Frozen 3c2 substrate: `530992b7ff1e8e9151fb2a69e86f3ff71c44f905`

Retained Task A candidate: `517703cbd2e469bf208f20a36248169536bca8b3`

Planning checkpoint before this round:
`8eee456af142eb056faf790a0e1d94948b08a197`

## Verdict

**TARGETED CUSTODY REDESIGN APPROVED; SALVAGE THE RETAINED CANDIDATE AS FOUR
SEQUENTIAL TASK-A CUTS.**

The candidate is not accepted or integrated, but it is not scrapped. Its
expected parent/root identities, birthtime strengthening, no-follow/nonblocking
opens, bounded enumeration, no-replace primitive, publication classifier, and
fault seams remain salvage inputs. The broken policy methods are revised or
deleted in place on the exact retained artifact.

Task A is now A1-A4. The rest of the accepted request-flight sequence remains
B-G, so the binding 3c2 implementation count is **ten tasks**. Task B, production
V3, and slice 3d remain blocked until A1-A4 are individually green, reviewed,
committed, and integrated as the accepted Task A result.

## Method, cap, and execution record

This round reused the established slice-3 method: freeze exact identities,
preserve the rejected artifact, declare a cap, send one common hard-read-only
brief to independent custody and executability lenses, source-adjudicate their
disagreements, then persist compile-correct red-first tasks before another
implementation turn.

Declared cap: one Sol/xhigh pass plus one Opus/xhigh pass and one operator
synthesis. No repair or implementation turn was authorized. Both counted passes
completed; no extension was used.

Two initial invocations refused at typed-brief lint because the input lacked the
required design front matter and section names. They stopped before the workflow
node or agent prompt and are inadmissible to the design:

- Sol: `exec-5e2f2a85d7977cbf7eedf31a6d515fe0` /
  `attempt-abcd8b5c4243bf8c9abe95fa8dee1536`;
- Opus: `exec-019115b94b527cb810c87acf28ecd770` /
  `attempt-f5f4b7eaa4b0c06db8c4b2f332f620ba`.

The corrected brief passed `a2a-bridge task-spec input`. Both configs validated;
both exact-agent doctor/model probes passed before dispatch. The candidate binary
was the exact main `42249b3d` release binary, 34,090,224 bytes, SHA-256
`18adb745020fc3a95ed210e81969670d89d5f0c20b4a3e5e02cc3e3083166168`.

| Lens | Execution / attempt | Terminal | Durable artifacts |
|---|---|---|---|
| Codex `gpt-5.6-sol` / xhigh / hard read-only | `exec-631156be8298fec021dec72158be302c` / `attempt-b3005480dc119e79787120d64250ad14` | `DESIGN LENS: NOT READY` with five owner rulings | original 29,757 bytes, SHA-256 `8560ca732495f70111e4e4dfa6f9cb41df02d28ceccac7b1c04f0a1d08cc1917`; repository copy normalizes one excess EOF blank: 29,756 bytes, SHA-256 `8ef60c9513f16086153f8a0862a895d7a995634e62b1b6abfb5cf17439a6a719` |
| Claude Opus `opus[1m]` / xhigh / plan | `exec-9dc37ebabf5805ae00b1caa8d8c536c9` / `attempt-f1803c86bc521d9a40e70ade466caaee` | `DESIGN LENS: NOT READY` with three owner rulings | terminal 5,802 bytes, SHA-256 `df352bc9d43279f834bce5c2394558b81bf08e4e77513c91f74df40ffe5f92a7`; full plan 47,085 bytes, SHA-256 `477b48c645b354e75dfe8b8fc1f9d4338568729b6afc4ce922452e8d563c619a` |

The common typed brief is 7,718 bytes, SHA-256
`940c12f882d9c50d1c082cd608be40e2424e5c7d35fad820f7aef7d44a95c387`.
The Sol config is SHA-256
`0866cda2cb45e778ec8087a59e37385e6ceec1553494cfb7eba024b7539784dc`;
the Opus config is SHA-256
`d6c6a986443a8fd0446140fa1f3745b6129cd64e8d8bf1becfd9cd9ccdc60ec9`.

The Opus plan-mode adapter wrote its full plan under the operator home despite
the hard-read-only prompt. It did not change either repository checkout. The
exact artifact is preserved in this repository alongside the terminal summary.

LSP warm-up reported ambiguous language markers and skipped. No LSP/Prism tools
were callable inside either design node. The lenses and operator therefore used
bounded symbol search plus direct definition/caller reads; that limitation is
not upgraded to type-resolved proof.

## Confirmed findings

### WRONG 1 - the route gap includes the pinned parent

`JournalRootCustodyV1::revalidate` checks two already-open descriptors against
their own immutable dev/inode/birthtime and then opens `root_name` relative to
the retained parent. Moving the parent directory leaves all three checks green.
The configured route can therefore name a replacement while a later operation
mutates or syncs the detached original subtree and may report success.

This strictly extends the already-confirmed root-entry check/use race. Another
recheck cannot close it; route authority must terminate at a declared trusted
anchor and be held by one operation lease.

### WRONG 2 - exact child replacement and retirement are name-selected

The candidate opens and verifies expected child A, then later calls replacing
`renameat` or `unlinkat` on the name. A last-boundary substitution of B causes B
to be overwritten or deleted. The required fix is not exchange-as-publication.
It is an atomic no-replace capture of whatever occupies the authoritative name
into bridge-reserved custody, verification there, and refusal/restoration before
the replacement is ever published.

### WRONG 3 - the same policy gap reaches every Task A mutator

The root binding gap reaches create/stage, no-replace publish, append-open,
replace, retire, enumeration, directory sync, and operation-lock acquisition.
All strong journal methods must therefore require one owned operation value;
fixing only unlink and replace would leave the class open.

### SMELL 1 - writable descriptor ownership is implicit

A verified `openat` descriptor cannot be redirected by a later name change, so
the closure review's raw-fd claim remains downgraded from WRONG. But returning a
raw writable `File` loses the held operation lease and the expected
object/content-position obligations. Stage and append become owned sessions
whose lifetimes retain the operation value until file sync and settlement.

### SMELL 2 - the lock regression does not prove contention

The candidate inspects a crate-visible guard fd. The replacement test must open
the renamed original lock object independently and prove a second nonblocking
flock returns `EWOULDBLOCK`; it must also prove the planted replacement is a
distinct lock cell. The guard fd returns to private visibility.

### SMELL 3 - the generic persistent-lock guard exposes a stale path

The candidate builds `PersistentLockGuard` with a path under the possibly
detached root. Its public `path()` can name a different lock object even while
the original fd remains locked. Task A uses a dedicated operation-lease type
with a private fd and no path projection. No current production caller uses the
candidate API, so this is a design blocker rather than an already-delivered
production failure.

## Source-adjudicated lens disagreement

Sol proposed Linux `RENAME_EXCHANGE` / macOS `RENAME_SWAP` for replacement and
retirement. Opus proposed capture-by-no-replace and pointed to the existing
production policy in `bin/a2a-bridge/src/local_file.rs` plus its deterministic
last-boundary exchange test.

Source favors no-replace capture:

1. exchange publishes the new record at the authoritative name before the code
   can learn whether it displaced expected A or substituted B;
2. rollback would be a second name-selected exchange and can race again;
3. no-replace capture first moves the selected predecessor into a free reserved
   name, never clobbers an existing custody entry, and keeps the replacement
   invisible until the captured object is verified;
4. the existing `local_file` test demonstrates preservation of a last-instant
   substitute, but its binary-specific hash naming and retirement-only recovery
   policy are prior art, not a reusable complete journal transaction.

Task A therefore reuses the shared `rename_child_no_replace` mechanism and adds
request-journal policy with distinct reversible replace, retire, stage, and
intent namespaces. `RENAME_EXCHANGE` / `RENAME_SWAP` remains only an adversarial
test primitive. There is no raw-path, replacing-rename, link/copy, or exchange
fallback.

## Binding owner rulings

1. **Threat model - cooperating bridge participants.** Confirmed success covers
   bridge participants that obey the one operation lease inside an owner-private
   namespace. An arbitrary or compromised same-UID peer can always race the last
   name-selected unlink/rename on both supported kernels; that stronger contract
   is impossible without a separate kernel-enforced principal/mount boundary.
   Noncooperating evidence may produce `Retained` or `Unknown`, never success.
2. **Trusted anchor and lock cell.** `JournalRootBindingV2` is supplied from
   outside the mutable journal root. It binds the trusted anchor, parent, root,
   and one sibling operation-lock object by exact identity. The lock is below the
   trusted anchor but outside the replaceable root. The constructor creates none
   of those objects. Later production arming must persist/supply this binding;
   reading it from the root it authenticates is forbidden.
3. **Birthtime is required.** Object identity is dev + inode + birthtime. A host
   or filesystem without birthtime returns typed `Unsupported` before mutation;
   there is no degraded success path. This preserves the accepted plan's
   required-identity refusal and inode-reuse resistance.
4. **Protective displacement is allowed, never success.** A last-instant foreign
   target may be captured into reserved custody. The implementation first tries
   an exact no-replace restoration when the target is free; otherwise it retains
   the object and returns protective debt. It never exposes the proposed
   replacement over that precondition violation.
5. **Retained debt blocks the attempt.** `Retained`, `Unknown`, unsupported
   recovery, malformed intent, or an over-cap recovery census prevents further
   journal admission. Only complete bounded recovery reopens writes.
6. **Scope is Task A's request-journal surface.** The shared generation journal,
   `PinnedDirectoryV1` production users, worktree custody, `local_file`, both
   reapers, and recursive directory removal are not migrated here. Their weaker
   contracts cannot construct or project Task A's `Complete` proof. This follows
   the accepted 3c2 non-scope; broadening them would be a new program slice.
7. **Budget is split, not raised into another big bang.** Four commits replace
   the exhausted one-commit Task A. Each has its own stop and review cap; the
   accepted A1-A4 aggregate may not exceed 700 production or 1,500 total changed
   lines relative to the frozen Task A substrate without a new planning stop.

## Binding contract and types

The implementation may revise names for compile correctness, but it must retain
these ownership boundaries:

```rust
pub struct ObjectIdentityV2 {
    pub dev: u64,
    pub ino: u64,
    pub btime: BirthTimeV1,
}

pub struct ContentPositionV1 {
    pub len: u64,
}

pub struct JournalRootBindingV2 {
    pub anchor: DirectoryIdentityV1,
    pub parent_name: ChildNameV2,
    pub parent: DirectoryIdentityV1,
    pub root_name: ChildNameV2,
    pub root: DirectoryIdentityV1,
    pub operation_lock_name: ChildNameV2,
    pub operation_lock: ObjectIdentityV2,
}

pub struct JournalRootCustodyV2 { /* retained descriptors + binding + local mutex */ }
pub struct JournalRootOperationV2<'root> { /* local guard + exact sibling flock */ }
pub struct StagedRecordV2<'operation, 'root> { /* owns stage fd and &mut operation */ }
pub struct AppendSessionV2<'operation, 'root> { /* owns append fd and &mut operation */ }
```

`JournalRootCustodyV2::open(anchor_path, binding, label)` opens and verifies the
anchor, parent, root, and pre-existing sibling lock without creating anything.
`begin_operation` takes the in-process mutex, opens/verifies/flocks the exact
lock object, then re-proves anchor -> parent -> root while the flock is held and
runs bounded recovery before returning the operation value.

`StagedRecordV2` owns the mutable borrow of the operation and exposes content
writing/sync plus consuming `publish_new` and `replace_exact` methods. This avoids
the uncompileable shape where a stage borrows an operation and the caller must
borrow the same operation again to settle it. `AppendSessionV2` similarly owns
the fd, expected object, content position, and mutable operation borrow through
`commit`. No raw writable `File` escapes.

```rust
#[must_use]
pub enum NamespaceTxnOutcomeV2<T> {
    Complete(T),
    NoEffect(NoEffectProofV2),
    Retained {
        target: ObservedTargetEffectV2,
        recovery: NamespaceRecoveryTicketV2,
    },
    Unknown {
        recovery: Option<NamespaceRecoveryTicketV2>,
        detail: String,
    },
    Unsupported {
        phase: NamespacePhaseV2,
        detail: String,
    },
}
```

Only `Complete` may project durable/destructive success. `NoEffect` requires
positive proof that the authoritative target returned to its starting state or
that a no-effect syscall precondition refused. A known target commit with
unretired predecessor/intent residue is still `Retained`, not `Complete`. No
`is_success` helper and no `Result<(), _>` wrapper may flatten protective arms.

## Transaction and recovery state machine

Each transaction has an immutable, synced intent recording operation kind,
target name, expected predecessor identity, staged identity/content snapshot,
and the deterministic reserved names. The intent and stage are synced before
capture; the root is synced after every namespace transition.

Replace:

1. create/sync stage and intent under the held operation;
2. no-replace capture `target -> swap` - the linearization point for selecting
   the predecessor;
3. reopen `swap`; if it is not expected A, restore it no-replace when possible,
   otherwise retain it; never publish the stage;
4. when it is A, publish `stage -> target` no-replace;
5. sync and verify target identity/content;
6. verify and retire exactly A from `swap`, prove its retained fd has zero links,
   sync, remove intent, sync, re-prove route; only then return `Complete`.

Retire uses the distinct `del` namespace. After an authorized capture of exact A,
recovery rolls forward; replacement recovery after capture but before publication
rolls back A. These policies require distinct names and an immutable intent.

Recovery runs under the same operation lease before every new admission. It is
bounded, idempotent, and handles every crash cut from stage creation through
final intent removal. Malformed, duplicated, foreign, over-cap, or identity-
ambiguous state is preserved and blocks writes. `Drop` performs no namespace
cleanup; it can warn and leave durable, bounded recovery debt only.

## Compile-correct Task A sequence

Every task starts from its exact predecessor commit in the retained candidate
line, runs focused red tests first, then the common full gate, refreshes the
handoff, and commits once. A red gate blocks the next frozen input.

### A1 - identity, name, and no-replace capture foundations

- Frozen input: `517703cbd2e469bf208f20a36248169536bca8b3`.
- Own: `crates/bridge-core/src/fs_custody.rs`, focused tests, handoff.
- Add the required object/content split, validated bounded `ChildNameV2`, distinct
  reversible reserved-name codec, immutable intent schema, and policy-neutral
  no-replace capture/restore classifiers. Keep legacy mechanism signatures used
  outside Task A.
- Red first: birthtime absent refuses; reserved-name round trip/bounds; capture
  never clobbers occupied custody; last-boundary A/B substitution is captured and
  classified before any replacement is visible; runtime/compile unsupported has
  no fallback call.
- Stop: 200 production or 450 total changed lines.

### A2 - trusted route binding and sibling operation lease

- Frozen input: exact A1 commit.
- Own: `fs_custody.rs`, narrow `liveness.rs` visibility only if needed, focused
  tests, handoff.
- Add `JournalRootBindingV2`, `JournalRootCustodyV2`, and the dedicated operation
  guard. Bind anchor -> parent -> root and the sibling lock by exact identity;
  acquire the already-open exact lock, flock, then re-prove the route. Remove the
  candidate's free-standing revalidate-as-authority and path-exposing journal
  lock result.
- Red first: parent/root replacement before, while waiting, and immediately after
  flock; wrong lock inode; two lock cells straddling root replacement; removed
  root never recreated; independent second-fd contention on the original inode.
- Stop: 220 production or 500 total changed lines.

### A3 - capture settlement and bounded crash recovery

- Frozen input: exact A2 commit.
- Own: preferably new `crates/bridge-core/src/namespace_transaction.rs`, narrow
  `fs_custody.rs` mechanisms/exports, focused tests, handoff.
- Implement replace/retire capture, immutable intent barriers, distinct rollback
  versus roll-forward recovery, recovery tickets, and the full protective result
  lattice. Do not wire request-journal callers yet.
- Red first: substitute at the actual capture syscall boundary; target takeover
  before republish; reserved-name substitution immediately before cleanup; every
  stage/intent/capture/publish/sync/unlink crash cut; repeated recovery; malformed,
  duplicate, foreign, and over-cap residue; no unchecked unlink.
- The load-bearing pair is: crashed replacement after capture restores A, while
  crashed retirement after capture completes A's authorized retirement.
- Stop: 320 production or 700 total changed lines.

### A4 - owned journal API and deletion of broken candidate methods

- Frozen input: exact A3 commit.
- Own: `fs_custody.rs`, `namespace_transaction.rs`, exports, candidate tests,
  handoff.
- Wire stage/publish/append/replace/retire/read/enumerate/sync through
  `JournalRootOperationV2`; make retained recovery debt write-blocking; delete the
  candidate raw writable-file, plain replacing-rename, name-unlink, and
  free-standing lock APIs. Restore lock-fd privacy.
- Red first: no-replace target appears at publication boundary; append expected
  object/length mismatch, partial write, file/root sync refusal; every mutator
  loses the route at its actual final boundary; protective arms cannot flatten
  to success; no Task A symbol has an unintended production caller.
- Stop: 280 production or 650 total changed lines.

Focused gates are the exact new test modules/selectors named by each task. Then
every task runs the plan's unchanged common gate: diff, format, locked workspace
check, warnings-denied Clippy, full locked all-feature workspace test, release
binary build, deny, and repository hygiene. Record totals and exclusions.

Per subtask review cap: one independent review; one targeted repair on the same
artifact plus one closure review only for a closed enumerable rejection. An
open-class/repeating family parks that subtask. No fresh restart.

## Scope shields and remaining blocks

- B's frozen input becomes the exact accepted A4 commit, not the rejected
  `517703cb` and not a moving branch.
- B must consume `NamespaceTxnOutcomeV2`; only exact `Complete` can advance a
  checkpoint or acknowledge retirement. Protective debt blocks attempt opening
  and admission.
- Existing production users of `PinnedDirectoryV1`, `CustodyPublicationV1`,
  `verify_then_remove`, `local_file`, worktree custody, and the shared generation
  journal retain their current vocabulary and are neither certified nor changed
  by Task A.
- The two-field cleanup carry-forward remains unchanged.
- Production V3 remains `None`; no provider, smoke, compatibility, deployment,
  or operator mutation is authorized.

**DESIGN ADJUDICATION: APPROVE A1-A4; TASK B AND 3D REMAIN BLOCKED.**
