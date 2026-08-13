I’m treating the checkout and three pinned commits as immutable evidence. I’ll use the `lsp-nav` workflow for type-resolved caller/implementation coverage where its local tools are available, backed by read-only Git and `rg`; I’ll also do the required lightweight memory pass for the prior 3c2 rejection context. I will not edit, build, test, install, browse, invoke agents/providers, or start helpers.The `lsp-nav` instructions are loaded, but no LSP/Prism navigation tools are callable in this session; I’ll preserve the same type-resolution discipline manually by pairing symbol searches with definitions, trait impls, and every literal production/test reference. The initial checkout is clean on `implement/impl-63492-ma5xqzan`; I’m now binding the exact commit ancestry and diff before reading behavior.Identity is bound: `HEAD` exactly equals retained candidate `517703cb`, with frozen substrate `530992b7` in its ancestry. One supplied metric does not match Git’s current range: `530992b7..517703cb` is 846 insertions/5 deletions (851 changed lines), including 756 production changes, not 800/450; I’ll distinguish that bookkeeping mismatch from the design evidence and also measure the candidate commit alone. Next I’m mapping all custody symbols and their callers before assessing the protocol.The code confirms both open-class defects mechanically: root `revalidate()` ends before later `openat`/`renameat`/`unlinkat` on the retained-but-possibly-detached root fd, and target identity verification ends before plain replacing `renameat`. The same window exists for create, append-open, enumeration/sync, and lock acquisition. I’m now locating the production contract that chooses the adversary model and enumerating every mutator/caller, including shared `local_file` users that bypass `JournalRootCustodyV1`.Source evidence does not yet authorize weakening to lock-cooperative peers: the shared primitive explicitly promises no-replace correctness against actors that ignore the owner lock, the focused boundary requires descriptor identity at every decision point, and the current rejection’s adversarial schedules assume out-of-lock substitution. A cooperative-only root contract therefore remains an owner decision, not an implementation detail. I’m checking whether an exchange transaction can retain displaced objects without unsafe cleanup and measuring the real writer ripple before deciding readiness.The writer ripple is broader than the candidate’s new API: `bridge-worktree::WorktreeCustodianV1::stage_and_settle` calls the identity-blind `PinnedDirectoryV1::replace_regular_child`, and `bin/a2a-bridge::local_file` has both a verify-then-`unlinkat` quarantine remover and a three-file plain-`renameat` replacement with best-effort raw unlinks. Those cannot inherit a stronger “arbitrary peer” claim unchanged. I’m now reading the retained seven-task design commit named by the handoff to locate the intended durable root trust anchor and avoid inventing a caller contract.The source now supports a conditional salvage plan, but not an unconditional READY verdict. Two mechanisms are impossible to specify without owner choices: a finite descriptor chain cannot protect the configured path against an arbitrary same-privilege peer renaming an ancestor, and exchange is an atomic swap—not identity-conditional CAS—so a last-instant foreign target can only be displaced into retained custody, never proven untouched. I’m doing the final read-only identity/status check and then I’ll present the conditional APIs, crash state machine, caller migration, and blockers.The retained candidate is salvageable, but the redesign is not executable yet. The code cannot satisfy both an arbitrary same-privilege namespace adversary and the proposed cooperative-lock design; that threat-model decision changes the APIs, proofs, tests, and caller scope.

Checkout evidence is clean and immutable: `HEAD` is exactly `517703cbd2e469bf208f20a36248169536bca8b3`. The candidate commit itself is exactly 800 changed lines (795 insertions, 5 deletions); the larger `530992b7..517703cb` range also includes two intervening planning-document commits.

## Confirmed findings

- WRONG — root mutation is not linearized. [`JournalRootCustodyV1::revalidate`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-core/src/fs_custody.rs:622) returns before later `openat`, `renameat*`, `unlinkat`, enumeration, flock, or `sync_all`. For example, create revalidates at line 649 and mutates at line 650. A peer can move `journal`, install a replacement, and make the syscall mutate the detached original while the configured route names the replacement.

