# R2f1a focused implementation boundary — profiles, fan-out policy, and per-node control

- **Status:** PARKED — final closure review rejected four closed blockers; no implementation has started
- **Frozen base:** `3f35ee6e07e9af314bb548b9d3ab694f3bba5fb1`
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Normative authority:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)
- **Parent plan:** [`2026-07-11-r2f-phase-aware-liveness.md`](2026-07-11-r2f-phase-aware-liveness.md)
- **Sol input:** `374ee10f8c4db570277c81803ad65e84520bb3f2aa0294a6e75057e1468ae9d6`
- **Fable input:** `d612788847a9142172cb38080bc77568e23c89116f44153ec0376b17327ce8c0`
- **Synthesis:** `644c2df21579bcb3dc9e07f347911f1516ebf61d6c0b9493433d117d83070a84`

This document freezes the proposed source boundary for adversarial review. It narrows, but does not replace, the
approved owner design and parent plan. It does not authorize implementation until its review findings are
adjudicated and the status is advanced.

## Dogfood and synthesis evidence

The operator built exact clean base `3f35ee6e07e9af314bb548b9d3ab694f3bba5fb1` in release mode. The resulting
30,219,840-byte bridge candidate had SHA-256
`5aa69467f179d187e24f897ae5128e4bf2ebe0363e69a9eeca425f11de526680`. Final preflight bound host
`@agentclientprotocol/codex-acp=1.1.7` to nested Codex `0.145.0`, and host
`@agentclientprotocol/claude-agent-acp=0.63.0` to Agent SDK `0.3.220` plus bundled Claude Code `2.1.220`.
The exact advertised selections were Codex `gpt-5.6-sol[xhigh]` / `xhigh` / `read-only` and Claude
`claude-fable-5[1m]` / `xhigh` / `plan`.

The initial three-node workflow used execution `exec-056b9536600e8b50ba4221d342a494f7` and attempt
`attempt-0b74a17edf795c75d70c8660ff7f5a6c`. Its independent Sol and Fable nodes both completed. Their final
assistant responses were recovered exactly from their local agent-session records because offline workflow
execution persisted only workflow-level history, not node checkpoints:

- Sol: 36,087 bytes, SHA-256 `374ee10f8c4db570277c81803ad65e84520bb3f2aa0294a6e75057e1468ae9d6`;
- Fable: 35,134 bytes, SHA-256 `d612788847a9142172cb38080bc77568e23c89116f44153ec0376b17327ce8c0`.

The Fable root session used internal Claude helper sessions. They remained inside the Fable lane and did not see
the Sol design, but this departs from the routing preference for a helper-free clean-room lens and is disclosed for
review rather than silently normalized away.

The initial Sol synthesis node received unresolved `{{executability}}` / `{{structure}}` placeholders because the
workflow named its upstream nodes `sol-design` / `fable-design`. Its 601-byte artifact, SHA-256
`6e930d1e78e7465563d34e4b5348096cedada08eea2e7287456787a8e31fec86`, correctly returned `BLOCKED` and is
inadmissible as design synthesis. A synth-only correction first refused before provider dispatch because its prompt
omitted the typed `{{input}}` marker; it created no workflow-history row, Codex session, or output file. After that
closed configuration defect was corrected, execution `exec-de1e138e488819ed88a7dbafe85dc859`, attempt
`attempt-1f6126c65a44b6782142f9161ee0ac20`, consumed only the two hash-bound designs and produced the 38,110-byte
READY synthesis at the hash recorded above.

These are billable design artifacts, not compatibility, provider-health, deterministic scheduler, release, or
production-operator evidence. The agents made no repository edit and ran no build or test. No incident was replayed,
no provider fallback occurred, and the long-lived served operator was neither restarted nor mutated.

The initial Sol/xhigh adversarial review ran as execution `exec-347e659294b49009a2c821e7bd4f369e`, attempt
`attempt-2d7aaf7acb949ab4e067037479c2041c`. Its 20,459-byte result had SHA-256
`8a8e76aecfd7b4e95b5ab187de20de3cb924fe4060861a16f177b89c4c1ee9f3` and rejected the first boundary with
five closed WRONG findings plus four DEFER smells. All nine were adjudicated as valid and are incorporated below:
trigger selection now precedes its first durable terminal, every provider attempt is bound to a frozen effective
node identity, fresh and resumed batch are owned, failure class is represented, causal node identity is bounded by
graph-bound references, Max overlay and trigger-barrier semantics are explicit, every durable outcome has projection
evidence, and history accounting version 2 has an exact equation. The review node completed despite optional
workflow-telemetry corruption and an unavailable ambiguous-language LSP warmup; neither condition altered its retained
terminal artifact. At that checkpoint, a single targeted closure review remained as the design gate.

That targeted closure review ran as execution `exec-f10d1668ceda06a01f4783a07ea47ea6`, attempt
`attempt-ea14ec07374939ff5c44faea7560a829`. Its 18,270-byte result had SHA-256
`7b6fd5af514192510cc1455b031e6333e148a41873502ce8bd3deca297dedb3c` and rejected the repaired boundary with
four closed WRONG blockers: provider-attempt identity must bind preflight/fallback selection and the exact resolved
entry; the node-terminal reserve must bound worst-case encoded JSON rather than raw cause bytes; arbitrary node IDs
must not be undercharged through a duplicated SQLite key; and a healthy offline history ledger needs its own durable
trigger-commit barrier result. The declared design-review cap is reached. This checkpoint records the rejected state;
it does not authorize implementation or another review round.

## 1. Frozen authority and slice boundary

D1–D11 remain settled:

| Decision | R2f1a treatment |
|---|---|
| D1 fan-out policy | Implement `bounded_independent` and event-driven `fail_fast`; implement `fixed_grace` structurally and under fake/manual time only. Preserve strict versus degraded synthesis and every node disposition. |
| D2 three clocks | Check in the vocabulary and exact profile values. Do not arm activity, progress, warning, grace, queue, or work-deadline timers in production. |
| D3 budget construction | Resolve, freeze, fingerprint, and persist the complete budget before provider/session effects. Invocation profile outranks checked-in workflow profile/task class, which outranks compatibility omission. |
| D4/D4.1 profiles | Check in the exact provisional legacy and review profiles. Max requires a bounded reason and an explicitly larger finite work cutoff. |
| D5 takeover | No takeover authority, process signaling, or public node-cancel API. |
| D6 telemetry | Keep workflow-summary telemetry fail-open. Primary task state remains independent and first. Structured node evidence uses bounded reservations where a history row is admitted. |
| D7–D10 runtime health, drain, deployment | No change. |
| D11 bounds | Persist 31-second control, six-second cancellation observability, 60-second cleanup, and ten-second reporting values as policy data only. Do not claim they are newly enforced. |

R2f1a therefore ships:

1. Versioned profile, task-class, policy, synthesis, trigger, and node-terminal types.
2. Deterministic config and invocation-profile resolution.
3. Fail-before-effects validation and a frozen V2 run specification.
4. One workflow cancellation source and one child source per running node.
5. Current-style `bounded_independent`.
6. Real event-driven `fail_fast`.
7. A complete fixed-grace controller exercised only with fake/manual time.
8. Structured dependency inputs, terminal/cleanup records, and `completed_degraded`.
9. Persistence, replay, resume, and projection of those facts.

R2f1a does not ship any production timer. In particular, a production execution selecting `fixed_grace` is refused during admission before task/history/session/provider effects. It is not silently treated as bounded-independent, fail-fast, or “deferred” execution. R2f1b activates fixed-grace expiry and outer deadlines only after preservation and cleanup-ownership prerequisites are green.

## 2. Divergence adjudication

| Divergence | Resolution |
|---|---|
| `liveness.rs` versus a new module | Use `crates/bridge-core/src/execution_policy.rs`. Existing `bridge_core::liveness` owns flock-based container liveness and must not mix policy semantics with OS lease ownership. |
| Seconds versus milliseconds | Use integer milliseconds throughout persisted policy. This represents D4.1 exactly and permits the smallest valid Max cutoff, `7_200_001` ms. |
| Invocation profile channel | Ship it across offline CLI, served A2A metadata, and MCP. D3 explicitly gives invocation profile first precedence. Do not add an invocation task-class override; an invocation profile supplies its mapped class. |
| Production fixed grace | Refuse it before effects in R2f1a. Accepting it while arming no expiry would implement neither fixed grace nor a bounded policy. |
| Trigger selection | Drain the currently ready completion batch, sort by `NodeId`, and select the lowest qualifying failed/timed-out node. Raw `FuturesUnordered` delivery order is not durable determinism. |
| Run-spec persistence | Persist the fully resolved controls in snapshot V2. Do not re-resolve checked-in constants from declarations on resume. |
| Fingerprint compatibility | Always include canonical resolved controls in new R2f1a workload fingerprints. Preserving the old hash would conflate pre-policy and bounded-policy calibration populations. |
| `completed_degraded` task status | Keep `tasks.status = completed`; persist an additive `workflow_outcome = completed_degraded`. Do not introduce an old-binary-unknown task-status token. |
| Node persistence | Use versioned terminal JSON on task checkpoints and normalized history node-terminal rows. Boolean `ok` remains a legacy compatibility projection only. |
| Legacy `ok=false` | Decode as `interrupted_legacy / unknown_legacy`, not a fabricated ordinary failure, timeout, or completed cleanup. |
| Trigger persistence | Persist the trigger before policy cancellation through a dedicated barrier and also attach it additively to the triggering `NodeFinished`. Do not add an old-reader-unknown journal event variant. |
| Strict/degraded outcome | Use explicit transitive `degraded_ancestry`; never infer failure or degradation by parsing output text. |

## 3. Domain and schema types

Add `crates/bridge-core/src/execution_policy.rs` and export it from `bridge-core/src/lib.rs`.

```rust
pub const EXECUTION_POLICY_SCHEMA_V1: u16 = 1;
pub const PROFILE_LEGACY_BOUNDED_V1: &str = "legacy_bounded_v1";
pub const PROFILE_REVIEW_HIGH_XHIGH_V1: &str = "review_high_xhigh_v1";

pub enum LivenessProfileIdV1 {
    LegacyBoundedV1,
    ReviewHighXhighV1,
}

pub enum TaskClassV1 {
    Other,
    ReviewHighXhigh,
}

pub enum SilenceCutoffV1 {
    None,
}

pub struct LivenessProfileV1 {
    pub schema_version: u16,
    pub id: LivenessProfileIdV1,
    pub queue_wait_ms: u64,
    pub control_observable_ms: u64,
    pub no_progress_snapshot_ms: u64,
    pub silence_cutoff: SilenceCutoffV1,
    pub work_cutoff_ms: u64,
    pub cancel_observable_ms: u64,
    pub cleanup_tail_ms: u64,
    pub reporting_tail_ms: u64,
    pub terminal_bound_ms: u64,
}

pub struct MaxQualificationV1 {
    pub work_cutoff_ms: u64,
    pub reason: BoundedReasonV1,
}

pub enum FanOutPolicyV1 {
    BoundedIndependent,
    FailFast,
    FixedGrace { grace_ms: u64 },
}

pub enum SynthesisModeV1 {
    Strict,
    Degraded,
}

pub enum ProfileSelectionSourceV1 {
    Invocation,
    WorkflowProfile,
    WorkflowTaskClass,
    CompatibilityOmission,
}

pub enum DeadlineActivationV1 {
    ManualOnlyR2f1a,
}

pub struct FrozenWorkflowControlsV1 {
    pub schema_version: u16,
    pub task_class: TaskClassV1,
    pub profile: LivenessProfileV1,
    pub profile_source: ProfileSelectionSourceV1,
    pub max_qualification: Option<MaxQualificationV1>,
    pub fan_out: FanOutPolicyV1,
    pub synthesis: SynthesisModeV1,
    pub deadline_activation: DeadlineActivationV1,
}
```

