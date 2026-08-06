# R2f1b pre-slice-2 gates and workspace-custody plan

**Status:** draft for owner decisions; documentation only. This document does not authorize implementation,
remote pushes, cleanup, automatic deadlines, provider turns, release, deployment, or served-operator mutation.

## 1. Purpose

Close the remaining inactive-foundation gates without allowing useful agent workspaces to accumulate until the
host runs out of space. The design must preserve useful work durably while retaining a local checkout only when
someone is actively implementing, reviewing, or repairing it.

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

The lifecycle is state-driven. Age and disk pressure may trigger inspection, but neither authorizes deletion.

| State | Local checkout | Remote durability | Permitted transition |
|---|---|---|---|
| `ActiveImplementation` | Required | Optional checkpoint ref | Keep while the write-capable run or owned verification child is live. |
| `AwaitingImmediateReview` | Retain | Exact committed head should be pushed by an operator-owned path | Reuse the same clean checkout under hard read-only review. |
| `RepairAuthorized` | Retain | Update the same slice/custody ref after each bounded commit | Reuse context; do not recreate merely because review rejected a closed finding. |
| `ApprovedPendingFold` | Retain | Reviewed commit/tree and target base must be remotely reachable | Fold with an exact target lease; do not delete before the fold receipt is verified. |
| `FoldedRemoteDurable` | Remove promptly | Feature/integration ref contains the exact accepted delta and remote fetch verifies it | Reap exact clone/worktree, then prune only its dead registration. |
| `ParkedRemoteDurable` | Normally remove after policy hold | Exact parked commit plus receipt is fetchable; no immediate repair/review is scheduled | Recover later into a new slice checkout without keeping the old directory. |
| `ProtectedUnknown` | Required | Missing, unverified, dirty, unpushed, or ambiguous | No automatic cleanup. Operator resolves custody. |

### 3.1 Remote-first feature and slice model

Recommended default:

1. Keep the long-lived feature branch remotely; do not reserve a standing local worktree for it.
2. Create a unique local quarantine checkout and unique slice branch from the exact remote feature tip.
3. Commit useful implementor work before handoff. Never push credentials, ignored secrets, or arbitrary untracked
   bytes merely to make cleanup possible.
4. Push the exact slice commit through an operator-owned, lease-bound path. Agents do not receive generic remote
   credentials or independent push authority.
5. Keep the local checkout through immediate read-only review and any one authorized bounded repair.
6. Fold the approved delta onto the remote feature branch with an exact lease and record source commit/tree,
   target-before, integration commit/tree, and cumulative diff digest.
7. Fetch/verify the remote integration result from a clean object view. Once the reviewed delta is durably reachable
   and no review/repair consumer remains, remove the local slice checkout.
8. Start the next slice from the new remote feature tip in a new checkout.
9. After aggregate review, main landing, and post-merge verification, retire feature and custody refs according to
   the accepted retention policy.

This preserves immediate-review locality without paying permanent local-storage cost for approved or parked work.

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

A cleaner may remove an exact checkout only when every item below is true at deletion time:

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

- Keep source checkouts small by placing regenerable Cargo/build/LSP caches in identified shared cache roots where
  correctness permits. Mark disposable cache roots with `CACHEDIR.TAG` and bind reuse to repo/toolchain/config
  identities.
- Do not confuse clone/worktree cleanup with Docker cleanup. Containers and images have separate live-owner and
  rollback-image gates.
- Use high/low free-space watermarks to trigger inventory and prioritized cleanup. Watermarks never override
  custody, process, Git, or protected-path gates.
- Reclaim in this order: verified disposable per-run build outputs; `FoldedRemoteDurable` checkouts; expired
  `ParkedRemoteDurable` checkouts; separately classified caches/images. Never choose by path age alone.
- Preserve the served operator, explicitly protected repositories/resources, live review/implementation work, and
  any `ProtectedUnknown` item.

## 7. Required fail-first cases for workspace custody

At minimum, the eventual implementation must prove:

