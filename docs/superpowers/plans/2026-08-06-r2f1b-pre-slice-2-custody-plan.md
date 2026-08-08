# R2f1b pre-slice-2 gates and workspace-custody plan

**Status:** revision 2, draft for owner re-review; documentation only. This document does not authorize
implementation, remote pushes, future cleanup, automatic deadlines, provider turns, release, deployment, or
served-operator mutation. The measured cleanup in §6.1 was separately authorized and is recorded as evidence, not
as continuing deletion authority.

## 1. Purpose

Close the remaining inactive-foundation gates without allowing useful agent workspaces to accumulate until the
host runs out of space. The design must preserve useful work durably while treating local source materialization,
build outputs, caches, and evidence as separate payloads. A local source checkout is retained only while a live or
imminent consumer justifies it within the storage budget; a retained checkout does not imply retention of its
regenerable build tree.

Two custody planes must remain distinct:

1. **R2f1b runtime worktree custody** protects workflow-created worktrees from cancellation, deadline, run-end,
   boot-sweep, and recovery races. Its durable record belongs in the V3 workflow snapshot and custody sidecar.
2. **Implement-run workspace custody** governs quarantine clones or linked worktrees used by
   `a2a-bridge implement`, immediate review, bounded repair, integration, and cleanup. Its durable source of code
   should normally be an exact remote Git ref plus a local receipt; it must not be smuggled into the runtime V3
   contract.

The planes may share descriptor-safe filesystem primitives, identities, and cleanup vocabulary. They do not share
deletion authority or persistence records.

## 2. Gate definitions

### 2.1 Runtime custody-test gate

**Safety property:** every materialized checkout is protected by durable, identity-bound evidence before a timer,
provider, process, cancellation, or destructive sweep can act.

The gate is green only when fail-first tests prove all of the following:

- the prepared custody intent is file-synced, atomically published, and parent-directory-synced before effects;
- parent-sync failure yields a typed ambiguous/unknown outcome and zero provider, process, timer, or sweep calls;
- prepared and preserved worktrees survive normal run-end and dead-run boot sweeps;
- corrupt, missing, mismatched, symlinked, hard-linked, or otherwise ambiguous evidence never authorizes deletion;
- failure after a target directory materializes preserves that target;
- an unused candidate is settled only after proving that its exact target never materialized;
- resume mints a successor attempt and atomically exchanges the exact preserved claim before provider effects; and
- global outcome owns deletion: node-local success cannot remove a checkout that a later sibling failure requires.

**Wrong result prevented:** a timeout, cancellation, or crash lands between checkout creation and durable custody
publication, and recovery deletes useful work.

### 2.2 Preparation-flight test gate

A preparation flight gives finitely bounded ownership to pre-effect work such as custody publication:

```text
Open -> BarrierSynced
     -> Transferred { recovery reason }
     -> Failed { typed cause }
```

**Safety property:** cancellation of the initiating waiter cannot abandon the only custody-preparation owner.

The gate is green only when a nonreturning or canceled sync operation transfers an exact guard to a recovery owner
within the approved bound, publishes a typed result, and makes zero provider/session/process/materialization or
destructive-sweep calls before the barrier is durable.

### 2.3 Resource-flight test gate

A resource flight owns one action against one exact physical process/container generation:

```text
Open -> AdmissionClosed -> IntentJournaled -> Signaling -> Settled(result)
```

**Safety property:** every actor requesting cleanup, takeover, release escalation, or retirement joins one retained
capability rather than reconstructing authority from a PID, PGID, name, or stale database row.

The gate is green only when tests prove:

- two nodes sharing one generation cause one signal/action and receive one shared result;
- cancellation of one waiter does not cancel or duplicate the resource action;
- cleanup-deadline transfer preserves the exact guard before the initiating future can finish;
- missing or ambiguous capability returns refusal, `Partial`, or `Unknown` rather than guessed success;
- collateral owners are complete and serialized with admission closure; and
- reused numeric identities, unrelated processes, and other generations survive.

The similarly named legacy worktree cleanup flight is not sufficient evidence for this V3 resource-flight gate.