- WRONG — exact-child replacement and retirement are check-then-pathname operations. [`replace_regular_child_impl`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-core/src/fs_custody.rs:1494) verifies A and later runs plain replacing `renameat`; [`unlink_regular_child_at`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-core/src/fs_custody.rs:1245) repeats the same shape for unlink. Substituting B at the last boundary overwrites or deletes B.

- WRONG under the arbitrary-peer contract — the family already reaches production beyond the new journal type:

  - [`WorktreeCustodianV1::stage_and_settle`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-worktree/src/custody_writer.rs:1066) calls identity-blind `PinnedDirectoryV1::replace_regular_child`.
  - [`probe_custody_record_presence`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-worktree/src/custody.rs:747) can observe absence in a detached root; that result can participate in deletion admission.
  - [`local_file::remove_regular_child_candidate_with_hooks`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/bin/a2a-bridge/src/local_file.rs:1325) verifies a quarantine name, then later unlinks it.
  - [`local_file::replace_regular_child`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/bin/a2a-bridge/src/local_file.rs:1546) uses plain `renameat` plus best-effort raw unlinks.
  - `storage_reap` and `storage_reap_clones` drive `verify_then_remove` into path-addressed recursive removal.
  - [`FileResourceFlightJournal::open`](/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan/crates/bridge-core/src/retained_resource_flight.rs:579) retains root/lock paths and later performs path-based create, append, enumeration, and removal. The replacement request journal will stop using it, but the shared process/container journal remains.

- SMELL — returning the verified writable `File` from append cannot redirect writes after name substitution, but its contract does not keep the operation lock or prove expected content, length, and resulting digest before success.

- SMELL — the persistent-lock test proves the returned guard retains the old inode, but it never independently tries to flock that original object while the first guard holds it. `_file` was widened to `pub(crate)` solely for inspection.

No production code currently references `JournalRootCustodyV1`, `RegularFileIdentityV1`, or its new methods; all direct references are candidate unit tests. That makes the artifact locally salvageable.

## Required threat-model decision

The current sources contain conflicting expectations:

- `rename_child_no_replace` explicitly claims target-absence correctness against an actor that ignores the owner lock.
- The worktree and `local_file` policies explicitly rely on owner locks excluding cooperating writers.
- The present brief’s last-boundary schedules assume an out-of-lock peer.

A finite descriptor chain cannot prove that a configured pathname still names an object against an arbitrary same-privilege peer that can rename any ancestor. Protecting `parent/root` merely moves the problem to the pathname naming `parent`. A last recheck always leaves another gap.

Therefore:

- Recommended executable contract: confirmed success covers cooperating bridge participants. Every namespace participant, including recovery and root lifecycle code, must acquire one immutable operation-lock object stored beneath a declared trusted parent anchor. An out-of-contract peer may cause `Retained` or `Unknown`, but the API makes no success guarantee against a mutation after the final proof.
- If arbitrary same-privilege peers remain in scope, Linux/macOS userland rename/flock primitives are insufficient. The design needs a stronger environmental authority—permissions separating principals, a protected namespace/mount, or another kernel-enforced exclusion boundary.

## Conditional API design

The candidate’s optional identities should become complete, non-optional authority types:

```rust
pub struct CompleteDirectoryIdentityV2 {
    canonical_path: String,
    dev: u64,
    ino: u64,
    btime: BirthTimeV1,
}

pub struct FileObjectIdentityV2 {
    dev: u64,
    ino: u64,
    btime: BirthTimeV1,
}

pub struct FileSnapshotV2 {
    object: FileObjectIdentityV2,
    len: u64,
    sha256: [u8; 32],
}

pub struct ChildNameV2(OsString);
pub struct NamespaceTxnIdV2([u8; 32]);

pub struct JournalRootBindingV2 {
    parent: CompleteDirectoryIdentityV2,
    root_name: ChildNameV2,
    root: CompleteDirectoryIdentityV2,
    operation_lock_name: ChildNameV2,
    operation_lock: FileSnapshotV2, // empty-content digest and len == 0
}
```