`FrozenWorkflowControlsV1::effective_work_cutoff_ms()` returns the Max qualification cutoff when present and the
named profile cutoff otherwise. `effective_terminal_bound_ms()` checked-adds the frozen 60,000-ms cleanup and
10,000-ms reporting tails to that effective cutoff. No consumer reads `profile.work_cutoff_ms` directly when a Max
qualification is present.

Both named profiles contain:

| Field | Value |
|---|---:|
| `queue_wait_ms` | `1_800_000` |
| `control_observable_ms` | `31_000` |
| `no_progress_snapshot_ms` | `1_800_000` |
| `silence_cutoff` | `none` |
| `work_cutoff_ms` | `7_200_000` |
| `cancel_observable_ms` | `6_000` |
| `cleanup_tail_ms` | `60_000` |
| `reporting_tail_ms` | `10_000` |
| `terminal_bound_ms` | `7_270_000` |

The IDs remain distinct even though their V1 numbers match. Changing a named profile value requires a new profile ID.

```rust
pub enum NodePrimaryDispositionV1 {
    Completed,
    Failed,
    TimedOut,
    CanceledWorkflow,
    CanceledPolicy,
    CanceledNode,
    SkippedDependency,
    NotStartedPolicy,
    InterruptedLegacy,
    Deadline, // reserved; no R2f1a production producer
}

pub enum NodeCleanupDispositionV1 {
    Complete,
    Failed,
    NotNeeded,
    UnknownLegacy,
}

pub struct NodeCleanupV1 {
    pub disposition: NodeCleanupDispositionV1,
    pub duration_ms: u64,
}

pub struct NodeCauseV1 {
    pub failure_class: NodeFailureClassV1,
    pub code: StaticBoundedCodeV1,                    // at most 64 bytes
    pub deepest_cause: Option<String>,                // sanitized, at most 512 bytes
    pub dependency_set: Option<DependencySetRefV1>,   // count plus SHA-256
}

pub struct PolicyNodeRefV1 {
    pub sorted_ordinal: u32,
    pub id_sha256: Sha256HexV1,
}

pub struct DependencySetRefV1 {
    pub count: u32,
    pub sorted_node_refs_sha256: Sha256HexV1,
}

pub struct NodeTerminalV1 {
    pub schema_version: u16,
    pub primary: NodePrimaryDispositionV1,
    pub cleanup: NodeCleanupV1,
    pub cause: Option<NodeCauseV1>,
    pub prompt_may_have_been_accepted: bool,
    pub degraded_ancestry: bool,
    pub policy_trigger_id: Option<ControlEventIdV1>,
}

pub struct PolicyTriggerV1 {
    pub schema_version: u16,
    pub id: ControlEventIdV1, // attempt id plus ordinal; R2f1a permits at most one
    pub node: PolicyNodeRefV1,
    pub policy: FanOutPolicyNameV1,
    pub grace_ms: Option<u64>,
}

pub enum WorkflowRuntimeOutcomeV1 {
    Completed,
    CompletedDegraded,
    Failed,
    Canceled,
}

pub enum WorkflowDurableOutcomeV1 {
    Completed,
    CompletedDegraded,
    Failed,
    Canceled,
    Interrupted,
}
```

Usage remains the existing separately bounded `UsageSnapshot` checkpoint field rather than being duplicated inside the 1,024-byte node-terminal JSON.

For `BridgeError::AgentFailure`, construct the cause from `FailureDiagnosticWire`, retaining its static code, deepest sanitized cause, closed failure class, and sticky prompt-acceptance bit. Other errors map through bridge-owned static codes, a closed non-diagnostic failure class, and `client_message()`, never `Debug` text.

`PolicyNodeRefV1` is canonical within one frozen graph: nodes sort by the exact `NodeId` bytes, receive a checked
`u32` ordinal, and bind the full ID by SHA-256. Decode verifies both fields against the graph. Full node IDs remain in
the graph and node-row key; terminals and triggers never truncate an ID. `DependencySetRefV1` binds the sorted set of
direct non-completed inputs. Its exact members are recomputed from the frozen graph and terminal map, so arbitrary ID
length or fan-in cannot inflate the 1,024-byte terminal JSON. The run-spec resolver refuses ordinal overflow before
effects.

Add in `crates/bridge-workflow/src/graph.rs`:

```rust
pub struct WorkflowControlDefaultsV1 {
    pub task_class: Option<TaskClassV1>,
    pub liveness_profile: Option<LivenessProfileIdV1>,
    pub fan_out: Option<FanOutPolicyV1>,
    pub synthesis: Option<SynthesisModeV1>,
    pub max_qualification: Option<MaxQualificationV1>,
}

pub struct FrozenNodeExecutionIdentityV1 {
    pub node: PolicyNodeRefV1,
    pub agent: AgentId,
    pub effective_model: Option<String>,
    pub effective_effort: Option<Effort>,
    pub effective_mode: Option<String>,
    pub identity_fingerprint: Sha256HexV1,
}

pub struct WorkflowRunSpecV1 {
    pub schema_version: u16,
    pub graph: WorkflowGraph,
    pub controls: FrozenWorkflowControlsV1,
    pub node_execution_identities: Vec<FrozenNodeExecutionIdentityV1>,
    pub controls_fingerprint: String,
    pub workload_fingerprint: String,
}
```

`WorkflowGraph` carries an additive optional declared-control block. Production executor entry points take `Arc<WorkflowRunSpecV1>`, not a bare graph plus optional defaults, so a production constructor cannot bypass freezing accidentally.

## 4. Configuration and invocation surfaces

Extend `WorkflowToml` in `bin/a2a-bridge/src/config.rs`:

```toml
[[workflows]]
id = "code-review"
task_class = "review_high_xhigh"
liveness_profile = "review_high_xhigh_v1"
fan_out_policy = "bounded_independent"
synthesis_mode = "degraded"
```

Fixed grace has no implicit duration:

```toml
fan_out_policy = "fixed_grace"
fixed_grace_ms = 30000
```

Max qualification is explicit:

```toml
max_work_cutoff_ms = 10800000
max_reason = "concurrency proof requires one tightly connected review"
```

Rules:

- `fixed_grace_ms` is required only for `fixed_grace`.
- It is forbidden for the other policies.
- It must be in `1..=effective_work_cutoff_ms`.
- `max_work_cutoff_ms` and `max_reason` are required when any effective node effort is Max.
- Max cutoff must be strictly greater than `7_200_000` ms.
- Max reason is trimmed, nonempty, sanitized, and at most 512 UTF-8 bytes.
- Max fields on a non-Max workflow are rejected as unused.
- The Max cutoff/reason pair is atomic at each source. An invocation supplies both or neither; a complete invocation
  pair replaces any complete workflow pair. Fields never mix across sources. A partial pair at either source refuses;
  a complete overridden workflow pair is retained only as declaration evidence and does not affect effective values.
- Arbitrary mutation of a named profile is not supported. A different non-Max budget requires a new checked-in profile ID.
- Every current graph node is policy-required. Optional-node trigger semantics are not added in R2f1a.

True omission resolves to:

```text
task_class = other
profile = legacy_bounded_v1
fan_out = bounded_independent
synthesis = degraded
deadline_activation = manual_only_r2f1a
```

Built-in review, spec-review, plan-review, design, and implement-review workflows declare `review_high_xhigh` and `review_high_xhigh_v1` explicitly. Other custom and implementation workflows may retain omission.

Invocation overrides:

```text
CLI:
  --liveness-profile <id>
  --max-work-cutoff-ms <u64>
  --max-reason <bounded text>

A2A metadata:
  a2a-bridge.liveness-profile
  a2a-bridge.max-work-cutoff-ms
  a2a-bridge.max-reason

MCP run_workflow arguments:
  liveness_profile
  max_work_cutoff_ms
  max_reason
```

Extend `bridge-coordinator/src/params.rs::OpParams`, `bridge-core/src/domain.rs::TaskMeta`, `task_meta_from_params`, the served workflow routes, and MCP parsing. Workflow routes continue rejecting agent/model/effort/mode overrides but must no longer strip liveness overrides.

Selection is:

1. Invocation profile, which supplies its mapped task class.
2. Explicit workflow profile, which must agree with any explicit workflow task class.
3. Workflow task class mapped to its named profile.
4. Compatibility omission.

Unknown, empty, wrong-typed, or internally inconsistent explicit values refuse. Nothing is inferred from workflow ID, agent name, prompt text, or model name.

## 5. Validation and freeze order

Use one pure `resolve_execution_policy` function from config validation, offline admission, served admission, MCP, and resume safety checks.

Order:

1. Deserialize TOML/JSON and reject unknown fields or wrong value types.
2. Validate workflow, node, prompt, and agent IDs and duplicates.
3. Validate closed profile, task-class, fan-out, and synthesis vocabularies.
4. Validate field combinations and true-omission semantics.
5. Validate the DAG, references, acyclicity, and exactly one terminal.
6. Resolve the profile and checked arithmetic.
7. Inspect effective configured effort without resolving or spawning a backend.
8. Validate Max qualification.
9. Validate retry counts and critical-path retry backoff.
10. Freeze each node's configured effective model/effort/mode and graph-bound execution-identity fingerprint; then
    construct the controls fingerprint and control-inclusive workload fingerprint from those frozen values.
11. Refuse inactive production behavior, including `fixed_grace` in R2f1a.
12. Only then create task/history rows, mutate context-cancel maps, construct/lookup a registry, create sessions, or contact a provider.

All arithmetic uses checked operations. `work_cutoff_ms + 70_000` must fit the persisted monotonic representation. Retry `max_attempts` becomes explicitly `1..=1024`; zero no longer means one. Zero backoff remains legal. Compute the maximum cumulative retry backoff on a DAG path, not the sum across parallel branches; it must be less than the frozen work cutoff.

At admission, persist:

- the complete frozen controls;
- their fingerprint;
- the control-inclusive workload fingerprint;
- the complete per-node frozen execution identities;
- task class and policy version `r2f1a`;
- expected node count and bounded node-evidence reservation.

Max validation uses the frozen per-node efforts. Immediately before every provider attempt, including retries and
resumed attempts, compare the registry's current configured effective model/effort/mode and identity fingerprint with
the node's frozen identity. Missing or different identity terminalizes that node as a typed pre-prompt configuration
drift failure; it never re-resolves controls, mutates the fingerprint, checks out/configures a session, or calls the
provider. An unrelated registry edit that leaves the effective triple unchanged is accepted. This closes the
admission-to-dispatch hot-reload window without freezing a live backend or crossing into generation-drain work.

All causal and reservation arithmetic is checked. Full node IDs remain accepted; graph-bound ordinal/digest references
keep terminal and trigger payloads bounded, while exact node-key bytes are included in the history charge below.

## 6. Scheduler and trigger state machines

Keep `FuturesUnordered`, `run_node`, and their cleanup ownership. Add a controller around them.

```rust
struct WorkflowFlightV1 {
    workflow_cancel: CancellationToken,
    node_flights: BTreeMap<NodeId, NodeFlightV1>,
    controller: FanOutControllerV1,
    terminals: BTreeMap<NodeId, NodeTerminalV1>,
}

struct NodeFlightV1 {
    cancel: CancellationToken,
    cancel_cause: Option<NodeCancelCauseV1>,
    state: NodeRuntimeStateV1,
}
```

Every running node receives `workflow_cancel.child_token()`. The executor retains the token by node until completion. Parent cancellation reaches every child; policy cancellation selects only still-running child tokens. Cancellation never removes a future from `FuturesUnordered`.