### 2.4 V3 snapshot-test gate

“Snapshot” here means the durable `WorkflowSnapshotV3`, not the 30-minute no-progress evidence snapshot.

**Safety property:** the durable attempt envelope commits every candidate requiring custody and cannot change
delivery, custody, activation, resource, or lineage semantics during resume.

The gate is green only when tests prove:

- every checkout candidate in the frozen node/preflight/fallback matrix has exactly one custody plan;
- custody plans are canonical, sorted, and unique by checkout fingerprint and custody id;
- adding, dropping, duplicating, or mutating a plan invalidates the contract;
- a successor retains exact delivery bytes and the exact R2f1b contract while using a new attempt id;
- predecessor digest, attempt ordinal, parent attempt, execution id, and origin delivery remain coherent;
- V2 remains manual-only and cannot be reinterpreted as automatic V3 evidence; and
- old readers ignore, and therefore never delete from, the V3 custody-sidecar namespace.

**Wrong result prevented:** a snapshot protects only a subset of possible checkouts or resume silently changes the
authority under which cleanup and deadlines run.

### 2.5 `fs_custody` versus `local_file` ownership gate

This is an implementation-ownership decision, not a choice between two custody policies.

- `bin/a2a-bridge/src/local_file.rs` contains the mature binary-private implementation: pinned directory identity,
  descriptor-relative access, durable publication, bounded readers, quarantine, replacement, and compatibility
  evidence behavior.
- `crates/bridge-core/src/fs_custody.rs` contains a new reusable subset for directory/file identity, sync barriers,
  and atomic no-replace publication.

The current source duplicates important logic. The written design expects the generic primitives to have one
library owner and binary-specific bounded-reader/quarantine policy to remain in `local_file`.

The gate is green only when:

- generic descriptor identity, no-follow access, sync, no-replace publication, and ambiguous-parent-sync handling
  have one authoritative implementation;
- `local_file` uses that implementation through narrow wrappers, or a deliberately chosen library boundary proves
  equivalent behavior without copy-paste;
- existing compatibility/fallback callers do not lose their bounded-reader and quarantine invariants; and
- Linux/macOS, symlink/hard-link, target-race, rename, file-sync, parent-sync, and injected-failure tests pass.

### 2.6 Workload-fingerprint gate

**Safety property:** manual and automatically timed executions cannot share one workload/calibration identity.

The existing run-spec workload fingerprint commits graph, controls, retries, and frozen node/provider identities,
but the V3 R2f1b contract is wrapped outside that older delivery-spec fingerprint. Before
`AutomaticR2f1b` becomes constructible, the versioned workload identity must also commit:

```text
existing frozen workload identity
+ explicit DeadlineActivationV2
+ validated FrozenR2f1bContractV1.contract_fingerprint
```

The gate is green only when tests prove:

- manual and automatic activation produce different fingerprints;
- any custody-plan or resource-contract change changes or invalidates the fingerprint;
- a separately valid replacement contract cannot retain the old workload identity;
- decode and successor resume recompute the same bound identity; and
- historical V2/manual behavior remains manual and does not inherit automatic semantics.

The explicit activation field is retained even though it also contributes to the contract fingerprint: the semantic
boundary remains auditable and domain-separated from the contract-hash algorithm.

**Wrong result prevented:** automatically timed attempts are pooled with manual observations, corrupting liveness
baselines, history grouping, or later compatibility claims.

## 3. Proposed implement-run workspace lifecycle

Phase, durability, local materialization, and payload class are orthogonal. Composite names such as
`ParkedRemoteDurable` hide the distinction between what happened to the work and where its bytes are safe.