`NamespaceTxnIdV2` uses 32 CSPRNG bytes from the already-depended-on `ring::rand::SystemRandom`. Transaction names are fixed ASCII forms such as `.a2a-ns-txn-<64hex>.intent` and `.a2a-ns-txn-<64hex>.stage`, created with `O_CREAT|O_EXCL`. A collision refuses; it does not retry into an unbounded search.

The root authority should be capability- and lifetime-shaped:

```rust
pub struct JournalRootCustodyV2 {
    parent: PinnedDirectoryV1,
    root: PinnedDirectoryV1,
    binding: JournalRootBindingV2,
    local_operation: std::sync::Mutex<()>,
}

pub struct JournalRootOperationV2<'a> {
    custody: &'a JournalRootCustodyV2,
    _local: std::sync::MutexGuard<'a, ()>,
    _shared: crate::liveness::PersistentLockGuard,
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl JournalRootCustodyV2 {
    pub fn open(
        trusted_parent_path: &Path,
        binding: JournalRootBindingV2,
        label: &str,
    ) -> Result<Self, CustodyOpenErrorV2>;

    pub fn begin_operation(
        &self,
        on_contended: &dyn Fn(),
    ) -> Result<JournalRootOperationV2<'_>, CustodyOpenErrorV2>;
}
```

`begin_operation` acquires the in-process mutex, opens the pre-existing operation-lock child descriptor-relatively, verifies its complete identity/content, flocks that already-open fd, and only then verifies `parent → root → lock` under the held flock. It creates nothing.

The liveness addition should be:

```rust
pub fn acquire_persistent_lock_file_blocking(
    file: File,
    diagnostic_path: PathBuf,
    on_contended: &dyn Fn(),
) -> std::io::Result<PersistentLockGuard>;
```

`PersistentLockGuard::_file` returns to private visibility. Contention tests prove behavior through a second fd, not internal field access.

The operation object exposes no writable `File`:

```rust
impl JournalRootOperationV2<'_> {
    pub fn read_bounded(
        &self,
        name: &ChildNameV2,
        limit: NonZeroUsize,
    ) -> Result<VerifiedReadV2, NamespaceReadErrorV2>;

    pub fn enumerate_child_names(
        &self,
        limit: NonZeroUsize,
    ) -> Result<BoundedNamesV2, NamespaceReadErrorV2>;

    pub fn publish_new(
        &mut self,
        target: &ChildNameV2,
        bytes: &[u8],
        limit: NonZeroUsize,
    ) -> NamespaceTxnOutcomeV2<FileSnapshotV2>;

    pub fn append_exact(
        &mut self,
        target: &ChildNameV2,
        expected: &FileSnapshotV2,
        bytes: &[u8],
        limit: NonZeroUsize,
    ) -> NamespaceTxnOutcomeV2<FileSnapshotV2>;

    pub fn replace_exact(
        &mut self,
        target: &ChildNameV2,
        expected: &FileSnapshotV2,
        replacement: &[u8],
        limit: NonZeroUsize,
    ) -> NamespaceTxnOutcomeV2<FileSnapshotV2>;

    pub fn retire_exact(
        &mut self,
        target: &ChildNameV2,
        expected: &FileSnapshotV2,
    ) -> NamespaceTxnOutcomeV2<()>;

    pub fn recover_namespace_transactions(
        &mut self,
        limit: NonZeroUsize,
    ) -> NamespaceRecoveryOutcomeV2;
}
```

A read-only verified descriptor may safely escape only as an opaque pinned-read type; bytes advertised as an exact snapshot require bounded read plus object/length/digest checks before and after. A persistent flock fd may escape inside its guard. Writable files and namespace-effect handles remain inside `JournalRootOperationV2` until settlement.

