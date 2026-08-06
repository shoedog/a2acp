# R2f1b focused boundary — preservation, warnings, absolute deadlines, cleanup ownership

## Context

R2f1a is landed on `main`. It froze *policy* (profiles, fan-out, per-node cancellation, structured
terminal state, checkout/provider identity) but deliberately shipped **no production timer**. The
`preserve_after_cancel` prerequisite named by owner design §5 does not exist, so today every cleanup
strength deletes the operator's worktree. Enabling a deadline on that base would force-remove useful
work — the exact WRONG-1 finding the owner design folded.

R2f1b closes that gap: durable worktree custody + retained resource capability first, timers second.
This document is the frozen focused boundary. **No source implementation is authorized by it.**

This checked-in version folds the five closed WRONG findings and two SMELL corrections from the
single Sol/xhigh closure review of draft SHA-256
`ea7dfc69fd68b830b5a3b6343b50a9770dc264b0a8566e5d6b6411f41c0dbfbe`. No second design review was
run: the declared cap permits one targeted repair of the same artifact because the review found no
open-class defect family.

---

## 1. Frozen authority, base, and R2f1a→R2f1b delta

### Custody (verified, clean)

| Field | Value |
|---|---|
| HEAD | `56334e98291c96c69f5a6fc37a15a8fdaf9634e0` |
| Tree | `17278a20f0d8e450784510c68f103d1c6c1041d4` |
| Parent | `bacb5036120578e09ecb6ec32abffc416b86d381` |
| `main` / `origin/main` | exact HEAD |
| Working tree | clean (incl. untracked) |

### Status reconciliation (stale prose vs Git)

`docs/reliability-execution-roadmap.md` still asserts R2f1a is `PARKED` / `IN PROGRESS` and cites
closure review 5 as current (`:15`, `:76`, `:952`, `:1018`). **Git is authority**: R2f1a landed
through `56334e9`. Closure review 5's single BLOCKER — "the worktree decorator rewrites the provider
cwd after the proposed effect binding" — is **closed in source**:
`bridge-worktree/src/provider_path.rs:67` `validate_bound_worktree()` validates the *persisted*
target and never derives a destination (its doc comment at `:64` states this explicitly).
Reconciling that prose is a pre-slice-1 custody step performed with this boundary, not a reason to redo
R2f1a.

Supplied completion evidence (3,192 passed / 0 failed / 12 ignored; green fmt/Clippy/diff/hygiene)
is **point-in-time supplied evidence, not re-measured here**. It implies no release, deployment,
operator restart, compatibility aggregate, or live-provider smoke.

### Delta

| Landed at `56334e9` | R2f1b adds | Deferred |
|---|---|---|
| `FrozenWorkflowControlsV1`, `LivenessProfileV1` (exact D4.1 values) | `DeadlineActivationV2::AutomaticR2f1b`; real clocks | — |
| `DeadlineActivationV1::ManualOnlyR2f1a` (sole variant, `execution_policy.rs:188`) | V2 activation enum | — |
| Production `fixed_grace` **refused before effects** (R2f1a boundary §1, divergence "Production fixed grace") | Lifts exactly that refusal under V3 | — |
| `FrozenCheckoutEffectV1::Worktree` + `validate_bound_worktree` | Durable custody record over that same frozen target | — |
| `NodeTerminalV1` (`Complete/Failed/NotNeeded/UnknownLegacy`) | cleanup-free `NodePrimaryRecordV3` plus an atomically pre-reserved `NodeCleanupRecordV2` (`Pending`/final), with preservation + collateral in the cleanup row (§2.4) | — |
| Per-node cancel sources, `bounded_independent`, `fail_fast` | Scheduler deadline arm, warning cadence, cutoff | — |
| — | Preservation, resource flights, collateral | R2f2 takeover UX; R2f3 close/health |

---

## 2. State machines, durable records, identity, ordering

### 2.1 Frozen contract — snapshot V3

`WorkflowSnapshotEnvelopeV2` (`run_spec.rs:66`) is `#[serde(deny_unknown_fields)]` and hard-checks
`v != 2`. Add a sibling, do not mutate:

```rust
struct WorkflowSnapshotEnvelopeV3 {
    v: u16,                                  // 3
    attempt: AttemptIdentity,                // current monotonic attempt
    delivery_spec: WorkflowRunSpecV1,        // immutable origin provider/worktree bytes
    predecessor_snapshot_digest: Option<Sha256HexV1>,
    r2f1b: FrozenR2f1bContractV1,
}

struct FrozenR2f1bContractV1 {
    schema_version: u16,
    activation: DeadlineActivationV2,           // ManualOnlyR2f1a | AutomaticR2f1b
    custody_plans: Vec<FrozenWorktreeCustodyPlanV1>,   // one per frozen checkout candidate
    resource_contract_version: u16,
    contract_fingerprint: Sha256HexV1,
}
```

`WorkflowRunSpecV1` stays byte- and fingerprint-identical as `delivery_spec` and remains the sole
source of provider, MCP, cwd, worktree, fan-out, and retry decisions. Its `attempt_id` is explicitly
the **delivery-origin attempt** for V3; it is not the current-attempt authority after resume. The outer
`attempt` is the sole source for current policy triggers, control events, session/resource ownership,
terminal rows, cleanup rows, and history. Fresh admission requires both attempt ids to match. Resume
mints a successor `AttemptIdentity`, binds the predecessor snapshot digest, and retains the unchanged
delivery bytes and checkout target. R2f1b never re-resolves or replaces provider/delivery identity.

