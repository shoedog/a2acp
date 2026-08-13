# R2f1b 3c2 Task A — namespace custody redesign (independent Opus/xhigh custody lens)

Read-only design pass. No edits, builds, tests, installs, network, providers, or nested
helpers were performed. All claims below are traced to exact symbols in the retained
candidate `517703cb` at `/Users/wesleyjinks/code/.a2a-implement/impl-63492-ma5xqzan`.

## Context

Task A's retained candidate adds `JournalRootCustodyV1` to
`crates/bridge-core/src/fs_custody.rs` and widens one field in
`crates/bridge-core/src/liveness.rs`. Two closure-review WRONGs of one open class
(root-entry TOCTOU, exact-child TOCTOU) parked it. This round designs the layer that
closes the whole family.

Three findings reframe the problem before any design starts.

**F1 — `revalidate` is two-thirds dead code, and the live third does not prove the route.**
`JournalRootCustodyV1::revalidate` (`fs_custody.rs:622`) runs three checks. The first two —
`verify_directory_file(&self.parent.file, …)` and `verify_directory_file(&self.root.file, …)`
— are tautologies. `verify_directory_file` (`fs_custody.rs:1085`) compares only `dev`, `ino`,
and `btime` (it never reads `canonical_path`), and all three are immutable properties of an
open inode. They can fail only if `fstat` itself fails. Only the third check does work:
`openat(parent_fd, root_name)` and compare to the pinned root identity.

That live check proves *"the pinned parent inode currently has a child named `root_name`
that is the pinned root inode."* It does **not** prove the configured path
`parent_path/root_name` names that object, because the parent inode is never re-anchored
after `PinnedDirectoryV1::open` canonicalized it. A peer that renames the **parent**
directory leaves every `revalidate` passing while the whole custody operates on a detached
subtree. This is strictly broader than the closure review's root-entry WRONG, and no
additional recheck inside `revalidate` can reach it.

**F2 — the workspace already contains the correct protocol, in production, with a working
red test.** `bin/a2a-bridge/src/local_file.rs` implements capture-by-no-replace-into-
quarantine: `removal_quarantine_name` (`:45`), `regular_child_removal_candidate` (`:1285`,
the crash-recovery resolver), and `remove_regular_child_candidate_with_hooks` (`:1325`) —
capture via `fs_custody::rename_child_no_replace`, sync, reopen the quarantine by
descriptor, re-verify identity, `unlinkat` the quarantine name, then prove
`child.file.metadata()?.nlink() == 0`. Its adversarial test
`descriptor_relative_removal_never_deletes_a_replacement_exchanged_after_verification`
(`:2330`) injects a real `RENAME_EXCHANGE`/`RENAME_SWAP` substitution **at the last
pre-syscall boundary** and proves the protocol refuses. The mechanism the brief asks about
in Q2 already exists, already ships, and is already adversarially tested — as the
*attacker's* primitive, not the defender's.

**F3 — `JournalRootCustodyV1` has zero callers outside `fs_custody.rs`.** A workspace grep
finds it only in its own definition and its own tests. `PinnedDirectoryV1`, the publication
primitives, `CustodyPublicationV1`, and `verify_then_remove` have real callers
(`custody_writer.rs`, `sweep.rs`, `storage_reap*.rs`, `local_file.rs`,
`compatibility_*.rs`). So the *policy* layer can be redesigned with zero production ripple,
while the *mechanism* layer must stay signature-stable. This is the salvage seam.

---

## 1. Invariants and state machines

### 1.1 The organizing invariant

> **I0 (linearization).** Every namespace mutation must derive its precondition from one of
> exactly three sources: **(A)** the syscall itself carries the precondition atomically
> (`RENAME_NOREPLACE`/`RENAME_EXCL`, `O_CREAT|O_EXCL`); **(B)** the object is addressed by
> descriptor, never by name (`fstat`, `read`, `write`, `fsync`, `flock`, `ftruncate`,
> `fdopendir`); **(C)** mutual exclusion under the operation lock makes a check/act pair
> effectively atomic. Nothing else is a linearization point.

An identity recheck is a *window narrowing*, never a source. This is precisely why the
closure review classified the defect open-class: `revalidate` + `openat`-verify + name-syscall
is check/act under none of A, B, or C. Every operation below is annotated with which of
A/B/C carries it.

**Impossibility result (must be stated, not designed around).** There is no POSIX, Linux, or
macOS syscall of the form "unlink/rename the entry at NAME only if it currently resolves to
INODE." `RENAME_EXCHANGE`/`RENAME_SWAP` does not provide one — it too selects by name. So a
retirement or replacement that is *provably* exact against an arbitrary local peer is
**unachievable at the syscall level**. Everything below is the strongest achievable contract,
and §6 O1 makes the residual an explicit owner decision rather than a silent weakening.

### 1.2 Authority invariants

- **R1 (anchor).** Route authority terminates at a declared anchor directory. The anchor's
  own reachability is a *deployment* obligation, not a code obligation: renaming the anchor
  requires write permission on the anchor's parent. `open` stats the anchor's parent and
  refuses if it is group- or world-writable (see O4).
- **R2 (route).** `RouteProofV1` is issuable only by proving, in one uninterrupted sequence:
  anchor fd → `openat(anchor, parent_name)` == pinned parent → `openat(parent, root_name)`
  == pinned root. Every proof is descriptor-relative from the anchor down. The two dead
  checks in `revalidate` are deleted.
- **R3 (route ≠ object).** "This record is durable in the root **object**" is provable by
  descriptor (B). "The root object is the configured journal" is provable only under the
  lock (C). These are separate facts and must be separately carried. `Committed` requires
  both; an operation whose object proof holds but whose route proof failed projects
  `Retained`, never `Committed` and never `NoEffect`.
