# R2f1b 3c2 API request-flight salvage redesign

Date: 2026-08-12

Status: **FULL TEN-TASK OPTION EVALUATED; A1 APPROVED AND RETAINED; OWNER PATH
DECISION PENDING; A2 NOT AUTHORIZED**

Landed base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

Preserved artifact: `feat/r2f1b-3c2-api-authority` at
`530992b7ff1e8e9151fb2a69e86f3ff71c44f905`

Design-record branch input: `d698e6f02f3229da3787dbc2a8630c03cb8b25df`

Historical execution update (2026-08-13): A1 candidate `bc262ad4` passed the
full host gate but its capped closure review found three operator-confirmed
BLOCKER WRONGs. That candidate was preserved rather than restarted.

Current execution update (2026-08-14): the separately authorized continuation
at `5cbeea1e` fixed the remaining A1 findings and received closure approval. A1
is retained and not integrated. This document now records the fully hardened
ten-task option that was evaluated; it is not an owner decision to execute that
whole option. The [rescope evaluation](../reviews/2026-08-13-r2f1b-reliability-rescope-evaluation.md)
records the smaller alternatives and rejects stopping after A1 or A2 as a
standalone endpoint. A2 has exact possible input `5cbeea1e` but no dispatch
authorization. See the current
[A1 adjudication](../reviews/2026-08-14-r2f1b-3c2-task-a1-owner-extension-adjudication.md).

This plan is the design escalation required by the repaired-tail adjudication.
It salvages the accepted identity, cancellation, lifecycle, diagnostic, and
HTTP work from the preserved artifact. It replaces only the still-unarmed
remote-request adaptation of the shared retained-flight mechanism. It does not
restart the slice, alter landed process/container semantics, arm production V3,
advance 3d, run a provider, or create a production journal root.

## Binding scope and non-scope

Keep from `530992b7`:

- `DedicatedRemoteRequestIdV1` and bridge-owned canonical minting;
- backend-global, non-rewinding turn authority and all stale-scope comparisons;
- exact request cancellation, the between-round cancellation fence, and the
  acceptance-aware diagnostic/error mapping;
- the first-send installation boundary and the repaired terminal projection;
- `BackendCleanupDispositionV1`, checked/observed backend surfaces, and the
  dormant `ApiConfig.resource_flight_route_v3 = None` production default.

Replace from `530992b7`:

- `DurableRemoteRequestFlightV3`, `RemoteRequestSettlementV1`, and
  `DurableProcessFlightAttemptV3::bind_remote_request`;
- request-key reservation, recovery, and publication branches added to
  `ResourceFlightRegistryV1` and `FileResourceFlightJournal`;
- the API request scope's ignored-drop settlement and blocking cleanup join;
- workflow retry logic that treats `Ok(Unknown)` as cleanup permission.

Do not change the shared process/container flight grammar or behavior merely to
serve requests. Request-specific additions to that shared grammar are removed
after the replacement route compiles. The pre-existing generation-flight file
journal remains outside this slice unless a red regression proves that the
request replacement itself depends on changing it.

## Authority model

Introduce `crates/bridge-core/src/remote_request_flight.rs`. Its public surface
is deliberately separate from `DurableProcessFlightAttemptV3`:

```rust
pub struct RemoteRequestAttemptIdV3(/* canonical opaque value */);

pub struct RemoteRequestAuthorityV3 {
    pub attempt_id: RemoteRequestAttemptIdV3,
    pub ordinal: u64,
    pub request_id: DedicatedRemoteRequestIdV1,
}

pub struct RemoteRequestAttemptV3 { /* lifetime lease, journal, admission */ }

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

`open_recovered` is the only production-capable constructor. It acquires an
exclusive nonblocking lifetime lock on an already-open child of the pinned
attempt root, completes recovery and publication debt, then exposes admission.
A second live opener returns `AttemptLive` without recovery or mutation.
Admission never runs recovery.

The total lock order is:

1. lifetime attempt lease, already held;
2. attempt admission/retirement mutex;
3. per-request transition mutex;
4. descriptor-relative journal operation lock;
5. release every lock before publisher or diagnostic callbacks.

No API session-map lock may be held during journal I/O, a core transition, a
publisher call, or a diagnostic callback.

## Descriptor-root contract

`JournalRootCustodyV1` reuses `PinnedDirectoryV1` and retains both the pinned
parent and pinned root plus their captured identities and the root's exact child
name. It supplies descriptor-relative create-new, append/open-no-follow,
atomic replace, bounded enumeration, identity-checked unlink, directory sync,
and persistent-lock acquisition. Before every mutation it proves that both open
descriptors still match their captured identities and that the parent's child
entry still names the same root object.

Root removal, rename, or replacement never redirects an existing route and
never recreates the root. It returns a typed custody refusal and projects
`Unknown`. Configuring request V3 on a host/filesystem without the required
identity and descriptor operations refuses; it never silently falls back to
LegacyV2. Production remains unaffected because the route stays `None`.

## Request journal and bounded retirement

One request child is the reservation and journal. There is no separate durable
reservation file and therefore no zero-row reservation in the new format. The
initial complete row is written to a private temporary child, synced, renamed
no-replace to its final authority name, and followed by a root sync. Only then
may the attempt checkpoint advance and the authority be returned.

The checkpoint contains the schema, exact attempt ID, `next_ordinal`, and a
chain/identity digest. Admission, under the attempt mutex:

1. validates the checkpoint and bounded child census;
2. refuses before ID mint or mutation if 4,096 active request children exist;
3. allocates by checked arithmetic from the checkpoint and active maximum;
4. atomically publishes the initial child containing authority and owner;
5. atomically advances and syncs `next_ordinal`;
6. returns the non-cloneable request authority only after both publications.

If step 5 fails, no authority returns; reopen advances the checkpoint from the
validated child and closes it as a pre-send failure. Acknowledged terminal
children are unlinked and the root is synced. A crash before unlink sees the
ack and retires without republishing; a crash after unlink has no remaining
debt. Enumeration reads at most capacity plus one, so a corrupt over-cap root
refuses explicitly rather than silently dropping entries. Old 3c2 request
journal roots return `LegacyMigrationRequired` without mutation because no
production writer was ever armed.

## Durable states and crash recovery

The new request-specific events are:

```text
Reserved(authority, owner)
  -> IntentJournaled
  -> DispatchAuthorized
  -> ProviderSendArmed
  -> TerminalPendingPublication(result, prompt_may_have_been_accepted)
  -> PublicationAcknowledged(delivery_id)
  -> retired by exact-child unlink
```

`ProviderSendArmed` is appended immediately before the provider-send future's
first poll, not when the future is constructed. The implementation must wrap
the future so no poll is possible before that row is durable.

| Last durable state | Recovery result | Accepted | Action |
|---|---|---:|---|
| private temp only | none | false | validate and remove the non-authoritative temp |
| `Reserved` | `Failed` | false | append terminal, publish, ack, retire |
| `IntentJournaled` | `Failed` | false | append terminal, publish, ack, retire |
| `DispatchAuthorized` | `Failed` | false | append terminal, publish, ack, retire |
| `ProviderSendArmed` | `Unknown` | true | append terminal, publish, ack, retire |
| `TerminalPendingPublication` | durable CAS winner | recorded | replay idempotently, ack, retire |
| `PublicationAcknowledged` | durable CAS winner | recorded | retire without republish |
| invalid order/identity/schema | none | unknown | refuse the entire attempt; preserve bytes |

The three pre-send states use `Failed`, not `Unknown`, because the first-poll
fence positively proves that provider code received no poll. Recovery never
reconstructs or resends provider authority.

## Publication contract

Requests do not use the shared void publisher. They require:

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

pub trait RemoteRequestResultPublisherV3: Send + Sync {
    fn publish_idempotent(
        &self,
        publication: &RemoteRequestTerminalPublicationV3,
    ) -> Result<RemoteRequestPublicationAckV3, RemoteRequestPublicationErrorV3>;
}
```

There is no no-op implementation. The acknowledgement must echo the exact
delivery ID; mismatch or refusal leaves the terminal outbox pending and blocks
admission from the reopened attempt. The sink durably deduplicates on delivery
ID. This preserves exactly one observable sink effect while allowing repeated
method calls after a crash; it does not weaken the observable contract.

