# R2f1b 3c2 implementer handoff — API request-flight authority

Date: 2026-08-12
Base: `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`

## Outcome

**Superseding status:** this code is a preserved, rejected partial artifact.
The design escalation has completed and approves salvage through a
request-specific replacement, but this exact head remains non-foldable. The
paragraphs below describe what the artifact implemented, not accepted delivery.

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
  tests, bounded reservation recovery, and a cross-handle journal lock.
- `crates/bridge-core/src/liveness.rs`: existing-only persistent-lock
  acquisition, so an operation cannot recreate a removed journal root.
- `crates/bridge-core/src/process.rs`: non-cloneable request flight plus
  attempt-owned request binding and pre-admission recovery using the attempt's
  existing registry, file journal, clock, and publisher.
- `crates/bridge-core/src/reaper.rs`: journal decorator support for the required
  reservation census and rollback contract.
- `crates/bridge-api/src/config.rs`: optional
  `ApiResourceFlightRouteV3 { attempt, node_id }`.
- `crates/bridge-api/src/backend.rs`: per-turn/per-request cancellation,
  request admission/settlement across the full HTTP census, V3 exposure and
  attachment, and deterministic Wiremock tests.
- `crates/bridge-api/src/lib.rs`: exports the route; deterministic ID injection
  remains crate-private and test-only.
- `crates/bridge-api/Cargo.toml`: test-only `tempfile` dependency already
  used elsewhere in the workspace.
- `bin/a2a-bridge/src/main.rs`: explicit production `None`.

The reviewed `772518a8` artifact changed 2,180 lines (2,125 insertions and 55
deletions) across ten files including this handoff, below the 2,250-line stop
threshold. Its targeted repair closed at exactly 800 net implementation lines.
Documentation and receipts are accounted separately.

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

`ApiBackend` owns one checked monotonic `next_turn_authority`; `SessionState`
owns `current_turn_epoch` and `cancelled_turn_epoch` separately from
`active_request`. Recreating a forgotten session therefore cannot alias a
retained prior turn capability.
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
- Attempt admission first discovers bounded durable reservations. A zero-row
  request reservation is rolled back exactly; a journaled, unterminated request
  is terminalized `Unknown` through the durable CAS and published once. No live
  request capability is reconstructed from journal evidence.

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
- `forgotten_session_authority_cannot_alias_a_recreated_session`, for both V2
  and V3 identity paths.
- `cancel_before_first_stream_poll_closes_the_lifecycle`.
- checked forget/release tests proving only a durably `Complete` request
  projects cleanup `Complete`; partial and terminal-refusal outcomes project
  `Unknown`.
- `poisoned_request_owner_attachment_and_admission_refuse`.
- exact request wire/strict-reader tests, key/identity mismatch, exact
  five-row lifecycle accounting, and pre-capability capacity refusal.

The stale-round test is mutation-sensitive: removing either identity comparison
makes A signal or clear B and fails the explicit live-B assertions. Removing the
turn epoch/preflight makes the between-round test mint/post B. Removing
conditional clear or request-scoped drop settlement fails the successor/drop
tests. These predicates are asserted directly in deterministic tests; a separate
mutated checkout was not created.

Existing V2 Wiremock coverage continues to own one/two-round success, in-flight
cancel, later-round fatal error, HTTP/SSE/non-stream failures, forget, and
successor behavior. No live provider is used by new tests.

## Rejected-run and blind-tail record

The bounded bridge artifact committed `c24f31b9849dc49b6b5b7d2d4dff69c21e5986ef`
in retained clone `impl-40762-wdpnf5tx`; its exact tree was carried to feature
commit `0b4a18d1`. The three configured attempts converged but reached the cap:

1. `tempfile` was added without the required `Cargo.lock` edge, so all
   `--locked` compile gates refused.
2. The lock was repaired; eight `E0277` async-stream control-flow errors and one
   private-field test `E0616` remained.
3. Those were repaired; seven `E0382` errors remained because terminal
   `RequestScope::settle(self, ...)` calls appeared reusable to the
   `async_stream` expansion.

The review node failed during `Authenticate` on every attempt and supplied no
admissible code-review findings. Deterministic compiler evidence independently
established each rejection.

At the declared cap the defect population was fewer, smaller, and
non-repeating, so the operator disclosed one blind-tail extension. The repair
stores each round's `RequestScope` in an `Option`, consumes it through one
`settle_request_scope` helper, and refuses a second terminalization with
`InvalidStateTransition`. Changing `settle` to borrow was rejected because it
would permit duplicate durable terminalization. The initially red forget test
was a test race: active-slot publication intentionally precedes POST
installation. It now waits for one received POST before exercising
`forget_session`, preserving its intended in-flight scenario while the separate
between-round test owns pre-send cancellation.

## Dual-lens adjudication and targeted repair

The exact `772518a8` artifact received two independent read-only lenses: Opus
returned `REVISE`; Sol/max returned `REJECT`. The operator verified six distinct
constructible WRONGs and four coverage/closure requirements. Canonical lens
records and adjudication live on the planning branch:

- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-opus-lens.md`
- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-solmax-lens.md`
- `docs/superpowers/reviews/2026-08-12-r2f1b-3c2-dual-adjudication.md`

The confirmed population was: invalid pre-first-poll diagnostic grammar;
forgot/recreate ABA authority; request-flight custody errors projected as
transient/replayable crashes; checked cleanup claiming `Complete` before the
durable request winner; no attempt-admission reservation recovery; and a
zero-row orphan when creation failed after reservation. The repair also keeps
provider failure class authoritative when settlement refuses, restricts ID
injection to tests, and closes the corresponding negative/edge coverage.

One bridge repair flight was declared with an +800-net-line cap. It failed
before editing during authentication, retained a clean `772518a8` clone, and
had no usable checkpoint to resume. Per no-restart discipline, the operator
repaired the existing feature artifact rather than rerolling a fresh clone.

The gates then found three closed, smaller blind-tail defects: a cross-handle
journal lock initially recreated a deliberately removed root; a failed durable
terminal read returned before storing refusal/waking cleanup observers; and the
diagnostic constructor contract required the single `agent_failure` call to
remain in `ApiLifecycle::failure`. Each received a same-environment exact-base
control where attribution was needed. The loop converged without a repeated
defect class and closed at the declared 800-line cap.

## Verification

Post-adjudication repair and host gates passed:

- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
- Focused reservation recovery, cross-handle rollback, removed-journal-root,
  and checked-cleanup regressions — all exit 0.
- `cargo test -p bridge-core --lib` — 566 passed, 0 failed, 0 ignored.
- `cargo test -p bridge-api` — 81 passed, 0 failed, 1 ignored live Ollama test
  across its unit/integration/doc harnesses.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D
  warnings` — exit 0. The first feature run found one change-local
  `await_holding_lock` in the repaired forget test; exact base passed the same
  command in the same environment. Scoping the assertion guard before the
  asynchronous request-count check repaired it, and the feature rerun passed.
- `cargo test --workspace --locked --quiet` — 3,980 passed, 0 failed, 13
  ignored across 90 harnesses. The ignored population is the repository's
  declared authenticated/live-provider integration set, including Kiro and
  local Ollama lanes; no runnable host test was excluded. A sandbox-only
  Wiremock bind denial was inadmissible; the approved host suite is the gate.
- `cargo build --release --bin a2a-bridge --locked` — exit 0.
- `cargo deny check` — exit 0: advisories, bans, licenses, and sources all
  passed; policy-allowed duplicate-version warnings remain. The sandbox-only
  advisory-lock refusal reproduced identically on exact base before the host
  run.
- `./target/release/a2a-bridge validate --repo-hygiene` — exit 0;
  40 tracked artifacts and 8 example configs validated.
- Production remains unarmed: the API constructor assigns
  `api_cfg.resource_flight_route_v3 = None`.

The current base-to-tree implementation plus this handoff is 3,042 insertions
and 126 deletions across twelve files. The reviewed artifact stayed below its
2,250-line stop threshold and the targeted repair met its separately declared
800-net-line cap.

## Repaired-tail review and park

One declared Sol/xhigh repaired-tail review completed read-only against exact
`772518a8..cecff376`. It returned `VERDICT: REJECT` with eight proposed BLOCKER
WRONGs. The 16,340-byte terminal artifact has SHA-256
`6a690d6191f1d2faadcb208b1ddcf03c19525bb1d75d6e188cab554b555ad6e2` and is
mirrored with the operator adjudication on the planning branch.

Operator source adjudication confirmed the mechanisms: successor admission can
recover a different live request; checked cleanup collapses active LegacyV2,
admission-pending, and unobservable-refusal states; drop can erase settlement
refusal; timeout detaches an unbounded blocking waiter; nonzero pre-intent
prefixes strand; the 4,097th retained reservation bricks later census; terminal
CAS can crash before its one required publication; and file-journal root
authority is path-based across open/replacement races. The review's claim that
every previously settled non-Complete request must taint later no-active cleanup
was broader than established, but that refinement collapses none of the live or
unresolved cleanup failures.

This population is larger than the repaired six-WRONG round and includes new
admission-serialization, publication-outbox, deadline-aware observation, and
descriptor-root design. It is open-class at the declared review cap. Per
convergence discipline, 3c2 is parked for spec/design; no second repair round,
fold, push, CI, provider, smoke, compatibility case, deployment, or running
operator action was performed. Production remains unarmed.

## Design escalation resolved — salvage plan approved

One capped independent design round ran against exact preserved head
`530992b7ff1e8e9151fb2a69e86f3ff71c44f905` and landed base `42249b3d`: Sol/xhigh
hard-read-only and Fable Opus/xhigh plan both returned `DESIGN LENS: READY` with
no unresolved blocker. Operator source adjudication selected the convergent
salvage result and resolved their architectural disagreement:

- keep this artifact's request identity, backend-global turn authority,
  stale-scope cancellation fences, lifecycle grammar, acceptance barrier, and
  error/diagnostic repairs;
- replace the request adaptation of `ResourceFlightRegistryV1` with a separate
  descriptor-rooted request journal/state machine;
- recover only once behind an exclusive attempt-lifetime lease and before route
  publication;
- make the initial durable child atomic, cap active children at 4,096 before
  mutation, and retire only publication-acknowledged terminal children;
- preserve exactly one observable sink effect through a durable idempotent
  delivery ID plus matching acknowledgement;
- distinguish pre-first-poll `Failed`/accepted=false from post-arm
  `Unknown`/accepted=true;
- move request cleanup/drop observation to an owned async cell with no detached
  blocking waiter; and
- pass the exact protective cleanup disposition to retry, where only `Complete`
  permits another provider attempt.

The binding seven-task plan, both lens records, original brief, Opus side-plan
artifact, operator adjudication, and roadmap cursor are durably checkpointed on
the planning branch at
`0d72415a1f826408891d9fe64b2ca5ceb2037adf`. Tasks A-G each freeze the exact
predecessor commit, stay at or below 500 production lines, start with a
pre-change red crash/concurrency regression, run the full workspace gate before
commit, and have a declared one-review plus one-targeted-repair/closure cap.
Planning verification record `7b397bf6` adds the exact **3,211 passed / 0 failed
/ 12 ignored across 85 harnesses** checkpoint result and the four preserved
pre-existing untracked-config hygiene exclusions; those totals cover the older
planning branch, not this feature code.

This is design approval, not implementation acceptance. Do not fold or push
this head, arm production V3, create a production request-journal root, or
advance 3d. The next action is Task A (descriptor primitives/root custody) on
this preserved branch; the exact resulting commit becomes Task B's frozen
input. The two-field `CleanupReportV1 { result, checkout }` carry-forward remains
mandatory in the first later slice that arms production V3 or wraps
`ContainerRw`.

## Salvage Task A implementation

Task A exact clone base is `771c0fb8deca88fca06cac631208c9c83b87ea53`;
the unchanged code substrate is
`S0 = 530992b7ff1e8e9151fb2a69e86f3ff71c44f905`.
Initial red evidence used the exact command
`CARGO_HOME=/cargo CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked
--offline journal_root_custody -- --nocapture` against the pre-implementation
code. Nine missing-Task-A API errors named `JournalRootCustodyV1`, regular-file
identity, bounded enumeration, and required operations. No zero-selected or
dependency-network refusal is counted as red evidence.

Review rejected exact artifact `40b9ac364495ee9ca90ac21660dc8e54e929427e`.
The same selector failed with five contract-specific missing APIs: expected
parent/root identities, birthtime, no-replace publication, and the expected
target rename seam. The repair binds supplied identities, requires birthtime,
makes reads nonblocking, publishes behind final route revalidation, and checks
route plus target immediately before replacing rename.

Changed paths are `crates/bridge-core/src/fs_custody.rs`, the narrow
already-opened lock helper in `crates/bridge-core/src/liveness.rs`, and this
handoff. `lib.rs`, shared journal, request state, API/HTTP, and production V3 are
unchanged. Budgets are exactly 450 production and 800 total changed lines.

Final verification:

- `cargo test -p bridge-core fs_custody`: 66 passed, 0 failed.
- Exact `journal_root_custody` selector: 8 passed, 0 failed, including all five
  review regressions plus root replacement/removal and lock-object retention.
- Workspace all-target/all-feature clippy with `-D warnings`: exit 0.
- `cargo fmt --all -- --check` and `git diff --check`: exit 0.
- Aggregate workspace tests again passed all 1,084 bridge-core tests, then hit
  the pre-existing `a2a-bridge` test
  `api_entry_resolves_and_serves_through_registry` with
  `api.prompt.error_body_read`; exact base `771c0fb8` reproduces it identically.

No live provider, smoke, compatibility, deployment, fold, push, API migration,
or running-operator action was performed. Production remains unarmed.

## Task A1 targeted closure repair

Date: 2026-08-13. Frozen parent: `517703cbd2e469bf208f20a36248169536bca8b3`; repaired rejected artifact: `c0b5993a6c2ce5884ffcbb004f26442f2ba52b64`.

### Repair result

The descriptor-relative contract now captures the target with `rename_child_no_replace`, classifies the actual captured regular object by mandatory dev/inode/birthtime, and restores an unexpected capture only by the same exact no-replace primitive. Occupied custody cannot be clobbered; target takeover retains both objects as protective debt.
`CustodyCaptureOutcomeV2` keeps no-effect refusal, expected capture, unexpected restoration, retained debt, unknown, compile unsupported, and runtime unsupported structurally distinct. There is no success projection or weaker fallback. Identity/name/intent behavior remains policy-neutral; real-file content length is outside identity, and both Replace/Retire bind distinct predecessor/staged identities and refuse overflow.

### Admissible red evidence

Focused tests were added before production repair and compiled against `c0b5993a` production with this exact cached-artifact command:
```text
CARGO_PKG_VERSION=0.3.1 rustc --edition=2021 --test crates/bridge-core/src/lib.rs -L dependency=target/debug/deps --extern async_stream=target/debug/deps/libasync_stream-b21bf6375faa6f1f.rlib --extern thiserror=target/debug/deps/libthiserror-09d2b04943c8968d.rlib --extern futures=target/debug/deps/libfutures-825610e75520da33.rlib --extern ring=target/debug/deps/libring-9970c10e834c7569.rlib --extern libc=target/debug/deps/liblibc-8844194d13f2ff95.rlib --extern trybuild=target/debug/deps/libtrybuild-f3ab7e2788b15dbc.rlib --extern serde_json=target/debug/deps/libserde_json-622dfe45f1596545.rlib --extern tokio=target/debug/deps/libtokio-aabdc131ccdff1d8.rlib --extern tempfile=target/debug/deps/libtempfile-53ead19774e889a3.rlib --extern tokio_stream=target/debug/deps/libtokio_stream-12d3b45a41c8900b.rlib --extern syn=target/debug/deps/libsyn-5515cf2d944c9107.rlib --extern serde=target/debug/deps/libserde-edd2d6fc2ac8d018.rlib --extern tracing=target/debug/deps/libtracing-e3db5561f662c768.rlib --extern toml=target/debug/deps/libtoml-5b8358668b668f07.rlib --extern async_trait=target/debug/deps/libasync_trait-4297081399e1ed63.so -o /tmp/bridge-core-custody-v2-red
```
It failed nonzero with 20 `E0425`/`E0433` errors naming the absent capture name/function, typed outcome/action, and retention reason: bridge-core was reached, so this was neither dependency refusal nor zero-selected red.

### Verification and limits

- Host Cargo selectors after repair: focused `custody_v2` 7 passed, 0 failed; full `fs_custody` 73 passed, 0 failed.
- The first host focused run exposed macOS `ENOTSUP` as `ErrorKind::Uncategorized` (6/7); a raw-errno discriminator fixed that closed blind-tail defect without a fallback.
- `cargo fmt --all -- --check` and `git diff --check`: exit 0.
- Delta from frozen parent: 200 production + 224 colocated-test + 26 handoff = 450 lines; paths are `crates/bridge-core/src/fs_custody.rs` and this handoff.

A2-A4, Task B, production V3, live providers, smoke, compatibility, deployment, fold, push, HTTP, and the running operator remain unarmed and unchanged.

## Task A2 trusted route binding and operation lease

Date: 2026-08-14. Frozen input: `.git/A2A_TASK.md` at parent `5cbeea1ed882afe448d3825984af9a3ed74bcb58`.
The two dated design/plan paths named by dispatch are absent from this frozen tree and reachable history; no substitute guidance was inferred.

### Admissible red evidence

Exact command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --offline journal_route_custody_v2 -- --nocapture`.
Cargo reached bridge-core and failed nonzero with 16 `E0425`/`E0433` errors naming only the absent `JournalRootBindingV2`, `ObjectIdentityV2`, and `JournalRootCustodyV2`; this was neither dependency refusal nor zero selection.