`custody_plans` enumerates every worktree checkout in the R2f1a node candidate matrix (preflight
*and* execution candidates — these have distinct session identities, so distinct targets). Identical
checkout fingerprints share one plan. No runtime path may invent another destination.

- Fresh R2f1b execution writes V3. Decode dispatches on `v`.
- A V2 snapshot stays manual-only R2f1a forever. It is **never upgraded** by consulting current
  config. An R2f1b-only entrypoint receiving V2 either runs the old manual contract explicitly or
  refuses before effects.
- A resumed V3 snapshot proves that the outer attempt is the exact successor of the prior outer
  attempt and that `delivery_spec` plus every custody-plan byte is unchanged. No consumer may use
  `delivery_spec.attempt_id` for current-attempt control or persistence.
- **Fingerprint:** `AutomaticR2f1b` activation and `contract_fingerprint` MUST enter
  `workload_fingerprint`. R2f1a's own divergence ruling ("Fingerprint compatibility") forbids
  conflating pre-policy and bounded-policy calibration populations; an armed-timer run is a
  different population from a manual one.

### 2.2 Worktree custody state machine

New `crates/bridge-worktree/src/custody.rs`:

```
ProtectionPrepared ─┬─► UnusedSettled
                    └─► Materializing ─► LiveProtected ─┬─► PreservationPrepared ─► Preserved
                              │              │          │                              │
                              │              │          └─► DeleteAuthorized ─► Removed│
                              │              └─────────────────────────────────────────┴─► RecoveredLive
                              └──────────────────────────────► PreservationUnknown{reason}
```

`Preserved` and `PreservationUnknown` are terminal for R2f1b; only R2f2 disposition releases them.
`RecoveredLive` is a protective live state carrying the successor attempt identity and predecessor
claim digest; it has the same sweep exclusion as `LiveProtected`. `UnusedSettled` is permitted only
after descriptor-bound proof that the candidate target does not exist and is not registered. It may
remove the unused marker but never calls provider removal.

```rust
struct WorktreeCustodyIdV1([u8; 32]);           // CSPRNG, never path-derived

struct PreservedWorktreeClaimV1 {
    schema_version: u16,
    custody_id: WorktreeCustodyIdV1,
    execution_id: ExecutionId, origin_attempt_id: AttemptId,
    current_attempt: AttemptIdentity,
    node: PolicyNodeRefV1, checkout_fingerprint: Sha256HexV1,
    source: WorktreeObjectIdentityV1, root: WorktreeObjectIdentityV1,
    worktree: WorktreeObjectIdentityV1, common_dir: WorktreeObjectIdentityV1,
    reason: PreservationReasonV1, created_wall_ms: i64,
    recovery_locator: RecoveryLocatorV1,
}
```

`WorktreeObjectIdentityV1` = canonical path + `DirectoryIdentity` (dev/ino on unix). Identity is
checked by **descriptor**, not by re-canonicalizing a string, at every decision point.

**Record naming.** V3 publishes `<target>.custody.v1.json` (custody-record version, independent of
snapshot version). It does **not** write the legacy `.meta.json`. `sweep.rs::sidecars()` only scans
`*.meta.json`, so an older binary cannot see — and therefore cannot delete — a V3 checkout. The new
boot sweep MUST scan **both** patterns: legacy `.meta.json` under existing bounded policy, and
`.custody.v1.json` under §5 policy. Without that, V3 checkouts would leak unreclaimed forever.

### 2.3 Durable publication primitives

`write_sidecar` (`provider_path.rs:129`) writes and renames with **no `sync_all` and no parent-dir
sync** — a crash can lose the record entirely. Legacy behavior stays unchanged; V3 must not use it.

`bin/a2a-bridge/src/local_file.rs` already implements exactly the needed primitives, but `pub(crate)`
inside the binary: `sync()`/`sync_all` (`:625`, `:645`), `sync_journal_recovery_barrier` (`:654`),
no-replace publication via `renameatx_np`/`renameat2` with parent sync (`:1405`–`:1531`), and the
fault hook `fail_sync_on_nth_call_for_test` (`:661`). **Extract the generic descriptor-relative
identity/publication core into `crates/bridge-core/src/fs_custody.rs`**; binary-specific bounded-reader
behavior stays in `local_file.rs`. This is reuse, not new machinery — and it carries the existing
crash-injection hook into custody tests for free.

### 2.4 Terminal records — the byte-budget adjudication

**A constraint the source imposes that must shape the design.** In `execution_policy.rs`:

```
MAX_NODE_TERMINAL_JSON_BYTES        = 2_048
DERIVED_NODE_TERMINAL_WORST_CASE_BYTES = 1_978     // const-asserted <= cap at :39
```

Only **70 bytes** of headroom. Adding `preservation`, `collateral`, `recovery_owner`, and
`resource_flight_id` to the same encoded blob would blow the const assert or force cannibalizing the
deepest-cause reserve — silently undoing R2f1a's W5 repair (which exists precisely to preserve the
deepest cause under overflow).

**Ruling:** split, don't widen, and do not pretend the V1 primary can represent pending cleanup.
`NodeTerminalV1` and its 2,048-byte budget remain untouched for V2. V3 uses a cleanup-free
`NodePrimaryRecordV3` with its own independently derived cap. Cleanup/preservation/collateral evidence
lives in its **own additive row** with its own constant and const-asserted derived worst case:

```rust
// Atomically reserve both rows with the attempt before effects:
//   primary = bounded uncommitted placeholder
//   cleanup = Pending { resource_flight_id }
reserve_node_terminal_rows_v3(...)

// CAS the V3 placeholder once; cannot touch cleanup.
put_node_primary_sequenced_v3(NodePrimaryRecordV3)

// CAS only Pending -> final; cannot touch primary/cause/output
settle_node_cleanup_sequenced_v3(NodeCleanupRecordV2)

struct NodePrimaryRecordV3 {
    schema_version: u16,
    primary: NodePrimaryDispositionV1,
    cause: Option<BoundedCauseV1>,
    prompt_may_have_been_accepted: bool,
    degraded_ancestry: bool,
    policy_trigger_id: Option<PolicyTriggerIdV1>,
}

struct NodeCleanupRecordV2 {
    cleanup: NodeCleanupV2,
    preservation: WorktreePreservationResultV1,
    collateral: Option<CollateralResultV1>,
}
enum NodeCleanupV2 {
    Pending  { resource_flight_id: ResourceFlightIdV1 },
    Complete { duration_ms: u64 },
    Partial  { duration_ms: u64, recovery_owner: RecoveryOwnerV1 },
    Failed   { duration_ms: u64, cause: BoundedCauseV1 },
    NotNeeded,
    Unknown  { duration_ms: u64, recovery_owner: Option<RecoveryOwnerV1> },
}
```

The attempt-reservation transaction pre-mints every `ResourceFlightIdV1` and atomically inserts both
rows for every graph node. A V3 primary can therefore never exist without a cleanup owner. Storage is
**additive SQLite tables**; no existing row changes meaning. Matching idempotent workflow-history
mutations and sequence forms `NodePrimaryCommittedV3` / `NodeCleanupSettledV3`.

Every V3 reader—TaskStore, history, detached coordinator, offline result, A2A, MCP, batch, resume, and
reporting—joins the two rows by `(attempt_id,node_id)`, validates both schema versions and rejects a
missing/duplicate pair. A crash after primary commit resumes only the cleanup CAS. V1 readers never
decode or project V3 rows; an unsupported surface refuses before effects.

Projection stays durable-first. If the primary store is unavailable, the observable envelope reports
`terminal_persistence_failed` and retains the in-memory structured map + recovery locator — it never
replaces the original node cause.

### 2.5 Publication and activation ordering

Before provider resolution, worktree creation, session config, spawn, or prompt:

1. Freeze and validate the V3 envelope.
2. Reserve task/history/control rows.
3. Arm the 30-minute admission/queue clock and create one retained, independently owned pre-effect
   preparation flight before the first open, lock, write, rename, or sync.
4. Under that flight, open the canonical worktree root **by descriptor**; capture persistent identity.
5. Acquire the persistent custody lock for every frozen candidate.
6. Create → `sync_all` → **no-replace** publish `ProtectionPrepared` → **parent sync**.
7. Reopen and verify by descriptor.
8. Arm the resource/control journal and absolute **work** deadline.
9. Only now admit provider/session/process/worktree-materialization effects.

Each potentially blocking preparation operation uses D11's 30-second internal action bound and
31-second observable control bound. Expiry before the prepared barrier is a typed pre-effect refusal,
not workflow cancellation: it admits zero provider/session/process or `git worktree add` effects and
transfers the exact preparation guard to the recovery flight rather than dropping it. Ambiguous
temp/final marker publication is quarantined/protective and never licenses deletion. The attempt can
therefore terminalize within its admission/reporting envelope even if an OS filesystem operation
remains recovery-owned. The two-hour work clock remains unarmed until protection completes.

`ProtectionPrepared` is already deletion-excluding (§5), so **no window exists in which a timer is
armed but the checkout is unprotected**. This is the acceptance criterion "no automatic deadline can
become reachable before a crash-safe protective state excludes both sweeps."

Materialization: durably replace with `Materializing` → `git worktree add` through a retained bounded
control flight → open source/target/common-dir/record by descriptor → verify frozen target and object
identities → replace with `LiveProtected` (sync + parent sync) → configure inner backend with the
**unchanged** R2f1a bound session spec.

**`cleanup_failed_add` is forbidden for V3.** Today `host_git.rs:42` does `remove_dir_all(wt)` on add
failure (called at `:137`, `:147`). Under V3 a partial add becomes `Preserved` or
`PreservationUnknown{materialization_inflight}` — never repaired by deletion.

### 2.6 Attempt identity on resume

Resume calls `AttemptIdentity::resume()` before a successor clock or effect, persists the new
`attempt_id`, ordinal, and parent link, and uses that successor identity for every policy, control,
resource, terminal, cleanup, session-name, and history key. It separately retains
`delivery_spec.attempt_id` as `origin_attempt_id` for the already-frozen checkout/delivery bytes. This
satisfies invariant 9 without changing the persisted worktree target or re-resolving provider state.

---

## 3. Source/component ownership and production entrypoints

| Component | Files |
|---|---|
| Core contracts | `bridge-core/src/execution_policy.rs`, new `resource_flight.rs`, new `preparation_flight.rs`, new `fs_custody.rs`, `ports.rs`, `process.rs`, `reaper.rs`, `attempt_activity.rs` |
| Frozen V3 admission | `bridge-workflow/src/run_spec.rs`, admission path |
| Worktree custody | `bridge-worktree/src/{provider_path,custody(new),backend,sweep,provider,host_git}.rs` |
| Scheduler | `bridge-workflow/src/{executor,fanout}.rs` |
| Backends (all 5 production impls) | `bridge-acp/src/acp_backend.rs:6485`, `bridge-api/src/backend.rs:644`, `bridge-container/src/lib.rs:1298`, `bridge-worktree/src/backend.rs:1152`, `bridge-acp/src/replay.rs:60` (refuses V3) |
| Destructive wrappers | `bridge-registry/src/registry.rs`, `bridge-controller/src/resilient.rs`, session manager, coordinator dispatch cleanup |
| Persistence | `bridge-core/src/{task_store,workflow_history}.rs`, `bridge-store/src/sqlite.rs` |
| Served | `bridge-coordinator/src/{coordinator,detached,batch}.rs`, A2A `server.rs`, `bridge-mcp/src/server.rs` |
| Offline/boot | `bin/a2a-bridge/src/main.rs` — `run-workflow`, `implement` fresh/resume, MCP boot, serve boot, every sweep install site |

