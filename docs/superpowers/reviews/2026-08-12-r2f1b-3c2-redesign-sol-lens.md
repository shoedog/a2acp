I’ll keep this strictly read-only: no edits, builds, tests, network, providers, or delegated helpers. I’m using the `lsp-nav` skill for exact type/reference coverage, alongside direct source inspection; I’ll first freeze checkout identity and consult the narrow prior-review memory index, then trace the state machine through production callers and persistence implementations.The narrow memory pass confirms earlier hazards but not the current checkout, so I’m treating it only as a search map. I’ll now bind the exact HEAD/base and read the adjudication plus the actual diff; no prior handoff or test claim will be used as delivery proof.Checkout identity is exact and clean at `530992b7`; the three August 12 review files named by the brief are not present in this checkout. I’ll treat the eight rulings embedded in the brief as supplied authoritative requirements and check whether those review artifacts exist in the adjacent main checkout for read-only context, while keeping every code conclusion bound to this frozen worktree.The `lsp-nav` capability is not callable in this session, so its read-only semantic reference pass cannot run. I’m falling back to bounded `rg` plus direct definition/caller inspection, and I’ll explicitly enumerate every production construction/trait/wrapper seam I can prove from this checkout.The source trace now shows a useful decomposition change: the unsafe request path is narrow enough to replace without restarting the artifact or disturbing the folded process/container flight core. The salvage plan will therefore keep shared process/container custody, replace only the remote-request adapter and void publication contract, and add a descriptor-safe substrate first; API migration comes only after those contracts are green.I have the authority model and task cut. I’m doing one last compile-surface check on crate names and exports so the proposed focused gates are literal commands, not placeholders.The preserved artifact is salvageable. The correct cut is a bounded replacement of the remote-request adapter and its journal/publication protocol, while retaining the proven identity, cancellation, diagnostic, cleanup vocabulary, and dormant production route. A fresh restart is neither necessary nor justified.

## Evidence boundary

Observed in the exact checkout:

- `HEAD` is `530992b7ff1e8e9151fb2a69e86f3ff71c44f905`; the worktree is clean.
- The reviewed delta is based on `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.
- Production still supplies `resource_flight_route_v3 = None` in [main.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/bin/a2a-bridge/src/main.rs:1615).
- The three 2026-08-12 adjudication documents are not present at this exact commit. I treated the eight WRONGs in the supplied brief as confirmed requirements and used adjacent copies only as context, not checkout proof.
- I did not edit, build, test, invoke providers, use the network, or start helpers. All proposed gates below are future implementation gates, not results from this turn.
- LSP navigation was unavailable, so callers were traced by bounded symbol search and direct source inspection.

## Salvage rulings

| Mechanism | Ruling | Exact seam | Reason |
|---|---|---|---|
| `DedicatedRemoteRequestIdV1` wire identity and parser | KEEP | [process.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/process.rs:734) | Distinct request identity remains valid. Move entropy injection below canonical minting so callers cannot supply an arbitrary identity. |
| Backend-global `next_turn_authority` | KEEP | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:601) | Its monotonic backend-wide coordinate closes forget/recreate ABA when included in every capability comparison. |
| Exact request cancellation guard | KEEP | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:316) | It compares session/turn/request identity and can remain once the new durable authority is added. |
| `ApiLifecycle` and provider-error precedence | KEEP | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:73) | Structured `prompt_may_have_been_accepted` diagnostics are sound; custody, not vocabulary, is missing. |
| `BackendCleanupDispositionV1` and checked/observed backend surfaces | KEEP | [ports.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/ports.rs:439) | The vocabulary is sufficient. API overrides must prevent the trait defaults from manufacturing `Complete`. |
| Shared `RetainedResourceFlight` for process/container owners | KEEP | [retained_resource_flight.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1131) | Do not destabilize the accepted process/container ownership mechanism. It must cease serving remote requests. |
| `FileResourceFlightJournalV1` root custody | REVISE | [retained_resource_flight.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:570) | Its stored `PathBuf`, create-capable open, lock-path reopening, and path joins violate immutable-root custody. Preserve its wire format but move all operations behind pinned descriptors. |
| API active-request slot | REVISE | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:256) | A cleanup cell must be installed before leaving the session lock, before any journal or provider work. |
| `RequestScope` cleanup/drop | REPLACE | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:364) | Clearing the slot after ignoring settlement refusal destroys the only observer. Drop must transfer ownership to a bounded custodian. |
| `DurableRemoteRequestFlightV3`, `RemoteRequestSettlementV1`, and `bind_remote_request` | REPLACE | [process.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/process.rs:755), [process.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/process.rs:945) | Per-bind recovery, generic reservations, blocking joins, and a void publisher cannot satisfy live exclusion, bounded waiting, retirement, or crash-safe publication without changing unrelated process/container contracts. |
| Void publication hook on the shared flight | REPLACE for requests; KEEP dormant elsewhere | [retained_resource_flight.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1017) | A void call after durable terminal CAS has no durable acknowledgement. Remote requests need an outbox/ack contract. |
| Production `None` route | KEEP | [config.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/config.rs:10) | No task may arm V3, create a journal root, or alter LegacyV2 production behavior. |
| Worktree inner/outer cleanup split | KEEP, explicitly protected | [backend.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-worktree/src/backend.rs:681) | `CleanupReportV1 { result, checkout }` and `CleanupCellState::inner_disposition` preserve separate inner and checkout custody. They must not be collapsed. |

The replacement is one mechanism, not an artifact restart: remote requests stop adapting the shared retained-flight registry and instead use a dedicated request state machine. Existing API identity and error-handling work remains.

## Required authority and state machine

Introduce `crates/bridge-core/src/remote_request_flight.rs` with these public contracts:

```rust
pub struct RemoteRequestAttemptIdV3(/* canonical opaque random ID */);