| Coordinate | Values | Meaning |
|---|---|---|
| **Phase** | `ActiveImplementation`, `AwaitingImmediateReview`, `RepairAuthorized`, `ApprovedPendingFold`, `Folded`, `Parked`, `ProtectedUnknown` | What may consume or mutate the work next. |
| **Durability** | `LocalOnly`, `RemoteVerified`, `BundleVerified`, `FoldReceiptVerified`, `Ambiguous` | Which exact committed/source identity is independently recoverable. |
| **Local materialization** | `ActiveRequired`, `HotLeased`, `HotPreferred`, `Evictable`, `Absent` | Whether this physical checkout must remain on this host. |
| **Payload class** | `SourceCheckout`, `BuildTarget`, `DependencyCache`, `Evidence`, `CredentialOrSecret`, `ContainerOrImage` | Which custody and cleanup rules apply to the bytes. |

`Parked + RemoteVerified + Evictable` is the ordinary remote-durable parked state. `Parked + BundleVerified +
Evictable` is the no-writable-remote equivalent. `Parked + LocalOnly` is protected local debt: it is not evictable,
consumes a finite protected quota, and can block admission of new work. `ProtectedUnknown` is reserved for genuinely
ambiguous custody rather than serving as the only non-remote state.

| Phase | Required durability after a stable commit | Local source policy | Build-output policy |
|---|---|---|---|
| `ActiveImplementation` | Checkpoint when safe; uncommitted bytes remain local-only | `ActiveRequired` while the writer or owned child is live | Retain only while the active build owns it |
| `AwaitingImmediateReview` | `RemoteVerified` or `BundleVerified` | `HotPreferred`; retain for immediate review when budget permits, otherwise reconstruct | Evict after verification unless an exact live reuse lease reserves it |
| `RepairAuthorized` | Refresh exact durable identity after every bounded commit | `HotLeased` only while repair is scheduled or live; otherwise evictable | Evict between inactive repair windows under pressure |
| `ApprovedPendingFold` | Reviewed commit/tree and target base remotely or bundle reachable | `HotPreferred` only when fold is imminent; otherwise evictable | Evict after the accepting verification |
| `Folded` | `FoldReceiptVerified` | Remove promptly | Remove promptly |
| `Parked` | Prefer `RemoteVerified`; accept `BundleVerified`; `LocalOnly` blocks eviction | Zero grace under pressure; optional short grace only above the inventory watermark | Remove promptly after useful evidence is separated |
| `ProtectedUnknown` | `Ambiguous` | Required until operator resolution | Remove only an independently classified regenerable payload |

Age and disk pressure trigger classification and priority. They never manufacture durability or deletion authority.
Local reuse is an optimization, not a custody invariant.

### 3.1 Remote-first feature and slice model

Recommended default:

1. Keep the long-lived feature branch remotely; do not reserve a standing local worktree for it.
2. Create a unique local quarantine checkout and unique slice branch from the exact remote feature tip.
3. Commit useful implementor work before handoff. Never push credentials, ignored secrets, or arbitrary untracked
   bytes merely to make cleanup possible.
4. Push the exact slice commit through an operator-owned, lease-bound path. Agents do not receive generic remote
   credentials or independent push authority.
5. Prefer the same clean source checkout for immediate hard-read-only review and one authorized bounded repair when
   it remains within the hot-storage budget. Delete its build outputs after verification. If pressure requires source
   eviction after durable restoration proof, reconstruct the exact checkout for the reviewer or repairer.
6. Fold the approved delta onto the remote feature branch with an exact lease and record source commit/tree,
   target-before, integration commit/tree, and cumulative diff digest.
7. Fetch/verify the remote integration result from a clean object view. Once the reviewed delta is durably reachable,
   remove build payloads immediately and remove the source checkout when no live lease requires it.
8. Start the next slice from the new remote feature tip in a new checkout.
9. After aggregate review, main landing, and post-merge verification, retire feature and custody refs according to
   the accepted retention policy.

This preserves immediate-review locality when affordable without treating local bytes as boundless. A reviewer can
reuse the exact source checkout without retaining its much larger target tree.

### 3.2 Original reviewed commit versus folded commit

Re-authoring, cherry-picking, tree composition, or squash merging can make the original reviewed commit unreachable
even when its change is present in the feature branch. Before deleting the only clone containing that commit, choose
one of these explicit policies:

- retain a remote per-slice custody ref through aggregate closure;
- perform an atomic multi-ref push of the custody ref and integration ref where supported; or
- prove that the integration receipt plus remotely reachable parent/tree/delta is the accepted durable identity and
  that no later evidence requires the original commit object.