## Result vocabulary

```rust
pub enum NamespaceTxnOutcomeV2<T> {
    ConfirmedNoTargetEffect {
        reason: NamespaceRefusalV2,
        residue: Option<NamespaceRecoveryTicketV2>,
    },
    Complete(CompleteNamespaceEffectV2<T>),
    ConfirmedCommittedRetained {
        committed: CommittedNamespaceEffectV2<T>,
        recovery: NamespaceRecoveryTicketV2,
    },
    Retained {
        recovery: NamespaceRecoveryTicketV2,
        state: RetainedNamespaceStateV2,
    },
    Unknown {
        recovery: Option<NamespaceRecoveryTicketV2>,
        cause: String,
    },
    Unsupported {
        phase: NamespacePhaseV2,
        recovery: Option<NamespaceRecoveryTicketV2>,
        cause: String,
    },
}
```

Only `CompleteNamespaceEffectV2<T>::into_value(self)` projects durable or destructive success. There is no `is_durable()` convenience predicate. `ConfirmedCommittedRetained` records that the target transition is known but cannot be flattened to success while displaced-object cleanup remains.

## Exact-child exchange and recovery

Linux uses `syscall(SYS_renameat2, ..., RENAME_EXCHANGE)` so the pinned `libc 0.2.186` compiles across GNU/musl declarations. macOS uses `renameatx_np(..., RENAME_SWAP)`. Other targets return build-time `Unsupported` without making a syscall.

Runtime `ENOSYS`, `EOPNOTSUPP`/`ENOTSUP`, `EINVAL`, `EXDEV`, `ENOENT`, and all other errors are preserved. No errno proves no effect: identities at both names are classified before deciding. There is no `renameat`, link/unlink, copy, or raw-path fallback.

Replacement and retirement share this protocol:

1. Create and sync a unique proposed file—replacement bytes or a tombstone.
2. Create an immutable intent containing operation kind, target name, expected target snapshot, proposed identity, and both transaction names.
3. Sync the intent file, then sync the root. No exchange is admitted before this barrier.
4. Exchange proposed and target. This is the namespace linearization point.
5. Open both names and classify:

   - target = proposed and custody name = expected: confirmed committed;
   - target = expected and custody name = proposed: confirmed no target effect;
   - anything else: retained/unknown; unlink nothing.

6. Sync the root.
7. Under the still-held cooperative operation lock, verify and remove the displaced object. Retirement additionally removes the tombstone from the target.
8. Sync the root after every removal phase.
9. Reverify final target state, remove the intent, and sync the root again.
10. Only then construct `Complete`.

Crash recovery runs under the same operation lock before new admission. It handles:

| Durable/observed cut | Recovery |
|---|---|
| Stage without complete intent | Retain as unowned residue; never infer deletion |
| Intent synced, target still expected, stage proposed | Confirm no target effect; exact cleanup |
| Exchange visible, root not yet synced | Verify both identities, sync, continue |
| Replacement visible, displaced expected retained | Finish displaced cleanup |
| Retirement tombstone at target | Remove exact tombstone, then displaced expected object |
| Target final, custody name absent, intent present | Complete final sync and retire intent |
| Foreign, missing, duplicate, malformed, or over-cap names | Preserve and return `Unknown` |

Exchange does not provide identity-conditional CAS. If B replaces A immediately before exchange, B may be moved into the custody name. The protocol preserves B and refuses success; it cannot promise that B was never temporarily displaced. If even temporary foreign displacement is forbidden, the proposed common Linux/macOS contract is insufficient.

## Root state machine

```text
Unopened
  → DescriptorsOpened
  → ExpectedLockOpened
  → FlockHeld
  → BindingValidated
  → Active
  → Settling
  → Released
```