- **R4 (lock).** Every mutator runs under `JournalOperationLockV1`, whose linearization
  point is the `flock(LOCK_EX|LOCK_NB)` on a descriptor obtained by `openat` from the root
  fd and identity-verified before the flock (B). The route is re-proved *after* the flock;
  a failed re-proof releases the lock and yields `RootRouteLost` — the lock was on a real
  object, just not the configured journal's.
- **R5 (private namespace).** The three reserved namespaces `.a2a-jrnl-swap-v1.*`,
  `.a2a-jrnl-del-v1.*`, `.a2a-jrnl-stage-v1.*` are writable only by lock holders. A
  no-replace capture into one of them is the linearization point (A). A peer that plants an
  object in a reserved namespace has already violated the contract; the protocol detects it
  and refuses, but cannot un-destroy an object the peer itself planted there.
- **R6 (destructive proof).** A destructive projection requires a *positive complete* proof:
  the retired object's retained descriptor reports `nlink() == 0` **and** the route proof
  held at both ends. Absent either, the outcome is `Retained` or `Unknown`.
- **R7 (bounded storage).** Reserved names are deterministic and reversible per target, so
  at most one of each kind exists per record name. Enumeration keeps the existing
  `EnumerationLimitExceeded` bound. Unattributable reserved-namespace entries are reported
  as orphans, never deleted.

### 1.3 Reversible reserved names (design decision, diverges from `local_file.rs`)

`local_file::removal_quarantine_name` hashes the target with SHA-256. That is fine there —
it only ever asks the *forward* question "is the quarantine for this known name present?".
Journal recovery must ask the *reverse* question, because the very state it recovers is the
one where the record name is absent from the directory. A hash makes recovery a partial
function.

```
swap_name(t)  = ".a2a-jrnl-swap-v1."  ++ lower_hex(t)
del_name(t)   = ".a2a-jrnl-del-v1."   ++ lower_hex(t)
stage_name(t) = ".a2a-jrnl-stage-v1." ++ lower_hex(t)
```

Hex is single-component-safe by construction (no `/`, no NUL, never `.`/`..`). Longest
prefix is 20 bytes, so `MAX_RECORD_NAME_LEN = 96` guarantees every derived name fits
`NAME_MAX = 255`. Record names longer than 96 bytes are refused at the validation boundary,
before any syscall. Recovery decodes the hex suffix and recovers the target name exactly.

### 1.4 Two quarantine namespaces are load-bearing — one is provably fail-destructive

`swap` and `del` **must not share a namespace.** Crash recovery reads the two-bit state
`(target present?, reserved present?)` and the correct action differs by intent:

| state | `swap` present | `del` present |
|---|---|---|
| `(absent, present)` | **roll back** — restore reserved → target | **roll forward** — finish the retirement |
| `(present, present)` | **roll forward** — retire the displaced predecessor | contract violation → `Unknown`, report |
| `(present, absent)` / `(absent, absent)` | nothing to do | nothing to do |

With a single namespace, the state `(target absent, reserved present)` is ambiguous. Under
`local_file`'s existing rule (`regular_child_removal_candidate:1302` maps `(false, true)` to
"the quarantine is the removal candidate") a crashed *replacement* recovers as a completed
*deletion*: the predecessor is destroyed and the target name stays empty. That is a
fail-destructive outcome reachable by a naive hoist of the existing helper. §5 pins it with
a paired red test.

### 1.5 Operation state machines

**Publish (free name).** Carried by A.
`lock → route → open staged fd, verify object → [BeforePublishRename] →
rename_no_replace(stage → target) → fsync(root) → reopen target by descriptor, verify ==
staged object → route re-proof → Committed`.
`EEXIST` stays a true `NoEffect` by the existing `classify_publication_rename_effect` rule 1
(the staged source is untouched, so the intact-source rule fires before the target is
consulted). This path is already correct in the candidate and is kept.

**Replace (exact target A → new N).** Carried by A + A + C.
```
1. lock; route proof; open stage, verify N
2. [BeforeCaptureRename] rename_no_replace(target → swap_name(target))     <-- linearization
3. fsync(root)
4. openat(swap_name), fstat
     != A  -> rename_no_replace(swap_name -> target)
                ok    -> NoEffect{RestoredExact}      (B was moved out and back; net zero)
                EEXIST-> Retained{swap_name}          (peer took the name; B is in our custody)
     == A  -> continue
5. [BeforeRepublishRename] rename_no_replace(stage -> target)
     EEXIST -> Retained{swap_name}   (peer published; A is safe, we did NOT clobber)
     other  -> classify_publication_rename_effect(...)  [existing, unchanged]
6. fsync(root); reopen target, verify == N
7. [BeforeQuarantineUnlink] openat(swap_name), verify == A, unlinkat(swap_name),
   assert A_fd.nlink() == 0
     failure -> Committed{ residue: Some(swap_name) }   (replacement is durable; residue bounded)
8. fsync(root); route re-proof -> Committed
```
Step 2 is the whole design. Whatever occupied `target` at the rename's linearization point is
now under a name only lock holders may create, and the "is it A?" question is answered by
descriptor (B) *before our record ever becomes visible at the authoritative name*. This is
**refuse-before-commit**. The candidate's `replace_regular_child_impl` (`fs_custody.rs:1523`)
verifies then renames — **detect-after-commit at best**, and in fact no detection at all.

**Retire (exact target A).** Carried by A.
`lock → route → [BeforeCaptureRename] rename_no_replace(target → del_name(target)) →
fsync → openat(del_name), fstat`; if `!= A` restore and report `NoEffect{RestoredExact}`; if
`== A` then `[BeforeQuarantineUnlink] unlinkat(del_name)`, assert `A_fd.nlink() == 0`,
`fsync`, route re-proof → `Committed`.

**Create (staged).** Carried by A. `openat(root, stage_name(target), O_CREAT|O_EXCL|O_RDWR|
O_CLOEXEC|O_NOFOLLOW, 0600)` returns a `StagedRecordV1<'lock>`, never a raw `File`.