Deleting a topic branch immediately after a squash merge is unsafe when retained evidence still names its commit.

## 4. Cleanup admission contract

A cleanup candidate is one exact payload, not an entire run directory inferred from a prefix. A cleaner may remove
an exact source checkout only when every item below is true at deletion time:

1. The path canonicalizes beneath the configured bridge-owned checkout root and matches the recorded object/path
   identity. A source repo, user checkout, workspace root, symlink target, or broad prefix is never a target.
2. The run is terminal and the per-run operation lock is held by the cleaner. The live run lease is free.
3. No implementor, reviewer, repair, verifier, merge, or resume phase is active or scheduled to consume the checkout.
4. No owned child process, container mount, current working directory, or open file refers to it. This is rechecked
   immediately before removal.
5. Git status, including untracked files and submodules, matches the retained checkpoint. Non-disposable ignored or
   untracked bytes either receive explicit custody or block cleanup.
6. Every useful tracked change is committed. The commit, tree, parent/base, diff digest, remote URL, remote ref, and
   expected remote object id are recorded.
7. A live remote query and clean fetch prove exact remote reachability. A local tracking ref or successful prior push
   exit status is not sufficient.
8. If the original reviewed commit will become unreachable after fold/squash, its accepted custody policy has been
   satisfied.
9. The removal mechanism matches the checkout kind: guarded standalone-clone removal or `git worktree remove` plus
   exact registration pruning. A broad `git worktree prune` is not a substitute for checkout classification.
10. Removal is verified, reclaimed space is measured separately from logical size, and failure leaves a durable
    partial/unknown cleanup record rather than claiming success.

### 4.1 Payload-specific cleanup gates

The source-checkout rules above must not be copied blindly onto other payload classes:

- **Build target:** may be removed independently of its retained source checkout only after its path/type is exact,
  it is proven to contain regenerable compiler output rather than sole evidence, no live process/open file/container
  owns it, and its source/lock/toolchain inputs remain recoverable. `CACHEDIR.TAG` is strong evidence but not a
  substitute for path and content classification.
- **Dependency cache:** may be removed only when it contains no credentials or unique offline package and no live
  worker has reserved it. Current shared repo/toolchain caches are preferred over duplicated per-run caches.
- **Evidence:** verifier logs, manifests, receipts, fingerprints, checkpoints, and accepted review artifacts are not
  build cache. They require their own retention/durability decision before removal.
- **Credential or secret:** never push or bundle as a cleanup shortcut. Retain or quarantine under its owning secret
  policy.
- **Container or image:** require runtime-visible consumer checks and separate current/rollback-image policy. A
  zero-link volume or unused image is only a candidate; ambiguous hash-named volumes remain parked.
- **Source checkout:** requires the complete ten-item contract above, including exact remote or bundle restoration
  proof for every useful tracked change.

The cleaner rechecks live processes, open files, operation locks, container consumers, and exact target identity at
the destructive boundary. A failed or nondiscriminating probe is inadmissible and parks that payload.

## 5. Remote persistence is not complete workspace persistence

Remote Git normally preserves committed tracked content. It does not automatically preserve:

- uncommitted or untracked work;
- ignored evidence, logs, databases, generated artifacts, or build outputs;
- credentials and secret material, which must not be pushed;
- Git LFS objects that were not uploaded;
- unavailable submodule commits;
- local toolchains, containers, caches, or filesystem metadata;
- a commit whose only remote ref is later deleted after squash/fold; or
- a repository with no writable remote, an offline remote, or insufficient operator authorization.

When exact remote durability is unavailable, the fail-safe choices are an owner-private Git bundle plus bounded
non-Git evidence inventory, or continued local retention. “Push failed” never becomes permission to clean locally.

Remote WIP/custody refs may also trigger CI, notifications, branch rules, storage billing, or external automation.
The chosen namespace must be explicitly excluded from unintended workflows or those effects must be accepted.

## 6. Storage policy