`run-workflow --serve` is only a client; the serving Coordinator owns the contract. Direct
unary/session traffic acquires no workflow deadline but **must** attach a resource owner if it can
share a process. A backend already spawned without a compatible resource flight cannot be adopted
from PID metadata — V3 resolution creates a compatible keyed generation or fails closed.

---

## 4. Scheduler, deadlines, and terminal-bound math

### 4.1 One clock

`bridge-core/src/attempt_activity.rs:142` already defines `MonotonicClock` with
`SystemMonotonicClock`. **Reuse it** — do not invent a parallel clock type. One `Arc<dyn MonotonicClock>`
per attempt feeds `SharedAttemptRecorder`, telemetry sinks, scheduler, cleanup, and reporting.

Wall timestamps identify records only. Monotonic offsets are audit data, never restartable wall
deadlines. Resume starts a new monotonic epoch under unchanged frozen policy.

The attempt's queue/admission epoch starts before custody preparation (§2.5). The two-hour work epoch
starts only after the prepared barrier. Both use the same injected monotonic clock; the pre-effect
preparation flight owns any operation that outlives its observable bound.

### 4.2 Frozen values (verified in `liveness_profile_v1()`, `execution_policy.rs:107`)

| Bound | ms | Source |
|---|---:|---|
| Queue wait | 1,800,000 | `queue_wait_ms` |
| Control observable | 31,000 | `control_observable_ms` |
| No-progress snapshot | 1,800,000 | `no_progress_snapshot_ms` |
| Work cutoff | 7,200,000 | `work_cutoff_ms` |
| Cancel observable | 6,000 | `cancel_observable_ms` |
| Cleanup tail | 60,000 | `cleanup_tail_ms` |
| Reporting tail | 10,000 | `reporting_tail_ms` |
| Terminal envelope | 7,270,000 | `terminal_bound_ms` |

The landed profile carries only the **observable** bounds. D11's **internal** action timers (30 s
control, 5 s cancellation grace) are not in the profile and must be added as R2f1b constants, with
the remaining second reserved for scheduling/fencing/publication. Lengthening the internal timers to
match the observable bounds is explicitly forbidden by D11.

Profile precedence (invocation → workflow/task-class → legacy) is unchanged R2f1a selection; R2f1b
only *activates* the frozen result. Max still requires a finite greater cutoff plus reason; neither
warnings nor fixed grace extend it.

Work clock starts **after** queue admission and the protection/resource barriers, immediately before
the first preflight/provider effect. It covers preflight, worktree creation, session config, prompt,
allowed retry/backoff, verification, and scheduling critical path.

### 4.3 Event loop