pub struct RemoteRequestAuthorityV3 {
    pub attempt_id: RemoteRequestAttemptIdV3,
    pub ordinal: u64,
    pub request_id: DedicatedRemoteRequestIdV1,
}

pub struct RemoteRequestAttemptV3 { /* lease, journal, admission, publisher */ }

impl RemoteRequestAttemptV3 {
    pub fn open_recovered(
        root: JournalRootCustodyV1,
        attempt_id: RemoteRequestAttemptIdV3,
        capacity: NonZeroUsize,
        publisher: Arc<dyn RemoteRequestResultPublisherV3>,
    ) -> Result<Arc<Self>, RemoteRequestAttemptOpenErrorV3>;

    pub fn admit(
        &self,
        owner: ResourceFlightOwnerV1,
    ) -> Result<OwnedRemoteRequestV3, RemoteRequestAdmissionErrorV3>;
}
```

`open_recovered` is the only public constructor. There is no public `recover` method and admission never performs recovery.

### Owner and exclusion proof

`RemoteRequestAttemptV3` owns:

1. A pinned parent directory and pinned journal-root descriptor.
2. An exclusive, nonblocking lifetime `flock` on `remote-request-attempt.lock`, opened relative to the root descriptor.
3. The recovery pass.
4. The admission mutex and all request cleanup debt for that attempt.

Opening a second route while the first lease is live returns `AttemptLive`; it neither recovers nor admits. A process crash releases the kernel lease. The successor must acquire it and finish the complete recovery/outbox pass before `open_recovered` returns an object usable by `ApiBackend`.

Thus:

- Recovery has positive deadness/quiescence proof.
- Recovery and admission cannot overlap.
- Live request A cannot be terminalized by B binding a new request.
- Failure to complete recovery or publication means no route object and therefore no admission.

Lock order is fixed:

1. Lifetime attempt lease, already held.
2. Attempt admission mutex.
3. Per-request transition mutex.
4. Journal operation mutex/file lock.
5. Release every lock before invoking the publisher or diagnostic observer.

No API session-map lock may be held while acquiring a core lock or doing journal I/O.

### Durable transitions

Use versioned events:

```text
FlightReserved
  -> RemoteRequestIdentityCaptured
  -> IntentJournaled
  -> DispatchAuthorized
  -> ProviderSendArmed
  -> TerminalPendingPublication
  -> PublicationAcknowledged
  -> Retired