**Append.** Carried by B. `openat(root, name, O_RDWR|O_APPEND|O_NOFOLLOW|O_NONBLOCK)`,
verify `ObjectIdentityV1` **and** `ContentPositionV1` (the resume offset), then return an
`AppendSessionV1<'lock>` that owns the fd. Post-open name replacement cannot redirect writes
through a fd — that is the operator's correctly-downgraded SMELL, and the type is what
discharges the remaining obligation: the session owns the length bookkeeping, borrows the
lock, and no raw `File` escapes.

**Enumerate / sync.** Carried by B. Bracketed by route proofs; results typed as hints
(§3.4).

**Lock acquisition.** Carried by A + B. `openat(root, ".a2a-jrnl.lock", O_RDWR|O_NOFOLLOW|
O_NONBLOCK)` — created with `O_CREAT|O_EXCL` on first use — verify `ObjectIdentityV1`,
`[BeforeLockFlock]`, `flock(LOCK_EX|LOCK_NB)` on that fd, then re-prove the route.
- **root replaced before acquisition:** we open the lock child under the old root fd and
  lock the old root's lock object; the post-flock route proof fails → release →
  `RootRouteLost`. Correctly refused.
- **root replaced during acquisition:** identical, same proof, same classification.
- **root replaced after acquisition:** under the cooperating model this requires the lock and
  cannot happen. Under the arbitrary-peer model it is caught by the closing route proof of
  whichever operation is in flight → that operation projects `Retained`/`Unknown`.
- **two concurrent bootstrappers straddling a rename:** they flock two different inodes and
  both would believe they hold the cell. The post-flock route proof resolves it: at most one
  process holds the flock on the inode the route currently names. The other releases.

### 1.6 Recovery (runs under the lock, at every acquisition)

Recovery is the *only* reclamation mechanism. `Drop` deliberately does not clean up (§3.5).

```
for each name in enumerate(root, LIMIT):
    if name matches a reserved prefix:
        target = hex_decode(suffix)            // total function, by §1.3
        classify (target present?, kind) per the §1.4 table and act
    else: leave alone
report any reserved name whose suffix does not hex-decode as Orphan (never delete)
```
The lock acquirer runs recovery for **all** residue, not only its own — a crashed owner
cannot clean up after itself. Recovery is idempotent and re-entrant across crashes because
every step is itself a no-replace rename or a verified unlink.

### 1.7 Sync ordering

Three barriers: after capture, after republish, after retire. Each makes the preceding
namespace state recoverable by §1.6. A failed barrier never yields `Committed` — it yields
`Retained` naming the reserved entry, so recovery can finish the job. The existing HONEST
LIMIT (`fs_custody.rs:2049`) still applies: no in-process test can prove `fsync` reached
stable media.

---

## 2. KEEP / REVISE / REPLACE salvage map

The candidate is salvageable. Its mechanism layer is sound and its Round-1 repairs (supplied
expected identities, birthtime in identity, nonblocking child opens, the no-replace
publication primitive) are all correct and carry forward. Only the four name-based mutators
and the route model are unsalvageable, and each has a named mechanism-level reason.

### KEEP — do not touch (mechanism layer + all 60 pre-existing tests)

| Symbol | Why |
|---|---|
| `BirthTimeV1` (`:32-106`) incl. pre-epoch canonicalization | correct, tested, independent |
| `DirectoryIdentityV1` + `matches` (`:110-137`) | external callers in `sweep.rs`, `r2f1b_deletion_gate.rs` |
| `rename_child_no_replace` + `RenameNoReplaceRefusalV1` (`:963-1033`) | **the load-bearing primitive.** Its compile-time-vs-errno discrimination is exactly right and is what makes the whole capture protocol possible |
| `rename_child_replacing` (`:1054`) | keep as mechanism; **remove all journal use** |
| `classify_publication_rename_effect` / `PublicationRenameEffectV1` (`:1553-1609`) | the evidence-order argument (intact-source before target, for the hard-link case) is sound and reusable verbatim at replace step 5 |
| `CustodyPublicationV1` + `settle_publication` + `publish_new_regular_child_impl` (`:209-588`, `:1409`) | production callers: `custody_writer.rs`, `compatibility_resolution.rs`, `compatibility_schedule_retention.rs` |
| `validated_child_name`, `ChildOpenOptionsV1`, `open_child_no_follow`, `stat_child_no_follow`, `open_directory_no_follow_raw`, `same_open_object`, `same_regular_file`, `child_name_cstring` | mechanism, correct |
| `FailureCountdownV1`, `PublicationRenameFaultV1`, `inject_publication_rename_fault` | fault seams, correct |
| `enumerate_directory_names` (`:1284`) | fd-bound and bounded; reuse as-is |
| `create_new_regular_child_at`, `open_regular_child_for_update`, `open_regular_child` | mechanism; rewrap, don't rewrite |
| `verify_then_remove` family (`:1839-2022`) | out of scope; its PARKED note stays. A3's capture primitive is the eventual building block for its parked gap — **not this task** |
| `open_options_create_new_owner_private` | correct |

### REVISE — salvage the body, change the contract

