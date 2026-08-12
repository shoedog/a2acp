# R2f1b 3c2 dual-lens — Sol/max report (verbatim)

Artifact: `feat/r2f1b-3c2-api-authority` @
`772518a8aaeaa6c52a2bbd445123fc945aeb8056` (base
`42249b3d926b49afd9d0dbd213d0ee3d3e459af6`).

Lens: `gpt-5.6-sol`, effort `max`, hard read-only ACP mode, independent
single-node `run-workflow code-review`. Workflow execution
`exec-76081ae34b8d4aaa8709604ca96a5775`, attempt
`attempt-05bf81b299c871be63ce60c3797600d6`; typed terminal
`completed / cleanup complete`. Input SHA-256
`e1150a772507da059e85fbd49153b5acf99c70ad19c85ba25d0b3002a55efd7d`;
report SHA-256
`1c7394367d9ba7284c76220fd5dfbc4615a049a83ba8046916a1e8ffd126156a`.

The following output is preserved verbatim. Operator adjudication is in
`2026-08-12-r2f1b-3c2-dual-adjudication.md`.

---

I’ll conduct this as a hard read-only review of the frozen `42249b3d..772518a8` artifact. I’m using the LSP and Prism navigation skills to verify type-resolved references and structural production callers, then I’ll read all ten changed files and the cited specifications line-by-line; I will not build, test, edit, or access anything outside this repository. I’m declaring one review pass with no retry rounds.The configured LSP/Prism navigation surfaces are not callable in this session, so I’m taking the required fallback: bounded read-only Git/search inventories, followed by direct inspection of every reachable call site. I found no task-specific prior-memory entry for 3c2, so the review will stand entirely on this checkout’s frozen evidence.The full ten-file diff is now inventoried. The central implementation establishes per-turn epochs, exact per-request cancel capabilities, durable request scopes, and a V3-only attachment gate; I’m now tracing every prompt exit and the journal’s recovery/terminal paths to distinguish true orphaning or stale-authority failures from merely imaginable races.Two concrete authority failures are now established from executable callers, not from the handoff: post-acceptance flight errors are surfaced as transient `AgentCrashed` and can replay an already accepted round, and the checked forget/release defaults can publish cleanup `Complete` before the active request flight reaches its durable terminal. I’m finishing the persistence-orphan and test-sensitivity audit before classifying severity and disposition.The remaining audit confirms the implementation’s exact-request guards are locally strong, but they do not close the lifecycle around session-state reuse or durable refusal/recovery. I’m pinning the final source anchors now, then I’ll issue the requirement-by-requirement verdict.The classification is now stable: five constructible correctness failures remain under a V3-injected `ApiBackend`; the default binary is still unarmed, so these are activation blockers rather than regressions in today’s `AgentKind::Api` path. I also found three bounded test/API compatibility smells that do not independently block.One reachability refinement from the base comparison: the forget/recreate ABA is not limited to future V3 activation. In LegacyV2, removal resets both the turn epoch and the legacy request counter, so stale A can exactly match and clear B as `(epoch=1, request=1)` in today’s API path. I’ll reflect that higher exposure in the final prioritization.Rejected. Five correctness blockers remain; one also affects today’s LegacyV2 API path.

## WRONG findings

### 1. WRONG — BLOCKER: forget/recreate permits stale turn A to clear successor B

Constructible sequence: A is active under session `S`; `forget_session(S)` signals A and removes its `SessionState`; B immediately reattaches/restarts with the same `SessionId`. The replacement state resets its epoch and legacy request counter to zero at [backend.rs:249](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:249). B therefore becomes epoch 1; in LegacyV2 it is also request 1. When A unwinds:

- A’s stale request capability exactly matches and clears B in LegacyV2 at [backend.rs:315](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:315).
- In both LegacyV2 and ProtectedV3, A’s `TurnScope::drop` sees B’s reused epoch 1 and clears B’s turn state at [backend.rs:269](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:269).
- A subsequent `cancel(S)` returns without signaling B because `current_turn_epoch` is gone at [backend.rs:1176](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1176). B completes instead of cancelling.

Trigger: cleanup/forget overlaps an in-flight API stream, followed by rapid same-ID reuse before the old future drops. Likelihood: **plausible**; `forget_session` returns immediately after removal at [backend.rs:1243](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1243), with no tombstone or join invariant. Exposure: current LegacyV2 API agents and future V3 agents; impact is cross-turn authority loss, an unresponsive cancellation, and potentially continued provider cost/effects.