- Replacement before acquisition: opening or final binding validation refuses with confirmed no target effect.
- Replacement while waiting for the lock: final validation after flock acquisition detects it.
- Replacement after acquisition by a cooperating participant: impossible because root lifecycle mutations require the same lock.
- Replacement by a noncooperating peer after validation: outside the cooperative success contract; a post-effect check can detect a persistent replacement and return `Unknown`, but cannot close a replacement-after-last-check schedule.
- Root exchange/quarantine is rejected as the normal authority mechanism. It makes the configured route temporarily name a placeholder, has the same ancestor trust problem, and an arbitrary peer can race the restoration. It may be useful only as protective recovery, never as the basis for `Complete`.

The operation lock must live beneath the declared trusted parent, not inside the replaceable root. Its exact identity must be persisted outside the mutable root. Task B cannot manufacture that trust by reading a manifest from the root it is trying to authenticate.

## KEEP / REVISE / REPLACE

| Classification | Existing mechanisms |
|---|---|
| KEEP | `BirthTimeV1`; validated one-component names; `open_child_no_follow` with `O_NONBLOCK`; `stat_child_no_follow`; `rename_child_no_replace`; bounded `readdir`; failure-countdown concept; simultaneous-open dev/inode comparison. |
| REVISE | `RegularFileIdentityV1` → object identity plus length/digest snapshot; `PinnedDirectoryV1` becomes mechanism-only; `JournalRootCustodyV1::{open,revalidate}` → binding plus operation guard; `classify_publication_rename_effect` → two-name exchange classifier; `acquire_persistent_lock_file` → blocking already-open helper; sync/error hooks move to actual syscall boundaries. |
| REPLACE | `CustodyPublicationV1` and `settle_publication`; plain replacing `renameat`; `unlink_regular_child_at`; public raw-file returns from create/append; `RegularChildRefV1` as caller-supplied authority; `acquire_persistent_child_lock` as a free-standing root method; identity-optional replacing target; any strong-authority use of `verify_then_remove`. |
| KEEP only as explicitly weaker legacy behavior | `verify_then_remove` and current path-based reaper classification may remain temporarily for their existing reporting contract, but cannot be cited as proof for the new exact namespace layer. |

Existing candidate tests map as follows:

- KEEP/retarget: identity mismatch, birthtime, symlink/FIFO refusal, child-name bounds, bounded enumeration.
- REVISE: root replacement/removal tests become before-lock and actual-boundary schedules.
- REPLACE: `journal_root_custody_revalidates_at_mutation_time`; its hook currently runs before revalidation, not at the last syscall boundary.
- REVISE: child-substitution test must substitute immediately before exchange and assert retained foreign custody, not merely precheck refusal.
- REVISE: persistent-lock test adds independent contention on the original renamed inode and removes `_file` inspection.

## Red-first schedules

Required deterministic tests include:

- `cooperating_root_replacement_blocks_at_last_pre_syscall_boundary`: peer obtains replacement authority only through the same operation lock; it cannot mutate until the first transaction settles.
- `noncooperating_root_replacement_at_last_boundary_cannot_be_proved_safe`: demonstrates why the arbitrary-peer contract cannot be greened by another recheck.
- `exchange_substitution_retains_foreign_target_without_success`: A is moved aside, B is installed immediately before exchange; B remains in custody and outcome is not `Complete`.
- `cleanup_substitution_never_unlinks_foreign_custody`: direct last-boundary substitution cannot be green under the arbitrary model; under the cooperative model the substituter blocks.
- One crash test after each step: stage create, stage sync, intent sync, exchange, first root sync, target tombstone removal, displaced cleanup, final root sync, intent unlink.
- Runtime unsupported injection both before and after a simulated exchange effect; no fallback call observed.
- Append negatives for same inode/wrong length, same length/wrong digest, partial write, file-sync failure, and root-binding loss.
- Persistent-lock test: guard 1 locks original; root name is moved; guard 2 opens that original under its custody name and receives `WouldBlock`; replacement-path lock remains a distinct object and cannot satisfy the expected identity.