### Completion and trigger ordering

For each wake:

1. Snapshot whether workflow cancellation is already observable.
2. Await one completion or control event.
3. Drain every additional completion currently ready without waiting.
4. Sort the ready batch by `NodeId`.
5. Finalize each actual node terminal and cleanup fact in memory without emitting it.
6. If workflow cancellation was already observable, suppress new policy action, stop admission, and let global cancellation own remaining causes.
7. Otherwise select the lowest `NodeId` whose disposition is `Failed` or `TimedOut` and attach the one trigger ID and
   graph-bound node reference to that terminal before any terminal in the batch is emitted.
8. Emit the sorted terminal batch exactly once. `PolicyTriggerBarrier` consumes the selected terminal and trigger as
   one operation; on served tasks its acknowledgement means checkpoint, trigger, `NodeFinished`, sequence allocation,
   and start-row deletion committed in one transaction. Offline it returns an explicit in-process linearization result.
9. Recheck workflow cancellation only after the selected terminal's barrier acknowledgement.
10. Apply the policy action if still applicable.
11. Admit or structurally terminalize downstream nodes.
12. Process fake fixed-grace expiry only after completions at or before the same logical boundary.

A ready natural completion is never relabeled canceled. Simultaneous failures all retain their own terminal facts. Manual node cancellation, workflow cancellation, dependency skip, and legacy interruption do not qualify as fan-out failure triggers.

`PolicyTriggerBarrier` returns exactly `PrimaryCommitted`, `OfflineLinearizedTelemetryUnavailable`, or
`PrimaryFailed`. The first two authorize policy action; the offline case emits the trigger in the terminal result and
records `telemetry_unavailable`. `PrimaryFailed` globally cancels and drains without targeted policy action. A failed
transaction leaves no write-once checkpoint, so there is no retry that attempts to attach a trigger to an existing
terminal. Optional history enrichment occurs after primary ordering and cannot change the barrier result.

### `bounded_independent`

- Record failures/timeouts immediately.
- Do not create a policy trigger.
- Do not alter running sibling tokens or frozen controls.
- Once dependencies are terminal, apply strict or degraded synthesis.
- Drain every scheduled node.
- A successful terminal over any typed failure ancestry is `CompletedDegraded`.

### `fail_fast`

On the first deterministic qualifying trigger:

1. Stop downstream admission.
2. Persist the trigger.
3. Cancel each still-running sibling once with `canceled_policy`.
4. Record every unscheduled node as `NotStartedPolicy/NotNeeded`.
5. Continue polling every running future through its real cleanup result.
6. Later failures cannot replace or renew the trigger.

If workflow cancellation becomes observable before policy action, global cancellation wins the cancel cause; the already-observed failure remains recorded.

### `fixed_grace`

The controller state machine is implemented and tested with an injected monotonic waiter:

1. First qualifying failure persists one trigger and one frozen grace.
2. Downstream admission stops.
3. Already-running siblings continue.
4. Later failures cannot renew or alter the grace.
5. At expiry, completions at or before the boundary win.
6. Remaining siblings receive `canceled_policy`.
7. Duplicate or late expiry is an idempotent no-op.
8. Every future is drained through cleanup.

No production constructor supplies the waiter in R2f1a, and production admission refuses this policy before effects.

### Workflow cancellation

Workflow cancellation:

- stops admission;
- propagates to all child tokens;
- preserves any already-recorded policy trigger;
- drains all in-flight futures;
- records actual primary and cleanup dispositions;
- projects `Canceled` only after drain unless a concrete cleanup/persistence failure requires `Failed`.

There is no public per-node cancellation endpoint.

## 7. Strict and degraded synthesis

Replace `(String, bool, usage)` dependency interpretation with typed inputs:

```rust
pub enum WorkflowInputV1 {
    Completed {
        node: NodeId,
        text: String,
        degraded_ancestry: bool,
    },
    NonCompleted {
        node: NodeId,
        terminal: NodeTerminalV1,
    },
}
```

Under `strict`:

- any direct non-completed input prevents prompting the node;
- record `SkippedDependency/NotNeeded`;
- the cause stores a `DependencySetRefV1` over the sorted direct non-completed inputs; the exact IDs are recoverable
  and verified from the frozen graph plus terminal map rather than copied into bounded cause text;
- skipping propagates structurally until no new node can be admitted.

Under `degraded`:

- render a canonical `a2a_bridge.node_failure.v1` JSON marker from the typed terminal;
- never parse or trust formatted node output as failure evidence;
- set `degraded_ancestry=true`;
- propagate ancestry through successful intermediate nodes;
- successful terminal output, including empty output, becomes `CompletedDegraded`.

Failed/timed-out inputs remain failed/timed-out in the terminal map. Synthesis never rewrites them as successful.

## 8. Cleanup ownership and terminal map

Change `NodeRunOutput` to carry its complete `NodeTerminalV1`. Audit every synthetic, preflight, retry, prompt, empty-final, cancellation, and cleanup exit.

Change the cleanup tracker from workflow-only interval aggregation to per-node records while retaining the existing aggregate observation. `cleanup_warm_turn` and every result-bearing `NodeTurnCleanup::on_exit_observed` call record:

- node ID;
- actual disposition;
- actual duration;
- failure cause if cleanup fails.

Pre-checkout exits are `NotNeeded`. Legacy rows are `UnknownLegacy`. No node is marked complete merely because the workflow-wide cleanup aggregate completed.

The executor must not emit its terminal event until:

1. every scheduled node future returned;
2. every unscheduled/skipped node received a structural terminal record;
3. every actual cleanup result is known;
4. the terminal map covers exactly the graph’s node set;
5. the policy trigger, when present, has reached its persistence barrier.

## 9. Persistence, resume, and serving

### Task store

Extend `task_node_checkpoints` with additive `terminal_json TEXT`, capped at 1,024 bytes. Keep `ok` and `usage_json` for legacy readers.