The system operates on a shared, storage-constrained host with multiple repositories and multiple workers per
repository. Retention is quota- and reservation-driven; it does not assume that local storage grows without bound.

### 6.1 Measured 2026-08-06 baseline and cleanup evidence

This is point-in-time evidence, not a permanent capacity guarantee or future cleanup authorization.

| Measurement | Before | After the separately authorized cleanup |
|---|---:|---:|
| Data-volume free space | 328.93 GiB immediately before deletion | 371.00 GiB |
| Physical free-space gain | — | 42.07 GiB |
| Current repository | 5.05 GiB, including a 4.73 GiB target | 0.318 GiB |
| `.a2a-implement` | 21.45 GiB across 113 directories | 16.37 GiB across the same 113 directories |
| R2f1b completion scratch | 20.24 GiB | 0.247 GiB |
| Old merge scratch | 5.96 GiB | effectively empty |
| OrbStack physical allocation | about 258.04 GiB | about 251.91 GiB |

The cleanup removed 36.41 GiB of exact host Cargo targets and 61.21 GB of logical zero-consumer Docker volumes.
Sparse OrbStack/APFS accounting meant that the logical Docker deletion produced only about 6.1 GiB of immediate
host recovery. Source checkouts, Git objects, logs, receipts, images, current caches, the served operator, and all
stockTrading/quant-platform resources were preserved.

Eight source-only linked worktrees measured only 18–24 MiB each. A retained full-suite target measured about 20 GiB.
Therefore the primary capacity unit is an isolated build payload, not a Git worktree registration.

### 6.2 Proposed watermarks, reservations, and quotas

These values are recommended defaults for owner re-review:

| Control | Proposed value | Behavior |
|---|---:|---|
| Inventory/reclaim trigger | 300 GiB free | Classify and reap exact remotely/bundle-durable idle payloads; do not wait for a weekly manual crisis. |
| New full-build admission floor | 250 GiB projected free | Refuse admission unless the declared reservation leaves at least this much free. |
| Critical floor | 200 GiB free | Start no new write-capable worker; checkpoint durable idle work and reclaim verified disposable payloads. |
| Cleanup target | 350 GiB free | Continue bounded classified cleanup until this target or until no authorized candidate remains. |
| Full-build reservation | 25 GiB | Reserve before one isolated full test/Clippy/build payload. |
| Full-slice reservation | 50 GiB | Reserve when implementation and aggregate-verification targets may coexist. |
| Per-repository concurrency | 2 full-build workers | Additional source-only readers do not consume a build reservation. |
| Global concurrency | 3 full-build workers | Shared across all repositories on the host. |
| Per-repository hot-storage soft cap | 50 GiB | Pressure-evict durable idle source/build payloads before admitting more. |
| Global A2A hot-storage soft cap | 100 GiB | Includes source materializations, build targets, and per-run caches; excludes explicitly protected operator/data services. |
| Local source checkout cap | 6–8 per repository | Operational/custody cap rather than a disk-capacity claim. |

Admission uses projected free space after reservations, not the last observed `df` value. A local-only protected
result consumes quota; when quota is exhausted, new work stops rather than deleting the sole copy or pretending
that local retention is unlimited.

### 6.3 Reclaim order and hot retention

1. Exact completed per-run `BuildTarget` payloads, even when their source checkout remains hot for review.
2. Duplicated inactive dependency caches after preserving the current shared repo/toolchain cache.
3. `Folded + FoldReceiptVerified + Evictable` source checkouts, with zero grace.
4. `Parked + RemoteVerified/BundleVerified + Evictable` source checkouts, with zero grace under pressure.
5. Separately classified zero-consumer containers/volumes and non-current, non-rollback images.

Immediate-review and repair locality are best-effort cache behavior. `HotPreferred` becomes `Evictable` when a
watermark or quota requires it; exact reconstruction replaces indefinite retention. Never select a user-owned or
protected path by prefix, age, or size alone.

### 6.4 Build-footprint policy

- Keep source checkouts small. Put regenerable Cargo/build/LSP caches in identified shared roots where correctness
  permits, mark disposable roots with `CACHEDIR.TAG`, and bind reuse to repo/toolchain/config identities.