### Result, gates, and limits

`JournalRootBindingV2` externally binds the anchor/parent/root route and the sibling operation-lock object with mandatory dev/inode/birthtime. `JournalRootCustodyV2::open` only opens and proves that route; it never creates it.
`begin_operation` serializes in-process acquisition, proves and nonblocking-flocks the exact retained lock object, then re-proves the held route. The opaque guard exposes no path, file, or descriptor surface and unlocks on drop.
Changed paths are `crates/bridge-core/src/fs_custody.rs`, the one visibility line in `crates/bridge-core/src/liveness.rs`, and this handoff.

- Focused `journal_route_custody_v2`: 5 passed, 0 failed; combined `custody_v2`: 16 passed, 0 failed; full `fs_custody`: 82 passed, 0 failed.
- Workspace all-target/all-feature check and clippy with `-D warnings`: exit 0.
- Aggregate workspace tests passed all 1,084 bridge-core tests, then hit the already-recorded `api_entry_resolves_and_serves_through_registry` / `api.prompt.error_body_read` failure; it was not repaired or rebaselined.
- Release `a2a-bridge` build and its repository-hygiene validation: exit 0; 40 tracked artifacts and 8 example configs validated.
- `cargo fmt --all -- --check` and `git diff --check`: exit 0.
- Delta from the frozen parent: 214 production (212 custody additions plus the one liveness replacement counted add/delete) + 244 colocated-test + 25 handoff = 483 changed lines.