| Symbol | Change | Salvaged |
|---|---|---|
| `JournalRootCustodyV1::open` (`:599`) | → `AnchoredJournalRootV1::open`; add anchor + anchor-parent permission check, `RENAME_NOREPLACE` capability probe, btime-policy probe, filesystem-class probe | the whole expected-parent/expected-root identity plumbing (Round-1 repair #2) verbatim |
| `revalidate` (`:622`) | → `prove_route() -> Result<RouteProofV1, _>`; **delete** the two tautological `verify_directory_file` self-checks; extend the live check up to the anchor | the live third check verbatim |
| `open_regular_child` (`:628`) | returns `ReadableRecordV1` (verified fd) | body |
| `enumerate_child_names` (`:691`) | returns `Vec<ChildNameHintV1>`; keep the existing proof bracket | body + the `EnumerationLimitExceeded` bound |
| `sync` (`:714`) | returns `SyncOutcomeV1` | body |
| `acquire_persistent_child_lock` (`:718`) | → `JournalOperationLockV1::acquire`; returns a self-contained guard, **never** a `PersistentLockGuard` | the openat→verify→flock-on-fd sequence verbatim — this is the one candidate mutator that is genuinely fd-bound and correct |
| `liveness.rs` diff | **revert both hunks.** Restore `_file` to private; drop `acquire_persistent_lock_file`. Instead promote `flock_nb`/`flock_blocking_exclusive` to `pub(crate)` (pure mechanism, no state). Net liveness diff: 2 visibility words | — |

**Why the `PersistentLockGuard` return must go (this is a live fail-open, not a style point).**
`acquire_persistent_child_lock` (`:728`) builds the guard with
`path: self.root.canonical_path.join(name)` — a *path* that may not name the locked object at
all once the root is detached. `PersistentLockGuard::path()` is public, and
`storage_report::probe_lock_path` (`:555`) opens a lock path by name and reports
`Free`/`Held` via `FsLeaseProbe`. Handing a journal lock's path to that probe, or to
`liveness::acquire_existing_persistent_lock_blocking` (`:249`, which re-opens by path), makes
a name substitution report `Free` while the real lock is held. Giving the journal its own
guard type makes that misuse unrepresentable.

### REPLACE — delete, with the mechanism-level reason

| Symbol | Reason it is unsalvageable |
|---|---|
| `unlink_regular_child` (`:703`) + `unlink_regular_child_at` (`:1245`) | verify-then-`unlinkat`-by-name. Under I0 it is carried by none of A/B/C. No recheck placement fixes it |
| `replace_regular_child` (`:674`) | verify-then-`renameat`-by-name; same reason. Detect-after-commit at best |
| `replace_regular_child_impl`'s `expected_target: Option<(&RegularFileIdentityV1, &JournalRootCustodyV1)>` (`:1498`) | a policy back-reference threaded into the mechanism layer to implement the broken check. Deleting it also restores the pre-candidate signature, removing ripple from `PinnedDirectoryV1::replace_regular_child` |
| `create_new_regular_child` + `_with_before_mutation` (`:632`, `:639`) | returns a raw `File` after a namespace effect with no owner and no cleanup path |
| `open_regular_child_for_append` (`:663`) | returns a raw writable `File` outside any transaction or lock |
| `RegularFileIdentityV1` (`:141`) | **conflates "which object" with "how much content."** `verify_regular_file_identity` (`:1179`) compares `len`, so (a) a caller's expected identity is stale the instant it appends, and (b) a *legitimate* prior append by a lock-holder invalidates a retirement that should be allowed. Retirement must key on object identity; append must key on both. One type cannot do both |
| the 8 `journal_root_custody_*` tests (`:4326-4611`) | they assert the replaced contract. Several assertions carry over verbatim into §5; the helper `journal_custody()` (`:4308`) is revised, not discarded |

---

## 3. Proposed types (compile-shaped)

All in `crates/bridge-core/src/fs_custody.rs` unless noted. `#[cfg(unix)]` throughout; the
`cfg(not(unix))` arms return `Unsupported` with **no semantic fallback**, matching the
existing module convention.

### 3.1 Identity, split

```rust
/// WHICH object. Immutable for the object's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectIdentityV1 {
    pub dev: u64,
    pub ino: u64,
    /// `None` only when the custody opened in `BirthtimePolicyV1::Degraded` (see §6 O2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btime: Option<BirthTimeV1>,
}

/// HOW MUCH content. Mutable; the append resume offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPositionV1 { pub len: u64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BirthtimePolicyV1 { Required, Degraded }
```

### 3.2 Unforgeable proof tokens

```rust
/// Issued only by `AnchoredJournalRootV1::prove_route`. No public constructor, no `Clone`,
/// no `Default`, no `Copy` — so a caller cannot manufacture or reuse one.
#[derive(Debug)]
pub struct RouteProofV1 { _seal: () }

/// Issued only by a descriptor-level identity comparison against a live fd.
#[derive(Debug)]
pub struct ObjectProofV1 { identity: ObjectIdentityV1, _seal: () }
```

`Committed` is constructible only through a private constructor taking **both** tokens by
value. That is the mechanism answer to brief item 4: an unverified effect cannot be projected
as durable, because the value that would encode it cannot be built.

### 3.3 Result vocabulary (brief item 5)

```rust
#[must_use = "a namespace outcome must be classified; every non-Committed arm is protective"]
#[derive(Debug)]
pub enum NamespaceOutcomeV1<T> {
    /// Positive complete proof. Requires RouteProofV1 + ObjectProofV1 at both ends.
    /// The ONLY arm that may project as destructive or durable success.
    Committed { value: T, residue: Option<OsString> },
    /// Positive proof that the namespace is in the state it started in.
    NoEffect(NoEffectProofV1),
    /// The object is safe under a name only lock holders may create, but the target or the
    /// route is unresolved. Names the reserved entry so recovery can finish.
    Retained { reserved: OsString, detail: String },
    /// Cannot decide. Never licenses deletion, retry, or a durability claim.
    Unknown(String),
    /// A required primitive is unavailable (compile-time platform, or a runtime filesystem
    /// refusal). Carries which, and the errno when there is one. No semantic fallback.
    Unsupported(UnsupportedReasonV1),
}

#[derive(Debug)]
pub enum NoEffectProofV1 {
    /// Refused before any syscall that could change the namespace.
    RefusedBeforeEffect(String),
    /// The target name was already taken (`EEXIST` under a no-replace rename), and the
    /// staged source is provably intact — `PublicationRenameEffectV1::NotRenamed`.
    TargetOccupied(String),
    /// Captured, found the wrong object, restored it exactly. Net namespace change is zero.
    RestoredExact { observed: ObjectIdentityV1 },
}

#[derive(Debug)]
pub enum UnsupportedReasonV1 {
    /// Compile arm only. Never inferred from an errno.
    PlatformLacksNoReplaceRename,
    /// Runtime: the filesystem refused the flag. Carries the errno verbatim.
    FilesystemRefusedNoReplaceRename(std::io::Error),
    /// Runtime: this filesystem exposes no birthtime and policy is `Required`.
    FilesystemLacksBirthtime,
    /// Runtime: advisory locking is not trustworthy on this filesystem class.
    FilesystemLockUntrusted(String),
}
```

The `PlatformLacksNoReplaceRename` / `FilesystemRefusedNoReplaceRename` split preserves the
candidate's own (correct) doctrine at `fs_custody.rs:953-962`: `ENOSYS`/`EOPNOTSUPP`/`ENOTSUP`
all decode to `ErrorKind::Unsupported`, so a compile-time limitation must never be inferred
from an errno.

### 3.4 The custody surface

```rust
pub struct AnchoredJournalRootV1 {
    anchor: File,                 // the declared authority terminus
    anchor_name_of_parent: OsString,
    parent: File,
    parent_identity: DirectoryIdentityV1,
    root_name: OsString,
    root: File,
    root_identity: DirectoryIdentityV1,
    birthtime_policy: BirthtimePolicyV1,
    capabilities: RootCapabilitiesV1,   // no-replace probe, lock-class probe
}

impl AnchoredJournalRootV1 {
    pub fn open(
        anchor_path: &Path,
        parent_name: &OsStr,
        root_name: &OsStr,
        expected_parent: &DirectoryIdentityV1,
        expected_root: &DirectoryIdentityV1,
        label: &str,
    ) -> Result<Self, FsCustodyError>;

    pub fn prove_route(&self, label: &str) -> Result<RouteProofV1, FsCustodyError>;

    /// Acquires the cell, re-proves the route, then runs recovery (§1.6). The returned
    /// guard borrows self, so no operation can outlive its route binding.
    pub fn lock(&self, label: &str)
        -> Result<(JournalOperationLockV1<'_>, RecoveryReportV1), FsCustodyError>;

    // --- fd-bound reads; safe WITHOUT the lock, carried by (B) ---
    pub fn open_record(&self, name: &OsStr, label: &str)
        -> Result<ReadableRecordV1, FsCustodyError>;
    pub fn enumerate(&self, limit: usize, label: &str)
        -> Result<Vec<ChildNameHintV1>, FsCustodyError>;
    pub fn sync(&self, label: &str) -> Result<SyncOutcomeV1, FsCustodyError>;
}

/// Self-contained. Releases with LOCK_UN on drop and never unlinks.
/// Deliberately NOT a `liveness::PersistentLockGuard`: it exposes no path, so it can never
/// be handed to a path-addressed probe or re-acquirer.
#[derive(Debug)]
pub struct JournalOperationLockV1<'root> {
    root: &'root AnchoredJournalRootV1,
    file: File,                       // private; the fd IS the authority
    locked: ObjectIdentityV1,
}

impl<'root> JournalOperationLockV1<'root> {
    pub fn stage_record(&self, target: &OsStr, label: &str)
        -> Result<StagedRecordV1<'_>, FsCustodyError>;

    pub fn publish(&self, staged: StagedRecordV1<'_>, target: &OsStr, label: &str)
        -> NamespaceOutcomeV1<ObjectIdentityV1>;

    pub fn replace_exact(
        &self, staged: StagedRecordV1<'_>, target: &OsStr,
        expected: &ObjectIdentityV1, label: &str,
    ) -> NamespaceOutcomeV1<ObjectIdentityV1>;

    pub fn retire_exact(&self, target: &OsStr, expected: &ObjectIdentityV1, label: &str)
        -> NamespaceOutcomeV1<()>;

    pub fn open_append(
        &self, target: &OsStr,
        expected: &ObjectIdentityV1, at: &ContentPositionV1, label: &str,
    ) -> Result<AppendSessionV1<'_>, FsCustodyError>;
}

/// A namespace effect that has NOT settled. Cannot be projected as anything.
#[derive(Debug)]
pub struct StagedRecordV1<'lock> {
    lock: &'lock JournalOperationLockV1<'lock>,
    name: OsString,
    file: File,
    identity: ObjectIdentityV1,
}
impl StagedRecordV1<'_> {
    pub fn writer(&mut self) -> &mut File;   // content only; the NAME is not the caller's
    pub fn sync(&self, label: &str) -> Result<(), FsCustodyError>;
}

/// Owns the writable fd. No raw `File` escapes; the lock cannot be released underneath it.
#[derive(Debug)]
pub struct AppendSessionV1<'lock> {
    lock: &'lock JournalOperationLockV1<'lock>,
    file: File,
    object: ObjectIdentityV1,
    position: ContentPositionV1,
}
impl AppendSessionV1<'_> {
    pub fn append(&mut self, bytes: &[u8], label: &str)
        -> Result<ContentPositionV1, FsCustodyError>;
    pub fn commit(self, label: &str) -> Result<ContentPositionV1, FsCustodyError>; // fsync
}

/// Enumeration returns HINTS, never authority. Acting on one requires re-proving identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildNameHintV1 { pub name: OsString, pub reserved: Option<ReservedKindV1> }
```

**Which operations may return a pinned fd (brief item 4).** Read-only opens
(`ReadableRecordV1`), enumeration, and sync may — content reads are fd-bound, so a later
rename is irrelevant. Create, append, replace, retire, and lock acquisition may **not**:
each is a namespace effect or an unbounded mutation and stays inside an owned object that
borrows the lock (`'lock`) and, transitively, the route binding (`'root`). The lifetimes are
the enforcement, not the documentation.

### 3.5 Drop and cleanup ownership

`StagedRecordV1::drop` and `AppendSessionV1::drop` **do not touch the namespace.** Two
reasons, both mechanism-level:

1. A Drop-time unlink would be name-addressed — the exact defect being closed.
2. Drop does not run after a crash. Recovery (§1.6) must exist regardless, and two
   reclamation mechanisms for one job is two places for it to diverge.

An unsettled drop emits a `tracing::warn!` naming the reserved entry and leaves deterministic
residue that the next lock acquisition reclaims. Storage is bounded by §1.3 (at most one
reserved entry per kind per record name) plus the existing enumeration bound.

---

## 4. Task sequence — green after every task

Four tasks, each independently reviewable, each leaving a compiling tree with a passing
workspace suite. `JournalRootCustodyV1` is **revised in place** rather than duplicated, so no
dead second implementation ever exists — which also satisfies the standing prohibition on
discarding a partially reviewed artifact.

| # | Scope | Files / symbols | Prod-line cap |
|---|---|---|---|
| **A1** | Anchored route + capability probes + identity split. No new mutators | `fs_custody.rs`: add `ObjectIdentityV1`, `ContentPositionV1`, `RouteProofV1`, `ObjectProofV1`, `RootCapabilitiesV1`, `BirthtimePolicyV1`, `UnsupportedReasonV1`; rename `JournalRootCustodyV1`→`AnchoredJournalRootV1`; rewrite `open`; `revalidate`→`prove_route` (delete the 2 dead checks); retype `open_regular_child`/`enumerate_child_names`/`sync` | **120** |
| **A2** | Journal operation lock bound to the root object | `fs_custody.rs`: `JournalOperationLockV1` + `acquire` + post-flock route proof + `Drop`. `liveness.rs`: **revert** both candidate hunks; `flock_nb`/`flock_blocking_exclusive` → `pub(crate)` | **100** |
| **A3** | Reserved-namespace capture primitives + recovery. Not yet wired to the journal mutators | `fs_custody.rs`: `swap_name`/`del_name`/`stage_name` + hex codec + `MAX_RECORD_NAME_LEN`; `capture_exact_child`, `restore_captured`, `retire_captured`; `recover_reserved_namespace`, `RecoveryReportV1`, `ReservedKindV1` | **130** |
| **A4** | Wire the mutators; delete the name-based ones | `fs_custody.rs`: `StagedRecordV1`, `AppendSessionV1`, `NamespaceOutcomeV1`, `publish`/`replace_exact`/`retire_exact`/`open_append`/`stage_record`. **Delete** `unlink_regular_child`, `unlink_regular_child_at`, `replace_regular_child`, `create_new_regular_child{,_with_before_mutation}`, `open_regular_child_for_append`, `RegularFileIdentityV1`, and `replace_regular_child_impl`'s `expected_target` param | **150** |

Total production: **500**, versus the candidate's 450 in one commit. The increase buys the
capability probes, the recovery path, and the reversible-name codec — none of which the
candidate has. If the owner holds 450 hard, A3's recovery moves to a fifth task; it must not
be dropped.

**Gates.** After each task, in order:
1. `cargo test -p bridge-core --locked --offline fs_custody`
2. `cargo test -p bridge-core --locked --offline journal_` (the new selector)
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo fmt --all -- --check` and `git diff --check`
5. **Full workspace suite**, totals reported. The pre-existing `a2a-bridge` failure
   `api_entry_resolves_and_serves_through_registry` (`api.prompt.error_body_read`) is
   reproduced on base `771c0fb8` in the same environment before attribution, per the
   attribution-control rule. It is reported, never re-baselined.
6. Commit boundary. One commit per task.

**Hard stop rule.** If any task's production diff exceeds its cap, STOP at that task —
do not carry overflow forward and do not start the next task. Re-plan the split with the
owner. A cap breach is a planning defect, not an implementation problem.

**Ripple.** Production ripple is **zero**: `JournalRootCustodyV1` has no caller outside
`fs_custody.rs` (F3). Two contained ripples remain:
- A4 restores `replace_regular_child_impl`'s pre-candidate signature. Verify
  `PinnedDirectoryV1::replace_regular_child`'s call site (`fs_custody.rs:530`) compiles
  unchanged — it already passes `None`.
- A2's `liveness.rs` revert removes `acquire_persistent_lock_file`. Confirm no caller was
  added outside `fs_custody.rs` (none exists today).

No test doubles need changing: `ReapEnv` fakes, `custody_writer`, and `sweep` touch only KEEP
symbols.

---

## 5. Deterministic adversarial schedules

All hooks fire at the **last statement before the syscall**, not before a recheck. The
candidate's `before_mutation` (`:639`) and `before_rename` (`:495`) run before `revalidate`,
so a passing test there proves only that the window is small. The new seam:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamespaceBreakpointV1 {
    BeforeRouteProof, BeforeLockFlock, BeforeCreateOpenat,
    BeforeCaptureRename, AfterCaptureBeforeVerify,
    BeforeRepublishRename, BeforePublishRename, BeforeQuarantineUnlink,
}
```
installed on `AnchoredJournalRootV1` as an `Option<Box<dyn Fn(NamespaceBreakpointV1)>>`,
compiled unconditionally for the same reason `FailureCountdownV1` is (`:301-310`): the
operational surface has no `cfg(test)` callers to key off, and an integration test cannot arm
a `cfg(test)` hook.

**Red-first adversarial (each must be red against `517703cb`):**

| Test | Injection | Required outcome |
|---|---|---|
| `retire_refuses_a_child_substituted_at_the_unlink_boundary` | at `BeforeCaptureRename`, `RENAME_EXCHANGE`/`RENAME_SWAP` target ↔ decoy (the exact primitive already used at `local_file.rs:2366`) | `NoEffect{RestoredExact}`; decoy intact; A intact. Candidate `unlink_regular_child` destroys the decoy |
| `replace_refuses_a_target_substituted_at_the_rename_boundary` | same, at `BeforeCaptureRename` | `NoEffect{RestoredExact}`; the new record is **not** visible at target. Candidate publishes over the substitute |
| `replace_retains_when_a_peer_takes_the_freed_target` | at `BeforeRepublishRename`, create the target | `Retained{swap_name}`; predecessor A safe under the reserved name; **not** `Committed` |
| `retire_refuses_a_substitution_in_the_reserved_namespace` | at `BeforeQuarantineUnlink`, swap the reserved entry | `Unknown`; `A_fd.nlink() != 0` detected; **never** `Committed` |
| `mutation_under_a_replaced_root_is_never_durable` | at `BeforeCaptureRename`, rename root away and create a new one | `Retained` or `Unknown` with a route detail; carries the candidate's assertion `!original.join("entry").exists() && !parent.join("journal/entry").exists()` verbatim |
| `mutation_under_a_replaced_parent_is_never_durable` | rename the **parent** directory away | must refuse. **This is red against the candidate today** — F1: `revalidate` cannot see it |
| `publish_keeps_eexist_a_true_no_effect` | peer publishes at `BeforePublishRename` | `NoEffect{TargetOccupied}`, via the existing rule-1 classifier. Regression guard on a KEEP path |

**Crash-cut recovery:**

```rust
pub enum CrashCutV1 { AfterCaptureBeforeSync, AfterCaptureSync,
                      AfterRepublish, AfterRepublishSync, AfterQuarantineUnlink }
```
The mutator aborts at the cut; the test drops the custody, opens a **fresh**
`AnchoredJournalRootV1` over the same directory, takes the lock (which runs recovery), and
asserts the namespace state.

The load-bearing pair — **both must pass simultaneously, which is impossible with one
reserved namespace (§1.4):**
- `replace_crash_after_capture_restores_the_predecessor`: cut `AfterCaptureSync` on a
  replace → recovery **restores** A at the target; A is not destroyed.
- `retire_crash_after_capture_completes_the_retirement`: cut `AfterCaptureSync` on a retire →
  recovery **finishes** the deletion.

Plus: `replace_crash_after_republish_retires_the_predecessor` (cut `AfterRepublish` →
recovery retires the swap entry, target holds N); `recovery_is_idempotent_across_repeated_
crashes` (run recovery twice, assert identical state); `recovery_reports_an_undecodable_
reserved_name_as_orphan_without_deleting_it`.

**Independent lock contention** (fixes the retained SMELL — the candidate's
`journal_root_custody_persistent_lock_stays_bound_after_name_replacement` (`:4582`) asserts
only that a peer fails the *identity* check, and reads `held._file` through the widened
field; it never contends):
- `journal_lock_is_held_on_the_original_object_after_name_replacement`: acquire; rename the
  lock child away and plant a replacement; then from a second thread `openat` the **renamed**
  original by its new name and `flock(LOCK_EX|LOCK_NB)` → must return `EWOULDBLOCK`. That is
  independent contention on the original inode, through no crate-private field.
- `journal_lock_on_a_replaced_name_does_not_contend`: `flock` the *replacement* inode → must
  succeed, proving the guard is inode-bound and the two are genuinely different cells.
- `two_bootstrappers_straddling_a_root_rename_cannot_both_hold_authority`: at
  `BeforeLockFlock` in thread A, thread B renames the root; exactly one acquirer survives its
  post-flock route proof, the other yields `RootRouteLost`.

**Capability probes:** `open_refuses_when_no_replace_rename_is_unavailable` (fault-inject the
probe → `Unsupported{FilesystemRefusedNoReplaceRename}`, and the custody does not open);
`open_records_degraded_birthtime_policy_when_the_filesystem_has_none`.

**Portability note on the existing suite.** `journal_custody()` (`:4308`) uses
`tempfile::tempdir()`, and `verify_directory_file` (`:1091`) returns `Unsupported` when
either birthtime is absent. On a filesystem that exposes no birthtime, every one of the eight
new tests fails at `JournalRootCustodyV1::open(...).unwrap()`. This is a *hypothesis with a
named mechanism*, not a verified observation — I did not run the suite. The falsifier is one
command on a Linux host with `TMPDIR` on tmpfs: `cargo test -p bridge-core journal_root_custody`.
The design's btime-policy probe (§6 O2) removes the dependency either way.

---

## 6. Owner decisions

**O1 — Threat model. Recommend: cooperating participants holding the operation lock, with an
explicit stated residual.** The impossibility result in §1.1 is the reason: no syscall on
either platform offers an inode-qualified unlink or rename, so "provably exact against an
arbitrary local peer" is unreachable *by any design*, including `RENAME_EXCHANGE`. The
strongest achievable contract, and what this design delivers:
- a peer cannot cause us to destroy any object outside our own reserved namespace;
- a peer that plants an object in the reserved namespace can cause that object to be
  destroyed — but placing it there is already a contract violation;
- every deviation projects `Retained` or `Unknown`, never `Committed`.

**The production lever is permissions, not syscalls.** If the journal root is mode 0700 under
a non-writable grandparent, "arbitrary local peer" collapses to "same-uid process," which is
the cooperating set plus any same-uid compromise — and same-uid compromise is inside the
trust boundary under every model. Decide by naming the production requirement that says
whether a non-owner uid can write the journal parent.

**O2 — Birthtime. Recommend: probe at open; `Degraded` (dev+ino only) with an operator-visible
attestation, not a hard refusal.** The candidate hard-requires birthtime in
`verify_directory_file` (`:1091`) and `regular_file_identity` (`:1169`), which makes the whole
journal unusable on any filesystem that exposes none — plausibly including container
overlayfs and some tmpfs configurations, i.e. discovered in production. `Degraded` loses
inode-recycling resistance and must be attested, never silent. *Evidence that collapses this:*
if the production requirement mandates recycle resistance, hard refusal is correct and the
deployment must place the journal on a btime-capable filesystem — decide which.

**O3 — Crash-cut rule for retirement. Recommend: roll forward.** A retirement that reached the
capture was already authorized; recovery finishes it. This matches
`local_file::regular_child_removal_candidate:1302`. It is safe **only** because §1.4 gives
replace and retire distinct namespaces. Confirm the rule so §5's paired test is binding.

**O4 — Anchor obligation. Recommend: enforce.** `open` stats the anchor's parent and refuses
if it is group- or world-writable, on the ground that renaming the anchor requires write on
its parent. Confirm the anchor path — most likely the bridge root — and whether refusal or
attestation is correct when the check fails.

**O5 — Hoist `local_file`'s quarantine into a shared mechanism now, or duplicate the policy?
Recommend: duplicate the policy for this task.** `local_file`'s messages are asserted verbatim
by `compatibility_*` tests, and its names are hashed while the journal's must be reversible
(§1.3). The module's own §A4 doctrine already says `fs_custody` owns the *mechanism* and each
binary owns its *policy*, and the shared mechanism (`rename_child_no_replace`) is already
shared. Revisit after Task A ships.

**O6 — Is `Retained` a terminal state the journal can sit in?** Or must a `Retained` outcome
escalate the journal to a blocked/degraded mode that refuses further writes until an operator
resolves the reserved entry? This is a journal-policy question this design cannot answer; it
determines whether A4 needs an escalation path.

---

## 7. Rejected alternatives

**`RENAME_EXCHANGE`/`RENAME_SWAP` as the common exact-child contract — rejected, with
mechanism.** The brief asks whether it can provide one contract. It cannot, for four
independent reasons:

1. **It is detect-after-commit, not refuse-before-commit.** After
   `exchange(stage, target)`, our record is already visible at the authoritative name. Only
   then can we reopen `stage` and learn whether we displaced A or a substituted B. If B, our
   record has already been published over a precondition violation and may already have been
   read. Rollback is a second racy exchange. Capture-by-no-replace decides the same question
   *before* our record is ever visible.
2. **macOS `RENAME_SWAP` requires both names to exist**, so it cannot express publish-into-
   a-free-name; a second primitive would be needed anyway, defeating "one common contract."
3. **`RENAME_EXCHANGE` support is narrower than `RENAME_NOREPLACE`.** The candidate's own
   doc (`:960`) already enumerates overlayfs, NFS, SMB, FUSE, and exFAT as runtime refusers of
   the no-replace flag; exchange is refused at least as widely. Standardizing on the narrower
   flag strictly shrinks the deployable surface.
4. **It buys nothing over capture.** Exchange also selects by *name*. It does not defeat the
   §1.1 impossibility result. The single-syscall atomicity is real but irrelevant, because the
   decision that matters ("is this A?") is a descriptor comparison that happens after either
   primitive.

The workspace's own verdict agrees: `RENAME_EXCHANGE`/`RENAME_SWAP` appears in this codebase
exactly once, at `local_file.rs:2366`, as the **adversary's** substitution primitive in a red
test that the capture protocol defeats.

**"One more identity recheck immediately before the syscall" — rejected.** This is the
candidate's approach and the reason the round parked it. It narrows the window; it does not
close it, because the pair still satisfies none of I0's A/B/C.

**"Do everything under a global lock and drop the quarantine" — rejected.** The lock closes
the *concurrency* half but not the *crash* half. A crash between capture and republish leaves
a state that only a reserved-namespace protocol can recover. Both are needed.

**"Return `Unsupported` when birthtime is missing" — rejected as the default** (see O2), but
retained as the owner-selectable `Required` policy.

**"`Drop`-based cleanup of staged records" — rejected.** Name-addressed, and does not run
after a crash (§3.5).

**"Restart Task A from scratch" — rejected, and no mechanism proves it unsalvageable.** The
mechanism layer is sound; the Round-1 repairs are correct and carry forward; the lock
acquisition sequence, the route check's live third of `revalidate`, the classifier, and the
publication path are all reused verbatim. What is replaced is four mutators and one type,
each with a named reason in §2.

---

## 8. Residual risks

1. **Advisory locking on network filesystems.** `flock` over NFS is emulated and unreliable;
   the cooperating model degrades to nothing. Mitigation: `fstatfs` filesystem-class probe at
   open → `Unsupported{FilesystemLockUntrusted}`. Not free; needs a per-OS `cfg` arm.
2. **`fsync` durability is unverifiable in-process.** The existing HONEST LIMIT (`:2049`)
   stands unchanged. `Committed` attests the typed plumbing and the namespace invariants, not
   that bytes reached stable media.
3. **Same-uid compromise is inside the trust boundary** under every model in O1.
4. **Non-Linux/macOS unix gets `Unsupported` everywhere**, with no semantic fallback — by
   design, and consistent with the module today. Note that `libc` is an unconditional
   dependency in `crates/bridge-core/Cargo.toml`, so the `cfg(not(unix))` arms are aspirational
   and unexercised by CI.
5. **Reserved-namespace collision with a sweep pattern.** The names are dotted, matching the
   existing `.custody-locks` and `.a2a-delete-v1-*` conventions, but every sweeper's selection
   pattern must be re-read before A3 lands — `sweep.rs` and `is_journal_append_residue_name`
   (`local_file.rs:39`) are the two to check.
6. **The `verify_then_remove` PARKED gap is untouched** and stays parked. A3's capture
   primitive is the eventual building block for it; deliberately out of scope.
7. **Recovery runs under the lock at every acquisition**, adding one bounded enumeration per
   acquisition. With the existing enumeration bound this is O(limit), but it is a per-operation
   cost that did not exist before.