- Set `CARGO_INCREMENTAL=0` for one-shot CI, full-suite, and aggregate-verifier runs. A preserved 15.77 GiB verifier
  target contained 6.88 GiB of incremental artifacts (about 44%). Interactive development may retain incremental
  compilation when it has an explicit hot-cache budget.
- Delete completed one-shot build targets after their gate outputs have been reduced to retained evidence. Do not
  retain a 20 GiB target merely to reuse a 20 MiB checkout for review.
- Prefer a shared current dependency registry/cache over a new 620 MiB per-run copy, while preserving isolation and
  toolchain/config compatibility.
- Do not confuse clone/worktree cleanup with Docker cleanup. Containers and images retain separate live-owner,
  project-data, current-image, and rollback-image gates.

### 6.5 Release-binary size policy

The current workspace defines no custom `[profile.release]`; normal release builds therefore use Cargo defaults:
`opt-level=3`, `strip="none"`, `lto=false`, `panic="unwind"`, `incremental=false`, and 16 codegen units. The installed
operator measured 30.46 MiB and retained 67,006 symbols, including about 5.23 MiB of symbol/string-table data.

Binary-size tuning does not solve multi-gigabyte target retention. A separately reviewed distribution experiment
may compare this custom profile with the unchanged release baseline:

```toml
[profile.dist]
inherits = "release"
lto = "thin"
strip = "symbols"
```

Retain an unstripped diagnostic artifact if `strip="symbols"` is adopted. Do not default to `opt-level="z"` without
measurement; Cargo does not guarantee that `s`/`z` are smaller. Do not use `codegen-units=1`/fat LTO as a storage
fix because they trade substantial build time/parallelism for final-binary optimization. Reject `panic="abort"`:
production observation, session, and reaper paths deliberately catch panics. Reject `no_std`: the bridge requires
filesystem, process, network, Tokio/Axum, SQLite, and OS services. Do not add UPX to the default release path; a
possible tens-of-megabytes distribution saving is immaterial to build storage and adds another platform/release
transformation. None of these release-profile experiments is authorized by this plan revision.

## 7. Required fail-first cases for workspace custody

At minimum, the eventual implementation must prove:

1. phase, durability, local materialization, and payload class change independently without fabricating authority;
2. implement completion followed by immediate review reuses the same source checkout when a hot lease and budget
   permit, while deleting its build target does not delete source or evidence;
3. a remote/bundle-verified review checkout can be pressure-evicted and reconstructed at the exact commit/tree;
4. a closed-enumerable review rejection retains or reconstructs the exact checkout for the authorized repair;
5. approved fold plus exact remote verification reaps only that slice's source and build payloads;
6. the remote feature branch remains usable without a standing local worktree;
7. push failure, stale lease, missing remote ref, fetch mismatch, or failed bundle restoration keeps source
   `LocalOnly` and blocks its cleanup;
8. dirty, untracked, ignored, submodule-dirty, or LFS-incomplete state blocks source cleanup unless explicitly
   dispositioned, but does not prevent independent removal of a proven regenerable target;
9. active lease, operation lock contention, process cwd/open file, or container mount blocks the exact payload;
10. squash/fold cannot strand the sole copy of the reviewed commit;
11. `Parked + RemoteVerified` and `Parked + BundleVerified` reconstruct exactly, while `Parked + LocalOnly` consumes
    quota and cannot be pressure-deleted;
12. concurrent review/repair and cleanup serialize so only one owns each transition;
13. a predicted build reservation crossing the 250 GiB admission floor refuses before materialization;
14. the 300/250/200/350 GiB watermark transitions prioritize candidates without bypassing custody gates;
15. completed one-shot verification with `CARGO_INCREMENTAL=0` retains test evidence but not incremental payloads;
16. logical Docker/cache deletion and physical free-space gain are recorded separately;
17. partial removal is recorded truthfully and is recoverable; and
18. disk-pressure inventory never selects user-owned, served-operator, stockTrading/quant-platform, current-cache,
    or rollback resources by prefix/age alone.

