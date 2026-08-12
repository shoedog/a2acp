# R2f1b 3c2 API authority — dual-lens adjudication

Date: 2026-08-12.
Artifact: `feat/r2f1b-3c2-api-authority` @
`772518a8aaeaa6c52a2bbd445123fc945aeb8056`; exact base
`42249b3d926b49afd9d0dbd213d0ee3d3e459af6`.

## Round and cap

One independent dual-lens review round was declared before dispatch. Opus
`opus[1m]`/xhigh/plan returned **REVISE** with two WRONGs; Sol/max/read-only
returned **REJECT** with five WRONGs. One finding converged independently:
forget/recreate reuses turn authority while the predecessor scope can still
unwind. The union is six distinct WRONGs. The feature worktree remained clean
at the frozen head throughout both reviews.

Repair cap: **one targeted bridge repair round on this existing artifact**.
No restart or fresh implementation distribution is authorized. At the cap,
closed enumerable compiler/gate failures may receive one disclosed operator
blind-tail completion only if the population is converging and non-repeating;
an open class parks for design.

## Operator adjudication — WRONG findings

### R1 — pre-first-round cancellation projects Failed, not Canceled — CONFIRMED, BLOCKER

State: `prompt()` establishes turn epoch 1; cancellation lands before round
0 admission. `prompt_inner` has opened `PromptStart`, but the
`PreparedRequest::TurnCancelled` branch calls
`complete_prompt_lifecycle` without the compensation used by the zero-round
path. `DiagnosticSequence` refuses `PromptStream/Completed` without
`PromptStream/Started`; the stream emits `InvalidStateTransition`, and the
translator cannot emit `TaskOutcome::Canceled`.

Mechanism anchors:
`crates/bridge-api/src/backend.rs:700-723,1072-1083`,
`crates/bridge-core/src/diagnostics.rs:1553-1574`, and
`crates/bridge-core/src/translator.rs:339-350`.

Repair: mirror the zero-round lifecycle opening before completion. Red
regression: deterministic LegacyV2 cancel after `prompt().await` but before
first poll/admission; require one clean cancelled `Done`, no error, and no
POST/flight.

### R2 — forget/recreate ABA lets stale A clear successor B — CONFIRMED by both lenses, BLOCKER

State: A is active for session S; `forget_session(S)` signals A and removes
the `SessionState`; B begins immediately with the same SessionId. The new
state restarts `next_turn_epoch` and `next_legacy_request` at zero. A's
retained `RequestCancelCapability` can match and clear B in LegacyV2, and
A's `TurnScope::drop` can clear B's turn epoch in both LegacyV2 and V3.
A later current cancel then returns without signaling B.

Mechanism anchors:
`crates/bridge-api/src/backend.rs:249-280,292-329,580-604,1243-1254`.
No generation, tombstone, or join-before-reuse invariant exists. The trait
surface and checked cleanup make forget callable while work is active.

Repair: assign each turn a backend-global monotonic authority coordinate (or
equivalent session-incarnation nonce) that cannot rewind when a map entry is
removed, and include it in every stale-scope comparison. Red regression:
delayed A, forget A, immediate same-ID B, allow A to unwind, then prove stale
A cannot clear B and one current cancel settles B exactly once.

### R3 — post-acceptance flight failures are transient and replayable — CONFIRMED, BLOCKER

State: round A may have reached the provider; round B hits reservation,
identity, journal, dispatch, or settlement failure. The new request-flight
paths propagate a raw `BridgeError::AgentCrashed` via
`request_flight_error`. `AgentCrashed` is transient, the cold executor may
retry the full prompt, and terminal projection records
`prompt_may_have_been_accepted=false` because only `AgentFailure` carries
that evidence.

Mechanism anchors:
`crates/bridge-api/src/backend.rs:519-521,713-740`,
`crates/bridge-core/src/error.rs:210-224`, and
`crates/bridge-workflow/src/executor.rs:222-228,4119-4139`.
There is no downstream acceptance-aware wrapper.

Repair: map every post-barrier admission/dispatch/settlement refusal to a
fatal, acceptance-aware diagnostic; preserve accurate pre-first-send
classification. Red regression: retry-enabled workflow, accepted round A,
injected round-B journal/terminal failure; require one POST/effect, no retry,
fatal disposition, and `prompt_may_have_been_accepted=true`.

### R4 — checked cleanup returns Complete before request settlement — CONFIRMED, BLOCKER

`ApiBackend` overrides only void `forget_session`. The trait defaults for
`forget_session_checked` and `release_session_checked` call the void
method and immediately return `BackendCleanupDispositionV1::Complete`.
An active V3 request can still be awaiting the provider or a refused terminal
append while workflow cleanup projects destructive success.

