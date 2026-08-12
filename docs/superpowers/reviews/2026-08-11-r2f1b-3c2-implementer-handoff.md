# R2f1b 3c2 implementer handoff — API request-flight authority

Date: 2026-08-12
Base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

## Outcome

Every HTTP POST/tool round issued by `ApiBackend` now owns a distinct,
attempt-registry-reserved `DedicatedRemoteRequest` flight when an injected V3
route is present. Cancellation owns two separate authorities:

- a monotonic turn epoch, which makes cancellation sticky between A settlement
  and B publication; and
- a request-local watch sender guarded by exact `(turn_epoch, request_identity)`
  comparison, which prevents a retained A control/drop/settlement from
  signaling or clearing B.

Without the optional route the backend remains `LegacyV2`. The production API
constructor explicitly assigns `resource_flight_route_v3 = None`; this slice
does not arm `AutomaticR2f1b`, alter watchdogs, or trigger the 3c1 container
cleanup-composite carry-forward.

## Measured anchors and changed paths

The base had one session-wide watch sender reused by every tool round, no
request-ID writer, `LegacyV2` API exposure, and no production API attempt
route. The implementation changes:

- `crates/bridge-core/src/resource_flight.rs`: validated
  `DedicatedRemoteRequestIdV1`.
- `crates/bridge-core/src/retained_resource_flight.rs`: typed request keys,
  exact request owner/identity evidence, lifecycle accounting, strict wire
  tests, and mismatch/capacity negatives.
- `crates/bridge-core/src/process.rs`: non-cloneable request flight plus
  attempt-owned request binding using the attempt's existing registry, file
  journal, clock, and publisher.
- `crates/bridge-api/src/config.rs`: optional
  `ApiResourceFlightRouteV3 { attempt, node_id }`.
- `crates/bridge-api/src/backend.rs`: per-turn/per-request cancellation,
  request admission/settlement across the full HTTP census, V3 exposure and
  attachment, and deterministic Wiremock tests.
- `crates/bridge-api/src/lib.rs`: exports the route and injectable ID source.
- `crates/bridge-api/Cargo.toml`: test-only `tempfile` dependency already
  used elsewhere in the workspace.
- `bin/a2a-bridge/src/main.rs`: explicit production `None`.

The measured source/manifest diff is 1,812 insertions and 62 deletions across
eight files. This 222-line handoff brings the task artifact to 2,096 changed
lines, below the 2,250-line stop threshold.

## Design notes

### Identity and admission order

`DedicatedRemoteRequestIdV1::mint` uses 32 CSPRNG bytes encoded as
`remote-request-` plus 64 lowercase hexadecimal characters. The all-zero
value, malformed bridge namespace values, empty values, oversized values, and
unsafe legacy characters refuse. The bounded legacy opaque parser shape remains
accepted so the existing schema-v1 `"req-1"` golden remains byte-identical.

Only a value returned by `mint` is safe to expose in full: those bytes contain
no provider, URL, model, prompt, session, owner, or round material. Parsed
legacy input does not inherit that guarantee.

For each round the turn epoch is checked before ID minting. With V3, admission
then mints the ID, reserves its typed key through the attempt's single
`ResourceFlightRegistryV1`, atomically journals the exact node/session owner
with the exact identity, closes owner admission, journals intent, and publishes
the active request slot. `begin_dispatch` durably records dispatch before the
POST future is installed. Duplicate IDs return `IdentityCollision`; they never
join a predecessor.

### Journal lifecycle and capacity

Schema remains 1. `LIFECYCLE_SLOTS = 4` and
`PROCESS_LIFECYCLE_SLOTS = 7` are unchanged. Existing process/container event
shapes and accounting remain unchanged.

A request-key `FlightReserved` record reserves the existing four lifecycle
slots immediately. The additive
`RemoteRequestIdentityCaptured { identity, owner }` event atomically consumes
one reserved slot and installs the in-memory owner. Intent consumes one,
dispatch consumes one, and terminal settlement consumes one. Thus a five-row
request flight is fully capacity-protected before its capability can be
created; insufficient capacity refuses at `FlightReserved`. The identity
event has an exact golden and top-level, nested identity, and nested owner
unknown-field negatives.

`IntentJournaled` means the exact owner snapshot and bridge-authorized request
intent are durable. `DispatchStarted` means the provider effect was
authorized and the send future may be installed; it does not claim the future
was polled or accepted. Terminal mappings are:

| Exit | Disposition |
| --- | --- |
| Successful parsed response, including a tool response | `Complete` |
| Session cancel or `forget_session` while active | `Partial` |
| Admission after flight creation, send, HTTP, body, SSE, or parse failure | `Failed` |
| Consumer/future drop after dispatch without cancellation evidence | `Unknown` |

Every settlement returns the durable CAS winner. Only the retained flight
publishes that winner, once, through the injected
`ResourceFlightResultPublisher`.

### Cancellation and linearization

`SessionState` owns `next_turn_epoch`, `current_turn_epoch`, and
`cancelled_turn_epoch` separately from `active_request`.
`cancel(session)` first marks the current epoch canceled, then captures an
exact request capability. The capability re-locks and signals only if both the
epoch and identity still match. Sending is idempotent when the watch is already
true.