Slice 5 remains the production writer of `NodeCleanupRecordV2.collateral` and
must implement that idempotence before any production request route is armed.

## API cleanup and drop custody

`ApiBackend` owns an `ApiRequestCleanupCustodianV3`. A cleanup cell keyed by the
backend-global turn authority is installed in `SessionState` while holding the
session lock, before Legacy or V3 admission leaves that lock. The cell is later
bound to the exact request authority. Its closed states cover:

```text
AdmissionPendingLegacy | AdmissionPendingV3
ActiveLegacy | ActiveV3
DropOwned | Terminal | SettlementRefused | TimedOut
```

`RequestScope::drop` first transfers the cell, settlement authority, acceptance
state, observer, and immutable cleanup deadline to the custodian. It never
clears the slot after ignoring a result. The custodian may retry a refused local
settlement only within the same request authority and never redispatches a
provider effect. The durable request prefix remains the crash backstop.

Observation is async (`tokio::sync::watch` or equivalent), deadline-bound, and
leaves no blocking worker or OS thread after timeout. The generic
`RetainedResourceFlight::join_blocking` remains unchanged for its existing
process caller; it is not used by requests.

Checked projection is exact:

| Captured state | Result |
|---|---|
| no request/admission authority existed | `Complete` |
| Legacy admission canceled with positive pre-send absence proof | `Complete` |
| overlapping Legacy admission/request/drop/refusal/timeout | `Unknown` |
| V3 canceled before initial durable child with positive absence proof | `Complete` |
| V3 terminal `Complete` plus matching publication ack | `Complete` |
| V3 `Partial`, `Failed`, `Unknown`, pending publication, refusal, or timeout | `Unknown` |

All four checked/observed forget/release surfaces use this cell. Removing the
session map entry cannot remove cleanup debt, and immediate same-ID recreation
uses a new backend-global authority. Void cleanup may discard the final
disposition, but it must perform the same custody transfer first.

`cleanup_cold_session` returns the exact backend disposition after recording it.
Only `Complete` authorizes retry. `Retained`, `Preserved`, `Unknown`, or an error
terminates retry with structured cleanup refusal; no later provider request is
dispatched.

## Compile-correct implementation tasks

Every task begins from the exact committed output of its predecessor. Before
dispatch, replace `S1`...`S6` below with the predecessor's 40-hex commit. A
branch name or moving ref is not a frozen input. Each task writes the lane
handoff before its commit and leaves the whole tree green.

### A1-A4. Descriptor primitives and journal-root custody

The first implementation attempt is preserved at exact retained commit
`517703cbd2e469bf208f20a36248169536bca8b3`. Its review cap exposed an open-class
route/namespace-CAS family, so it is the salvage input rather than accepted
delivery. The binding custody adjudication is
[`2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`](../reviews/2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md).

Task A executes as four sequential, individually green and reviewed commits:

1. **A1 - identity, names, and no-replace capture foundations.** Start from exact
   `517703cb`; add required object/content identity separation, bounded reversible
   reserved names, immutable intent grammar, and policy-neutral no-replace
   capture/restore classification. Stop at 200 production / 450 total.
2. **A2 - trusted route binding and sibling operation lease.** Start from exact
   A1; bind trusted anchor -> parent -> root plus the exact sibling lock object,
   acquire/flock/re-prove before returning an owned operation, and remove
   revalidate/path projection as authority. Stop at 220 / 500.
3. **A3 - capture settlement and bounded crash recovery.** Start from exact A2;
   add distinct replace/retire/stage/intent namespaces, replace rollback, retire
   roll-forward, recovery tickets, and the protective outcome lattice. Stop at
   320 / 700.
4. **A4 - owned journal API and broken-method deletion.** Start from exact A3;
   wire stage/publish/append/replace/retire/read/enumerate/sync through the owned
   operation value, make retained debt write-blocking, delete the candidate's
   raw writable-file/plain-replace/name-unlink/free-standing-lock APIs, and
   restore lock-fd privacy. Stop at 280 / 650.

The A1-A4 aggregate stops at 700 production / 1,500 total changed lines relative
to `S0`. A cap breach parks before more code or before B; it never authorizes a
path, exchange, replacing-rename, link/copy, or unchecked-unlink fallback.