## Caller ripple

- `JournalRootCustodyV1` has no production callers, so its candidate API can be replaced without compatibility shims.
- `bridge-worktree::WorktreeCustodianV1` must capture the current record snapshot and consume the typed transaction result. Its `.custody-locks` directory is currently inside the replaceable root; that is not a valid trusted root lock without the cooperative/trusted-parent ruling.
- `probe_custody_record_presence` may return `ProvablyAbsent` only from a root-bound operation; root ambiguity becomes `Inconclusive`.
- `local_file` replacement/removal and its compatibility callers need either migration or an explicit narrower owner-lock contract. They may not import the new `Complete` proof without migration.
- Both storage reapers need a separate directory-object transaction design if arbitrary peers remain in scope; regular-file exchange does not make recursive path removal exact.
- The shared `FileResourceFlightJournal` is not migrated by request Task A/B. It must be explicitly excluded as a different cooperative contract or scheduled as its own migration; otherwise “whole family closed” is false.
- Future `remote_request_flight` tests/doubles must return the full outcome vocabulary. No double may turn `Retained`, `Unknown`, or `Unsupported` into `Result<(), _>`.

## Green-after-each-task sequence

The requested A1/A2 split is too coarse. The smallest reviewable sequence is four core tasks plus a caller-closure task, all salvaging `517703cb`.

1. A1 — platform exchange and identity foundations.

   - Files: `crates/bridge-core/src/fs_custody.rs`, `liveness.rs`.
   - Symbols: complete identities/snapshots, `ChildNameV2`, `NamespaceTxnIdV2`, exchange report/classifier, blocking already-open flock helper.
   - Before: plain replacement and optional/persisted identity are conflated.
   - After: target-specific syscall mechanics and classification exist but no production caller changes.
   - Red first: Linux/macOS exchange, simulated error-after-effect, runtime/build unsupported, original-object flock contention.
   - Order: types → cfg syscall arms → classifier → lock helper → tests.
   - Focused gate: `cargo test -p bridge-core fs_custody::tests::namespace_exchange` and liveness persistent-lock tests.
   - Stop: 200 production or 450 total changed lines.
   - Commit only after the common whole-tree gate; exact commit becomes A2 input.

2. A2 — trusted-parent root operation lease.

   - Files: `fs_custody.rs`, `liveness.rs`, narrow `bridge-core/src/lib.rs` export if needed.
   - Symbols: `JournalRootBindingV2`, `JournalRootCustodyV2`, `JournalRootOperationV2`; remove candidate free-standing `revalidate` authority.
   - Before: revalidation and mutation are separate.
   - After: every strong operation requires one held root-operation value; lock acquisition is bound to the persisted lock and root identities.
   - Red first: replacement before/waiting/after lock, removed root, wrong lock inode, split-lock replacement.
   - Order: open-only binding → lock acquisition → chain validation → root sync/read/enumeration → adversarial hooks.
   - Focused gate: `cargo test -p bridge-core journal_root_operation`.
   - Stop: 300 production or 650 total.
   - Full gate and one commit.

3. A3 — exact replacement/retirement transaction and crash recovery.

   - Files: preferably new `crates/bridge-core/src/namespace_transaction.rs`, plus narrow `fs_custody.rs` primitives and `lib.rs`.
   - Symbols: immutable intent, `NamespaceTxnOutcomeV2`, recovery ticket, `replace_exact`, `retire_exact`, bounded recovery census.
   - Before: target verification is not coupled to the namespace effect.
   - After: exchange is classified from both identities; no displaced name is unlinked before exact recovery proof.
   - Red first: last-boundary A→B substitution; every exchange/cleanup crash cut; malformed/duplicate/over-cap intent; no-fallback platform failure.
   - Order: outcome/proof types → intent codec → exchange settlement → retirement → recovery → fault matrix.
   - Focused gate: `cargo test -p bridge-core namespace_transaction`.
   - Stop: 450 production or 850 total.
   - Full gate and one commit.