Fix: give each map incarnation a backend-global monotonic nonce and compare `(incarnation, turn_epoch, request_identity)` everywhere, or retain a tombstone until all old scopes settle before allowing reuse. Cost/blast radius: medium, localized to API session/cancellation state and tests. Red regression: delay A and B, forget A, recreate B with the same `SessionId`, let A unwind, then prove A cannot clear B and one current cancel settles B `Partial`.

Confidence: **99/100**. A deterministic race test would raise it to 100; a proven caller invariant forbidding reuse until every old stream is dropped would lower it; an incarnation/tombstone fence would collapse it.

### 2. WRONG — BLOCKER: post-acceptance flight errors are transient and can replay the prompt

After round A may have reached the provider, a round-B reservation, identity, journal, dispatch, or settlement failure escapes through raw `?` at [backend.rs:716](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:716) or [backend.rs:730](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:730). All typed flight errors become `AgentCrashed` at [backend.rs:519](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:519).

`AgentCrashed` is transient at [error.rs:214](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/error.rs:214), so a retry-enabled cold workflow retries the whole attempt at [executor.rs:4119](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-workflow/src/executor.rs:4119). It also records `prompt_may_have_been_accepted=false`, because terminal projection recognizes only structured `AgentFailure` at [executor.rs:222](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-workflow/src/executor.rs:222).

Observable result: A’s POST/provider/tool effect can execute twice, while terminal evidence falsely says no prompt may have been accepted. The new round-two journal-failure test only asserts “some error” and one direct-call POST at [backend.rs:1865](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1865); it does not exercise executor retry or inspect the diagnostic.

Trigger: V3 route, at least one accepted tool round, retry enabled, followed by journal loss, ENOSPC/I/O refusal, collision, or terminal-CAS failure. Likelihood: **plausible** operationally, though uncommon on healthy storage. Exposure: future/injected V3 workflow runs; severity is high because provider and billable effects can duplicate.

Fix: route every post-barrier admission/dispatch/settlement error through an acceptance-aware fatal `AgentFailure`, while preserving an accurate pre-first-send classification. Cost: small-to-medium, primarily `bridge-api` diagnostics plus an executor regression. Red regression: inject the existing round-two journal failure into a retry-enabled workflow and assert one accepted POST, no retry, `Fatal`, and `prompt_may_have_been_accepted=true`; add a terminal-append failure after the first POST.

Confidence: **99/100**. An executed retry-level fault test would raise it; proof that every possible V3 caller disables cold retry would lower it; fatal acceptance-aware mapping plus the regression would collapse it.

### 3. WRONG — BLOCKER: checked cleanup reports `Complete` before request custody settles

`ApiBackend` overrides only the void `forget_session`. Therefore `forget_session_checked` calls it and immediately returns `Complete` through the trait default at [ports.rs:445](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/ports.rs:445); `release_session_checked` behaves equivalently at [ports.rs:468](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/ports.rs:468).

Constructible state: a V3 request is still awaiting the provider, or its terminal append has refused. Checked cleanup signals/removes the session and returns `Complete` before the request publishes a durable winner—or even when it can never do so. Production cold cleanup calls these observed checked methods at [executor.rs:1355](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-workflow/src/executor.rs:1355), and projects `Complete` as successful cleanup at [executor.rs:845](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-workflow/src/executor.rs:845).

Trigger: cleanup concurrent with an active request, or cleanup following a settlement/journal failure. Likelihood: **plausible** once V3 is armed. Exposure: workflow terminal/custody consumers; impact is a false destructive-success assertion over an unresolved provider effect.

Fix: override checked/observed forget and release. Capture a cloneable exact settlement observer before signaling, await the durable winner outside the session lock, and return only genuine `Complete`; terminal refusal or unresolved work must project `Unknown`/`Retained`. A smaller safe interim fix is to return `Unknown` whenever an active request cannot be joined. Cost: medium across API state and the flight join seam. Red regression: call checked forget/release during a delayed request and prove it cannot report `Complete` before one terminal CAS; inject terminal failure and require a protective disposition.

Confidence: **99/100**. A production-level delayed-cleanup test would raise it; a contract redefining `Complete` as merely “signal attempted” would lower it, but contradicts the enum’s custody semantics; checked overrides that join/project the winner would collapse it.

### 4. WRONG — BLOCKER: dispatched request flights have no reachable recovery after crash or terminal refusal

A consumer/drop settlement error is ignored at [backend.rs:367](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:367) and again in the durable wrapper at [process.rs:826](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/process.rs:826). Terminal append failure becomes sticky at [retained_resource_flight.rs:1396](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1396), after which the active slot is still cleared and no caller retains a settlement capability.