`executor.rs:4619` today is a bare `inflight.next().await` — no deadline arm, so a nonterminating
sibling blocks terminalization (this *is* #22). Keep `FuturesUnordered`; replace the bare wait with a
`biased` select:

1. Drain immediately-ready node completions (preserving R2f1a's ready-batch sort by `NodeId`).
2. Durable trigger-barrier acknowledgements.
3. Workflow/external cancellation.
4. Fixed-grace expiry.
5. Absolute cutoff.
6. Mechanically proved impossibility.
7. Due no-progress snapshots.
8. Wait on node / activity / control / clock.

Ties: completion ready **at** the cutoff wins for that node; unfinished nodes are then canceled.
Warning loses to both completion and cutoff.

Warning cadence: `ordinal = floor((now - last_meaningful_progress) / 30m)`; each positive ordinal
emits once per progress epoch. Activity without meaningful progress updates only the activity clock.
Progress resets the warning epoch. **Silence never cancels.**

### 4.4 Early automatic cancellation — closed list

Permitted **only** on constructive facts:
- a retained child exited while its sole producer result is pending;
- a named container generation is proved absent after spawn settlement;
- all producer/final routes are irreversibly closed with no terminal result possible.

Not proof: unknown child state, no output, elapsed silence, file mtime, process age, provider
slowness. (Invariant 2 / D2.)

### 4.5 Fixed grace

R2f1a *refuses* production `fixed_grace` before effects. R2f1b lifts exactly that refusal under
`AutomaticR2f1b` and arms a real, **one-shot, non-renewable** grace timer. It records the separately
named policy trigger and never rewrites the sibling's recorded node deadline (invariant 3 / D1).

---

## 5. Preservation, cleanup, collateral, restart, corruption

### 5.1 `preserve_after_cancel`

Today **both** cleanup strengths delete: `backend.rs:865-886` runs `forget_session_checked` *or*
`release_session_checked`, then unconditionally `provider.remove(...)`. That is the mechanism owner
design §5 names as blocking deadlines.

Every non-success exit — failure, external cancel, fixed-grace/fail-fast cancel, mechanical
impossibility, absolute cutoff, cleanup ambiguity — takes this path:

1. Lock the same custody cell used by creation and sweep selection.
2. Close deletion admission.
3. Sync + parent-sync `PreservationPrepared`.
4. If target identity is complete, atomically replace with the full claim; parent-sync again.
5. Mark the live lease transferred to recovery ownership.
6. **Only then** may session cancel or a resource signal occur.
7. Return `Preserved` / `Partial` / `Unknown`. Never call provider remove, reset, clean, checkout,
   or prune.

A node-local success is **not** a checkout disposition. It settles the node session but leaves its
checkout `LiveProtected` under a workflow-level disposition flight. After every node is terminal, an
all-healthy global workflow outcome may mint deletion capabilities and perform the existing normal
success cleanup. Any failed, degraded, canceled, timed-out, mechanically impossible, or
cleanup-ambiguous global outcome runs `preserve_after_cancel` for **every materialized checkout**,
including nodes that completed earlier. Thus a successful sibling cannot delete useful work before a
later sibling determines the workflow outcome.

If materialization is unresolved, publish `PreservationUnknown{materialization_inflight}` **before**
terminating its control process. That state is sweep-ineligible. Recovery may later finalize an
exact claim; it may never infer permission to delete.

**Globally healthy workflow success is the only automatic deletion path.** It CASes to
`DeleteAuthorized` and mints an unforgeable `DeletionCapabilityV1`. `HostGitWorktree::remove_v2`
takes that capability — not a raw path — revalidates source/root/target/common-dir identities
immediately before Git removal, and verifies registration + target absence afterward
(`host_git.rs:153-161` already implements those post-conditions; reuse them), then records `Removed`.
Once a preserved claim exists, only R2f2's explicit local retain/archive/delete disposition can clear
it; no later healthy projection or TTL can mint deletion authority.

### 5.2 Sweeps

`WorktreeRunEndGuard::Drop` (`sweep.rs:109`) deletes every readable sidecar whose `run_id` matches;
`sweep_orphans` (`:87`) deletes when `classify(...) == Verdict::Dead`. Replace with a custodian:

- Explicit run-end settlement converts unresolved live V3 entries to preserved/unknown.
- Its `Drop` backstop is **non-destructive**; the already-synced protection record is authoritative.
- Boot sweep parses V3 state **before** examining run ids or leases.
- Ineligible for deletion: `ProtectionPrepared`, `Materializing`, `LiveProtected`, `RecoveredLive`,
  `PreservationPrepared`, `Preserved`, `PreservationUnknown`, and every corrupt / missing / mismatched
  V3 pair.
- `UnusedSettled` permits removal of marker metadata only after exact absence and registration proof;
  it never invokes Git/provider removal.
- A free lease means *recover ownership*, not *delete*.
- Boot sweep emits a bounded report: recovered / preserved / unknown / legacy-deleted / refused.
- It never runs `git`, `remove_dir_all`, reset, clean, or checkout for a V3 record.

Existing guards stay: the sidecar↔sibling match check and the under-root check (`:49`, `:57`) already
defeat a forged record pointing outside the root; keep both.

### 5.3 Retained resource flights

New `bridge-core/src/resource_flight.rs`:

```rust
enum ResourceIdentityV1 {
    AcpProcess { generation, spawn_nonce: [u8;32], pid, pgid, immutable_start: ProcessStartIdentityV1 },
    ManagedContainer { generation, runtime, immutable_container_id, ownership_labels_digest },
    DedicatedRemoteRequest { request_id },
}
enum ResourceFlightStateV1 { Open, AdmissionClosed, IntentJournaled, Signaling, Settled(ResourceActionResultV1) }
```

Each per-node session/worktree flight holds exactly **one** `Arc<RetainedResourceFlight>`. A
multiplexed ACP process or shared container uses **one generation flight**; a dedicated API request
uses a dedicated flight. Owner attachment is serialized with action admission and journaled before
dispatch; if bounded journal capacity cannot accept another owner, admission refuses before provider
work.

Before any resource-level action: close generation admission under the same transition lock used to
attach owners → snapshot all active owners in deterministic order → persist initiator, cause, exact
capability digest, and collateral owner set → signal **only** through the retained capability →
record child/root/container dispositions once → publish one result to every owner in the snapshot
plus any owner discovered before settlement.

PID, PGID, container name, and persisted start timestamps cannot reconstruct authority. Missing or
ambiguous capability → `Partial`/`Unknown`. No `pkill`, no process-name lookup, no broad Docker scan,
no late PID signaling.

### 5.4 Backend/wrapper obligations

Additive `AgentBackend` methods (`ports.rs:153`) with **refusing defaults** (`R2f1bUnsupported`) so
unmodified implementations fail before effects: `attach_bound_owner_v2`, `configure_bound_session_v2`,
`settle_session_v2`, `resource_flight`.

- **AcpBackend** — flight owns `Supervised`, the process group, immutable start evidence, and the
  optional `:ro` container controller. `cancel`, `escalate_terminate` (`acp_backend.rs:5160`),
  `retire`, registry retirement, and `Drop` all join it. Ordinary session release detaches only that
  owner and cannot signal the shared process.
- **ContainerRwBackend** — promote the existing per-generation `ReapController`
  (`bridge-core/src/reaper.rs:78`) into the resource flight; the inner ACP process is subordinate to
  the same composite boundary, not a second independently signaled flight.
- **WorktreeBackend** — owns custody/session state, forwards the inner flight. V3 failure/cancel →
  preservation; only normal success requests deletion.
- **ApiBackend** — wrap the existing per-session watch cancellation as a dedicated remote-request
  flight.
- **ReplayBackend** — test double; returns `R2f1bUnsupported`.
- **`Supervised`** — capture spawn nonce + immutable OS start identity. `Drop` today unconditionally
  `kill(-pgid, SIGKILL)` (`process.rs:539-545`). Construction becomes internal to
  `OwnedProcessTreeV1`; raw `Drop` may not signal outside a journal-capable flight.
- **Registry** — `wait_for_slot_drain` breaks on grace and calls `retire()` **even with leases
  outstanding** (`registry.rs:509-544`). Race-loss, invalidation, reload retirement, and keyed
  retirement must all request/join the flight.
- **`ResilientWarm`** — `resilient.rs:178-183` does `retire()` → `(self.reset_worktree)()` → rebuild
  after a transient failure. That path **must not** be reachable for an R2f1b-protected attempt.
- **ACP watchdog** — AutomaticR2f1b admission refuses before registry/session/provider effects when
  any selected agent/candidate has legacy `[agents.watchdog]` settings. V2 workflows and direct
  sessions retain the exact configured idle/hard-wall behavior. A later separately reviewed schema may
  freeze a smaller hard-wall bound and require explicit operator opt-in before making idle observation
  actionless; R2f1b never silently demotes an existing limit.

### 5.5 Cancellation → terminal flow

1. Stop new node and resource-owner admission.
2. Complete or durably type preservation for every materialized worktree, including already-completed
   nodes whose checkout disposition was held for the global outcome.
3. Persist the policy/deadline/mechanical cause.
4. Close relevant resource-flight admission; journal collateral.
5. Cancel per-node tokens; send ordinary session cancellation.
6. By **6 s**, persist/publish each initiating disposition or transfer its exact owner.
7. Escalate only through the retained flight.
8. By **60 s**, settle cleanup or record `Partial`/`Unknown` + retained recovery owner.
9. By the **10 s** reporting tail, publish the complete structured terminal map.

Cleanup deadlines anchor to the cancellation event but are capped by `work_cutoff + 60 s` / `+70 s`.
A node future is **never** dropped while it is the sole cleanup owner (invariant 5); at the cleanup
deadline its exact guard transfers to the backend/registry recovery flight.

### 5.6 Failed root + nonterminating sibling (#22 closure)

1. Root failure persists immediately as immutable primary, cleanup `Pending`.
2. Its worktree → `Preserved`, not provider removal.
3. Under `bounded_independent` the sibling continues to its own completion or the workflow cutoff.
4. At cutoff, sibling is preserved then canceled.
5. Shared generation + escalation → **one** journaled action listing root and sibling as collateral;
   a different generation/process is untouched.
6. Cleanup settles or transfers by 60 s.
7. Every graph node gets a terminal disposition, including never-started downstream nodes.
8. Workflow terminal published by 2:01:10 with root cause unchanged.

### 5.7 Crash / corruption matrix

| Fault point | Required result |
|---|---|
| Before `ProtectionPrepared` publication | Ordering proves no worktree/provider/process effect occurred |
| Temp written, before no-replace rename | Final absent; quarantine temp; no effects |
| Prepared synced, before `git add` | Marker excludes sweeps; only exact proof of no materialization may remove the unused marker |
| During/after partial add, before live identity | Prepared/Materializing excludes; report preservation unknown; never delete target |
| Claim renamed, parent sync ambiguous | Prior prepared state or ambiguous claim remains protective; report unknown |
| Claim synced, lease not yet transferred | Live lease **and** durable claim both protect; resume waits for exact lease |
| Admission closed, intent not journaled | No signal permitted |
| Intent journaled, crash before/during signal | Join same flight if live; never reconstruct from PID/name; else record unknown |
| SIGTERM ignored | Same retained flight may child-first SIGKILL; every shared owner gets one collateral result |
| Cleanup pending at 60 s | Transfer exact owner; terminal records partial/unknown |
| Crash after primary commit, before cleanup settle | Resume preserves primary; performs only the cleanup CAS |
| Crash after preserved terminal | No automatic provider replay; claim awaits R2f2 |

### 5.8 Resume exchange

1. Reconcile already-terminal task/history state first.
2. Mint and durably claim the successor `AttemptIdentity` with the same execution id, incremented
   ordinal, and exact parent attempt.
3. Acquire custody lock + the claim's recovery lease.
4. Open and validate exact source/root/worktree/common-dir objects and claim digest by descriptor.
5. Validate task/execution/node lineage, unchanged delivery-origin bytes, and successor attempt.
6. Atomically publish `RecoveredLive { predecessor_claim_digest, successor_attempt }`; parent-sync;
   acquire successor live lease.
7. **Only then** resolve/configure a backend from the unchanged delivery spec.
8. Never clean or recreate the worktree.

A committed policy trigger, deadline, or terminal node is not replayed. Terminal
failed/canceled/timed-out claims stay preserved for R2f2; only genuinely still-Working crash recovery
exchanges the lease.

---

## 6. Test matrix, gates, rollback

Every test must **fail against `56334e9`** before implementation. Clock tests use a manual monotonic
clock; crash tests use explicit fault hooks at file sync, rename, parent sync, journal commit, signal,
cleanup transfer, and terminal publication — reusing `fail_sync_on_nth_call_for_test`.

| Invariant | Fail-first regression | Negative / edge |
|---|---|---|
| Protection precedes clocks/effects | `automatic_activation_waits_for_parent_synced_protection` | Parent-sync failure → zero provider/process/timer calls |
| Preparation is finitely owned | `nonreturning_custody_sync_transfers_pre_effect_owner` | Typed bounded terminal/transfer; zero provider/session/process/materialization/destructive-sweep calls |
| Both sweeps exclude protection | `prepared_and_preserved_survive_run_end_and_dead_boot_sweep` | Corrupt/missing/mismatched/symlinked/multi-link → unknown, never deleted |
| Partial add preserved | `add_failure_after_target_creation_never_removes_target` | Failure before any target exists may settle the unused marker only |
| Candidate settlement | `unused_candidate_settles_only_after_exact_absence` | Crash after claim exchange remains `RecoveredLive` and sweep-protected |
| Exact resume exchange | `successor_attempt_and_claim_exchange_precede_resume_provider_effect` | Reused current attempt, wrong origin/digest/lineage/parent refuses |
| Cancel cannot delete | `completed_sibling_survives_later_workflow_cutoff` | Node-local success cannot remove; global healthy success with capability removes exactly once |
| Silence only warns | `thirty_minute_no_progress_crossing_is_actionless` | Activity w/o progress doesn't reset; progress at 29:59 does |
| Warning cadence | `warning_ordinal_emits_once_per_progress_epoch` | Duplicate wake and cutoff/warning tie don't duplicate |
| Mechanical proof may cancel | `exited_owned_child_cancels_before_cutoff` | Unknown liveness and healthy silence never cancel |
| Cutoff always bounds | `pending_sibling_terminalizes_by_2h_1m_10s` | Completion ready exactly at cutoff is retained, not relabeled |
| Fixed grace real + one-shot | `committed_fixed_grace_expires_once_without_extending_cutoff` | Uncommitted/failed barrier or duplicate expiry never cancels; **R2f1a-refusal lift proved** |
| One flight per generation | `two_nodes_one_generation_signal_once_and_share_result` | Missing capability → partial; other generation survives |
| Collateral complete | `shared_acp_escalation_lists_every_active_owner` | Owner detach/attach race serialized with admission closure |
| Sole cleanup owner never dropped | `cleanup_deadline_transfers_exact_guard_before_terminal` | Failed transfer → unknown, flight retained |
| #22 closure | `failed_root_and_pending_sibling_preserve_root_and_terminal_map` | Cleanup/store failure cannot overwrite root cause |
| Terminal monotonicity | `v3_primary_and_pending_cleanup_are_atomically_reserved` | Crash after primary projects Primary+Pending; conflicting replay and final→final rewrite refuse |
| **Byte budget** | `node_terminal_v1_budget_unchanged_and_cleanup_row_within_own_cap` | Forced overflow still preserves deepest cause (W5 unbroken) |
| Surface parity | offline / A2A / MCP / batch / resume snapshot+result goldens | Legacy backend/wrapper refuses before provider effect |
| Watchdog compatibility | `automatic_v3_refuses_legacy_watchdog_before_effects` | V2/direct session watchdog behavior remains byte-for-byte unchanged |
| Destructive wrappers join flight | registry reload/invalidation/retire/Drop, resilient failure | `reset_worktree` never called for protected V3 |
| Capability-bound identity | PID-reuse / wrong-container / unrelated-process | Persisted numeric/name data cannot signal |
| Rollback | old parser ignores V3 snapshot + custody sidecar | Old binary cannot resume V3, cannot select its worktree for sweep |

### Gates (per slice and aggregate)

```bash
git diff --check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --bin a2a-bridge
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Final CI parity adds `cargo deny check`, coverage thresholds, serialized bridge-store platform tests,
macOS + Linux custody/process-identity lanes, migration/downgrade tests, and a fresh adversarial
combined-diff review. Report exact passed/failed/ignored totals and name every excluded platform or
live check. **No provider, compatibility, smoke, release, or deployment step is in these gates.**

### Rollback

Fail-safe by construction: V2 snapshots stay manual R2f1a; V3 uses a new envelope *and* a new sidecar
name; SQLite changes are additive; trait additions have refusing defaults; an older binary cannot
decode V3 and ignores (never deletes) its custody sidecar; a newer binary does not reinterpret legacy
state as R2f1b evidence; no downgrade re-resolves provider or worktree identity.

---

## 7. Slices and commit order

1. **Contracts + persistence, inactive.** V3 envelope with current-versus-origin attempt identity,
   custody/resource/preparation/terminal types, atomically paired primary+cleanup reservations,
   additive TaskStore/history/SQLite APIs, shared-clock injection, `fs_custody` extraction. No
   activation.
2. **Custody + sweep conversion.** V3 record, creation ordering, recovery-only sweeps, claim exchange,
   deletion capability, `cleanup_failed_add` prohibition. All worktree crash tests green before
   continuing.
3. **Resource authority.** `OwnedProcessTree`, generation flights, five backend adapters, `ReapController`
   integration, registry/resilient/session wrappers, collateral journal. Unrelated-process-survival and
   one-flight tests green.
4. **Scheduler activation.** Progress notifications, queue/control/fixed-grace/absolute clocks,
   preservation-first cancellation, bounded cleanup transfer, fixed-grace refusal lift, #22 closure.
   **`AutomaticR2f1b` first becomes constructible here.**
5. **Persistence + serving parity.** Detached sink, terminal CAS, coordinator, A2A, MCP, batch, resume,
   offline/implement/boot wrappers.
6. **Aggregate closure.** Migration/rollback tests, provider-free fault matrix, final roadmap evidence
   update, full suite, combined-diff review.

**No slice may enable a deadline before 1–3 are green.** Declared review cap: one round per slice
artifact. Closed-enumerable findings get one targeted repair on the existing artifact; open-class
findings stop the round and escalate to design.

**Slice cost to name up front:** ~40 `impl AgentBackend` test doubles exist across
`bridge-coordinator`, `bridge-a2a-inbound`, `bridge-workflow`, and `bin`. Refusing defaults mean every
double used on a V3 path needs updating. This is mechanical but not free; it lands in slice 3.

---

## 8. Risks, non-goals, owner decisions

### Risks (trigger / likelihood / impact / fix cost / disposition)

| Risk | Trigger & likelihood | Impact | Fix cost | Disposition |
|---|---|---|---|---|
| Node-local success deletes before a later global failure | Parallel materialized nodes; plausible/routine | Irreversible loss of completed sibling edits | Medium — workflow-level disposition flight, §5.1 | **BLOCKER — resolved in this design** |
| Resume reuses predecessor attempt authority | Every V3 crash resume; certain without separation | Colliding/misattributed control, cleanup, and history | Medium — outer successor plus immutable origin delivery, §§2.1/2.6 | **BLOCKER — resolved in this design** |
| V3 primary falsely embeds pending cleanup in V1 | Every primary-before-cleanup path; certain | False state or broken projection/CAS | Medium — versioned primary plus atomic cleanup reservation, §2.4 | **BLOCKER — resolved in this design** |
| Custody preparation stalls before all clocks | Wedged filesystem/sync; rare but credible | Unbounded attempt before provider effects | Medium-high — finite preparation flight, §2.5 | **BLOCKER — resolved in this design** |
| Terminal byte budget overflow | Any V2 terminal carrying preservation+collateral inline; **certain** if inlined | Const-assert break, or silent loss of deepest cause (undoes W5) | Low — separate row, §2.4 | **BLOCKER — resolved in this design** |
| V3 checkouts invisible to legacy sweep | New sidecar name; certain by construction | Unbounded leak if new boot sweep doesn't scan both patterns | Low — dual-pattern scan, §2.2 | **BLOCKER — resolved in this design** |
| Fixed-grace refusal not lifted | R2f1a refuses production `fixed_grace`; certain for any workflow selecting it | Feature silently unavailable after R2f1b ships | Low — §4.5 + named regression | **BLOCKER — resolved in this design** |
| Fingerprint conflation | `AutomaticR2f1b` not in `workload_fingerprint`; certain | Armed and manual runs pooled into one calibration population; corrupts D4 baselines | Low — §2.1 | **BLOCKER — resolved in this design** |
| Watchdog either preempts V3 policy or is silently demoted | Operator has `[agents.watchdog]` + V3 workflow; plausible | Invariant-2 violation or silent runtime/cost compatibility break | Low design — pre-effect refusal, §5.4 | **BLOCKER — resolved in this design** |
| `write_sidecar` non-durability | Crash between rename and disk flush; rare but real | Custody record lost → checkout unprotected | Low — V3 uses `fs_custody`; legacy untouched | Resolved |
| Test-double churn | Slice 3; certain | Large mechanical diff, review fatigue | Medium, bounded | **DEFER** — named, scheduled, not blocking |
| `git worktree add` inside 31 s control bound on a very large repo | Huge repo + cold cache; uncommon | Control bound exceeded → typed `cleanup_pending` transfer, not a wrong result | Medium | **DEFER** — degrades to the designed unknown/partial path; no incorrect output |
| `dev/ino` identity on non-unix | Windows; not a supported target today | N/A on supported platforms | — | **DEFER** — name the platform exclusion in gates |

Rejected as non-risks (no constructible incorrect result): "an old binary might mis-handle V3" — it
cannot decode the envelope and cannot see the sidecar, so it leaks rather than corrupts; "PID reuse
after `Supervised::Drop`" — `kill_on_drop` holds the leader unreaped, so the PID is not reusable while
the group kill is in flight.

### Non-goals

R2f2 takeover CLI/artifact and retain/archive/delete UX; R2f3 ACP `session/close`, debt retries,
capacity, health/quarantine/drain, successor routing, bridge-process handoff; R2f4 live closure
evidence; R2g stable ingress; any release, deployment, compatibility claim, operator mutation, or
provider turn; #24/#47 closure.

### Owner decisions

**None open inside R2f1b.** The bounded review repaired the two original adjudications as follows:

1. **Terminal evidence is split into a V3 cleanup-free primary and a separately bounded cleanup row,**
   both atomically reserved before effects. V1 remains unchanged and keeps W5's deepest-cause proof.
2. **AutomaticR2f1b refuses legacy watchdog configuration before effects.** V2/direct behavior is
   unchanged; any future versioned smaller-bound/idle-observation integration requires its own review.

---

## Verification of this design

Every mechanism claim above was checked against the frozen checkout, not inherited from the source-
mining artifact. Verified directly: `provider_path.rs:67` (repair landed), `:129` (no fsync),
`backend.rs:865-886` (both strengths delete), `sweep.rs:87,109` (both sweeps delete),
`host_git.rs:42,137,147,153-161`, `executor.rs:4619` (no deadline arm), `execution_policy.rs:32-40`
(byte budget), `:107` (profile values), `:188` (sole activation variant), `:301-312` (no Pending),
`process.rs:539-545` (unconditional group SIGKILL), `registry.rs:509-544` (retire despite leases),
`resilient.rs:178-183` (reset after transient), `run_spec.rs:49-69` (V2 envelope, deny_unknown_fields),
`reaper.rs:78` (existing single-flight), `attempt_activity.rs:142` (existing `MonotonicClock`),
`local_file.rs:625-1531` (durable publication primitives), and the five production `AgentBackend`
impls. No build, test, provider turn, or state mutation was performed.

R2F1B DESIGN: READY