A3-A4, Task B, production V3, shared journal/request state, API/HTTP, live providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task A3 owner-authorized targeted repair

Date: 2026-08-15. Frozen input: `b1b55a218c0b78213ec4a719ab96831cd766bd87`.

### Admissible red evidence

- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --offline namespace_transaction -- --nocapture`: 9 tests selected; the same-inode/same-length corruption regression failed because recovery returned `Complete`; the snapshot-only live path had the same defect.
- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --offline custody_v2_compile_unsupported_and_io_are_typed_without_fallback -- --nocapture`: 1 test selected and failed because injected `ENOTSUP` was `Unknown`, not `RuntimeUnsupported`.
- The namespace command's earlier compile-red named the absent deterministic identity/capture and mutex-entry seams. All reds reached `bridge-core`; none was dependency/network refusal or a zero-selected selector.

### Result and verification

Typed custody failures now survive setup, cleanup, and recovery; pre-stage incapacity creates no residue, while runtime capture incapacity rolls back its synced stage/intent before returning `Unsupported`. Immutable replace intents bind streamed SHA-256 successor content; live/recovery mismatches retain swap and never complete.
Retire persists no digest because its held-descriptor zero-link proof establishes exact predecessor removal. Its six-cut matrix pins `NoEffect`, `Complete`, `Retained`, or `Ready`; post-unlink/zero-link `Retained` remains permanent protective debt, with residue disposition reserved to the later-slice ledger.
Changed paths: `namespace_transaction.rs`, narrow `fs_custody.rs`, and this handoff. Normal-format source delta: +116 net production (193 insertions/77 deletions) and +105 net focused tests (140/35); this 17-line handoff replaces the prior 42-line section.
Focused aggregate: 93/0; `namespace_transaction` 9/0, `custody_v2` 18/0, `journal_route_custody_v2` 7/0, `fs_custody` 84/0. `git diff --check` and direct `rustfmt --check --edition 2021` are green; required `cargo fmt --all -- --check` cannot start because this Cargo has no `fmt` subcommand.
A4, Task B, production V3, every production caller, live providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task A4 owned journal surface and candidate deletion

Date: 2026-08-15. Frozen input: `6114596d58cce4ae3577afc6c015a212eb50c3c1`.

### Admissible red evidence

- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib -- journal_owned_surface_v2_stage_publish_append_read_enumerate_and_sync --nocapture` reached `bridge-core` and failed with ten A4-specific compile errors naming the absent mutation outcome and `stage`, `publish`, `append`, `read`, `enumerate`, and `sync` methods; it was neither dependency refusal nor zero selection.
- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib -- namespace_transaction_recovery_rechecks_target_before_finish --nocapture` selected one test and failed because recovery returned `Complete` after the target was mutated between its first verification and `finish`, rather than retaining the predecessor capture. Temporarily degrading that `finish` expectation to `None` reproduced the same red; the mutation was immediately reversed.

### Result, deletion inventory, and gates

`JournalRootOperationV2` now owns staged writes, no-replace publication, verified-position append, bounded read/enumeration, and root sync. Write sessions retain the operation borrow and descriptor through file/root sync; partial or failed append settlement must prove rollback or leaves a synced reserved marker plus retained debt. `NamespaceTransactionV2` remains the owned replace/retire/recover surface. Every mutator blocks on retained residue until recovery is clean, and only recovery may inspect and reduce debt.

Recovery now re-proves the committed replacement in `finish` after the fail-first target-mutation transition; the regression requires `Retained` and preserves the predecessor capture. Publication has an exact pre-rename route barrier and the target-appearance proof is protective, never success-flattened.

Deleted `JournalRootCustodyV1` and its raw writable open, revalidate authority, replacing-target seam, name-based unlink, free-standing child lock, non-Unix stub, candidate-only helper/tests, and now-unused identity/unlink helpers. Repository search reports zero remaining references to `JournalRootCustodyV1`, `acquire_persistent_child_lock`, `verify_regular_file_identity`, and `unlink_regular_child_at`. The externally used `PinnedDirectoryV1` replacement API and global liveness lock type remain unchanged and outside Task A.

Changed paths are `crates/bridge-core/src/fs_custody.rs`, `crates/bridge-core/src/namespace_transaction.rs`, and this handoff; `lib.rs` needed no broader export. Post-format accounting is production +280/-219 (net +61), focused tests +174/-312 (net -138), and handoff +25/-0: aggregate +479/-531 (net -52), within the 650-line insertion cap. Candidate deletion is the negative side, not retained dead surface.

- Required focused aggregate: 93 passed, 0 failed.
- `CARGO_INCREMENTAL=0 cargo check --workspace --all-features --locked`: exit 0. The broader all-target check reached changed `bridge-core`, then failed in unchanged `bridge-api` integration-test crate resolution (`E0463` for `bridge_api`/`wiremock`).
- Direct `rustfmt --check --edition 2021` on both changed Rust files and `git diff --check`: exit 0. Required `cargo fmt --all -- --check` cannot start because this Cargo installation has no `fmt` subcommand, the same frozen-environment limitation recorded by A3; no `rustfmt::skip` was introduced.

Task B, production V3, shared journal/request consumers, API/HTTP, providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task A4 owner-authorized closure repair

Date: 2026-08-15. Exact frozen input: `04e5957949575bec053b0739b21d42dc670cbbcf`.

### Admissible red evidence

- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline namespace_transaction_residue_free_retained_blocks_same_handle_until_clean_recovery -- --nocapture`: 1 selected, 1 failed because residue-free `Retained` did not block the next journal mutation.
- The same command with selector `reserves_`: 10 selected, 8 passed and 2 failed on missing stage/replace/retire headroom; with selector `reserved_targets_refuse_before_effect`: 2 selected and 2 failed on missing typed prefix refusal.
- Every red reached `bridge-core`, was nonzero, and was neither dependency/network refusal nor zero selection.
- Mutation controls temporarily disabled clean-recovery clearing and transaction-outcome recording in turn; the debt regression failed 1/1 each time at the corresponding post-recovery and immediate-block assertion. Both degradations were immediately reversed.

### Result, scope, and gates

All journal and namespace protective outcomes now record through one handle choke point. Admission reserves maximum transient entries (stage/append 1, publish/sync 0, replace 2, retire 1), and over-cap census is typed `ProtectiveDebt`; all five reserved-prefix variants are refused before effect and leave root bytes unchanged.
Recovery alone clears externally visible debt after an empty reserved census, successful root sync, and fresh route proof; residue parsing and recovery's bounded census remain unchanged.
Accepted debt scope: residue-backed debt is durable across handles/restarts through the residue itself; residue-free durability uncertainty self-heals on the next successful route-proof-plus-sync clean recovery. This re-argument is submitted to closure review.
Changed paths: `crates/bridge-core/src/fs_custody.rs`, `crates/bridge-core/src/namespace_transaction.rs`, and this handoff.
Post-format accounting versus the frozen input: production +100/-38 = 138 changed lines; colocated tests +178/-0; handoff +26/-0; aggregate +304/-38 = 342 changed lines.

- Required selectors: `namespace_transaction` 18/0, `custody_v2` 22/0, `fs_custody` 80/0, `journal_route` 11/0, and `journal_owned_surface` 4/0.
- Bridge-core all-target/all-feature offline clippy with `-D warnings`: exit 0.
- Full bridge-core library sweep: 605 passed; the sole failure was the unchanged process-group host test `term_ignoring_child_with_descendant_is_group_killed_host_signal_semantics`, reproduced alone and outside owned paths.
- Direct `rustfmt --check --edition 2021` and `git diff --check`: exit 0; required `cargo fmt --all -- --check` cannot start because this Cargo installation has no `fmt` subcommand. No `rustfmt::skip` was introduced.

Task B, production V3, consumers, live providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.


## Task B1 request journal grammar and atomic admission

Date: 2026-08-15. Exact frozen input: `d8ec93ad4a03a29d6da80c4fdf9fa818c8572459`.

### Admissible red evidence

Exact command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight --nocapture`.
Cargo reached `bridge-core` and failed nonzero with 26 Task-B-specific `E0425`/`E0433` errors naming the absent request journal, child/checkpoint wires, admission boundaries, typed refusals, and Task A outcome consumer. It was neither dependency/network refusal nor a zero-selected selector.

### Result and cap-directed split

The new owned request root has a strict schema-v1 checkpoint and request-child grammar, exact attempt binding, canonical dedicated request IDs, owner binding, ordinal/checkpoint/authority digests, `deny_unknown_fields`, bounded capacity-plus-one enumeration, and typed corrupt/foreign/digest/legacy/protective refusals. Admission validates census and headroom before ID mint or mutation, allocates from the checkpoint and active maximum with checked arithmetic, publishes one complete child through Task A stage/no-replace/root-sync operations, advances the checkpoint only on exact `Complete`, and returns a non-cloneable authority only after both publications. Six injected admission cuts prove every visible request-shaped file is a nonempty strictly decodable row; B1 reopen refuses staged or unowned active rows without mutation.

Normal formatting could not fit bounded retirement inside the 500-production-line cap. Per the dispatch split rule, B1 stops here. B2 is the named remainder: recover Task A transaction residue, advance and close a step-5 orphan as pre-send failure, persist terminal acknowledgement, identity-check unlink plus root sync, prove ack-before-unlink and after-unlink restart idempotence, and prove retirement frees bounded capacity across more than the cap's sequential cycles. Those self-heal/retirement criteria are not claimed by B1.

Changed paths are `crates/bridge-core/src/remote_request_flight.rs`, the two-line Unix `lib.rs` export, and this handoff. Accepted Task A files and behavior were not changed. Post-format accounting is 500 production additions (498 module plus 2 export), 247 colocated-test additions, and this appended handoff; there are no deletions.

### Verification and exclusions

- Focused `remote_request_flight`: 4 passed, 0 failed, 612 filtered out.
- Required Task A regression selector: 118 passed, 0 failed, 498 filtered out.
- `CARGO_INCREMENTAL=0 cargo check -p bridge-core --lib --locked --offline`: exit 0.
- `git diff --check`: exit 0. Required `cargo fmt --all -- --check` cannot start because this Cargo installation has no `fmt` subcommand; direct `rustfmt --check --edition 2021` passed for both changed Rust files. The new module has no `rustfmt::skip`; nine skips are pre-existing in frozen Task A `fs_custody.rs`.
- The reduced capacity fixture admits the exact positive edge of five active rows at cap 8, then refuses before the sixth ID mint or mutation.
- Corrupt/foreign/digest/legacy and over-cap cases refuse without mutation; the over-cap fixture enumerates cap plus one.
- Protective Task A outcome classes remain typed and only exact `Complete` advances the checkpoint.
- No production caller, request route, shared-journal migration, provider send, API/HTTP surface, live smoke, or compatibility action was added.

B2, Tasks C-G, production V3, every production caller, providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed.

## Task B1 targeted authority/admission repair

Date: 2026-08-15. Exact frozen input: `2815259d3a7a3b2869f0968c33cea010a4a1ede1`.

### Admissible red evidence