Add nullable task columns:

```text
workflow_outcome
policy_trigger_json
```

`workflow_outcome` uses:

```text
completed
completed_degraded
failed
canceled
interrupted
```

`tasks.status` remains the existing closed vocabulary. `completed_degraded` stores as:

```text
tasks.status = completed
tasks.workflow_outcome = completed_degraded
tasks.result = terminal synthesis text
```

Extend `put_node_checkpoint_sequenced` so the selected checkpoint terminal JSON, trigger, journal `NodeFinished`, sequence allocation, and start-row deletion share the existing transaction. The controller selects and attaches the trigger before this first and only write; no post-checkpoint trigger attachment exists.

Extend `OrchEventKind::NodeFinished` and `FrameKind::NodeFinished` with additive optional terminal and trigger fields. Do not add a new journal event kind.

### Workflow history

Extend `AttemptReservation` with bounded controls JSON, controls fingerprint, expected node count, and exact node-evidence reservation. Add:

```text
workflow_attempt_node_terminals(
    attempt_id,
    node_id,
    terminal_json,
    terminal_reserve,
    charged_bytes,
    PRIMARY KEY(attempt_id, node_id)
)
```

Each terminal JSON is at most 1,024 bytes. Accounting schema V2 adds bounded constants
`MAX_CONTROLS_JSON_BYTES = 4_096` and `MAX_POLICY_TRIGGER_JSON_BYTES = 512`. At admission, create one placeholder node
row per canonical graph node with its full key and a 1,024-byte terminal reserve. Its logical charge is
`1_024 + node_id.as_bytes().len()`, checked per row and in the sum.

The V2 configured-store invariant is exact:

```text
attempt_summary_charge = 16_384 + 4_096 + 512
attempt_charge = attempt_summary_charge
               + 1_024 attachment charge
               + sum(1_024 + exact node-id bytes for every node row)
allocation.charged_bytes = sum(attempt_charge for every retained attempt)
allocation.slots_used = count(retained attempt summaries)
```

Legacy rows keep their V1 `16_384 + 1_024` charge and zero node charge. `accounting_version = 2` admits variable
charges with one checked compare-and-debit; each summary stores its whole derived attempt charge, and retention
subtracts that stored value while cascading attachment/node rows. Migration sets `migrating`, creates and verifies
the new rows/columns, rederives every stored and aggregate charge from authoritative rows, and flips to `ready` in one
transaction; restart repeats safely, and any mismatch is corruption rather than a rebaseline.

For the platform store, materialize the same placeholder reserves before effects. The existing aggregate physical
gate remains conservative and is called with:

```text
physical_request = attempt_charge
                 + incoming permanent-identity charge
                 + MAX_TERMINAL_JSON_BYTES
                 + MAX_POLICY_TRIGGER_JSON_BYTES
                 + 2 * current_page_size * expected_node_count
                 + 4 * current_page_size
```

It requires current main database plus live WAL/journal/SHM bytes, minus reusable freelist bytes but plus the existing
disk-transaction headroom, plus `physical_request`, to remain at or below 128 MiB. The two extra pages per node cover
B-tree/overflow-page structure in addition to the exact logical payload charge. Replacing a placeholder cannot grow
beyond its reserve. Crash/failpoint tests cover the V1-to-V2 allocation transition and every debit/credit boundary.

Add a bounded `policy_trigger_json` field to the attempt row. Exact replay is idempotent; a different terminal or trigger for the same key is a persistence conflict.

`AttemptTerminal` adds the trigger and accepts `completed_degraded`. `NodeCounts` derives from structured terminals:

- `completed`: `Completed`;
- `failed`: `Failed` and existing smaller-bound `TimedOut`;
- `canceled`: workflow/policy/node cancellation;
- `deadline`: zero for every R2f1a production attempt;
- `cleanup_partial`: failed or unknown cleanup.

Telemetry incompleteness may keep `degraded=true`, but it never fabricates the `completed_degraded` outcome.

### Snapshot and resume

Bump the persisted workflow snapshot to V2:

```json
{
  "v": 2,
  "run_spec": {
    "schema_version": 1,
    "graph": {},
    "controls": {},
    "node_execution_identities": [],
    "controls_fingerprint": "...",
    "workload_fingerprint": "..."
  }
}
```

Resume rules:

- V2 verifies the controls fingerprint and uses the frozen controls verbatim.
- It seeds exact structured node terminals and degraded ancestry.
- A seeded fail-fast failure is evaluated before the first admission wave.
- Post-submit config/profile edits cannot change the resumed budget.
- Every provider attempt rechecks the current effective identity against its frozen node identity; drift fails that
  node before checkout/configuration/prompt without changing the frozen spec.
- V1 snapshot maps to `other / legacy_bounded_v1 / bounded_independent / degraded / manual_only_r2f1a`.
- Legacy `ok=true` maps to `Completed/UnknownLegacy`.
- Legacy `ok=false` maps to `InterruptedLegacy/UnknownLegacy`.
- Unknown versions, corrupt controls, or fingerprint mismatch remain fail-closed under the existing interruption/reconciliation machinery.
- Existing terminal-first and hidden-projection reconciliation ordering remains unchanged.

### Projection

- Internal workflow event: `CompletedDegraded`.
- Durable task status: `completed`.
- Durable workflow outcome: `completed_degraded`.
- Offline exit: zero, terminal synthesis remains stdout/`--out`; bounded per-node terminal and trigger summaries go to stderr.
- `TaskStatusDto`: additive `workflow_outcome`, `policy_trigger`, and `nodes`.
- MCP status and workflow result preserve the same fields.
- A2A task state remains standard `completed`; task metadata adds `a2a-bridge.workflow-outcome=completed_degraded`.
- Reattach/replay derives the outcome from durable state, not ephemeral stream memory.
- Empty terminal text does not erase the structured outcome or node map.
- `completed`, `completed_degraded`, `failed`, `canceled`, and boot/reconciliation `interrupted` each preserve the
  full structured node map and optional trigger through durable task state, reattach/replay, `TaskStatusDto`, MCP, and
  A2A metadata. Runtime code produces the first four; only durable reconciliation produces `interrupted`.