## 8. Owner inputs

The technical safety properties above are not optional. The following policy choices require owner direction.
Recommended defaults are stated so planning can continue without inventing authority.

### 8.1 Input needed for each remaining gate

| Gate | Input needed from the owner | Recommended answer |
|---|---|---|
| **Runtime custody tests** | Confirm that automatic deletion is allowed only for an exact globally healthy outcome, never from node-local success, elapsed age, remote presence alone, or ambiguous custody. | Yes; preserve or return `Unknown` everywhere else. |
| **Preparation flight** | No new duration is needed if the already approved D11 pre-effect/control bounds remain authoritative. Confirm that timeout transfers ownership instead of canceling the underlying preparation. | Retain D11 and transfer the exact guard to recovery. |
| **Resource flight** | Confirm that cleanup/takeover/release/retirement must join one capability-bound generation flight even when waiting is slower than returning an optimistic success. | Prefer truthful `Partial`/`Unknown` or retained ownership over guessed success. |
| **V3 snapshot** | Confirm that the snapshot enumerates runtime checkout candidates only; remote feature/custody refs belong in implement-run receipts, not `WorkflowSnapshotV3`. | Keep the two custody planes separate. |
| **Filesystem implementation owner** | Select one reusable owner for generic descriptor/sync/publication primitives. | Finish `fs_custody`; retain bounded-reader/quarantine policy in `local_file`. |
| **Workload fingerprint** | Approve a new automatic-R2f1b fingerprint domain while preserving historical V2/manual fingerprints. | Yes; there is no automatic production history whose old identity needs preservation. |
| **Implement-run cleanup** | Approve the orthogonal phase/durability/materialization/payload model plus remote, bundle, quota, retention, and protected-resource policy. | Adopt §§3–6, including pressure eviction of durable idle source and immediate independent build-target cleanup. |

| Decision | Recommended default | Why owner input is needed |
|---|---|---|
| **Remote push authority** | Per-repository opt-in authorizes an operator-owned bridge path—not an agent—to create/update exact lease-bound feature and custody refs. | A push is an external effect and some target repos may be private, forked, local-only, or read-only. |
| **Feature-branch model** | One remote feature branch, no standing local worktree; one unique local checkout/branch per slice. | Confirms the lifecycle proposed by the owner. |
| **Immediate-review reuse** | Prefer the exact clean source checkout through review and one bounded repair only while a hot lease fits quota; delete build outputs and reconstruct under pressure. | Reuse saves setup time, but local retention is a cache policy rather than a custody invariant. |
| **Custody-ref namespace and CI effects** | Use an explicit `a2a/custody/<run-id>` or equivalent namespace excluded from ordinary push CI; never guess that it is effect-free. | Naming, visibility, branch protection, and CI rules are repository policy. |
| **Original reviewed-commit retention** | Keep the per-slice custody ref until aggregate feature review and main landing are green; then retain only the integration receipt unless an audit requires longer. | Squash/fold can otherwise make the reviewed commit unreachable. |
| **Parked-work hold** | Zero local grace under pressure after remote/bundle restoration proof; optional short grace only above 300 GiB free. | Balances repair latency against real storage pressure without making retention indefinite. |
| **Untracked/ignored material** | Refuse cleanup unless every item is classified as disposable cache or preserved through an approved private artifact path. Never auto-push it. | Git remote durability does not cover these bytes and they may contain secrets. |
| **No-writable-remote fallback** | Create an owner-private Git bundle plus bounded evidence manifest; if that fails, retain locally. | Cross-repo workflows cannot assume a writable `origin`. |
| **Free-space watermarks** | Approve proposed 300 GiB inventory, 250 GiB admission, 200 GiB critical, and 350 GiB cleanup-target values. | Machine capacity and concurrent workloads are operator policy; measured baselines can drift. |
| **Reservations and quotas** | Approve 25 GiB/full build, 50 GiB/full slice, two full-build workers per repo, three globally, 50 GiB/repo hot, and 100 GiB global A2A hot storage. | Admission and fairness across repositories require owner-selected finite budgets. |
| **One-shot build policy** | Set `CARGO_INCREMENTAL=0` for CI/full-suite/aggregate verification and evict completed targets after evidence reduction. | A measured verifier cache devoted about 44% of its target to incremental artifacts. |
| **Distribution profile experiment** | Permit a separate non-blocking comparison of the default release against `lto="thin"` plus `strip="symbols"`; keep unwind panics and an unstripped diagnostic artifact. | This changes release/debug characteristics and must not be confused with the high-value target-retention fix. |
| **Protected roots/resources** | Explicitly preserve the user checkout, served operator/release, active bridge stores, and owner-named repositories/containers. | Broad cleanup inference is unsafe. |
| **Fingerprint compatibility** | Accept a new version/domain for automatic R2f1b; retain historical V2/manual fingerprints unchanged. | Confirms that no nonexistent automatic-history compatibility is being preserved. |
| **Filesystem ownership** | Finish the shared `fs_custody` extraction and make `local_file` a higher-level caller rather than retaining duplicated primitives. | The owner may reject this only by selecting another single library owner; duplication is not an acceptable outcome. |