4. A4 — publication, append, and candidate API replacement.

   - Files: `fs_custody.rs`, `namespace_transaction.rs`, candidate tests and handoff.
   - Symbols: `publish_new`, `append_exact`, bounded read/enumeration; delete raw candidate methods and `CustodyPublicationV1` projection.
   - Before: writable `File` and boolean durability can escape.
   - After: complete bytes/length/digest and namespace settlement precede the only success proof.
   - Red first: target appears at no-replace boundary; append length/digest mismatch; partial write/sync failures; root substitution during every method.
   - Order: bounded read → create/publish → append → remove obsolete APIs → migrate candidate tests.
   - Focused gate: full `cargo test -p bridge-core fs_custody`.
   - Stop: 400 production or 800 total.
   - Full gate and one commit.

5. A5 — authority-census closure before Task B.

   - Files: at minimum `bridge-worktree/src/custody_writer.rs`, `custody.rs`, `custody_lock.rs`; additional `local_file`, reaper, and shared-journal tasks depend on the owner’s scope ruling.
   - Before: production callers can still use the weaker family while importing stronger language.
   - After: every caller either consumes the new proof type or is explicitly typed/documented as a narrower legacy contract that cannot project `Complete`.
   - Red first: worktree-record substitution; false-absence probe under root replacement; local-file quarantine substitution; both reapers’ last-boundary replacement; shared-journal root replacement if retained in scope.
   - Split one subsystem per commit. Maximum 350 production/700 total per subsystem.
   - Task B remains blocked until the owner-approved census is green.

After each task, run focused tests first, then:

```text
git diff --check
cargo fmt --all -- --check
CARGO_INCREMENTAL=0 cargo check --workspace --all-targets --all-features --locked
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
CARGO_INCREMENTAL=0 cargo test --workspace --all-features --locked
CARGO_INCREMENTAL=0 cargo build --release --bin a2a-bridge --locked
cargo deny check
target/release/a2a-bridge validate --repo-hygiene
```

Record exact totals and exclusions. A red gate blocks the commit and successor. Crossing a size stop blocks before more code is added; two consecutive cuts that cannot stay below their limits are mechanism-level evidence for unsalvageability.

## Rejected alternatives

- Another final identity recheck: leaves the same gap.
- A path-opened flock: root replacement creates a second lock namespace.
- Plain replacing rename followed by verification: it may already have destroyed B.
- Exchange followed by immediate unchecked unlink: moves the race to the quarantine name.
- Random custody names alone: collision resistance is not exclusion after enumeration.
- Root exchange as ordinary locking: temporarily breaks the configured route and does not solve ancestor replacement.
- Raw-path or copy/link fallback when swap is unavailable.
- Returning writable `File` plus caller comments.
- Restarting from `530992b7`: the candidate’s nonblocking opens, birthtime, expected identities, bounded enumeration, descriptor primitives, and tests remain salvageable.

No edit, build, test, dependency operation, network call, provider invocation, or helper was performed. Final status remained clean at exact `517703cb`; only read-only inspection and `git diff --check` ran.

DESIGN LENS: NOT READY

- Owner must choose cooperative bridge participants versus arbitrary same-privilege namespace peers. The latter is not implementable with the proposed cross-platform userland protocol.
- Owner must designate and persist a trusted parent anchor plus the exact sibling operation-lock identity outside the mutable journal root.
- Owner must decide whether temporary displacement of a foreign last-instant target into retained quarantine is an acceptable protective result. If not, exchange is insufficient.
- Owner must define “whole defect family” scope: request journal only, or also worktree custody, `local_file`, both reapers, custody-presence probes, and the shared process/container file journal.
- Task B and slice 3d must remain blocked until those rulings are incorporated and A1–A5 are green and committed.