Task A remains scoped to the new request journal. Do not migrate the shared
generation journal, worktree custody, `local_file`, either reaper, or recursive
directory removal. Those callers cannot construct Task A's `Complete` proof.

The binding red schedules include parent and root replacement at the actual
pre-syscall/flock boundary; A/B target substitution before capture; takeover of
the freed target; reserved-name substitution before cleanup; independent flock
contention on the renamed original inode; every intent/capture/publish/sync/
cleanup crash cut; and simultaneous proof that crashed replacement rolls back
while crashed retirement rolls forward. Required birthtime absence and runtime
primitive refusal return typed `Unsupported` with no fallback.

Focused gates are the exact A1-A4 modules/selectors from the custody
adjudication. Each cut also runs the common full gate below and writes the lane
handoff before its one commit.

### B. Request journal, atomic admission, and bounded retirement

- Frozen input: exact accepted A4 commit (`S1`).
- Own: new `remote_request_flight.rs`, `bridge-core` exports, tests, handoff.
- Implement the request child/checkpoint grammar, atomic initial publication,
  4,096-active cap, checked ordinal allocation, ack retirement, and strict
  decoding. Consume the full Task A namespace outcome: only `Complete` may
  advance the checkpoint or acknowledge retirement; `Retained`, `Unknown`, or
  `Unsupported` blocks the attempt. The module is unreachable outside tests.
- Red tests: no zero-row reservation at every admission crash cut; capacity
  refuses before ID mint/mutation; more than 4,096 sequential acknowledged
  requests succeed; corrupt/over-cap census refuses; checkpoint-before-return
  and ack-before-unlink restart schedules self-heal.
- Focused gate: `cargo test -p bridge-core remote_request_flight::tests::journal`.
- Stop/split: 500 production or 900 total. If retirement cannot fit, land the
  journal grammar first and name B2 before any consumer is added.

### C. Attempt lease, complete recovery, and outbox acknowledgement

- Frozen input: exact `S2`.
- Own: `remote_request_flight.rs`, focused tests, handoff.
- Add the lifetime lease, only `open_recovered`, the complete recovery table,
  idempotent publisher/ack types, and admission gating on recovered state.
- Red tests: every durable prefix; second live opener cannot recover/admit;
  recovery precedes admission; crash before publisher; sink commit before ack;
  mismatched ack; old/corrupt root refuses without mutation.
- Focused gates: request recovery and outbox modules.
- Stop/split: 500 production or 900 total. Split recovery and outbox if either
  would exceed the limit; do not expose admission between them.

### D. Owned request driver and bounded observation

- Frozen input: exact `S3`.
- Own: `remote_request_flight.rs`, narrow core exports/tests, handoff.
- Add `OwnedRemoteRequestV3`, durable transition methods, first-poll admission
  token, durable-CAS-winner settlement, async watch observation, and refusal
  debt. Do not remove the old API adapter yet.
- Red tests: peer admission cannot affect a live request; pre-poll recovery is
  Failed/accepted=false; post-arm recovery is Unknown/accepted=true; timeout
  leaves zero live waiters; settlement returns the durable winner; drop retains
  refusal debt.
- Focused gate: `cargo test -p bridge-core remote_request_flight`.
- Stop/split: 450 production or 850 total. Any necessary process/container
  semantic change stops for ownership adjudication.

### E. API cleanup cell and exact checked-cleanup projection

- Frozen input: exact `S4`.
- Own: `crates/bridge-api/src/backend.rs`, API tests, handoff.
- Preinstall the cleanup cell for Legacy and the existing injected V3 route,
  add the custodian/observation path, and override all checked/observed cleanup
  methods. The old request adapter still compiles and production remains `None`.
- Red tests: active Legacy never claims Complete; cleanup in the bind/publication
  window is Unknown; terminal-refusal debt survives slot removal; drop retains
  acceptance-aware persistence diagnostics; proven completed work does not taint
  later independent cleanup; forget/recreate stale A cannot touch B.
- Focused gate: `cargo test -p bridge-api cleanup` plus full `bridge-api`.
- Stop/split: 500 production or 900 total. Split Legacy/cell foundation from V3
  observation before touching HTTP execution if the cap is reached.