## 9. Proposed implementation sequence

Keep runtime custody and operator workspace cleanup as separate reviewed artifacts even if they share primitives.

### Track A — R2f1b inactive-foundation closure

1. Reconcile the roadmap to the merged PR #50 identity and freeze exact current main.
2. Enumerate every missing section-6 runtime custody/flight/snapshot test against current source; zero-selection or
   setup failures are inadmissible.
3. Land fail-first contract tests without activating deadlines.
4. Finish the single-owner `fs_custody` extraction with parity/fault tests.
5. Bind activation plus contract fingerprint into a versioned workload fingerprint; retain V2/manual behavior.
6. Run focused package gates, full workspace acceptance, platform custody lanes, hygiene, and one capped cumulative
   review. `AutomaticR2f1b` remains unconstructible.

### Track B — implement-run remote custody and local cleanup

1. Freeze ADR-0026/0027/0040 checkpoint, merge, operation-lock, and clone-reap behavior on exact current main.
2. Add a read-only inventory/dry-run classifier that emits phase, durability, local materialization, payload class,
   measured bytes, reservation owner, and every blocking gate for each exact bridge-owned item. It performs no push
   or deletion.
3. Add versioned remote-custody and fold receipts, exact lease-bound operator push, live remote verification, and
   no-writable-remote fallback. No agent receives push authority.
4. Add explicit hot leases for immediate review and bounded repair, with pressure eviction and exact reconstruction
   after `RemoteVerified`/`BundleVerified` proof.
5. Add independent exact guarded cleanup for completed build targets and dependency caches without deleting retained
   source/evidence; then add guarded source cleanup for `Folded + FoldReceiptVerified` and accepted
   `Parked + RemoteVerified/BundleVerified` states.
6. Add reservation admission, per-repository/global quotas, and the 300/250/200/350 GiB transitions without allowing
   storage pressure to bypass custody, process, Git, evidence, protected-project, current-cache, or rollback gates.
7. Set `CARGO_INCREMENTAL=0` for one-shot verifier execution, retain raw gate evidence, and remove completed targets
   according to the exact payload receipt.
8. Verify remote/bundle restoration, pressure-evicted review/repair reconstruction, logical-versus-physical reclaim
   reporting, concurrency with review/resume/merge, full suite, hygiene, and one capped adversarial review.

Track B may precede later R2f1b production slices to reduce operator storage pain, but it does not prove runtime
deadline custody and cannot green Track A by analogy.

The optional distribution-profile comparison in §6.5 is a separately scoped follow-on. It is not a Track B blocker
and must not delay the much larger build-target and incremental-artifact controls.

## 10. Planning exit

Planning is ready for bounded implementation briefs only when the owner has answered the decisions in §8, including
the proposed watermarks/reservations/quotas, the roadmap is reconciled to merged `main`, exact current source has been
re-audited for missing tests/callers, and Track A and Track B have separate path ownership, review caps, and
acceptance totals.