Exact command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight --nocapture`.
Cargo selected eight tests and reached `bridge-core`; five passed and three failed on the nested-attempt unknown-field, duplicate-mint, and capacity-plus-two regressions. This was neither a compile/dependency/network refusal nor a zero-selected selector.

### Repair result and limits

`RemoteRequestAuthorityV1` now has only a private field, no public constructor, no `Clone`/`Copy` implementation, and one borrowed identity accessor. The sole construction expression remains the successful admission tail, so external code cannot construct or duplicate authority while the journal owns minting.
Checkpoint and request-child decoding use a private Serde remote wire for `AttemptIdentity` with nested `deny_unknown_fields`. Admission refuses an already-censused minted identity before staging; exact enumeration-limit overflow is `Capacity`; other custody failures remain typed Task A outcomes; and the checkpoint read no longer adds a needless reference.

Focused tests preserve complete root bytes and prove zero mint on strict-decode/capacity refusal, exact no-mutation duplicate refusal, the positive-edge `next_ordinal`, and a real Task A replacement-capture residue surfacing as `ProtectiveDebt` without publishing a request or changing the checkpoint.
Versus the frozen input, the module delta is +172/-38 including tests; with this 26-line handoff the repair totals 236 changed lines. The production-region patch is +43/-38 (deletions bounded by the file's 38 total), below the 120-line repair stop. Versus `d8ec93ad`, production remains exactly 500 additions: 498 non-test module lines plus the existing two-line `lib.rs` export; test-only module additions total 381 lines (879 module lines = 498 production + 381 test). B2 is still split and unimplemented. Recorded policy for the closure lens: duplicate-mint refusal deliberately leaves the handle requiring reopen — a repeated CSPRNG identity means the identity source itself is suspect, so freezing the handle fail-closed is intentional protective behavior, not an oversight.

### Verification

- Focused `remote_request_flight`: 8 passed, 0 failed, 612 filtered out.
- Required combined Task A/B1 selectors: 126 passed, 0 failed, 494 filtered out.
- Workspace all-target/all-feature clippy with `-D warnings`: exit 0 from a fresh disposable target; bridge-core's same gate also exits 0.
- Direct `rustfmt --check --edition 2021` and `git diff --check`: exit 0. `cargo fmt --all -- --check` still cannot start because this Cargo installation has no `fmt` subcommand.

B2, Tasks C-G, production V3, all callers/routes, providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task B2 acknowledged retirement and reopen self-healing

Date: 2026-08-15. Exact frozen input: `6033fd34fccb2fb8fbbb45585df5472eb95331df`.

### Admissible red evidence

Exact pre-production command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight --nocapture`.
Cargo reached `bridge-core` and exited 101 with 11 B2-specific compile errors naming the absent pre-send-failure state, acknowledgement/retirement methods, exact-complete refusal, and real Task A boundary seam. It was neither dependency/network refusal nor zero selection. The owner/census and fault-boundary rider regressions were in this same pre-production batch; the boundary seam contributed the missing-API red.
A later strict-terminal corruption regression selected 15 tests and failed 1 because unit-style tagged states accepted an unknown nested field. Empty struct variants closed that grammar hole while retaining the minimal three-state wire.

### Result and restart census

The strict child marker is now `active`, `pre_send_failure`, or `terminal_acknowledged`. Only exact `ResourceActionDispositionV1::Complete` may persist acknowledgement. Acknowledgement uses Task A identity-bound replacement; retirement requires that marker, uses identity-bound removal, and root-syncs. Refused, retained, unsupported, protective-debt, and injected I/O-unknown outcomes stay exact typed refusals at real stage, acknowledgement-replace, and retirement-removal boundaries without root mutation.
`open` first invokes Task A transaction recovery and refuses ambiguous staged/reserved residue. A published child ahead of the checkpoint advances the checkpoint, then closes without authority reissue as pre-send failure. A durable acknowledgement is retired on reopen; a completed unlink has no residual debt. Replayed reopen is byte-idempotent. Every cut before unlink, after unlink, and after root sync is pinned.
Shared owner validation reparses the node ID and rejects empty, oversized, or control-bearing owner keys before mint and during census. Strict state/owner nesting plus schema, authority digest, and child-name corruption all refuse without mint, checkpoint advance, or additional root mutation.
A reduced capacity of 5 was used: eight sequential admit/acknowledge/retire cycles succeed on one root, with the checkpoint asserted monotonically after every cycle.

### Verification and post-format accounting

- Focused `remote_request_flight`: 15 passed, 0 failed, 612 filtered out.
- Required aggregate command: 133 passed, 0 failed, 494 filtered out.
- Bridge-core library all-feature offline clippy with `-D warnings`: exit 0. The tests-inclusive clippy command reached unchanged integration-test crate resolution and failed with the already-recorded `E0463`/dependent inference errors.
- Direct `rustfmt --check --edition 2021` and `git diff --check`: exit 0. Required `cargo fmt --all -- --check` cannot start because this Cargo installation has no `fmt` subcommand; no `rustfmt::skip` was added.
- Versus `6033fd34`, module production is +195/-21 (216 churn) and test-only code is +339/-9 (348 churn). This handoff adds 28 lines, for 592 total changed lines; production deletions 21 do not exceed the module's 30 total deletions.
- Versus `d8ec93ad`, the module is 673 production plus 710 test-only additions, the existing `lib.rs` export is 2 production additions, and the handoff is 85 documentation additions; there are no deletions on that comparison.

The attempt-bound authority identity remains Task C scope. Tasks C-G, production V3, every caller/route, providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task B2 targeted repair

Date: 2026-08-15. Exact frozen input: `6115c93e78dd1bd35b0fcd56139e52f23d1dc5df`.
### Admissible red evidence