Request publication checks the epoch both before durable admission and again
under the publication lock. A cancellation in the A-to-B gap prevents B ID
minting. A cancellation racing durable B admission either prevents
publication and settles B `Failed`, or observes the published B slot and
signals it. Settlement/drop clears only via the same exact epoch+identity
comparison. `TurnScope` clears only turn epoch state; it cannot clear a
request slot. A later turn refuses any orphan slot and starts with a new epoch,
so stale A authority cannot contaminate it.

### Exposure and aggregation

`ApiResourceFlightRouteV3` binds the attempt authority and exact node ID as one
optional value. `ProtectedV3` is exposed only when that route exists.
`attach_resource_flight_owner_v1` must succeed for a session before its first
prompt; missing and poisoned attachment states refuse. Each request journals
`ResourceFlightOwnerV1 { node_id: route.node_id, owner_key: session }`.

A two-request tool prompt therefore publishes two distinct flight IDs to the
same exact node/session owner, each once. This supplies flight-side aggregation
only; `NodeCleanupRecordV2.collateral` remains slice 5's durable-writer scope.

### Exit and recovery census

- Capacity/reservation failure: refuses before a flight capability and POST.
- ID collision: refuses before the affected POST; predecessor evidence is
  unchanged.
- Owner/identity/journal/intent failure after creation: reserved terminal
  capacity settles `Failed`.
- Dispatch-journal failure: scope drop settles `Failed`; no POST is installed.
- Send error, rejected HTTP/error-body read, SSE read/frame/incomplete EOF,
  non-stream body read/parse: explicit `Failed`.
- Normal response/tool response: `Complete`, exact slot cleared before any
  successor.
- Max-round terminal: every issued round is already `Complete`; no active
  request remains.
- Cancel: exact active request `Partial`; turn epoch stays canceled through
  the gap.
- Consumer drop: `Unknown`, exact clear, then turn release.
- `forget_session`: sends the exact active watch, removes the session, and the
  request settles `Partial` once without an owner/slot orphan.
- Fresh successor turn: new epoch and new request identity; stale prior controls
  refuse.
- `max_tool_rounds = 0`: no ID, reservation, flight, or POST.
- Recovery can adopt only the durable key/intent/owner evidence and durable
  terminal CAS winner; no registry or request authority is reconstructed from
  `records()`.

The existing prompt-level diagnostic acceptance barrier remains monotonic
across all rounds. Once any provider request may have been accepted, a later
round error remains fatal/non-replayable.

## Load-bearing and negative tests

Added tests include:

- `stale_round_one_cancel_cannot_cancel_round_two`: retains A's capability,
  observes B publication, proves stale A signal and clear both refuse, proves B
  is still live, then sends current cancellation twice and observes one B
  `Partial` aggregation.
- `cancellation_between_round_terminal_and_successor_publication_prevents_post`:
  blocks synchronously in tool policy after A terminal and proves cancel
  prevents B ID mint and POST.
- `round_two_journal_failure_refuses_before_post_and_preserves_round_one`.
- `request_capacity_refusal_precedes_flight_creation_and_post`.
- `round_two_identity_collision_does_not_post_or_rewrite_round_one`.
- `consumer_drop_settles_and_clears_only_the_exact_request`.
- `forget_session_cancels_and_settles_the_exact_request_once`.
- `zero_rounds_mints_no_request_flight_and_missing_attachment_refuses`.
- `fresh_turn_is_live_and_stale_prior_turn_control_cannot_affect_it`.
- `poisoned_request_owner_attachment_and_admission_refuse`.
- exact request wire/strict-reader tests, key/identity mismatch, exact
  five-row lifecycle accounting, and pre-capability capacity refusal.

The stale-round test is mutation-sensitive: removing either identity comparison
makes A signal or clear B and fails the explicit live-B assertions. Removing the
turn epoch/preflight makes the between-round test mint/post B. Removing
conditional clear or request-scoped drop settlement fails the successor/drop
tests. These mutations were not executed because dependency resolution was
blocked; the predicates are asserted directly in the deterministic tests.

Existing V2 Wiremock coverage continues to own one/two-round success, in-flight
cancel, later-round fatal error, HTTP/SSE/non-stream failures, forget, and
successor behavior. No live provider is used by new tests.

## Verification

Passed locally:

- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo metadata --no-deps --format-version 1` (85,447-byte metadata output)
- production-unarmed grep: the API constructor assigns
  `api_cfg.resource_flight_route_v3 = None`
- no `bridge-container`, custody, cleanup lattice, history-store, watchdog,
  auth, deployment, compatibility, or operator files changed

Blocked environment gates:

- `CARGO_INCREMENTAL=0 cargo check -p bridge-core -p bridge-api -p a2a-bridge`
  could not download `a2a-lf`; the configured CONNECT proxy returned HTTP 403.
- The offline retry could not resolve `arc-swap`; the shared Cargo source
  cache contains neither `arc-swap` nor `a2a-lf`.
- `CARGO_INCREMENTAL=0 cargo run --offline -p a2a-bridge -- validate
  --repo-hygiene` stopped in dependency resolution on the same missing
  `arc-swap` package, before the hygiene binary could execute.

Consequently, zero dependency-backed tests, Clippy targets, release builds, or
repo-hygiene binaries were executed in this clone. No provider, smoke,
compatibility case, release, deployment, or running operator was invoked.