- Failed/canceled/interrupted offline runs retain their structured result artifact and follow the existing non-success
  exit contract; an empty synthesis string never changes outcome selection.

## 10. Compatibility and migration

1. Existing config omission remains valid and preserves current scheduling: no sibling policy cancellation, failed markers may reach synthesis, and all scheduled futures drain.
2. The compatibility profile is now frozen bounded data, but R2f1a does not claim its outer bounds are enforced.
3. Built-in review/design workflows receive explicit review profile declarations.
4. New fingerprints intentionally differ because controls are a calibration dimension.
5. Existing task/checkpoint rows remain readable through Boolean fallback.
6. New binaries read V1 and V2 snapshots; old binaries cannot resume V2 working tasks and may mark them interrupted.
7. New task columns and journal fields are additive; old task readers still see the known `completed` status.
8. Workflow-history accounting V2 uses the exact equation above; migration is transactional, idempotent,
   schema-admitted, and rederives exact charges from authoritative rows. Legacy rows receive zero node-evidence charge.
9. Rollback after the allocation migration or V2 working-task creation requires stopping the new binary and restoring the pre-migration database snapshot. There is no in-place down-migration.
10. No migration infers timeout, policy trigger, cleanup completion, or degraded ancestry from legacy text.

## 11. Compile-correct build and ownership order

### Stage 1 — serial foundation

One owner changes:

- `crates/bridge-core/src/execution_policy.rs`
- `crates/bridge-core/src/lib.rs`
- `crates/bridge-workflow/src/graph.rs`
- new `crates/bridge-workflow/src/fanout.rs`
- run-spec serialization helpers

Land the pure types, resolver, exact constants, fingerprinting, and controller transition tests first. The workspace must compile before parallel work begins.

### Stage 2 — parallel siblings from the same frozen Stage-1 base

- **Configuration owner:** `bin/a2a-bridge/src/config.rs` and config-only tests.
- **Controller owner:** `crates/bridge-workflow/src/fanout.rs` and fake/manual state-machine tests, without touching executor integration.
- **Persistence owner:** `crates/bridge-core/src/task_store.rs`, `workflow_history.rs`, `orch.rs`, and `crates/bridge-store/src/sqlite.rs`.

Each sibling owns disjoint paths, has an independently runnable test target, and does not modify manifests, roadmap, generated files, executor integration, or serving adapters.

### Stage 3 — single integration owner

After integrating all Stage-2 siblings, one owner changes the shared seams:

- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-coordinator/src/detached.rs`
- `crates/bridge-coordinator/src/batch.rs`
- `crates/bridge-coordinator/src/coordinator.rs`
- `crates/bridge-coordinator/src/params.rs`
- `crates/bridge-coordinator/src/session_manager.rs`
- `bin/a2a-bridge/src/main.rs`
- `crates/bridge-a2a-inbound/src/server.rs`
- `crates/bridge-mcp/src/server.rs`

This stage owns every fresh/resumed batch and non-batch entrypoint, freezing, per-attempt effective-identity drift
checks, event widening, terminal ordering, resume, projections, CLI/A2A/MCP overrides, and compile fixes. These files
must not be split among concurrent implementors.

### Stage 4 — checked-in configs, docs, and aggregate gate

The integration owner updates built-in workflow declarations, generated init templates, operator documentation, the stale R2f0b status headers in historical planning surfaces, and the roadmap cursor. Run the aggregate suite and cumulative-diff review only after all sibling commits are integrated.

Manifests, lockfiles, roadmap, generated artifacts, and cross-cutting cleanup remain integration-owned.

## 12. Fail-first and negative matrix

Every behavior begins with a test demonstrated red against exact base `3f35ee6…`, followed by a same-path negative or edge case.

### Resolution and validation

- Exact profile constants round-trip.
- Omission maps to legacy/other; explicit review maps to review.
- Invocation profile outranks workflow profile.
- Empty, unknown, and wrong-typed profile/class/policy/synthesis values refuse.
- Workflow profile/task-class mismatch refuses.
- Fixed grace missing/zero/over-cutoff refuses; grace on another policy refuses.
- Production fixed grace refuses before task, history, registry, session, or provider counters change.
- Max with reason and `7_200_001` ms succeeds.
- Missing/blank/oversized reason, exactly `7_200_000`, tail overflow, and Max without explicit cutoff refuse.
- Max fields on a non-Max workflow refuse.
- Max source pairs are atomic; every partial or cross-source mixed pair refuses. A complete invocation pair replaces a
  complete workflow pair, and effective cutoff/terminal-bound accessors return `7_200_001`/`7_270_001` in the minimum
  valid Max case with checked overflow refusal.
- Retry zero/1025 refuses; zero backoff is accepted; checked overflow and critical-path excess refuse.
- Parallel retry paths use maximum, not sum.
- Offline, served, and MCP refusal ordering use store/registry/provider zero-effect counters.
- Controls change the new fingerprint; graph/order canonicalization remains stable.
- Paused downstream dispatch plus `high -> max`, `max -> high`, model, or mode reload refuses before that provider
  attempt; an unrelated reload with the same effective triple proceeds. The same matrix covers retries and resume.
- Fresh and resumed batch fixed-grace refusal, profile freeze, Max validation, V2 resume, and fingerprints match the
  non-batch surfaces and record zero provider/session effects on refusal.
- Arbitrarily long accepted node IDs, high fan-in, ordinal overflow, node-ref mismatch, and dependency-set digest
  mismatch either retain exact graph-bound identity or refuse before effects; no bounded payload truncates an ID.

### Scheduler

- Bounded-independent failed root plus running sibling: no sibling cancel, trigger absent, sibling completes, typed marker reaches synthesis, terminal is completed-degraded.
- All healthy: completed, no degraded ancestry.
- Fail-fast: trigger persists before cancellation, running sibling cancels exactly once, pending nodes never start, every future drains.
- Simultaneous failures with reversed delivery select the same lowest `NodeId`; the selected node's first and only
  checkpoint atomically includes the trigger, and all failures persist exactly once.
- Trigger-persistence failure causes global drain and no targeted policy action.
- Workflow cancel already observable suppresses a new policy action.
- Fixed grace with fake time: before-expiry completion, exact-boundary completion wins, expiry cancels remaining nodes, later failure does not renew, duplicate expiry is a no-op.
- Paused production time advanced beyond two hours causes no warning, snapshot, grace expiry, outer timeout, preservation, or cancellation.
- Manual per-node cancel affects only the selected child; duplicate and late cancellation are no-ops.
- Strict mode skips downstream prompt with the verified sorted dependency-set reference.
- Degraded mode admits the same input and propagates taint through a successful intermediate and empty terminal.
- Existing provider/command timeout maps to `TimedOut` without producing an outer `Deadline`.
- Cancellation storm retains all in-flight futures until exact cleanup results return.
- Held cleanup prevents terminal publication; immediately ready cleanup still precedes terminal; cleanup failure records `Failed`, never `Complete`.

### Persistence, migration, and projection

- Memory and SQLite round-trip frozen controls, trigger, every node terminal, ancestry, and cleanup duration.
- Exact replay succeeds; conflicting replay refuses.
- Accounting-V2 migration is idempotent and the exact mixed V1/V2 row equation rederives under boundary capacity,
  retention, rollback-required, concurrent admission, and crash/failpoint fixtures.
- Legacy checkpoint fallback never invents timeout or cleanup completion.
- V1 working snapshot resumes with frozen legacy defaults.
- V2 resumes frozen controls despite changed config.
- Corrupt/mismatched V2 controls refuse before prompt.
- Seeded fail-fast failure prevents new admission.
- `NodeCounts` derives correctly and `deadline == 0`.
- Completed-degraded maps to task `completed`, durable outcome `completed_degraded`, A2A metadata, MCP/status node map, and offline exit zero for both empty and nonempty synthesis text.
- Ordinary completion remains ordinary completed.
- Failed, canceled, and reconciliation-interrupted outcomes retain the complete terminal map and trigger across task,
  reattach, A2A, MCP, and offline artifacts; corrupt V2 produces interrupted with zero provider calls.
- Two diagnostic failure classes sharing one static code remain distinct through memory, SQLite, journal, task DTO,
  MCP, and A2A projections; prompt acceptance remains independently sticky.
- Old-format journal rows parse; new optional NodeFinished fields do not require a new variant.
- Primary terminal commits before optional history enrichment.
- Telemetry failure cannot rewrite outcome or trigger cancellation.
- Pre-R2f1a migration/downgrade fixtures cover active tasks, terminal tasks, pending terminal projections, and retention counters.

No provider turn is admissible evidence for these behaviors.

## 13. Entry and exit evidence

### Entry evidence

- Checkout: exact clean `3f35ee6e07e9af314bb548b9d3ab694f3bba5fb1`.
- Both design hashes matched.
- Roadmap says R2f0b merged and R2f1a next.
- Current executor has one shared cancellation token, Boolean checkpoints, `FuturesUnordered`, no fan-out/profile vocabulary, and snapshot V1.
- The roadmap’s current post-R2f0b evidence reports `3,089 passed / 0 failed / 12 ignored`; this synthesis did not rerun it and does not treat it as evidence for R2f1a.

### Required exit evidence

Report actual results, not expected totals, for:

```text
git diff --check
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo build --release --workspace --locked
cargo deny check
cargo run -p a2a-bridge -- validate --repo-hygiene
cargo test --workspace --all-features --no-fail-fast --locked -- --quiet
```

Also validate every changed checked-in example config and report exact passed/failed/ignored totals. If the aggregate suite exposes a failure, run the exact pre-change base in the same environment before attributing it to R2f1a.

Require one fresh cumulative hard-read-only correctness review. Review-round cap is one full review plus at most one targeted closure round for closed enumerable findings. If the second round exposes an open class, stop and escalate rather than extending the cap.

Explicitly name as unexercised:

- live providers;
- production operator;
- any real R2f1a outer timing behavior;
- fixed-grace production expiry;
- preservation/takeover;
- compatibility canary;
- release, deployment, or operator mutation.

## 14. Non-goals

No production queue, warning, stagnation, silence, fixed-grace, or absolute-work timer; no `preserve_after_cancel`; no worktree recovery claim; no takeover or relaunch; no new cleanup escalation; no process-tree or backend-generation action; no public node cancellation; no ACP session-close/debt work; no health/quarantine/drain work; no provider fallback or retry; no adapter protocol change; no compatibility claim; no #22/#24/#47 closure claim; no release, deployment, or production-operator change.

## 15. R2f1b entry conditions

R2f1b may begin only after:

1. The full R2f1a fail-first/negative matrix is green.
2. The complete repository gate reaches completion with exact totals.
3. Config, offline, served, MCP, batch, and resume constructors all demonstrably supply the frozen run spec.
4. V1/V2 migration, replay, resume, and completed-degraded projection are green.
5. Per-node cancellation and held/immediate cleanup tests prove futures are never dropped while owning cleanup.
6. The no-real-deadline control proves R2f1a cannot autonomously fire a timer.
7. The accepted commit and roadmap cursor are reconciled.
8. A cumulative adversarial review approves within the declared review cap.

R2f1b must then land `preserve_after_cancel`, durable preserved-worktree custody, retained resource capabilities, joined cleanup ownership, collateral reporting, and survival tests before enabling fixed-grace expiry, warning snapshots, or the absolute work cutoff.

R2F1A FOCUSED BOUNDARY: PARKED — FOUR CLOSED BLOCKERS