The recovery primitive requires an already-known flight ID at [retained_resource_flight.rs:1588](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1588). The journal trait exposes no reservation/flight enumeration at [retained_resource_flight.rs:317](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:317), and registry recovery runs only when the same key is reserved again at [retained_resource_flight.rs:1953](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1953). Normal successor requests deliberately mint new keys, so they never recover the abandoned request.

Constructible result: crash after `IntentJournaled`/`DispatchStarted`, or fail terminal append after the provider may have accepted the POST. The journal permanently lacks a terminal row and no aggregation is published.

Trigger: process restart/crash or persistent/transient journal failure. Likelihood: **plausible** over long-running deployments. Exposure: V3 requests whose provider outcome is uncertain; severity is high because durable custody never reaches a closed result.

Fix: enumerate exact reservation records during attempt recovery and terminalize journaled request intents as `Unknown` through the existing CAS before admitting new provider effects; retain a join/recovery token when live settlement refuses. Cost/blast radius: medium-to-large in the journal/attempt recovery boundary, without reconstructing dispatch authority. Red regression: reopen an attempt after abandoning a dispatched request and prove exactly one `Unknown` terminal and one publication, with no POST capability reconstructed.

Confidence: **96/100**. A deterministic reopen test would raise it; discovery of an existing production reservation enumerator would lower it—repository-wide search found none; integrated attempt-start recovery would collapse it.

### 5. WRONG — BLOCKER: capacity refusal leaves an unrecoverable durable reservation

The file journal durably creates the key reservation at [retained_resource_flight.rs:657](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:657). Only afterward does the registry create the flight at [retained_resource_flight.rs:1974](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1974), whose first `FlightReserved` row is appended at [retained_resource_flight.rs:1047](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:1047).

With the added test’s capacity of four, admission requires five entries and refuses at [retained_resource_flight.rs:406](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/retained_resource_flight.rs:406). The `RetainedResourceFlight` is never inserted, but the reservation file remains. Reopening the same key finds no intent and returns `ReservationUnavailable`; normal requests mint different keys. The test at [backend.rs:1919](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1919) verifies no POST/publisher call but never inspects the durable reservation.

Trigger: an undersized journal cap or failure writing the initial event after reservation creation. Likelihood: **plausible** under operator misconfiguration; repeated refusals create repeated permanent files. Exposure: V3 attempt custody and journal storage; impact is a durable orphan and an unclosable reserved identity, although provider work is correctly prevented.

Fix: make reservation plus initial `FlightReserved` one atomic admission operation, or add a conditional exact-record rollback that is safe only when no flight row became durable. Cost: medium in the journal CAS boundary. Red regression: run the capacity-four refusal, inspect/reopen the journal, and require either no reservation or a closed durable `Failed` result.

Confidence: **99/100**. Filesystem inspection would raise it; an unseen cleanup path could lower it—none exists in the repository; atomic admission/rollback would collapse it.

## SMELL findings

### 1. SMELL — DEFER: guard and exit tests are not fully mutation-sensitive

The stale-round test at [backend.rs:1638](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1638) detects removal of the identity comparison because A and B share one epoch, but cannot detect removal of the epoch comparison. The fresh-turn test waits for A to finish and gets a distinct legacy request number, so identity alone still rejects the stale capability.

Likewise, the between-round test at [backend.rs:1793](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:1793) cancels before B begins admission. It detects removal of the first cancellation check, but not removal of the post-reservation check because that check is never reached in the tested cancellation schedule.

The V3 suite also lacks durable-disposition assertions for send, rejected HTTP/error-body read, SSE read/frame/incomplete EOF, and non-stream read/parse failures. Existing diagnostic tests such as [r2b3_api_diagnostics.rs:335](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/tests/r2b3_api_diagnostics.rs:335) run LegacyV2 and would remain green if V3’s explicit `Failed` settlements degraded to Drop’s `Unknown`.

Trigger: guard removal or exit-path refactoring. Likelihood: **plausible**; exposure is future regression detection, with medium custody impact. Fix: add barriers inside ID mint/journal binding and table-driven V3 fault tests for every exit. Cost: small-to-medium, tests only. Red mutations: independently remove identity, epoch, first cancellation check, second check, and each explicit disposition.

DEFER because it establishes coverage weakness, while the current incorrect behaviors are already captured by the WRONG blockers. Confidence: **98/100**. Mutation execution would raise it; hidden tests could lower it; independent guard/failure tests would collapse it.

### 2. SMELL — DEFER: a public test seam can bypass bridge-minted identity