Mechanism anchors:
`crates/bridge-api/src/backend.rs:1089-1255`,
`crates/bridge-core/src/ports.rs:439-480`, and checked cleanup callers in
`crates/bridge-workflow/src/executor.rs`.

Repair: override both checked methods. Join/project the exact durable request
winner when possible; otherwise return a protective `Unknown` or
`Retained`, never `Complete` merely because signaling/removal was
attempted. Red regressions cover delayed settlement and terminal refusal for
both forget and release.

### R5 — dispatched request flights lack reachable crash/refusal recovery — CONFIRMED, BLOCKER

A dropped request ignores settlement failure, terminal refusal becomes sticky,
the active slot is cleared, and no caller retains a settlement capability.
The recovery primitive requires an already-known flight ID. The journal trait
has `reserve_flight`, `append`, `append_terminal`, and
`records(id)`, but no reservation enumeration. Attempt construction only
creates a new registry over the journal; successor requests mint new keys and
cannot encounter the abandoned reservation.

Mechanism anchors:
`crates/bridge-api/src/backend.rs:367-380`,
`crates/bridge-core/src/process.rs:824-872,924-995`, and
`crates/bridge-core/src/retained_resource_flight.rs:317-346,1396-1401,1588-1649,1918-1950`.

Repair: add bounded exact reservation discovery at attempt recovery and
terminalize journaled request intent as `Unknown` through the existing
terminal CAS before admitting new provider effects. Do not reconstruct request
dispatch authority. Red regression: reopen an attempt containing an abandoned
dispatched request; require one Unknown terminal and one publication, with no
new POST capability.

### R6 — capacity refusal leaves an unrecoverable durable reservation — CONFIRMED, BLOCKER

The file journal durably creates the key-to-flight reservation before
`RetainedResourceFlight::create` appends `FlightReserved`. If admission
capacity refuses that first row, the registry inserts no flight but the
reservation survives. Re-reserving the exact key finds zero rows and returns
`ReservationUnavailable`; normal requests mint different keys.

Mechanism anchors:
`crates/bridge-core/src/retained_resource_flight.rs:383-414,656-742,1028-1055,1918-1984`.

Repair: make reservation plus initial row one atomic admission boundary, or
provide an exact conditional rollback proven safe only when no row became
durable. Red regression: capacity-four refusal followed by journal reopen;
require no orphan reservation or one durably closed Failed result.

## Coverage closures required in the repair

C1. Independently mutation-pin request identity, turn authority, the first
cancellation check, the post-reservation cancellation check, and
`TurnScope::drop`'s stale guard.

C2. Add V3 durable-disposition assertions for send, rejected HTTP/error-body,
SSE read/frame/incomplete EOF, non-stream body read/parse, and terminal append
failure; LegacyV2 diagnostic tests are not substitutes.

C3. Close Opus's related admission-diagnostic gap: request-flight failures
must not leave `PromptStart` open or mask a prior provider classification.
Where settlement itself fails, preserve the stronger acceptance/failure
evidence.

C4. Restrict the public request-ID test seam unless a production extension
contract is explicitly justified. Production IDs must retain the canonical
bridge-minted shape.

## Deferred SMELL ledger

- Per-request registry/journal growth is unbounded over an attempt. Slice 4
  must size or retire completed request entries before production arming.
- Synchronous journal I/O occurs on the runtime thread once per POST; decide
  whether to move it behind a blocking seam before production arming.
- Concurrent prompts on one API session now refuse instead of interleaving;
  no reachable production concurrent caller was found.
- Poisoned API session state maps to `ResourceFlightUnsupported`, losing
  diagnostic precision.
- Schema-v1 request-ID reading is narrower than the former string wire, but no
  pre-change production writer or persisted sample was found.
- `DispatchStarted` precedes send-future installation; a later diagnostic
  refusal can conservatively settle Unknown although no future was polled.

## Stable evidence and next state

Verbatim lens records are adjacent. Scratch SHA-256 values:

- review brief:
  `e1150a772507da059e85fbd49153b5acf99c70ad19c85ba25d0b3002a55efd7d`
- Opus report:
  `5471489a6181c9dd79970df7319758019fd064b16967ed328800f62508129b17`
- Sol/max report:
  `1c7394367d9ba7284c76220fd5dfbc4615a049a83ba8046916a1e8ffd126156a`

Next: dispatch R1-R6 and C1-C4 as the one declared repair round on the existing
3c2 feature artifact. Production V3 remains unarmed throughout.