1. implement completion followed by immediate review reuses the same checkout without recreation;
2. a closed-enumerable review rejection retains the checkout for the authorized repair;
3. approved fold plus exact remote verification reaps only that slice checkout;
4. the remote feature branch remains usable without a standing local worktree;
5. push failure, stale lease, missing remote ref, or fetch mismatch retains the checkout;
6. dirty, untracked, ignored, submodule-dirty, or LFS-incomplete state blocks cleanup unless explicitly dispositioned;
7. active lease, operation lock contention, process cwd/open file, or container mount blocks cleanup;
8. squash/fold cannot strand the sole copy of the reviewed commit;
9. a parked remote-durable slice can be reconstructed in a fresh checkout with the recorded tree and diff;
10. concurrent review and cleanup serialize so only one owns the transition;
11. partial removal is recorded truthfully and is recoverable; and
12. disk-pressure inventory never selects user-owned or protected paths by prefix/age alone.

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
| **Implement-run cleanup** | Supply the remote, ref, retention, untracked-material, protected-root, and disk-watermark policies below. | Adopt the remote-first lifecycle in §3 and the fail-safe cleanup contract in §4. |

| Decision | Recommended default | Why owner input is needed |
|---|---|---|
| **Remote push authority** | Per-repository opt-in authorizes an operator-owned bridge path—not an agent—to create/update exact lease-bound feature and custody refs. | A push is an external effect and some target repos may be private, forked, local-only, or read-only. |
| **Feature-branch model** | One remote feature branch, no standing local worktree; one unique local checkout/branch per slice. | Confirms the lifecycle proposed by the owner. |
| **Immediate-review reuse** | Retain the exact clean checkout through review and one authorized bounded repair; mount or enforce hard read-only during review. | Reuse saves setup time but requires a clear mutation boundary. |
| **Custody-ref namespace and CI effects** | Use an explicit `a2a/custody/<run-id>` or equivalent namespace excluded from ordinary push CI; never guess that it is effect-free. | Naming, visibility, branch protection, and CI rules are repository policy. |
| **Original reviewed-commit retention** | Keep the per-slice custody ref until aggregate feature review and main landing are green; then retain only the integration receipt unless an audit requires longer. | Squash/fold can otherwise make the reviewed commit unreachable. |
| **Parked-work hold** | Remove the local checkout after exact remote restoration proof; optionally keep a short owner-selected local grace period when space allows. | Balances repair latency against disk pressure. |
| **Untracked/ignored material** | Refuse cleanup unless every item is classified as disposable cache or preserved through an approved private artifact path. Never auto-push it. | Git remote durability does not cover these bytes and they may contain secrets. |
| **No-writable-remote fallback** | Create an owner-private Git bundle plus bounded evidence manifest; if that fails, retain locally. | Cross-repo workflows cannot assume a writable `origin`. |
| **Free-space watermarks** | Owner supplies inventory-trigger and cleanup-target free-space values; no destructive default. | Machine capacity and concurrent workloads are operator policy. |
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
2. Add a read-only inventory/dry-run classifier that emits the lifecycle state and every blocking gate for each exact
   bridge-owned checkout. It performs no push or deletion.
3. Add versioned remote-custody and fold receipts, exact lease-bound operator push, live remote verification, and
   no-writable-remote fallback. No agent receives push authority.
4. Add explicit immediate-review and bounded-repair retention transitions.
5. Add exact guarded cleanup for `FoldedRemoteDurable` and accepted `ParkedRemoteDurable` states, including live
   process/container/open-file rechecks and truthful partial outcomes.
6. Add disk-watermark-triggered inventory and priority selection without allowing the watermark to bypass custody.
7. Verify restoration from remote/bundle, concurrency with review/resume/merge, full suite, hygiene, and one capped
   adversarial review.

Track B may precede later R2f1b production slices to reduce operator storage pain, but it does not prove runtime
deadline custody and cannot green Track A by analogy.

## 10. Planning exit

Planning is ready for a bounded implementation brief only when the owner has answered the decisions in §8, the
roadmap is reconciled to merged `main`, exact current source has been re-audited for missing tests/callers, and Track A
and Track B have separate path ownership, review caps, and acceptance totals.