`RemoteRequestIdSource` is publicly re-exported at [lib.rs:9](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/lib.rs:9), and any caller can replace the CSPRNG source through [backend.rs:567](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-api/src/backend.rs:567). Because `DedicatedRemoteRequestIdV1::parse` accepts bounded opaque strings at [resource_flight.rs:62](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/resource_flight.rs:62), an external integrator can inject deterministic or sensitive IDs into new journal records.

Trigger: a library caller uses the apparent extension API outside tests. Likelihood: **rare**; all in-repository uses are tests and the production constructor uses the system source. Exposure: such integrators; impact is collision or diagnostic/journal data exposure.

Fix: make the seam private/test-only, or inject entropy bytes below a single canonical minting function. Cost: small API cleanup. Red regression: an external compile-fail visibility test plus a runtime assertion that production IDs always have canonical bridge shape.

DEFER because no in-repository production caller bypasses minting. Confidence: **91/100**. An external production caller would raise it; a documented public deterministic-ID contract would lower it; restricting the seam would collapse it.

### 3. SMELL — DEFER: schema-v1 reading is narrower than the prior wire type

Before this range, schema-v1 represented dedicated request IDs as unrestricted `String`. The new transparent deserializer validates through [resource_flight.rs:90](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/crates/bridge-core/src/resource_flight.rs:90), rejecting spaces, Unicode, values over 128 bytes, and malformed `remote-request-*` strings. Thus an old schema-valid row can become undecodable after upgrade.

Trigger: an external or hand-authored pre-change schema-v1 request record containing such an ID. Likelihood: **rare**; repository search found no pre-change production writer. Exposure: external journal consumers; impact is blocked recovery.

Fix: retain a permissive legacy wire deserializer while validating all new writes, or version/migrate the schema explicitly. Cost: small-to-medium serialization work. Red regression: decode a prior schema-v1 fixture such as an ID containing a space while proving new minting remains canonical.

DEFER because no reachable in-repository producer for such old records was established. Confidence: **89/100**. A real persisted sample would raise it; a documented historical character constraint would lower it; a compatibility reader or migration would collapse it.

## Mechanism verdict for requirements 1–10

| Req. | Verdict | Evidence |
|---|---|---|
| 1 | PARTIAL | Default minting, typed key/identity, active-slot carriage, mismatch refusal, and collision refusal are exact; the public source seam can bypass bridge minting. |
| 2 | FAIL | Normal live rounds use the attempt’s one registry and return the terminal CAS winner, but reservation/terminal failures can become unrecoverable. |
| 3 | FAIL | Provider admission is correctly blocked, but capacity and terminal failures can leave durable reservations or flights orphaned. |
| 4 | FAIL | Normal, tool, max-round, cancel, drop, and forget paths have explicit dispositions; checked cleanup, forget/recreate, terminal refusal, and restart do not close safely. |
| 5 | PARTIAL | The two-round negative proves identity fencing and current B cancellation. It does not prove epoch fencing or the post-reservation cancellation guard. The between-round test proves no mint/POST only when cancellation precedes admission. |
| 6 | PASS for live execution | Two rounds bind distinct keys/flight IDs to the same node/session owner, publish one live winner each, and do not add slice-5 collateral authority. |
| 7 | PASS with deferred compatibility smell | Source retains schema `1`, `LIFECYCLE_SLOTS=4`, and `PROCESS_LIFECYCLE_SLOTS=7`; process/container goldens remain asserted and strict new-field negatives cover top-level and nested forms. |
| 8 | PASS | Route absence remains LegacyV2; route presence exposes ProtectedV3 and requires attachment; the production constructor explicitly supplies `None` at [main.rs:1624](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/s3c2/bin/a2a-bridge/src/main.rs:1624). |
| 9 | FAIL | Post-acceptance flight errors escape as transient `AgentCrashed`, permitting replay and losing acceptance evidence. |
| 10 | PASS | The zero-round path creates no request identity, reservation, flight, or POST; its direct test asserts all four observables. |

Evidence assessment: I read the full committed ten-file diff and changed tests, then traced retry, checked cleanup, registry/journal persistence, recovery, and terminal projection. The production V3 route remains unarmed, but WRONG 1 also reaches current LegacyV2. I did not build or run tests, per the hard read-only contract; supplied green totals and the handoff are therefore supplied evidence, not independently verified gates. The out-of-repository frozen task file was not read because the contract restricted reads to this repository. Overall review confidence: **98/100**.

VERDICT: REJECT
SUMMARY: Five BLOCKERs remain: forget/recreate ABA, post-acceptance replay, false checked-cleanup completion, unreachable request recovery, and durable reservation orphaning.