```

The reservation record and `FlightReserved` are one descriptor-relative atomic publication. There is no zero-row durable reservation.

`ProviderSendArmed` is appended immediately before the provider-send future receives its first poll. Merely constructing the future or reaching `DispatchAuthorized` is not acceptance. Conservatively, a crash after arming reports `prompt_may_have_been_accepted = true`, even if the provider did not actually observe the request.

Recovery mapping is mandatory:

| Last durable state | Recovery terminal | Accepted? | Publication |
|---|---|---:|---|
| Legacy zero-row reservation | Exact rollback after identity validation | false | None |
| `FlightReserved` | `Failed` | false | None; owner is not yet durable |
| `RemoteRequestIdentityCaptured` | `Failed` | false | Publish to durable owner |
| `IntentJournaled` | `Unknown` | false | Publish |
| `DispatchAuthorized` | `Unknown` | false | Publish |
| `ProviderSendArmed` | `Unknown` | true | Publish acceptance-aware diagnostic |
| `TerminalPendingPublication` | Preserve durable CAS winner | Recorded value | Replay idempotently |
| `PublicationAcknowledged` | No republish | Recorded value | Compact/retire |
| Invalid order, identity conflict, or corrupt record | Refuse entire attempt | Unknown | Preserve bytes; no admission |

Recovery never reconstructs provider authority and never resends a provider request.

### Durable outbox

The request publisher must not have a no-op default:

```rust
pub struct RemoteRequestPublicationIdV3 {
    pub authority: RemoteRequestAuthorityV3,
    pub owner: ResourceFlightOwnerV1,
}

pub struct RemoteRequestTerminalPublicationV3 {
    pub delivery_id: RemoteRequestPublicationIdV3,
    pub result: ResourceActionResultV1,
    pub prompt_may_have_been_accepted: bool,
    pub diagnostic: Option<RemoteRequestPersistenceDiagnosticV3>,
}

#[async_trait]
pub trait RemoteRequestResultPublisherV3: Send + Sync {
    async fn publish_idempotent(
        &self,
        publication: RemoteRequestTerminalPublicationV3,
    ) -> Result<RemoteRequestPublicationAckV3, RemoteRequestPublicationErrorV3>;
}
```

The acknowledgement must echo the exact delivery ID. The sink contract requires durable deduplication by that ID.

Crash cuts then behave as follows:

- Before terminal append: recovery creates the appropriate terminal.
- After `TerminalPendingPublication`, before publisher call: recovery publishes it.
- After sink commit, before local ack: recovery calls again; sink deduplication produces no second observable effect.
- After `PublicationAcknowledged`, before retirement: recovery only compacts.
- Refusal, mismatched acknowledgement, or timeout leaves the outbox pending and prevents new admission.

This promises exactly one observable sink effect, not exactly one method invocation. No observable contract is weakened.

### Bounded lifecycle and 4,096 capacity

A checkpoint stores:

```rust
struct RemoteRequestCheckpointV3 {
    format_version: u16,
    attempt_id: RemoteRequestAttemptIdV3,
    next_ordinal: u64,
    retired_floor: u64,
    completed_ahead: BTreeSet<u64>,
    chain_head: [u8; 32],
}
```

Rules:

- Admission uses checked arithmetic and requires
  `next_ordinal - retired_floor < capacity`.
- Production capacity is exactly 4,096; smaller injected capacities are test-only.
- Capacity is checked before minting a request ID, publishing a file, or constructing provider work.
- Acknowledged terminals enter `completed_ahead`.
- Contiguous acknowledged ordinals advance `retired_floor`.
- The checkpoint is descriptor-relatively replaced and parent-fsynced before corresponding request files are unlinked.
- A crash before checkpoint publication replays the acknowledged file; after checkpoint publication, leftover files are recognized as already retired.
- At 4,096 outstanding ordinals, the next admission returns a typed protective capacity refusal. It does not enumerate through an error or corrupt later recovery.
- A gap at the floor can consume the bounded window, but attempt-start recovery must close that gap. If recovery cannot publish/ack it, admission remains intentionally refused.
- V3 will not adopt the current artifact’s old request-journal roots. An old format returns `LegacyMigrationRequired` without mutation. This is acceptable because the production request route has never been armed. Any later migration is a separate operator-authorized task.

### Descriptor-relative root custody

Extend [fs_custody.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/fs_custody.rs:349) and [liveness.rs](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/liveness.rs:160):

```rust
pub struct JournalRootCustodyV1 {
    parent: Arc<PinnedDirectoryV1>,
    root: Arc<PinnedDirectoryV1>,
    child_name: JournalChildNameV1,
    parent_identity: DirectoryIdentityV1,
    root_identity: DirectoryIdentityV1,
}