Exact pre-production command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight --nocapture`; Cargo reached `bridge-core`, selected 19 tests, and exited 101 with 16 passed / 3 failed / 612 filtered, so this was neither dependency/network refusal nor zero selection.
The issued-active regression failed because reopen wrote `PreSendFailure`; the gap/multiple-ahead regression accepted and mutated the census; the foreign-attempt interrupted-retirement regression ran Task A recovery and changed root bytes before returning `ForeignAttempt`. Thus repairs 1 and 2 each had a behavioral frozen-input red.
The exact owner selector command used the same prefix with `remote_request_flight_owner_validation_precedes_mint_and_census_is_nonmutating --nocapture`; separately disabling the `WIRE_CAP + 1` and control-character predicates produced 0 passed / 1 failed behavior reds, and each mutation was immediately restored.
Checkpoint schema, digest, and attempt identity are now read and validated before recovery. No checkpoint remains the shipped typed `Malformed("checkpoint is absent")` refusal, now without admitting recovery or mutation.
Reopen leaves every active ordinal below `next_ordinal` byte-identical, heals only the unique active child exactly at `next_ordinal`, and refuses duplicate, gapped, or multiple-ahead censuses before mutation; the step-5 orphan remains idempotently healed.
Fault injection now supplies raw journal/filesystem/namespace results to the production mappers; the publish regression proves the owned publish ran, and the retirement regression proves the real identity-bound removal ran, before raw result mapping.
Post-unlink and post-zero-link Task A crash states have the same durable absent-capture surface and both reopen as typed `Retained` without root mutation, authority, or panic. Permanent retention is unchanged per the owner-ledgered later-slice residue-disposition item in “Task A3 owner-authorized targeted repair” above.
### Verification and accounting

The required aggregate command passed 137 / 0 with 494 filtered; focused `remote_request_flight` is 19 / 0 with 612 filtered, and production `cargo check -p bridge-core --lib --locked --offline` passes. `git diff --check` and direct `rustfmt --check --edition 2021` pass; required `cargo fmt --all -- --check` was run but this Cargo has no `fmt` subcommand, and no `rustfmt::skip` was added.
Versus `6115c93e`: production +68/-31 (99 churn), colocated tests +259/-22 (281), and this handoff +17 = 397 total. Versus `6033fd34`: production +269/-33 (302), tests +563/-21 (584), and handoff +45 = 931 total. Tasks C-G, production V3, callers/routes, providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed.

### Task B2 operator completion and convergence extension (exact head record)

Operator completion `2e472a09` (+43/-21) drove stage, acknowledgement
replacement, and orphan-checkpoint healing through the wrap-actual injection
seam red-first, fixed the three prescribed clippy lints, and narrowed the
`request_paths` helper to published children. Cumulative accounting at that
head versus `6033fd34`: module +859/-59 (production +279/-34, tests
+580/-25) plus handoff +45; versus `6115c93e` the repair-line churn was 455
total (production 130), 55 above the originally declared 400 — recorded here
as the disclosed operator authorization.

The B2 closure review returned one blocker: heal ordering advanced the
checkpoint before relabeling the orphan, so an interrupted heal stranded a
proven never-issued child as active below the checkpoint. Under the
disclosed operator convergence extension (population 3 -> 2 -> 1 across
rounds, non-repeating classes), the current head reorders healing —
relabel to pre-send failure first, sync, then advance the checkpoint — and
reopen recognizes the unique pre-send-failure child at exactly
`checkpoint.next_ordinal` as the resumable intermediate, completing it
idempotently. The heal checkpoint advance carries its own injection
boundary; admission checkpoint advance and all three root-sync sites now
consume real adapter results through wrap-actual seams. Red evidence: the
resume, heal-seam, and admission-checkpoint-seam regressions all failed on
the pre-change head (retained log). The mid-retire permanent-`Retained`
residual remains the accepted, owner-ledgered Task A semantics; the
below-checkpoint send-state discrimination remains Task C scope, as does
the attempt-bound authority identity. Tasks C-G and production V3 remain
unarmed.

## Task C attempt lease, recovery table, and acknowledged outbox

Date: 2026-08-15. Exact frozen input: `dbf514bd548f00ab4563d36ee48dcecf2cd343b8`.

### Admissible red evidence

Exact pre-production command: `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight --nocapture`.
Cargo reached `bridge-core` and exited nonzero with five Task-C-specific missing-symbol failures naming `RemoteRequestDeliveryIdV1`, `RemoteRequestTerminalPublicationV1`, and `RemoteRequestResultPublisherV1`. It was neither dependency/network refusal nor zero selection.

### Result and recovery contract

Initialization now creates the empty attempt-lock child. The sole production constructor is `open_recovered(custody, attempt, capacity, publisher)`; it takes a nonblocking whole-attempt lease before recovery or mutation, returns typed `AttemptLive` to a second opener without changing bytes, and releases the lease when the journal drops. The order is attempt lease, admission mutex, request transition, then Task A operation. No Task A operation, admission mutex, or transition borrow is held across the publisher callback; the lifetime attempt lease remains held as the opener exclusion invariant.

The strict child progression is reserved, intent journaled, dispatch authorized, provider-send armed, terminal pending publication, publication acknowledged, then retirement. Exact-order transitions use Task A replacement and invalid order is nonmutating. Recovery maps reserved/intent/dispatch to `Failed,false`, armed to `Unknown,true`, replays pending publication without provider resend, and retires an exact acknowledged delivery without republishing. Corrupt, foreign, over-cap, ambiguous, and unknown-field roots remain fail-closed and byte-preserving.

The publisher receives the full private attempt+ordinal+request delivery identity, terminal result, and acceptance bit. Its trait contract requires durable sink deduplication before echoing that exact identity; callback count may exceed one, while the committed sink effect is exactly once. Refusal or mismatched acknowledgement leaves the durable pending row in place and blocks constructor success/admission until a later exact echo. Test-only B-era openers retain their original recovery behavior; no production legacy opener was exported.

### Verification and accounting

- Required aggregate command `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib -- remote_request_flight namespace_transaction custody_v2 fs_custody journal`: 147 passed, 0 failed, 494 filtered out.
- Final focused Task C/module sweep: 29 passed, 0 failed, 612 filtered out.
- Direct `rustfmt --edition 2021 --check crates/bridge-core/src/remote_request_flight.rs`, `git diff --check`, and bridge-core library all-feature offline clippy with `-D warnings`: exit 0.
- Required `cargo fmt --all -- --check` was run but cannot start because this Cargo installation has no `fmt` subcommand; the available direct `rustfmt` check passed and no `rustfmt::skip` was added.
- No live/billable agent, smoke, compatibility, provider, network, or deployment action ran.

Changed paths are `crates/bridge-core/src/remote_request_flight.rs` and this handoff; `lib.rs` required no change. Post-format accounting versus the frozen input is 480 raw pre-test production changed lines, 336 colocated-test changed lines, and 29 handoff additions: 845 total churn, below both stop limits.

Tasks D-G, production V3, every production caller/route, live providers, smoke, compatibility, deployment, fold, push, and the running operator remain unarmed and unchanged.

## Task C targeted repair: lease-first open and lease-aware headroom

Date: 2026-08-15. Exact frozen input:
`4db414f08b96541d8471707b4143903d7a4a75e6` (`4db414f0`).

### Admissible fail-first evidence

All three commands reached `bridge-core`, selected exactly one test, and exited
101 on the frozen production mechanisms; none was a dependency, network, or
zero-selection refusal:

- `CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline -- remote_request_flight_capacity_counts_permanent_lease_before_mint_or_mutation --nocapture`
  failed 0/1 because the 4,094-entry root admitted, minted, and mutated instead
  of returning `Capacity` before the mint.
- The same command with selector
  `remote_request_flight_interrupted_positive_edge_reopens_with_healing_headroom`
  failed 0/1 because reopen returned exact `TaskA(ProtectiveDebt, "close orphan
  request: ...")` at the old positive edge.
- The same command with selector
  `remote_request_flight_task_c_attempt_lease_precedes_contended_operation`
  failed 0/1 at the exact-`AttemptLive` assertion while the ordering token proved
  the first admission closure still held its Task A operation.

### Repair and bounded evidence

`ADMISSION_FOOTPRINT` is now four: checkpoint replacement headroom plus the
permanent checkpoint and attempt-lease children are accounted before mint or
mutation. The exact maximum census now refuses byte-identically, and the
positive-edge fixture moves down one child; an interruption before checkpoint
advance reopens, relabels, advances, and completes without protective debt.

The only Task A production addition is the one
`pub(crate) JournalRootCustodyV2::acquire_existing_regular_child_lease`
accessor (plus its non-Unix refusing form). It proves the bound route before
and after, opens an exact existing regular child descriptor-relatively with
no create and no follow, takes a nonblocking flock, rechecks that the name
still denotes the opened object, and returns only the lease guard plus content
snapshot: no file, route path, or mutation authority. Colocated tests cover
wrong-route identity, wrong type, no creation, contention, drop release, and
subsequent acquisition. `open_recovered` uses this accessor first; only a held
lifetime lease can precede any Task A operation. A live holder returns exact
`AttemptLive` with unchanged bytes, then drop permits a successful open.

Folded evidence is explicit. A one-line mutation of armed-send recovery from
`Unknown` to `Failed` made
`remote_request_flight_task_c_recovers_every_durable_prefix_without_resend`
fail 0/1 at prefix 3 with left `(Failed, true)` versus right `(Unknown, true)`.
A separately restored one-line inversion of exact acknowledgement matching
made `remote_request_flight_task_c_pending_outbox_replays_until_exact_ack`
fail 0/1 with `PublicationAcknowledgementMismatch` on the later exact echo.
The refusal and mismatch arms now assert their exact distinct errors, and a
real `PreSendFailure` recovery publishes exact `Failed,false` then retires.

Named migrated setup inventory: `initialized` retains the test-only B-era
opener, while its cap-edge setup moved from reduced capacity to the real 4,096
Task A boundary because that is where replacement headroom is enforced;
`unchecked` continues through production `open_recovered`, so its callers drop
the prior journal before a second custody handle acquires the lease; the
foreign-checkpoint setup intentionally remains on test-only `open_with_capacity`
because it isolates checkpoint authorization before Task A recovery rather
than exercising publisher recovery.

### Verification and accounting

- Focused `remote_request_flight`: 32 passed, 0 failed, 614 filtered out.
- Required aggregate command: 152 passed, 0 failed, 494 filtered out.
- `git diff --check`, direct `rustfmt --edition 2021 --check`, and bridge-core
  library Clippy with `-D warnings` pass. Required `cargo fmt --all -- --check`
  cannot start because this Cargo has no `fmt` subcommand; no `rustfmt::skip` was added.
- No live/billable agent, provider, smoke, compatibility, network, deployment,
  fold, push, or running-operator action ran.

Post-format churn versus `4db414f0` is 76 production lines and 216 colocated-
test lines. This repair record adds 79 documentation lines, for 371 total
changed lines. The 180-production / 450-total stop limits are not approached.

Tasks D-G and production V3 remain unarmed; every production caller/route is
unchanged and no later task is implied or activated.

### Task C closure extension (exact head record)

The C closure review confirmed both round-1 blockers FIXED and found one
fresh ordering defect: recovery ran before the full request census was
validated, so an attempt containing both Task A residue and an
independently corrupt row surfaced recovery's protective classification
(and could mutate legitimate residue) instead of refusing byte-preserved on
the corrupt row. Under the disclosed operator convergence extension, `open`
now runs a residue-tolerant validation pass (`scan_with`) over every
ordinary row before any recovery; reserved Task A entries are skipped there
because recovery owns their classification, and the full scan still runs
after recovery. Red evidence: the composite regression (stage residue plus
corrupt sibling) failed on the pre-change head with recovery's
`ProtectiveDebt("missing or duplicate intent")` where `Malformed` was
required; it now refuses `Malformed` with every root byte preserved, and
the residue-only control still surfaces the recovery-side classification.
This closes the validate-before-recover class at its terminal scope (the
full census). Tasks D-G and production V3 remain unarmed.

## Task D owned request driver and bounded observation

Date: 2026-08-15. Exact frozen input:
`832221c905e3e32d541d311931a177637a2d0f28` (`832221c9`).

### Admissible red evidence

Exact pre-production command:
`CARGO_INCREMENTAL=0 cargo test -p bridge-core --lib --locked --offline --
remote_request_flight_task_d --nocapture`.
Cargo reached `bridge-core` and exited 101 with eleven errors specific to the
absent Task D `RemoteRequestDriverV1`, owned first-poll/settlement surface,
and `ObservationTimedOut` refusal. It was neither a dependency or network
refusal nor a zero-selected selector.

### Result, recovery, and bounds

The production opener now yields a shared driver whose admission returns one
non-cloneable owned request. Its authority-bound methods durably journal intent
and dispatch authorization in exact order. The generic provider-send wrapper
appends `ProviderSendArmed` before the inner future's first poll; an injected
no-effect Task A refusal never polls the inner future and settles
`Failed,false`. Crash recovery retains the Task C prefix table:
pre-arm is `Failed,false`, while post-arm is `Unknown,true`.

Terminal settlement records one durable winner, publishes it only after every
journal/Task A lock is released, requires the exact delivery acknowledgement,
and retires the row. Racing settlers return the same winner. A watch observer
uses deadline-bounded Tokio waits and an RAII waiter count; timeout leaves zero
live waiters and creates no thread. Publication refusal leaves the pending row
and blocks new admission; drop does not retry or erase that debt, and reopen
drains it through the existing Task C outbox.

The required aggregate selector passes 159/0; focused Task D passes 6/0.
Bridge-core library Clippy with `-D warnings`, direct Rustfmt check, and
`git diff --check` pass. Required `cargo fmt --all -- --check` was run but
cannot start because this Cargo has no `fmt` subcommand; no
`rustfmt::skip` was added. Post-format churn versus `832221c9` is 428
production lines (412 additions/16 deletions), 261 colocated-test lines
(252/9), and this 46-line handoff: 735 total changed lines.

Only `remote_request_flight.rs` and this handoff changed; `lib.rs` needed
no broader export. Tasks E-G, every production caller/route, provider send,
API/HTTP work, live smoke, compatibility, deployment, and production V3 remain
unarmed and unchanged.