### F. Migrate API request execution and remove the shared-flight adapter

- Frozen input: exact `S5`.
- Own: API config/backend/lib, new request module, request-specific sections of
  process/resource/retained-flight/reaper tests, Cargo manifests if required,
  handoff.
- Change the injected route to `Arc<RemoteRequestAttemptV3>`, wrap the actual
  send future so `ProviderSendArmed` is durable immediately before first poll,
  and migrate every V3 test. Remove the old remote request driver and revert
  request-only reservation/recovery/operation-lock/publication additions to the
  shared process/container flight core. Preserve the 3c2 identity, ABA,
  cancellation, lifecycle, and post-acceptance error repairs.
- Red tests: zero-round/no-poll does not mint/admit; every send/error/SSE/unary
  terminal path has the expected durable result; first-poll fence controls
  acceptance; cancellation between rounds prevents the successor send; all old
  request adapter symbols have zero references; process/container focused tests
  remain unchanged.
- Focused gates: full `bridge-api`, remote-request core, process, reaper, ACP,
  and container focused suites touched by fixture migration.
- Stop/split: 500 production or 900 total. If HTTP migration and old-adapter
  removal cannot fit, land migration with the old adapter private/unreferenced,
  then remove it in F2 before any review of the aggregate.

### G. Protective disposition consumers and reconciliation shields

- Frozen input: exact `S6`.
- Own: `crates/bridge-workflow/src/executor.rs`, affected workflow/worktree test
  doubles, production-route assertion, roadmap, handoff.
- Return the exact cleanup disposition from `cleanup_cold_session`; gate retry
  on exact `Complete`; enumerate all callers and wrappers. Add guards that V3
  remains unarmed and the two-field ContainerRw cleanup contract is unchanged.
- Red tests: `Ok(Unknown)` cannot redispatch; post-acceptance persistence failure
  is fatal/nonretryable; worktree inner/checkout outcomes remain separate;
  production API route remains `None`.
- Focused gates: cleanup/retry tests in `bridge-workflow`, cleanup tests in
  `bridge-worktree`, and the production-route assertion.
- Stop/split: 350 production or 700 total. If another consumer collapses a
  protective disposition, split one consumer per task.

## Common gate, review, and convergence contract

After every task, before its one commit:

```bash
git diff --check
cargo fmt --all -- --check
CARGO_INCREMENTAL=0 cargo check --workspace --all-targets --all-features --locked
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
CARGO_INCREMENTAL=0 cargo test --workspace --all-features --locked
CARGO_INCREMENTAL=0 cargo build --release --bin a2a-bridge --locked
cargo deny check
target/release/a2a-bridge validate --repo-hygiene
```

No gate fetches or updates dependencies. Record totals, exclusions, and a
same-environment base control for any attributed failure. A red gate blocks the
commit and next frozen input; it is not rebaselined.

Per task, the declared review cap is one independent implementation review. A
closed enumerable rejection permits one targeted repair on the same artifact
and one closure review. At that cap, a shrinking nonrepeating population may
receive only a disclosed operator extension; repeated/open-class findings park
the task for design. Never restart from a fresh implementation distribution.

After G, run one aggregate dual-lens round on the exact combined diff: Sol/xhigh
for concurrency/custody correctness and Fable/Opus xhigh for release,
compatibility, rollback, and cross-slice authority. Each lens gets one completed
pass and no automatic retry after prompt start. Fold only after both reports are
operator-adjudicated and all required gates are green on the exact candidate.

## Binding carry-forward and done condition

The two-field cleanup split remains mandatory in whichever later slice first
arms production V3 or wraps `ContainerRw`:

```rust
CleanupReportV1 {
    result: inner_disposition,
    checkout: checkout_disposition,
}
```

Both fields travel independently through persistence and terminal projection;
only `Complete + Complete` may project `Complete`.

3c2 is done only when A1-A4 plus B-G (ten implementation tasks) are individually
committed and reviewed, the exact
aggregate diff passes the full gate, both aggregate lenses are adjudicated,
the fold is byte-identical to the gated tree, and CI is green after landing.
Until then 3c2 remains unarmed and 3d remains blocked.