impl JournalRootCustodyV1 {
    pub fn create_beneath(
        parent: Arc<PinnedDirectoryV1>,
        child: JournalChildNameV1,
    ) -> Result<Self, JournalRootCustodyErrorV1>;

    pub fn reopen_expected(
        parent: Arc<PinnedDirectoryV1>,
        child: JournalChildNameV1,
        expected: DirectoryIdentityV1,
    ) -> Result<Self, JournalRootCustodyErrorV1>;
}
```

Add descriptor-relative operations for create-new regular files, append/open-no-follow, atomic replacement, identity-checked unlink, bounded directory enumeration, and acquiring a persistent lock from an already-open file. Use the existing Unix `openat`/`renameat` style rather than adding a path-reopening convenience.

Before every mutation, verify:

- The root descriptor still matches its captured identity.
- `fstatat(parent_fd, child, AT_SYMLINK_NOFOLLOW)` still identifies that same root.
- The parent descriptor still matches its captured identity.

If the root is renamed, removed, or replaced:

- Existing descriptors are not redirected.
- The original directory is not silently recreated.
- Further mutation refuses with `RootIdentityChanged` or `RootUnlinked`.
- Cleanup projects `Unknown`.
- Restart against a replacement root fails the expected-identity check.

On non-Unix hosts, or Unix filesystems where the required persistent identity cannot be established, construction returns `RootCustodyUnavailable`. It must not fall back to path authority or silently select LegacyV2.

## Checked cleanup and drop custody

Install `Arc<RequestCleanupCellV3>` in `SessionState` while holding the session lock and before starting Legacy or V3 admission. The cell is keyed by backend-global turn authority and, once available, `RemoteRequestAuthorityV3`.

Required states:

```text
Idle
AdmissionPendingLegacy
AdmissionPendingV3
ActiveLegacy
ActiveV3
DropOwned
Terminal
SettlementRefused
TimedOut
```

An attempt-owned `ApiRequestCleanupCustodianV3` receives ownership before a request can be polled. `RequestScope::drop` only performs synchronous state transfer; it never clears the cell or ignores a result. The custodian owns:

- The settlement capability.
- The original structured lifecycle observer.
- The acceptance state or durable authority needed to reconstruct it.
- One immutable deadline set at admission.
- Any pending diagnostic/outbox debt.

Observation uses `tokio::sync::watch` or an equivalent async notification:

```rust
pub async fn observe_until(
    &self,
    deadline: tokio::time::Instant,
) -> RemoteRequestCleanupObservationV3;
```

There is no `spawn_blocking(join_blocking)`. Custodian work is pure async and owned in an attempt-level `JoinSet`. At its immutable deadline it records or durably retains the diagnostic, transitions to `TimedOut`, and exits. Attempt shutdown joins until the same bound, then aborts and drains pure-async tasks. No operating-system thread or blocking waiter survives the bound.

Projection is exact:

| Captured cleanup state | Checked result |
|---|---|
| No request authority ever installed | `Complete` |
| Legacy admission canceled and acknowledged before any request/send authority exists | `Complete` |
| Legacy admission active, request active, overlap terminal, drop, refusal, or timeout | `Unknown` |
| V3 admission before initial durable row and canceled with positive absence proof | `Complete` |
| V3 durable terminal `Complete` and matching publication ack | `Complete` |
| V3 terminal `Partial`, `Failed`, or `Unknown` | `Unknown` |
| V3 pending publication, refusal, timeout, or drop-owned debt | `Unknown` |
| Observer unavailable or diagnostic delivery lost | `Unknown` |

An idle Legacy session whose request finished and cleared before cleanup began may return `Complete`; a cleanup that captured an overlapping Legacy request may not infer durable completion afterward.

`ApiBackend` must override all four relevant surfaces:

- `forget_session_checked`
- `release_session_checked`
- `forget_session_observed`
- `release_session_observed`

Void `forget_session`/`release_session` may discard the returned disposition, but must first transfer the cell and observer to the custodian. Removing the session map entry must not remove cleanup debt. A recreated session receives a new backend-global authority, so stale A cannot clear or cancel B.

## Closure of the eight confirmed WRONGs

1. Recovery moves exclusively to `open_recovered`, behind an exclusive lifetime attempt lease and before route publication.
2. A preinstalled cleanup cell covers admission-before-slot. Legacy overlap and every unresolved V3 state project `Unknown`.
3. Drop transfers settlement, diagnostic, acceptance, and deadline to the custodian before clearing any local field.
4. Blocking `join_blocking` is removed from the request path; bounded observation is pure async and drained.
5. Recovery explicitly terminalizes `FlightReserved` and `RemoteRequestIdentityCaptured`, as well as all later prefixes.
6. Admission uses a retiring 4,096-wide ordinal window; acknowledged terminals compact, and the 4,097th outstanding request refuses before mutation.
7. `TerminalPendingPublication` plus an idempotent sink and durable acknowledgement closes every publication crash cut.
8. Journal creation, locking, reads, writes, replacement, enumeration, and removal are relative to pinned parent/root descriptors; replacement never becomes authority.

## Implementation sequence

Every task is sequential. `S0` is the exact current commit. For later tasks, `Sn` means the exact 40-hex commit produced by the preceding accepted task; it must be written into the next task specification before dispatch. A branch name or moving ref is not an acceptable frozen input.

Each task gets at most two review/fix rounds initially. At that cap, closed and shrinking findings may receive a disclosed extension; repeating open-class findings park the lane. Never replace the preserved branch with a fresh implementation.

### Task 1 — Descriptor custody foundation

- Frozen input: `S0 = 530992b7ff1e8e9151fb2a69e86f3ff71c44f905`.
- Owned files:
  - `crates/bridge-core/src/fs_custody.rs`
  - `crates/bridge-core/src/liveness.rs`
  - `crates/bridge-core/src/retained_resource_flight.rs`
  - Direct `FileResourceFlightJournalV1::open` test callers in `process.rs`, `reaper.rs`, `bridge-api/src/backend.rs`, `bridge-acp/src/acp_backend.rs`, and `bridge-container/src/lib.rs`
  - The existing 3c2 implementer handoff
- Before: file-journal operations repeatedly resolve a mutable path.
- After: callers supply `JournalRootCustodyV1`; the journal retains descriptors and captured identities, never creates or reopens its root by path, and its wire records remain unchanged.
- Red-first deterministic tests:
  - `file_journal_root_rename_replace_never_redirects`
  - `file_journal_removed_root_never_recreates`
  - `file_journal_reopen_rejects_expected_identity_mismatch`
  - `persistent_lock_from_open_child_survives_path_replacement`
- Implementation order:
  1. Add descriptor child primitives and tests.
  2. Add open-file lock acquisition.
  3. Add `JournalRootCustodyV1`.
  4. Change `FileResourceFlightJournalV1` storage.
  5. Update every constructor/test double.
- Focused gates:
  - `cargo test -p bridge-core fs_custody`
  - `cargo test -p bridge-core liveness`
  - `cargo test -p bridge-core retained_resource_flight`
- Stop/split: stop before journal migration if Unix primitives exceed 600 production lines or require separate implementations larger than 300 lines per supported OS. Land primitives first; do not create a compatibility path wrapper.
- Commit boundary: only after common whole-tree gates pass.

### Task 2 — Request journal, admission, and retirement

- Frozen input: exact `S1` from Task 1.
- Owned files:
  - New `crates/bridge-core/src/remote_request_flight.rs`
  - `crates/bridge-core/src/lib.rs`
  - `crates/bridge-core/src/resource_flight.rs` exports only
  - Handoff
- Before: request reservations use the generic nonretiring registry and a 4,096-entry failing census.
- After: the new journal atomically publishes the initial record, uses ordinal-window capacity, and compacts acknowledged terminals.
- Red tests:
  - `initial_reservation_and_flight_reserved_are_one_commit`
  - `capacity_refuses_4097th_outstanding_before_id_mint`
  - `acked_terminals_allow_more_than_4096_sequential_requests`
  - `out_of_order_ack_never_advances_floor_past_gap`
  - `checkpoint_before_unlink_is_restart_safe`
  - checked-overflow tests for ordinal arithmetic
- Implementation order:
  1. Add compiling skeleton returning typed `Unsupported`; demonstrate red tests.
  2. Add authority/events and decoder validation.
  3. Add initial atomic publication.
  4. Add checkpoint/window accounting.
  5. Add ack retirement and crash-safe compaction.
- Focused gate: `cargo test -p bridge-core remote_request_flight::tests::admission`.
- Stop/split: 700 production or 800 test lines. If compaction crosses the limit, split it into Task 2b before adding consumers.
- Dependency: Task 1.
- Commit boundary: full gates green; no API call site changes yet.

### Task 3 — Quiescent recovery and outbox/ack

- Frozen input: exact `S2`.
- Owned files: `remote_request_flight.rs`, its exports/tests, handoff.
- Before: no attempt lease, prefix-complete recovery, or durable publication acknowledgement.
- After: `open_recovered` owns lifetime exclusion, performs full prefix recovery, and returns only after every terminal outbox is acknowledged.
- Red tests:
  - A table-driven crash test for every durable prefix in the recovery table.
  - `second_open_while_attempt_live_never_recovers_or_admits`
  - `recovery_finishes_before_first_admission`
  - `crash_before_publish_replays_once`
  - `crash_after_sink_commit_before_ack_has_one_observable_effect`
  - `mismatched_ack_blocks_route_and_preserves_outbox`
  - `corrupt_or_legacy_root_refuses_without_mutation`
- Implementation order:
  1. Add publisher/ack types with no default implementation.
  2. Add lifetime lease.
  3. Add terminal-pending and ack events.
  4. Implement recovery state table.
  5. Add idempotent test publisher with durable call/effect counters.
  6. Add retirement after ack.
- Focused gates:
  - `cargo test -p bridge-core remote_request_flight::tests::recovery`
  - `cargo test -p bridge-core remote_request_flight::tests::outbox`
- Stop/split: 800 production or 900 test lines; split recovery and outbox before either acquires a consumer.
- Dependency: Task 2.
- Commit boundary: full gates green; route remains unreachable.

### Task 4 — Core remote-request driver and bounded observation

- Frozen input: exact `S3`.
- Owned files:
  - `remote_request_flight.rs`
  - `process.rs`
  - `resource_flight.rs` exports
  - Corresponding core tests and handoff
- Before: `bind_remote_request` performs per-bind recovery; settlement exposes blocking join; Drop ignores refusal.
- After: `RemoteRequestAttemptV3::admit` returns `OwnedRemoteRequestV3`; request transitions are durable; observation is async and deadline-bound.
- Remove/replace:
  - `DurableRemoteRequestFlightV3`
  - `RemoteRequestSettlementV1::join_blocking`
  - `DurableProcessFlightAttemptV3::bind_remote_request`
- Red tests:
  - `live_request_survives_peer_admission`
  - `settlement_timeout_leaves_zero_live_waiters`
  - `drop_refusal_retains_terminal_and_diagnostic_debt`
  - `send_armed_not_dispatch_authorized_controls_acceptance`
  - `terminal_result_returns_durable_cas_winner`
- Implementation order:
  1. Add `OwnedRemoteRequestV3` transition API.
  2. Add watch-based observation.
  3. Add drop/settlement debt state.
  4. Remove the old request adapter.
  5. Preserve generic process/container retained-flight APIs.
- Focused gates:
  - `cargo test -p bridge-core remote_request_flight`
  - `cargo test -p bridge-core process::tests`
- Stop/split: 650 production or 800 test lines. Any required change to process/container settlement semantics stops the task for a separate ownership review.
- Dependency: Task 3.
- Commit boundary: full gates green; API still does not consume the new route.

### Task 5 — API cleanup cell, custodian, and route migration

- Frozen input: exact `S4`.
- Owned files:
  - `crates/bridge-api/src/backend.rs`
  - `crates/bridge-api/src/config.rs`
  - `crates/bridge-api/src/lib.rs`
  - API tests and handoff
- Before: admission occurs before slot publication; cleanup can return false `Complete`; Drop clears after ignored settlement; send acceptance is not durably coordinated.
- After:
  - `ApiConfig.resource_flight_route_v3` holds `Option<Arc<RemoteRequestAttemptV3>>`.
  - Cleanup cell is installed before leaving the session lock.
  - `RequestScope` transfers to the custodian.
  - All checked/observed cleanup overrides use the exact projection matrix.
  - `ProviderSendArmed` is written at the first-poll boundary.
- Red tests:
  - Matrix tests for LegacyV2 and ProtectedV3 covering admission-before-slot, active, terminal, refusal, timeout, and drop.
  - `legacy_active_cleanup_never_returns_complete`
  - `v3_complete_requires_terminal_and_matching_publication_ack`
  - `drop_refusal_preserves_acceptance_aware_diagnostic`
  - `forget_recreate_stale_a_cannot_clear_or_cancel_b`
  - `cleanup_between_rounds_prevents_next_send`
  - `zero_round_request_never_mints_or_admits`
- Implementation order:
  1. Add cleanup cell and custodian with route still unused.
  2. Add four checked/observed overrides.
  3. Migrate admission to the new attempt.
  4. Move send-armed transition to first poll.
  5. Replace `RequestScope` Drop.
  6. Delete old API request-flight glue and entropy injection seam.
- Focused gates:
  - `cargo test -p bridge-api cleanup`
  - `cargo test -p bridge-api resource_flight`
  - `cargo test -p bridge-api`
- Stop/split: 900 production or 1,000 test lines, or 1,000 production lines in `backend.rs`. If exceeded, land cleanup-cell/overrides first with route still unreachable, then migrate HTTP execution.
- Dependency: Task 4.
- Commit boundary: full gates green. Production remains `None`; only injected tests reach V3.

### Task 6 — Consumer closure and binding shield

- Frozen input: exact `S5`.
- Owned files:
  - `crates/bridge-workflow/src/executor.rs`
  - Its cleanup/retry test doubles
  - Regression-only tests in `bridge-worktree/src/backend.rs`
  - `bin/a2a-bridge/src/main.rs` assertion/test only
  - Handoff
- Before: [cleanup_cold_session](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-workflow/src/executor.rs:1346) erases the disposition into `Result<()>`; retry code can treat `Ok(Unknown)` as permission to continue.
- After:

```rust
async fn cleanup_cold_session(...)
    -> Result<BackendCleanupDispositionV1, AgentError>;
```

Retry continues only on `Complete`. `Retained`, `Preserved`, `Unknown`, or `Failed` ends the retry path with a structured cleanup refusal and cannot redispatch provider work.

- Red tests:
  - `retry_does_not_redispatch_after_cleanup_unknown`
  - `post_acceptance_persistence_failure_is_fatal_and_not_retried`
  - `worktree_complete_checkout_preserves_inner_unknown`
  - `worktree_inner_complete_does_not_erase_checkout_refusal`
  - `production_api_config_keeps_resource_flight_route_none`
- Implementation order:
  1. Make the executor return the disposition.
  2. Gate retry on exact `Complete`.
  3. Audit inbound, smoke, workflow tracking, and Worktree projections for vocabulary preservation.
  4. Add structural regression tests for the two-field split.
  5. Review the complete `S0..S6` production diff.
- Focused gates:
  - `cargo test -p bridge-workflow cleanup`
  - `cargo test -p bridge-workflow retry`
  - `cargo test -p bridge-worktree cleanup`
  - `cargo test -p a2a-bridge resource_flight_route`
- Stop/split: 450 production or 600 test lines. If an additional consumer collapses protective dispositions, split one consumer per task; do not batch unrelated wrapper changes.
- Dependency: Task 5.
- Commit boundary: full gates green, combined-diff review approved, and 3d remains blocked until this commit lands.

## Common whole-tree gate and commit boundary

After every task, before its single commit:

```bash
cargo fmt --all -- --check
git diff --check
CARGO_INCREMENTAL=0 cargo check --workspace --all-targets --all-features --locked
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
CARGO_INCREMENTAL=0 cargo test --workspace --all-features --locked
CARGO_INCREMENTAL=0 cargo build --release --bin a2a-bridge --locked
cargo deny check
target/release/a2a-bridge validate --repo-hygiene
```

No gate may fetch or update dependencies. If a gate is not runnable in the implementation environment, the task is not green: record the exact exclusion and do not commit or advance the frozen input.

Each handoff refresh must record the exact head, exact commands, totals, and exclusions. Historical handoff totals are supplied claims, not current verification.

## Mandatory binding carry-forward

This slice must not arm or redesign `ContainerRw`. It preserves:

```rust
CleanupReportV1 {
    result: inner_disposition,
    checkout: checkout_disposition,
}
```

and the separate `CleanupCellState::inner_disposition`. The later slice that first arms production V3 or wraps `ContainerRw` must carry both fields independently through persistence and terminal projection. Only final projection may combine them, and only `Complete + Complete` may become `Complete`.

Task 6 adds regression shields but no production arming. The roadmap must continue to show 3d blocked until the final 3c2 commit is landed and aggregate-reviewed.

## Owner decisions

The design selects recommended defaults so none blocks execution, but changing one reopens design review:

1. Use a separate request journal/state machine rather than extending the shared retained-flight schema.
2. Treat `IntentJournaled` and `DispatchAuthorized` recovery as `Unknown` with acceptance false.
3. Require an idempotent durable result/diagnostic sink; do not permit a no-op publisher.
4. Fix production capacity at 4,096 outstanding ordinals and refuse rather than evict unresolved authority.
5. Refuse old request-journal roots instead of attempting an implicit migration.
6. Fail closed where durable Unix directory identity cannot be established.

## Residual SMELL/DEFER items

- SMELL/DEFER: The generic process/container void publisher retains a terminal-to-publication gap. The new request path no longer uses it, and production request V3 remains unarmed. Its owners should address it before any later activation relying on exactly-once process/container publication.
- SMELL/DEFER: Descriptor-relative synchronous journal I/O may affect Tokio latency. No incorrect output is demonstrated. Measure before arming; do not “solve” it with detached blocking workers.
- SMELL/DEFER: A permanently refusing publisher intentionally holds the 4,096 window closed. That is protective unavailability, not false success. Operator tooling for inspection/retry belongs in a later, separately authorized slice.
- SMELL/DEFER: The current positive tests do not prove the new contracts. The red tests above are acceptance gates, not optional coverage.

## Unsalvageability threshold

The preserved artifact would become genuinely unsalvageable only if implementation proves one of these mechanism-level conditions:

- Backend-global turn authority cannot be retained without changing public session identity semantics.
- The API request future cannot expose a first-poll boundary, making conservative send acceptance impossible.
- The configured result/diagnostic destination cannot provide durable idempotency by delivery ID and the owner refuses to weaken the exactly-once observable promise.
- Supported hosts cannot provide descriptor-relative open/rename/unlink plus a persistent directory identity.
- Separating remote requests from the shared retained-flight registry requires invasive process/container behavior changes despite the currently dormant route.
- The task cut repeatedly exceeds its stop thresholds because authority is inseparable across modules.

Current source evidence establishes none of those conditions.

DESIGN LENS: READY

Unresolved blockers: none.
