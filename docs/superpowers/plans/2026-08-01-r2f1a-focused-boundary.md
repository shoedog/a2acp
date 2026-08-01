# R2f1a focused implementation boundary — profiles, fan-out policy, and per-node control

- **Status:** AWAITING SOL CLOSURE REVIEW — the four closed technical blockers from Sol/xhigh closure review 2 and
  its deferred encoder finding are repaired below; the roadmap cursor reconciliation is already applied; no
  implementation has started
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
trigger-commit barrier result. All four are closed enumerable design defects rather than an open class.

For that round, the operator authorized exactly one targeted Opus/xhigh design repair of those four blockers,
followed by one Sol/xhigh closure review. That historical extension did not authorize implementation, a second
repair pass, another review, a live gate, or any R2f1b scope. §16 retains its blocker-to-mechanism disposition.

The first Tier-3 setup execution, `exec-53f8494cc17d7a1d3145dadddd4c5471` /
`attempt-02f8b2aced9b0e213403560b131365fb`, refused during `ConfigApply` with
`acp.config.mode_rejected`, explicitly recorded `prompt_may_have_been_accepted: false`, and left the document at
SHA-256 `e17a741011f07c4ff9ef26520bb162b718b582ff8e1a8f33acdf3ced491444aa`. It is setup evidence only, not a
repair attempt. After removing only that rejected mode override and repeating validate, doctor, and model discovery,
the admissible repair ran as execution `exec-4b373e1c5170baa8de7f8cffd838d3f9`, attempt
`attempt-771a8533ed80526553ad86266f97b242`, on exact advertised `opus[1m]` / `xhigh` / default mode in immutable
image `sha256:c4be66eb232809a1ab411d37fea6f660418db3e42b5b53b8be796329f998cb00`. Its 4,645-byte terminal artifact
has SHA-256 `3e46f43e6fa1f0b4fdd04c12fadd4c1adbd73d0f5d33d2f7b6910a22c358e3b2`. The repair edited only this
document: no source, test, prompt, configuration, generated artifact, or roadmap surface changed, and no build, test,
review approval, release, deployment, or operator effect is claimed. No execution id, attempt id, artifact hash, or
verdict existed for the then-pending Sol review.

The one authorized Sol/xhigh closure review ran as execution `exec-93cba8dc1634cca58db99cfc1b004d03`, attempt
`attempt-14a5a8523118657b99ab193e0734b1cc`, against clean commit
`ff62b1030f1c611a58e4b75aadb5c3b468b7eb9d` and focused-artifact SHA-256
`3e1a959514f12ba6d09892f5ca5a7cd56bcb841602385e917a061d4c94deb28b`. Its 15,985-byte terminal artifact
had SHA-256 `871931e92fe3906d34f3995448074fc2f2161b565cecbc2b29a164c34a00c967`; the checked-in
[`review record`](../reviews/2026-08-01-r2f1a-sol-closure-review-2.md) differs only by the repository-standard final
newline. The review closed the encoded-terminal reserve, deferred the theoretical overflow-fallback evidence loss,
and rejected five blockers: provider-effect identity remains incomplete, the proposed `WITHOUT ROWID` metadata
assertion is false, the arbitrary-ID page formula is not conservative, `Collision` is simultaneously fail-open and
fail-closed, and the authoritative roadmap still directs a fresh freeze followed by implementation. It found no
SMELL and classified the remaining population as closed enumerable.

The owner then approved a fresh, capped convergence round: one Sonnet/xhigh source-mining turn, one Opus/xhigh
Tier-3 repair, and one Sol/xhigh closure review. The Sonnet turn reached `PromptStart` but failed in `PromptStream`
with `upstream.unknown` as execution `exec-86204bb975fee00dfb5510c76342fbdb`, attempt
`attempt-a1541f6cbfa6d3363b04f02bb3dbcf93`. Its 427-byte terminal artifact has SHA-256
`dd64d9d80f05a018aa5b57de15c841d31f4c8df186391472e77472739683f4af`; prompt acceptance could not be
excluded, so it was not replayed and produced no mining report.

The operator completed the bounded source and SQLite mining locally and retained a 14,132-byte report at SHA-256
`c85c68831dcbe34fcca7d328ba9c0efd15eb635d787d984ef9c7f66a77dfe26c`. The one Opus repair then ran in the
reviewed immutable Tier-3 image on exact `opus[1m]` / `xhigh` as execution
`exec-cd4d557340381e39eb7dd5497d502cd7`, attempt `attempt-48ea7c6d9d4d01e93582283357b1e9b5`.
Its 9,099-byte terminal artifact has SHA-256
`0f3501a6483852c14efb8e52e83fb29b838cba816c24f2116dbd0881bd0b657f`, and it edited only this focused
document.

The Tier-3 container could not read the host-sibling mining report despite its pre-dispatch host hash being exact;
the task carried the prescribed mechanisms, but the report-visibility predicate was therefore unresolved inside
the model turn. The operator did not replay Opus. Instead, a complete source-bound comparison of its diff folded the
closed discrepancies that the report had already enumerated: ordinary-workflow provider-field exclusions,
after-bind completion on the pinned old slot, one fresh bound use per preflight candidate and real turn, exact
slot-plus-entry invalidation and digest-keyed preflight cache, the complete hard physical-admission regime, exact PK
metadata/column order, and the full current `Collision` producer family. These are bounded corrections to W1–W4,
not a fresh implementation or a new provider repair.

This revision repairs W1–W4, folds deferred W5, and reconciles the authoritative roadmap to the pending-review
state. It is a design repair only: only this document and the roadmap cursor change. No source, test, configuration,
prompt, generated artifact, implementation, review approval, release, deployment, or live operator effect is
claimed, and no execution id, attempt id, artifact hash, or verdict yet exists for the pending Sol closure review.

The pre-freeze docs gate ran `cargo run -p a2a-bridge -- validate --repo-hygiene` successfully in the scratch clone
and reported **39 tracked artifacts / 7 validated example configs**. `git diff --check` and direct existence checks
for the changed documents' roadmap, owner-design, parent-plan, prior-review, and ADR targets also passed. The hygiene
command compiled the dev binary in the scratch clone; no Rust test suite, compatibility case, smoke, or provider
behavior was exercised by these deterministic checks.

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
| Provider-effect and selection freeze | Freeze two distinct canonical digests per node whose combined identity covers every ordinary-workflow provider input: a **provider-effect digest** over the spawn, checkout, credential, session, watchdog, and MCP fields, and a **selection digest** over the agent, preflight flag, primary model, exact ordered fallback candidate list, effort, and mode. Freezing only the selection tuple leaves the provider call itself mutable — a tuple-identical hot reload of `base_url` or `api_key_env` passes a selection-only digest — and is rejected. Fields used only by compatibility resolution, guarded R2d fallback, or presentation are explicitly excluded by source boundary. |
| Check-to-use binding | Lease-first, same-entry, bound-use. One bind per provider attempt takes a registry **lease** before reading the entry, yields exactly one immutable `Arc<AgentEntry>`, validates both frozen digests against that value, and is the sole source consumed for candidate selection, configuration, and dispatch. Re-reading the registry between validation and use is forbidden. There is no registry generation or revision API on current main; §5 specifies the minimal additive replacement and keeps the durable digests distinct from an opaque process-local use token. |
| Fingerprint compatibility | Always include canonical resolved controls and every per-node provider-selection digest in new R2f1a workload fingerprints. Preserving the old hash would conflate pre-policy and bounded-policy calibration populations. |
| `completed_degraded` task status | Keep `tasks.status = completed`; persist an additive `workflow_outcome = completed_degraded`. Do not introduce an old-binary-unknown task-status token. |
| Node persistence | Use versioned terminal JSON on task checkpoints and normalized history node-terminal rows. Bound the JSON by proven worst-case *encoded* size, not raw field bytes. Store the history rows in a `WITHOUT ROWID` compound-primary-key table so an arbitrary node ID has exactly one physical key copy. Boolean `ok` remains a legacy compatibility projection only. |
| Physical node-row accounting | Do not predict pages. No static logical-to-page formula is sound for an uncapped `NodeId` under SQLite's local-payload and overflow rules, so R2f1a **actually materializes** every full-size placeholder inside the pre-effect reservation transaction and gates on measured postconditions under the existing hard physical controls, rolling the whole transaction back on refusal. |
| Legacy `ok=false` | Decode as `interrupted_legacy / unknown_legacy`, not a fabricated ordinary failure, timeout, or completed cleanup. |
| Trigger persistence | Persist the trigger before policy cancellation through a dedicated barrier with four closed results, and also attach it additively to the triggering `NodeFinished`. A healthy admitted offline history ledger durably commits the trigger; only genuine optional-ledger unavailability may fall open to the in-process marker. Do not add an old-reader-unknown journal event variant. |
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
    pub code: StaticBoundedCodeV1,                    // at most MAX_STATIC_CODE_BYTES raw
    pub deepest_cause: Option<String>,                // sanitized, at most MAX_DEEPEST_CAUSE_BYTES raw
    pub cause_truncated: bool,                        // set when encoded-aware truncation shortened the cause
    pub evidence_overflow: bool,                      // set only by the bounded-evidence fallback below
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

Usage remains the existing separately bounded `UsageSnapshot` checkpoint field rather than being duplicated inside the
node-terminal JSON.

For `BridgeError::AgentFailure`, construct the cause from `FailureDiagnosticWire`, retaining its static code, deepest sanitized cause, closed failure class, and sticky prompt-acceptance bit. Other errors map through bridge-owned static codes, a closed non-diagnostic failure class, and `client_message()`, never `Debug` text.

`PolicyNodeRefV1` is canonical within one frozen graph: nodes sort by the exact `NodeId` bytes, receive a checked
`u32` ordinal, and bind the full ID by SHA-256. Decode verifies both fields against the graph. Full node IDs remain in
the graph and node-row key; terminals and triggers never truncate an ID. `DependencySetRefV1` binds the sorted set of
direct non-completed inputs. Its exact members are recomputed from the frozen graph and terminal map, so arbitrary ID
length or fan-in cannot inflate the node-terminal JSON. The run-spec resolver refuses ordinal overflow before effects.

### Bounded encoding invariant

Bounds are stated on **encoded** bytes. Raw UTF-8 field length is not evidence about serialized length: a 512-byte
sanitized cause consisting only of `"` or `\` occupies 1,024 bytes once escaped, which alone exhausts a 1,024-byte
terminal reserve before any other field is written. The following contract replaces that defect.

**Canonical serializer.** Exactly one deterministic constructor/serializer produces node-terminal, policy-trigger, and
frozen-controls JSON:

- RFC 8259, compact, no insignificant whitespace, no trailing separators;
- a fixed compile-time key order per type, all keys ASCII, every field always present (absent optionals encode as
  `null`), so the key/punctuation skeleton is a constant string rather than an input-dependent shape;
- integers as shortest decimal, no floats, no exponents, no `NaN`/infinity;
- **non-ASCII scalars are emitted verbatim; the serializer never emits a `\uXXXX` escape.** Emitting `\u` escapes for
  astral scalars would expand a 4-byte sequence into a 12-byte surrogate pair and break the bound below, so a
  configuration that escapes non-ASCII is a fail-closed defect, not a style choice.

**Two-times expansion lemma.** Every string value is sanitized by the existing redactor before construction, which
drops every control scalar except `\t` and truncates on a UTF-8 character boundary. After sanitization the only
scalars the canonical serializer escapes are `"` → `\"`, `\` → `\\`, and `\t` → `\t`, each one source byte to two
encoded bytes; every other surviving scalar, including multi-byte UTF-8, is copied verbatim. Therefore the encoded
length of any sanitized string value is at most twice its raw UTF-8 byte length, and the enclosing quote pair is
charged in the skeleton.

**Constants.**

```rust
pub const MAX_STATIC_CODE_BYTES: usize = 64;        // raw, matches the diagnostics redactor
pub const MAX_DEEPEST_CAUSE_BYTES: usize = 512;     // raw, matches the diagnostics redactor
pub const MAX_CONTROL_EVENT_ID_BYTES: usize = 128;  // raw; validated attempt id plus ordinal
pub const MAX_CLOSED_TOKEN_BYTES: usize = 32;       // raw ceiling for every closed enum token
pub const NODE_TERMINAL_SKELETON_CEILING_BYTES: usize = 352;
pub const POLICY_TRIGGER_SKELETON_CEILING_BYTES: usize = 160;

pub const MAX_NODE_TERMINAL_JSON_BYTES: usize = 2_048;
pub const MAX_POLICY_TRIGGER_JSON_BYTES: usize = 1_024;
pub const MAX_CONTROLS_JSON_BYTES: usize = 4_096;
```

`MAX_NODE_TERMINAL_JSON_BYTES` is a new constant and is deliberately **not** the existing
`bridge_core::workflow_history::MAX_TERMINAL_JSON_BYTES`, which is the 8-KiB attempt-terminal bound. The two names
must not be conflated in any formula; every R2f1a node-evidence constant and equation below names the node constant
explicitly.

**Derived worst case for `NodeTerminalV1`.** Every permitted field has a conservatively derived encoded ceiling; the
worst case is their sum with `cause`, `deepest_cause`, `dependency_set`, and `policy_trigger_id` all present:

| Encoded component | Ceiling | Derivation |
|---|---:|---|
| canonical skeleton: every key, colon, comma, brace, and value-enclosing quote pair | 352 | fixed compile-time string; a constant test asserts its exact length is at most the ceiling |
| `schema_version` | 8 | `u16` shortest decimal |
| `primary` | 32 | longest closed token is `skipped_dependency` (18) |
| `cleanup.disposition` | 32 | longest closed token is `unknown_legacy` (14) |
| `cleanup.duration_ms` | 20 | `u64` shortest decimal |
| `cause.failure_class` | 32 | longest closed token is `container_credentials` (21) |
| `cause.code` | 128 | 64 raw bytes × 2 |
| `cause.deepest_cause` | 1,024 | 512 raw bytes × 2 |
| `cause.cause_truncated` | 5 | `true`/`false` |
| `cause.evidence_overflow` | 5 | `true`/`false` |
| `cause.dependency_set.count` | 10 | `u32` shortest decimal |
| `cause.dependency_set.sorted_node_refs_sha256` | 64 | fixed 64 lowercase hex |
| `prompt_may_have_been_accepted` | 5 | `true`/`false` |
| `degraded_ancestry` | 5 | `true`/`false` |
| `policy_trigger_id` | 256 | 128 raw bytes × 2 |
| **derived worst case** | **1,978** | sum of the ceilings above |

`MAX_NODE_TERMINAL_JSON_BYTES = 2_048` remains the proven bound: the derived worst case is 1,978 encoded bytes, which
is the previous 1,941-byte derivation plus the 32-byte skeleton allowance and 5-byte value for the additive
`evidence_overflow` indicator. The bound is unchanged and still has 70 bytes of margin. The
invariant is exact and checked, not asserted by inspection:

```text
derived_node_terminal_worst_case_bytes <= MAX_NODE_TERMINAL_JSON_BYTES
```

The same derivation over `PolicyTriggerV1` — 160-byte skeleton, 8-byte version, 256-byte control-event id, 10-byte
ordinal, 64-byte digest, 32-byte policy token, 20-byte grace — gives a 550-byte worst case, so
`MAX_POLICY_TRIGGER_JSON_BYTES` is 1,024, the next power of two. The previous 512-byte trigger reserve was unsound by
the same escaping argument and is replaced. `MAX_CONTROLS_JSON_BYTES` stays 4,096: the frozen controls are dominated
by the 512-byte Max reason at 1,024 encoded bytes plus bounded profile numerics and closed tokens, and 4,096 leaves
margin for the additive declared-control evidence block. Each constant carries the same checked
`derived_worst_case <= constant` assertion.

**Encoded-size-aware construction.** `NodeTerminalV1::encode_canonical()` is the only producer of terminal bytes:

1. sanitize and raw-bound every field as above;
2. serialize canonically and measure the exact encoded length;
3. if the length exceeds `MAX_NODE_TERMINAL_JSON_BYTES`, shorten `deepest_cause` on a UTF-8 character boundary using
   an encoded-size-aware step that keeps the **deepest** cause text rather than substituting a shallower ancestor,
   set `cause_truncated = true`, and re-serialize;
4. if the result still exceeds the bound, fail closed to `NodeTerminalV1::bounded_evidence_fallback`, which
   **preserves the primary failure evidence** — the primary disposition, cleanup disposition and duration, the
   original `failure_class`, the original static `code`, the prompt-acceptance bit, degraded ancestry, and the
   trigger id — drops only `dependency_set`, whose exact members remain recomputable from the frozen graph and
   terminal map, sets `evidence_overflow = true`, and retains the **deepest UTF-8 suffix** of `deepest_cause` that
   fits the remaining encoded budget, setting `cause_truncated = true` whenever that shortened the cause;
5. return the exact byte string. A value still over bound after step 4 is an invariant violation: the node
   terminalizes through the typed over-bound path and no over-bound row is ever written or admitted.

**Bounded-evidence fallback.** The earlier fallback discarded `deepest_cause` and *replaced* the static code with
`terminal_encoding_overflow`, which destroyed the most useful failure evidence exactly when an invariant had already
been violated, and contradicted the owner design's requirement that failed roots and strict/degraded results retain
the deepest bounded cause. Overflow is therefore indicated **separately** from the failure it describes:

- `failure_class` and `code` always keep their original values; no code is overwritten, and `evidence_overflow` is
  the only signal that the encoder hit its bound. Two distinct failures that overflow remain distinguishable.
- the retained suffix is computed by measurement, not from a constant: serialize the mandatory shape with an empty
  cause, measure its exact encoded length, and give the cause the whole remaining budget, stepping down on UTF-8
  character boundaries from the deepest end until the re-measured encoding fits. Keeping the suffix retains the
  innermost text rather than substituting a shallower ancestor.
- the mandatory shape is small enough to leave a real budget. Removing the 1,024-byte cause ceiling and the 74 bytes
  of dependency-set components from the 1,978-byte derivation leaves 880 bytes, plus at most a few bytes where a
  `null` replaces a shorter quoted value, so at least 1,168 encoded bytes remain for the retained suffix under
  `MAX_NODE_TERMINAL_JSON_BYTES`. Because a sanitized cause is already bounded at 512 raw bytes and expands at most
  2×, a current-schema cause is retained whole.
- if even the empty-cause mandatory shape exceeds the bound, that is the step-5 invariant violation and the
  fail-closed control still holds: no over-bound row is written or admitted.

Steps 3 and 4 cannot trigger for a valid current input because step 1 plus the derivation already prove the bound.
They are enforcement mechanisms rather than assumptions, and step 5 is their fail-closed control. This is the
bounded evidence-preservation correction for the deferred W5 finding; it changes no bound, adds no new failure
class, and leaves the 2,048-byte proven reserve intact.

**One encoding, every projection.** The bytes produced by `encode_canonical()` are the same bytes stored in the task
checkpoint `terminal_json`, the history node-terminal row, the additive journal `NodeFinished` field, `TaskStatusDto`,
MCP status, A2A metadata, and the offline artifact. A projection either forwards those exact bytes or decodes and
re-encodes through the same canonical encoder; no projection may re-serialize with a different escaping, key order, or
whitespace policy. Round-trip equality of the exact byte string is a checked property of every projection.

Add in `crates/bridge-workflow/src/graph.rs`:

```rust
pub struct WorkflowControlDefaultsV1 {
    pub task_class: Option<TaskClassV1>,
    pub liveness_profile: Option<LivenessProfileIdV1>,
    pub fan_out: Option<FanOutPolicyV1>,
    pub synthesis: Option<SynthesisModeV1>,
    pub max_qualification: Option<MaxQualificationV1>,
}

pub struct FrozenProviderSelectionV1 {
    pub agent: AgentId,
    pub preflight: bool,
    pub effective_model: Option<String>,          // primary candidate
    pub ordered_fallback_models: Vec<String>,     // exact declaration order; no sort, dedup, or filter
    pub effective_effort: Option<Effort>,
    pub effective_mode: Option<String>,
    pub selection_digest: Sha256HexV1,
}

/// Durable, canonical digest over ordinary-workflow effect fields not carried by selection.
/// This is semantic identity: it is persisted, replayed, and compared across processes.
pub struct FrozenProviderEffectV1 {
    pub agent: AgentId,
    pub effect_digest: Sha256HexV1,
}

pub struct FrozenNodeExecutionIdentityV1 {
    pub node: PolicyNodeRefV1,
    pub effect: FrozenProviderEffectV1,
    pub selection: FrozenProviderSelectionV1,
    pub identity_fingerprint: Sha256HexV1,
}

pub struct WorkflowRunSpecV1 {
    pub schema_version: u16,
    pub graph: WorkflowGraph,
    pub controls: FrozenWorkflowControlsV1,
    pub node_execution_identities: Vec<FrozenNodeExecutionIdentityV1>,
    pub ledger_admission: LedgerAdmissionV1,
    pub controls_fingerprint: String,
    pub workload_fingerprint: String,
}

pub enum LedgerAdmissionV1 {
    DurablePrimaryTaskStore,                                  // served execution with a durable task store
    HistoryLedgerAdmitted { kind: HistoryAllocationKindV1 },  // reserved, writable configured or platform ledger
    HistoryLedgerUnavailable { reason: BoundedLedgerReasonV1 },
}
```

`WorkflowGraph` carries an additive optional declared-control block. Production executor entry points take `Arc<WorkflowRunSpecV1>`, not a bare graph plus optional defaults, so a production constructor cannot bypass freezing accidentally.

### Frozen provider effect and selection

Selection identity alone is incomplete. `FrozenProviderSelectionV1` covers what the workflow *chose*; it does not
cover what the registry entry *does*. The constructible defect is a tuple-identical hot reload: freeze an API agent
against endpoint A, then rewrite only `base_url` to endpoint B before a queued node binds. Every selection field is
byte-identical, so a selection-only digest passes, and the attempt invokes B under an identity frozen against A. The
same hole exists for `api_key_env`, `kind`, `cmd`/`args`, `cwd`, `session_cwd`, `sandbox`, `auth_method`,
`pre_authenticated`, `watchdog`, `mcp`, and `mcp_delivery`. R2f1a therefore freezes **both** digests.

**Provider-effect digest.** `effect_digest` is SHA-256 over a canonical, injective, length-prefixed encoding of every
current `AgentEntry` field that can alter ordinary-workflow backend selection, spawn, checkout, credentials, session
mint, watchdog behavior, or MCP prompt transport and is not already carried by the selection digest:

| Effect group | Fields |
|---|---|
| backend construction and transport | `kind`, `cmd`, `args`, `base_url` |
| credentials and authentication | `api_key_env` (the variable **name** only, never its value), `auth_method`, `pre_authenticated` |
| checkout, isolation, and session location | `cwd`, `session_cwd`, `sandbox`, `watchdog` |
| tool surface offered to the agent | `mcp` with exact server/argument/environment order and configured values, plus `mcp_delivery` |

`id` is carried beside `effect_digest` in `FrozenProviderEffectV1` and is also part of `selection_digest`.
`model`, `effort`, `mode`, `preflight`, and `fallback_models` are carried by that separate selection digest rather
than redundantly changing both digests. `model_provider` is consumed by compatibility resolution only;
`host_fallback_eligible` is consumed by guarded R2d fallback smoke planning, which is a non-goal here; and `name`,
`description`, `tags`, `version`, and `extensions` are presentation, agent-card, or compatibility metadata with no
ordinary workflow provider-path consumer. Those eight fields are therefore excluded, so an extension or fallback-
authorization edit cannot manufacture R2f1a drift. The classification is exhaustive by construction — the digest
builder destructures `AgentEntry` with no `..` rest pattern and explicitly routes each field to provider effect,
selection, carried identifier, or excluded-by-source-boundary metadata. Adding a field fails compilation until it is
classified. The ambient bearer selected by `api_key_env` is never read or hashed; only its configured variable name
participates. Configured MCP environment values do alter the delivered tool surface and therefore participate in the
domain-separated effect digest, but no raw MCP command, argument, or environment value is persisted, projected, or
logged by this identity mechanism.

**Selection digest.** `selection_digest` is unchanged: SHA-256 over a canonical, injective encoding of the whole
selection tuple, each component emitted as a length-prefixed byte string in fixed order — agent id, preflight flag,
model presence plus bytes, fallback count plus each fallback in declaration order, effort token, mode presence plus
bytes. Length prefixing is required so a concatenation such as `["a|b"]` cannot collide with `["a", "b"]`. It remains
separate from `effect_digest` because the two answer different questions: the selection digest is what the workflow
froze and what a candidate must belong to; the effect digest is what the registry entry would actually do. A node
whose selection is satisfied by an entry that has been re-pointed at another endpoint must fail on the effect digest,
and a projection must be able to say which of the two drifted.

`identity_fingerprint` is SHA-256 over the node reference, `effect_digest`, and `selection_digest`, so either a
selection change or a provider-effect change always changes the node identity. All three are durable semantic
identity: canonical over configuration bytes, persisted in snapshot V2, and meaningful across processes and restarts.
They are never derived from a pointer, address, counter, or other process-local value — §5 keeps that concern in a
strictly separate opaque token.

The frozen ordered candidate set is derived once and only from frozen state:

```text
frozen_candidates = [effective_model] ++ ordered_fallback_models.map(Some)   when preflight is true
frozen_candidates = [effective_model]                                        when preflight is false
```

This mirrors the existing `preflight_candidates` walk exactly and adds no new provider retry, fallback, replacement
attempt, or candidate. A node with `preflight = false` has a one-element set and therefore no admissible fallback.
Selecting any model outside the frozen ordered set is a typed refusal, never a silent substitution.

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
7. Inspect the effective configured provider entry — every ordinary-workflow effect field in §3 plus the selection tuple of
   agent, preflight flag, model, ordered fallback list, effort, and mode — without resolving, checking out,
   configuring, or spawning a backend.
8. Validate Max qualification.
9. Validate retry counts and critical-path retry backoff.
10. Freeze each node's complete `FrozenProviderEffectV1` and `FrozenProviderSelectionV1`, compute the canonical
    provider-effect digest, the canonical selection digest, and the graph-bound execution-identity fingerprint over
    both, then construct the controls fingerprint and control-inclusive workload fingerprint from those frozen
    values.
11. Select the one authoritative ledger and record its `LedgerAdmissionV1` disposition in the frozen run spec, so no
    later barrier has to guess whether an offline history ledger was healthy.
12. Refuse inactive production behavior, including `fixed_grace` in R2f1a.
13. Only then create task/history rows, mutate context-cancel maps, construct/lookup a registry, create sessions, or contact a provider.

All arithmetic uses checked operations. `work_cutoff_ms + 70_000` must fit the persisted monotonic representation. Retry `max_attempts` becomes explicitly `1..=1024`; zero no longer means one. Zero backoff remains legal. Compute the maximum cumulative retry backoff on a DAG path, not the sum across parallel branches; it must be less than the frozen work cutoff.

At admission, persist:

- the complete frozen controls;
- their fingerprint;
- the control-inclusive workload fingerprint;
- the complete per-node frozen execution identities, including each `FrozenProviderEffectV1` and
  `FrozenProviderSelectionV1` with both digests and the combined identity fingerprint;
- the frozen `LedgerAdmissionV1` disposition;
- task class and policy version `r2f1a`;
- expected node count and bounded node-evidence reservation.

Max validation uses the frozen per-node efforts.

### Lease-first check-to-use binding for every provider attempt

Comparing the registry against the frozen identity and then resolving the registry again is a check-then-use gap: a
hot reload landing between the two reads makes the validated value and the used value different objects. R2f1a
therefore validates and consumes **one** immutable value.

#### What current main actually provides

The mechanism must be built from the seams that exist. On current `bridge-registry`:

- a `Slot` holds `entry: ArcSwap<AgentEntry>`, a `OnceCell` backend, a `retired` flag, and a lease counter;
- `AgentRegistry::resolve` returns `Resolved { entry, backend, lease }`, and `Lease::is_retired()` is the only
  lifecycle signal exposed to a caller;
- `entry_snapshot` returns a bare `Arc<AgentEntry>` and takes **no lease**, so nothing it returns is pinned;
- `apply` reuses the live slot for a **config-only** edit and swaps `slot.entry` in place, but allocates a **fresh
  slot** whenever `cmd`, `base_url`, `args`, `cwd`, `auth_method`, `pre_authenticated`, `kind`, `sandbox`,
  `session_cwd`, `api_key_env`, `watchdog`, `mcp`, or `mcp_delivery` changes, and marks every replaced slot
  `retired` **synchronously** before handing it to a detached drain;
- `invalidate` is keyed by agent id and retires whichever slot is mapped at call time.

There is **no** slot generation, revision, or version accessor anywhere on `AgentRegistry`, `Resolved`, or `Lease`.
The previous revision's `RegistryEntryGenerationV1` and its "slot generation token" assumed an API that does not
exist, so that mechanism was not implementable as written and is replaced below. Two consequences of the reconcile
rule above are load-bearing and are what make the replacement work without inventing a generation counter:

1. every **spawn-frozen** effect change retires the old slot, while a lease acquired before retirement keeps that
   exact slot alive long enough for the already-bound attempt to finish under its old entry and backend;
2. every **config-only** effect change leaves the slot alive and swaps its entry, so a lease cannot observe it — but
   an attempt that never re-reads `slot.entry` is unaffected by definition, because it keeps using the exact value
   it validated.

#### The bound-use contract

Immediately before every provider attempt — each preflight candidate, the real post-preflight turn, first node
attempt, retry, resumed attempt, and every batch child — the executor performs exactly one registry bind for that
attempt:

```rust
/// Additive on `AgentRegistry`; the default returns `None` and is an explicit opt-out
/// exactly like today's `entry_snapshot`. Non-spawning: it takes a lease and reads the
/// slot entry once, and never initializes the backend.
fn bind_entry_use(&self, id: &AgentId) -> Option<BoundEntryUseV1> { None }

/// Additive on `AgentRegistry`; resolves the backend of the **exact slot** already bound,
/// never by agent id again. Default returns typed `BindUnsupported`.
async fn resolve_bound(
    &self,
    bound: &BoundEntryUseV1,
    observer: Arc<dyn DiagnosticObserver>,
) -> Result<Arc<dyn AgentBackend>, BridgeError>;

/// Additive on `AgentRegistry`; retires the bound slot only while both that exact slot and
/// exact entry Arc are still mapped for its id. Default no-op.
async fn invalidate_bound(&self, bound: &BoundEntryUseV1) {}

pub struct BoundEntryUseV1 {
    pub entry: Arc<AgentEntry>,   // the exact immutable value validated AND used
    pub lease: Box<dyn Lease>,    // pins the slot through replacement and normal drain
    pub use_token: EntryUseTokenV1,
}

/// Opaque, process-local, non-durable. Identifies the exact bound slot and entry object
/// within this process only.
pub struct EntryUseTokenV1(/* private */);

struct FrozenEntryUseV1 {
    bound: BoundEntryUseV1,
    effect: FrozenProviderEffectV1,         // the frozen value, carried alongside for comparison
    selection: FrozenProviderSelectionV1,   // the frozen value, carried alongside for use
}

fn bind_frozen_entry(node: &FrozenNodeExecutionIdentityV1)
    -> Result<FrozenEntryUseV1, ConfigurationDriftV1>;
```

This is the minimum additive surface that makes the binding implementable, and it is added only where necessary:
`bind_entry_use` because no existing accessor returns a pinned entry, `resolve_bound` because `resolve` re-looks-up
by agent id and would reintroduce the gap, and `invalidate_bound` because id-keyed invalidation is not exact-bound.
Each has a default that keeps every current implementation source-compatible; a registry that does not implement
them is an explicit opt-out and refuses the R2f1a bound path rather than silently falling back to an unbound one.

**Durable identity versus the use token.** These are deliberately different things and must not be conflated:

| | Durable semantic identity | Opaque process-local use token |
|---|---|---|
| Value | `effect_digest`, `selection_digest`, `identity_fingerprint` | `EntryUseTokenV1` |
| Derived from | canonical configuration bytes | the bound `Arc<Slot>` and `Arc<AgentEntry>` object identity plus a process-local counter |
| Persisted | yes — snapshot V2, history rows, replay keys | **never** |
| Compared across processes or restarts | yes | **never** |
| Decides | drift, replay conflict, resume admissibility | same-slot linearization inside one process only |

A drift decision that terminalizes a node is always made on the durable digests. The token never appears in a digest,
fingerprint, projection, or persisted row, and a token mismatch is a linearization fault rather than a
configuration-drift verdict.

`bind_frozen_entry` takes the lease **before** reading the entry, obtains exactly one `Arc<AgentEntry>`, recomputes
the canonical provider-effect digest and selection digest from that exact value, and compares both plus the identity
fingerprint with the node's frozen identity. Because current `apply` publishes the replacement map before marking
old slots retired, `is_retired()` alone is not a sufficient current-slot test. The bind loop therefore:

1. loads one state snapshot and its slot, increments that slot's lease, and refuses/retries if it is already retired;
2. loads exactly one entry Arc, then re-loads the registry state and requires that the same slot Arc is still mapped
   under the id and is still not retired;
3. drops the lease and retries on a mapping mismatch; otherwise the successful same-slot/entry observation is the
   linearization point and the returned token owns both exact Arcs.

A config-only entry swap that races the entry load linearizes on whichever immutable Arc was loaded; a slot
replacement between the two state observations cannot escape on the not-yet-retired old slot. After successful
linearization, later replacement is allowed to retire the mapping while the lease pins the old use. The remaining
rules are:

- A missing entry or any digest/fingerprint difference terminalizes that node as a typed pre-prompt
  `configuration_drift` failure carrying which digest drifted — `effect` or `selection`. It never re-resolves
  controls, mutates a fingerprint, rewrites the frozen spec, checks out or configures a session, or calls the
  provider.
- An unrelated registry edit that leaves both frozen digests byte-identical is accepted; the digests, not object
  identity, are the test. A `name`, `description`, `tags`, or `version` edit is therefore not drift.
- On success, `FrozenEntryUseV1` is the sole source for the rest of that provider attempt. The selected candidate, the
  `SessionSpec`/`AgentOverride` model, effort, and mode, and the `configure_session`/`configure_turn` arguments all
  read `use.bound.entry` and `use.selection`. `resolve_bound` on the already-bound handle is the **only** further
  registry call the attempt may make; no code path may call `resolve`, `entry_snapshot`, `configured_effective`, or
  any other entry-reading accessor again. Because the attempt never re-reads `slot.entry`, a config-only reload
  landing after the bind cannot reach it: the validated value is the used value.
- Each candidate's backend resolution goes through `resolve_bound`, which resolves the backend of the exact bound
  slot rather than re-looking-up by agent id, and initializes that backend from the same bound entry if needed.
  `bind_entry_use` increments the lease before checking retirement and retries the current mapping if the chosen slot
  was already retired or no longer mapped; the successful same-slot/entry revalidation is the linearization point. A
  spawn-frozen reload landing
  **after** it may mark the old slot retired and map a fresh slot, but the attempt deliberately does not require the
  token to name the current mapping and does not convert `is_retired()` into drift. It finishes under the exact old
  slot, entry, backend, and lease it bound. Thus the only two admissible race outcomes are use of A consistently, or
  zero-effect digest drift when B won before the bind — never use of B under A's persisted identity.
- A slot force-retired mid-turn by the existing drain grace is an ownership condition, not drift. It surfaces through
  the existing backend error path with prompt-acceptance uncertainty sticky, and authorizes no retry, fallback, or
  replacement attempt.
- Retry invalidation is exact-bound: a node that invalidates its backend calls `invalidate_bound` with its own bound
  handle, and retirement occurs only if both its exact slot and exact entry Arc are still current. Id-keyed
  `invalidate` would retire whichever slot is mapped at call time, while a slot-only check would retire a warm slot
  whose entry was swapped after the bind; either can disrupt newer work after an intervening reload.
- Before any provider effect, the selected candidate is asserted to be a member of `frozen_candidates`. A selected
  model outside the frozen ordered set is a typed `provider_selection_out_of_set` refusal before checkout,
  configuration, and prompt — it is never written into `eff.model` or a checkout override.
- The pre-acceptance fallback walk consumes `frozen_candidates` in frozen order. When the frozen primary `M` fails
  before prompt acceptance, it calls `invalidate_bound` on M's exact use, drops that use, and the next admissible
  candidate `F1` takes a **new** bind and revalidates the same persisted digests before any effect. A reloaded `F2`
  is not in the frozen set, and any intervening effect/selection reload is detected at that new bind. Acceptance
  uncertainty stays sticky and no additional candidate is authorized unless the prior failure proved
  pre-acceptance.
- The run preflight cache key is `(agent, effect_digest, selection_digest)`, not agent alone. A successful decision
  under one frozen provider identity can single-flight and replay only within that exact identity; a changed digest
  cannot reuse it.
- Resume rebinds against the frozen identity persisted in snapshot V2, including the provider-effect digest. It never
  re-derives an effect or selection from current configuration.
- Replay is identity-bound: an exactly equal persisted pair of effect and selection digests replays idempotently, and
  a different effect digest, selection digest, or identity fingerprint for the same `(attempt_id, node_id)` is a
  persistence conflict, not last-writer-wins.

**Every path binds identically.** The bind is a property of a provider attempt, not of an entry point, so the same
`bind_frozen_entry` → `resolve_bound` sequence owns every one of these:

| Path | Binding obligation |
|---|---|
| Inline single-node execution | One bind per attempt before checkout, configuration, and prompt. |
| Workflow dispatcher | One bind per node attempt; the dispatcher's non-spawning config discovery is the bind itself, not a separate `entry_snapshot` read. |
| Preflight candidate walk | One bind per candidate provider attempt. A proven-pre-acceptance failure invalidates and drops that exact bound use; the next frozen candidate takes a fresh bind and revalidates both persisted digests before resolving a backend. |
| Real turn after successful preflight | Takes its own fresh bind against the same persisted digests; it never reuses the preflight candidate's slot or a cache entry keyed only by agent. |
| Retry | A retry is a new attempt and takes a new bind; it never reuses a stale bound handle across the retry boundary, and its invalidation is exact-bound. |
| Resume | Rebinds against the persisted frozen identity before any effect; drift refuses that node without changing the frozen spec. |
| Fresh and resumed batch children | Bind exactly as non-batch attempts do, with the same refusal ordering and zero provider/session effects on refusal. |

This closes the admission-to-dispatch and observation-to-use hot-reload windows without freezing a live backend,
adding provider retry or fallback behavior, inventing a registry generation API, or crossing into generation-drain
work.

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
   one operation and returns one of the four closed results defined below.
9. Recheck workflow cancellation only after the selected terminal's barrier acknowledgement.
10. Apply the policy action if still applicable.
11. Admit or structurally terminalize downstream nodes.
12. Process fake fixed-grace expiry only after completions at or before the same logical boundary.

A ready natural completion is never relabeled canceled. Simultaneous failures all retain their own terminal facts. Manual node cancellation, workflow cancellation, dependency skip, and legacy interruption do not qualify as fan-out failure triggers.

### `PolicyTriggerBarrier` results

A three-variant barrier that collapses "offline" into "telemetry unavailable" mislabels a healthy run and lets it
cancel siblings with no durable trigger anywhere. The barrier therefore has four closed results, selected from the
frozen `LedgerAdmissionV1` disposition rather than from the execution surface's name. **Offline** here means exactly
"no durable primary task store owns this attempt", which includes served execution backed by an in-memory task store.

| Result | Precondition | What committed | Policy action |
|---|---|---|---|
| `ServedPrimaryCommitted` | `DurablePrimaryTaskStore` | checkpoint terminal JSON, trigger, journal `NodeFinished`, sequence allocation, and start-row deletion in one durable transaction | authorized |
| `OfflineHistoryCommitted` | `HistoryLedgerAdmitted` | the trigger and its triggering node terminal in one durable history transaction | authorized |
| `OfflineTelemetryUnavailable { reason }` | `HistoryLedgerUnavailable`, or an attempted commit that failed with a reason the classifier below marks fail-open | nothing durable; in-process linearization only | authorized, fail-open |
| `PrimaryFailed` | the durable primary transaction failed, or the commit failed with a fail-closed reason — `Collision`, the durable trigger/terminal conflict | nothing | refused; global cancel and drain |

Rules:

- With a healthy admitted offline ledger the barrier **must attempt** the durable commit and must not return the
  fail-open marker without attempting it. `fail_fast` cancellation, `fixed_grace` arming, and downstream structural
  terminalization all wait for that acknowledgement.
- Only genuine optional-ledger unavailability may use the in-process marker. The reason is bounded and
  low-cardinality; raw database text is never projected.
- A durable conflict is not unavailability. A different trigger or a different triggering terminal already present
  for the same key is `PrimaryFailed`, matching the exact-replay-idempotent/conflict-refuses rule in §9.

**Exhaustive reason classification.** "Every bounded reason falls open" and "collision must fail closed" cannot both
hold, because `Collision` is itself a current `LedgerUnavailableReason` variant. The barrier therefore classifies the
enum exhaustively rather than by prose, over the exact 14 variants that exist today:

| `LedgerUnavailableReason` | Classification | Barrier result |
|---|---|---|
| `Open` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Permission` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `ReadOnlyDatabase` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `ReadOnlyLock` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `ReadOnlyParent` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `AdvisoryLockUnsupported` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `AdvisoryLockIo` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Locked` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Migration` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Schema` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Corruption` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Io` | availability | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `CapacityProtected` | capacity | `OfflineTelemetryUnavailable { reason }` — fail-open |
| `Collision` | durable conflict | **`PrimaryFailed`** — fail-closed |

Every current availability and capacity reason is fail-open; `Collision` **alone** is `PrimaryFailed`. This is exact
rather than stylistic: current producers reserve `Collision` for identity, lineage, lease-ownership, reservation, or
terminal replay conflicts, including — but not limited to — `TerminalWrite::Conflict`. It therefore means the
ledger reached a state whose identity or ownership cannot be reconciled safely, not ordinary optional-ledger
unavailability. Treating that as unavailability would let the barrier authorize targeted sibling cancellation while
the durable trigger identity is rejected or ambiguous. A producer audit is part of acceptance: every current
`Collision` construction must remain conflict/ownership-class, and any future use for mere availability must split
to an availability variant rather than silently inherit fail-closed semantics.

`Collision` therefore **cannot authorize targeted cancellation** under any policy. It takes the `PrimaryFailed` path:
global cancel and drain, no targeted sibling token, no `telemetry_unavailable` fail-open marker, and no second
ledger. The classifier is a total `match` with no wildcard arm, so a new `LedgerUnavailableReason` variant fails
compilation until it is explicitly classified as fail-open or fail-closed. It is never silently defaulted in either
direction.
- `PrimaryFailed` globally cancels and drains without targeted policy action. A failed transaction leaves no
  write-once checkpoint, so there is no retry that attempts to attach a trigger to an existing terminal.
- Optional history enrichment on a served durable attempt still occurs after primary ordering and cannot change the
  barrier result. On an `OfflineHistoryCommitted` attempt the trigger commit is policy-ordering evidence rather than
  optional enrichment; later enrichment of that same attempt remains fail-open.

**Trigger transaction and replay identity.** The durable trigger is keyed by `(attempt_id, control_event_id)` with
`control_event_id = attempt_id` plus ordinal; R2f1a admits at most one per attempt. The transaction writes the trigger
onto the attempt row and replaces the triggering node's placeholder terminal row in the same transaction, so a reader
never sees a trigger without its triggering terminal. Byte-identical replay is idempotent; any other value for the
same key is `policy_trigger_conflict`.

**Crash ordering.** Commit strictly precedes cancellation on both durable paths. A crash after the acknowledgement
and before cancellation leaves a durable trigger with siblings still running, which boot reconciliation resolves as
`interrupted` with the trigger retained. A crash before the acknowledgement leaves no trigger and no canceled sibling.
There is no window in which a sibling was canceled by a policy trigger that no durable record explains, except the
explicitly marked `OfflineTelemetryUnavailable` case, where the absence of durability is itself recorded.

**Outcome and projection semantics.** `OfflineHistoryCommitted` projects the trigger from durable state through the
attempt row, the structured result artifact, and the bounded stderr summary, and does **not** set
`telemetry_unavailable`. `OfflineTelemetryUnavailable { reason }` carries the trigger only in the in-process terminal
result and the offline artifact, sets `telemetry_unavailable{reason}` on the first status and terminal envelope, and
may set `degraded=true`; it never rewrites the workflow outcome, never fabricates `completed_degraded`, and never
tries a second ledger. All four results preserve the complete structured node map.

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
2. Reach a `PolicyTriggerBarrier` result. `ServedPrimaryCommitted` and `OfflineHistoryCommitted` mean the trigger is
   durable; `OfflineTelemetryUnavailable` means it is explicitly marked non-durable; `PrimaryFailed` refuses targeted
   action entirely.
3. Only then cancel each still-running sibling once with `canceled_policy`.
4. Record every unscheduled node as `NotStartedPolicy/NotNeeded`.
5. Continue polling every running future through its real cleanup result.
6. Later failures cannot replace or renew the trigger.

If workflow cancellation becomes observable before policy action, global cancellation wins the cancel cause; the already-observed failure remains recorded.

### `fixed_grace`

The controller state machine is implemented and tested with an injected monotonic waiter:

1. First qualifying failure reaches the barrier with one trigger and one frozen grace, under the same four-result
   contract as `fail_fast`.
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

Extend `task_node_checkpoints` with additive `terminal_json TEXT`, capped at `MAX_NODE_TERMINAL_JSON_BYTES`
(2,048 encoded bytes). Keep `ok` and `usage_json` for legacy readers.

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

```sql
CREATE TABLE workflow_attempt_node_terminals(
    attempt_id       TEXT NOT NULL,
    node_id          TEXT NOT NULL,
    terminal_json    TEXT NOT NULL,
    terminal_reserve INTEGER NOT NULL,
    charged_bytes    INTEGER NOT NULL,
    PRIMARY KEY(attempt_id, node_id)
) WITHOUT ROWID;
```

#### Single-copy key schema

The `NodeId` contract stays as stated: node IDs are arbitrary length, are never capped, and are never truncated in a
payload. The schema must therefore be honest about what SQLite physically stores. An ordinary rowid table with
`PRIMARY KEY(attempt_id, node_id)` stores the key twice — once in the table B-tree record and once in the automatic
`sqlite_autoindex` entry — so an accepted node ID spanning many pages is charged once and stored twice. `WITHOUT
ROWID` makes the primary-key B-tree the table itself, giving **exactly one** physical copy of the key bytes.

The single-copy property is an invariant, not an assumption. The correct metadata assertion is **not** that
`PRAGMA index_list` is empty. For a `WITHOUT ROWID` table SQLite still reports the main primary-key B-tree through
`index_list` with `origin='pk'`, so an emptiness assertion rejects exactly the schema it is meant to admit and no
conforming implementation could pass its own schema gate. The invariant is:

- this table has no secondary index, no additional `UNIQUE` constraint, and no indexed foreign key;
- a schema-shape regression asserts that `sqlite_master.sql` for the table contains `WITHOUT ROWID`; that
  `PRAGMA index_list(workflow_attempt_node_terminals)` reports **exactly one** entry, that entry has `origin='pk'`
  `unique=1`, and `partial=0`, and no entry has any other origin; that `PRAGMA index_xinfo` reports the two key
  columns in exact order — `attempt_id`, then `node_id` — with no additional key column; and that
  `SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND tbl_name='workflow_attempt_node_terminals'` is **zero**,
  proving the primary key has no separately rooted B-tree and is the table itself;
- adding any index later adds a second `index_list` entry and a separate `sqlite_schema` root, so the regression
  fails until every charge below is re-derived in the same change.

#### Placeholder materialization

At admission, create one placeholder node row per canonical graph node with its **full** key and a
`MAX_NODE_TERMINAL_JSON_BYTES` terminal reserve. The placeholder is materialized at exactly its reserve size: its
`terminal_json` is canonical filler of exactly `MAX_NODE_TERMINAL_JSON_BYTES` bytes, and its `terminal_reserve` and
`charged_bytes` integer columns are written at admission at their **final** values and never rewritten. Replacing the
filler with a real terminal therefore writes an equal-or-smaller payload — a shorter TEXT also has an equal-or-smaller
serial-type varint — into an already-allocated cell, so replacement cannot grow the row, split a page, or add an
overflow page. The reserve is consumed at admission or not at all.

#### Logical accounting

Accounting schema V2 uses `MAX_NODE_TERMINAL_JSON_BYTES = 2_048`, `MAX_POLICY_TRIGGER_JSON_BYTES = 1_024`, and
`MAX_CONTROLS_JSON_BYTES = 4_096` from §3, plus:

```text
NODE_ROW_OVERHEAD_BYTES = 256   // bounded attempt-id key bytes, the two integer columns,
                                // the SQLite record header/serial-type array, and B-tree cell overhead
```

Each node row's logical charge is
`MAX_NODE_TERMINAL_JSON_BYTES + NODE_ROW_OVERHEAD_BYTES + node_id.as_bytes().len()`, checked per row and in the sum.
The exact node-key bytes appear once because the schema stores them once.

The V2 configured-store invariant is exact:

```text
attempt_summary_charge = 16_384 + MAX_CONTROLS_JSON_BYTES + MAX_POLICY_TRIGGER_JSON_BYTES
                       = 16_384 + 4_096 + 1_024
attempt_charge = attempt_summary_charge
               + HISTORY_ATTACHMENT_CHARGE            // existing 1,024-byte attachment charge
               + sum over every node row of
                     (MAX_NODE_TERMINAL_JSON_BYTES + NODE_ROW_OVERHEAD_BYTES + exact node-id bytes)
allocation.charged_bytes = sum(attempt_charge for every retained attempt)
allocation.slots_used = count(retained attempt summaries)
```

Legacy rows keep their V1 `16_384 + 1_024` charge and zero node charge. `accounting_version = 2` admits variable
charges with one checked compare-and-debit; each summary stores its whole derived attempt charge, and retention
subtracts that stored value while cascading attachment/node rows by exact key. Migration sets `migrating`, creates and
verifies the new `WITHOUT ROWID` table and columns, rederives every stored and aggregate charge from authoritative
rows using the equation above, and flips to `ready` in one transaction; restart repeats safely, and any mismatch is
corruption rather than a rebaseline. The migration transaction is itself subject to the physical gate below and
rolls back while leaving `migrating` intact rather than exceeding the cap.

This equation is a **logical accounting charge** over bounded column contents. It is deliberately not a page count
and never stands in for one: physical admission for the platform ledger is decided by materialization and
measurement below.

#### Physical accounting for the platform ledger

**No static formula predicts pages.** The previous `2 * current_page_size * expected_node_count` term and its
replacement per-node `ceil(payload / page_size)` derivation are both removed, and no formula takes their place. Any
static logical-to-page prediction for an uncapped `NodeId` is unsound: SQLite's index-format B-tree pages carry only
a reduced embedded payload fraction, and the remainder spills into an overflow chain whose pages hold
`usable_size - 4` bytes each, so a sufficiently long ID needs strictly more pages than `ceil(payload / page_size)`
charges. The `4n`-pointer lemma that justified folding overflow pointers into two structure pages is false for the
same reason and is deleted. R2f1a does not predict the page cost of a node row — it **materializes the rows and
measures the result** under the physical controls that already exist.

`MAX_ATTEMPT_TERMINAL_JSON_BYTES` is the existing `bridge_core::workflow_history::MAX_TERMINAL_JSON_BYTES`, renamed at
its use sites so the attempt-terminal bound and the node-terminal bound can never be swapped in an equation.

**The existing hard controls are reused unchanged.** Current `bridge-store` already owns every physical control this
needs, and R2f1a weakens none of them. These are preconditions of the reservation path, not optional post-hoc
observations:

| Control | Current value and effect |
|---|---|
| `MAX_CHARGED_BYTES` | 128 MiB aggregate ceiling over the main database plus live `-wal`, `-journal`, and `-shm` |
| `HISTORY_SIDECAR_HEADROOM_BYTES` | 72 MiB, so the **main database ceiling is 56 MiB**, enforced as `page_count <= main_budget / page_size` |
| `HISTORY_DISK_TRANSACTION_HEADROOM_BYTES` | 68 MiB rollback-journal reserve, so even a whole-database rewrite cannot cross the aggregate ceiling |
| `PRAGMA max_page_count` | before schema migration or any history mutation, set and verify at no more than `floor(56 MiB / page_size)`; `SQLITE_FULL` at that ceiling maps to `capacity_protected` |
| platform journal policy | force and re-verify `DELETE`, `TRUNCATE`, or `PERSIST`; refuse WAL/MEMORY/OFF, set `cache_spill=OFF` and `journal_size_limit=0`, and checkpoint the legacy WAL before admitting a write |
| `history_growth_fits` | admission gate that subtracts reusable freelist bytes and adds the journal reserve |
| `ensure_terminal_rewrite_headroom` | already materializes a real `zeroblob` and asserts a **measured** freelist postcondition rather than predicting pages |

The last row is the precedent this repair follows: the proven pattern in current source is materialize-then-measure,
expressed "solely as SQLite's durable freelist count".

**Pre-effect reservation transaction.** All of the following happens in one transaction, before any provider,
session, checkout, or task effect:

1. under the serialized platform-store ownership/lease, revalidate the descriptor-bound main database and live
   `-wal`/`-journal`/`-shm` objects; re-read `page_size`, `page_count`, `freelist_count`, `max_page_count`, journal
   mode, and cache-spill policy; and refuse unless the hard page ceiling, supported rollback-journal mode, and
   existing aggregate-plus-transaction-headroom gate all still hold;
2. derive the logical `attempt_charge` from the §"Logical accounting" equation and perform the checked
   compare-and-debit against the allocation. This is an accounting charge, not a page prediction;
3. gate with the existing `history_growth_fits(conn, attempt_charge)` before writing any row;
4. materialize the attempt summary row, the attachment row, and **one full-size placeholder node row per canonical
   graph node**, in canonical `NodeId` order, each carrying its exact full key bytes and an exactly
   `MAX_NODE_TERMINAL_JSON_BYTES` filler payload. `SQLITE_FULL` from the hard ceiling is translated to the bounded
   `capacity_protected` refusal rather than generic I/O;
5. provision the node-terminal rewrite pool by extending the existing `ensure_terminal_rewrite_headroom` mechanism to
   node rows, so a later placeholder replacement is served from proven-reusable pages;
6. re-read the same PRAGMAs and descriptor-bound live sidecars and evaluate the authoritative postconditions **inside
   the still-open transaction**;
7. commit only if every postcondition holds; otherwise roll back. Immediately after commit, verify the aggregate
   physical ceiling again; an impossible mismatch quarantines the ledger and fails the invariant rather than
   rebaselining or authorizing another write.

**Authoritative postconditions.** These are measured facts about the database, not derived predictions. Inside the
transaction, after materialization:

```text
P1  page_count * page_size <= MAX_CHARGED_BYTES - HISTORY_SIDECAR_HEADROOM_BYTES     // 56 MiB main ceiling
P2  freelist_count >= pages required to replace one full-size node terminal in place
P3  every materialized placeholder is present with its exact full key and exact reserve length
P4  the debited logical charge equals the sum of the stored per-attempt charges
P5  max_page_count, rollback-journal mode, cache-spill policy, live sidecars, and the conservative
    aggregate transaction-headroom gate still satisfy the existing 128-MiB physical regime
```

`PRAGMA page_count` is the authoritative main-file measure here because it is transaction-visible and already
reflects every page this transaction allocated, including overflow pages for a long node ID. Filesystem size is not
used mid-transaction, since the main file need not have been extended yet; the aggregate `-wal`/`-journal`/`-shm`
concern is instead checked through the descriptor-bound live-sidecar measurement and owned by the pre-existing
72-MiB sidecar headroom, 68-MiB journal reserve, 4-MiB framing margin, supported rollback-journal policy, and hard
main-page ceiling. The page-count check is therefore not a measure-after-unsafe-write argument: `max_page_count`
prevents main-file overrun while materialization is in flight, and the rollback-journal policy bounds the transient
sidecar population before P1–P5 are evaluated.

**Rollback and refusal.** If any postcondition fails, the entire reservation transaction rolls back: no placeholder,
no summary row, no attachment, and no debit survives, and admission refuses with the existing bounded
`capacity_protected` reason before effects. Because the outcome depends on the real page cost of the real key bytes,
an attempt whose canonical graph carries node IDs too large for the remaining space is refused rather than admitted
on a formula that undercounted it. That refusal is a data-dependent capacity refusal proved by measurement, before
effects — it is **not** a `NodeId` length cap, is not a silent truncation, does not hash or shorten any ID, and does
not weaken the 128-MiB invariant. The identical graph admits on a ledger with headroom.

The surrounding properties are unchanged and remain true under measurement:

1. **Exact key bytes.** The single-copy `WITHOUT ROWID` schema stores the full `node_id` bytes exactly once, so the
   materialized row is the whole physical cost of the key.
2. **Placeholder replacement.** The placeholder occupies its full reserve from admission and its integer columns are
   final, so replacement writes an equal-or-smaller payload, adds no page, and draws only on the P2 pool.
3. **Migration.** The V2 migration runs inside this same gate and rolls back leaving `migrating` intact rather than
   exceeding the ceiling. It also rebuilds the accounting table, because the current
   `accounting_version INTEGER NOT NULL CHECK(accounting_version=1)` constraint cannot admit version 2 in place.
4. **Retention.** Collection removes the oldest unpinned terminal summaries first, cascading their attachment and
   node rows by exact key and crediting the exact stored attempt charge, so a long-ID attempt returns the bytes it
   consumed.
5. **Mixed V1/V2.** Legacy V1 attempts contribute zero node rows and keep their V1 charge; a mixed allocation sums
   both forms under one checked compare-and-debit.

Configured stores are not platform ledgers and have no page gate; they enforce the same 128-MiB ceiling through the
logical equation above, which charges the same exact key bytes.

Crash/failpoint tests cover the V1-to-V2 allocation transition, the rollback path, and every debit/credit boundary.

Add a bounded `policy_trigger_json` field to the attempt row, capped at `MAX_POLICY_TRIGGER_JSON_BYTES`. Exact replay
is idempotent; a different terminal, trigger, frozen provider-effect digest, or frozen provider-selection digest for
the same key is a persistence conflict.

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
    "node_execution_identities": [
      {
        "node": { "sorted_ordinal": 0, "id_sha256": "..." },
        "effect": {
          "agent": "...",
          "effect_digest": "..."
        },
        "selection": {
          "agent": "...",
          "preflight": true,
          "effective_model": "...",
          "ordered_fallback_models": ["..."],
          "effective_effort": "...",
          "effective_mode": "...",
          "selection_digest": "..."
        },
        "identity_fingerprint": "..."
      }
    ],
    "ledger_admission": {},
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
- Every provider attempt rebinds through the lease-first `bind_frozen_entry` against its persisted frozen provider
  effect and selection, including the preflight flag and the ordered fallback list. Either digest drifting fails that
  node before checkout/configuration/prompt without changing the frozen spec, and the resumed attempt never re-derives
  an effect or selection from current configuration. The opaque use token is process-local and is never restored from
  a snapshot; a resumed attempt mints a new one at its bind.
- The frozen `LedgerAdmissionV1` disposition is restored with the spec; a resumed attempt does not reselect a ledger.
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
- An `OfflineHistoryCommitted` attempt projects its trigger from the durable history attempt row and carries no
  `telemetry_unavailable` marker. An `OfflineTelemetryUnavailable` attempt projects the same trigger from the
  in-process terminal result and the offline artifact and carries the bounded reason. The two are distinguishable in
  every projection; neither is relabeled as the other.
- Every projection carries the exact canonical terminal and trigger bytes described in §3; no projection re-serializes
  with different escaping, ordering, or bounds.

## 10. Compatibility and migration

1. Existing config omission remains valid and preserves current scheduling: no sibling policy cancellation, failed markers may reach synthesis, and all scheduled futures drain.
2. The compatibility profile is now frozen bounded data, but R2f1a does not claim its outer bounds are enforced.
3. Built-in review/design workflows receive explicit review profile declarations.
4. New fingerprints intentionally differ because controls and both per-node provider-effect and provider-selection
   identities are calibration dimensions.
5. Existing task/checkpoint rows remain readable through Boolean fallback.
6. New binaries read V1 and V2 snapshots; old binaries cannot resume V2 working tasks and may mark them interrupted.
7. New task columns and journal fields are additive; old task readers still see the known `completed` status.
8. Workflow-history accounting V2 uses the exact logical equations above; migration is transactional, idempotent,
   schema-admitted, and rederives exact charges from authoritative rows. It creates the node-terminal table as
   `WITHOUT ROWID` with exactly one `unique=1`, `origin='pk'`, `partial=0` metadata entry, exact
   `(attempt_id, node_id)` key order, and no separately rooted index; verifies that shape before flipping to `ready`;
   rebuilds the accounting table whose current `CHECK(accounting_version=1)` constraint cannot admit version 2 in
   place; and rolls back inside the hard page/journal/sidecar gate rather than exceeding the ceiling. No static page
   formula participates: physical admission is decided by materializing the placeholders under `max_page_count` and
   the rollback-journal regime, then measuring P1–P5. Legacy rows receive zero node-evidence charge, and a mixed
   V1/V2 allocation sums both forms under one checked compare-and-debit.
9. Rollback after the allocation migration or V2 working-task creation requires stopping the new binary and restoring the pre-migration database snapshot. There is no in-place down-migration.
10. No migration infers timeout, policy trigger, cleanup completion, or degraded ancestry from legacy text.
11. The node-terminal, policy-trigger, and controls JSON bounds are encoded-byte bounds produced by one canonical
    serializer. `MAX_NODE_TERMINAL_JSON_BYTES` is a new constant and never aliases the existing 8-KiB attempt-terminal
    `MAX_TERMINAL_JSON_BYTES`; the earlier 1,024-byte node reserve and 512-byte trigger reserve were unsound under
    JSON escaping and are superseded everywhere in this document.
12. The additive `bind_entry_use`, `resolve_bound`, and `invalidate_bound` registry methods all carry defaults, so
    every existing `AgentRegistry` implementation stays source-compatible. A registry that does not implement them is
    an explicit opt-out whose nodes refuse the bound path; none silently falls back to an unbound resolve.
13. `NodeCauseV1.evidence_overflow` is additive and defaults to `false`. It indicates encoder overflow separately and
    never replaces a failure class or static code, so existing code/class vocabularies are unchanged.

## 11. Compile-correct build and ownership order

### Stage 1 — serial foundation

One owner changes:

- `crates/bridge-core/src/execution_policy.rs`
- `crates/bridge-core/src/lib.rs`
- `crates/bridge-core/src/ports.rs`
- `crates/bridge-workflow/src/graph.rs`
- new `crates/bridge-workflow/src/fanout.rs`
- run-spec serialization helpers

Land the pure types, resolver, exact constants, the canonical terminal/trigger/controls serializer with its derived
worst-case assertions and bounded-evidence fallback, the frozen provider-effect and provider-selection digests with
their exhaustive `AgentEntry` classification, the exhaustive `LedgerUnavailableReason` classifier, fingerprinting, and
controller transition tests first. Stage 1 also lands the additive `AgentRegistry` bind methods and their defaults in
`crates/bridge-core/src/ports.rs`, since Stage-2 and Stage-3 owners both depend on that signature. The workspace must
compile before parallel work begins.

### Stage 2 — parallel siblings from the same frozen Stage-1 base

- **Configuration owner:** `bin/a2a-bridge/src/config.rs` and config-only tests.
- **Controller owner:** `crates/bridge-workflow/src/fanout.rs` and fake/manual state-machine tests, without touching executor integration.
- **Persistence owner:** `crates/bridge-core/src/task_store.rs`, `workflow_history.rs`, `orch.rs`, and `crates/bridge-store/src/sqlite.rs`.

Each sibling owns disjoint paths, has an independently runnable test target, and does not modify manifests, roadmap, generated files, executor integration, or serving adapters.

### Stage 3 — single integration owner

After integrating all Stage-2 siblings, one owner changes the shared seams:

- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-registry/src/registry.rs`
- `crates/bridge-coordinator/src/detached.rs`
- `crates/bridge-coordinator/src/batch.rs`
- `crates/bridge-coordinator/src/coordinator.rs`
- `crates/bridge-coordinator/src/params.rs`
- `crates/bridge-coordinator/src/session_manager.rs`
- `bin/a2a-bridge/src/main.rs`
- `crates/bridge-a2a-inbound/src/server.rs`
- `crates/bridge-mcp/src/server.rs`

This stage owns every fresh/resumed batch and non-batch entrypoint, freezing, the lease-first `bind_frozen_entry`
check-to-use path for every provider attempt, event widening, terminal ordering, the four-result trigger barrier,
resume, projections, CLI/A2A/MCP overrides, and compile fixes. It also owns removing every remaining registry read
inside an attempt, so no path can re-derive a model, effort, mode, preflight flag, fallback candidate, or any other
effect-bearing field after the bind, and converting retry invalidation to the exact-bound form. The concrete
`bind_entry_use`/`resolve_bound`/`invalidate_bound` implementations in `crates/bridge-registry/src/registry.rs` are
part of this stage because they must land with their only production callers. These files must not be split among
concurrent implementors.

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
- Arbitrarily long accepted node IDs, high fan-in, ordinal overflow, node-ref mismatch, and dependency-set digest
  mismatch either retain exact graph-bound identity or refuse before effects; no bounded payload truncates an ID.

#### Frozen provider effect, selection, and lease-first binding

- The frozen selection round-trips agent, preflight flag, primary model, ordered fallback list, effort, and mode; the
  canonical digest is injective, so `["a|b"]` and `["a", "b"]` produce different digests, and reordering the fallback
  list changes the digest.
- The frozen provider-effect digest round-trips and changes for a change to **each** ordinary-workflow
  effect-bearing field individually — `kind`, `cmd`, `args`, `base_url`, `api_key_env`, `auth_method`,
  `pre_authenticated`, `cwd`, `session_cwd`, `sandbox`, `watchdog`, `mcp`, and `mcp_delivery`. The selection digest
  independently changes for `id`, `model`, `effort`, `mode`, `preflight`, or ordered `fallback_models`; the agent id
  is also carried in `FrozenProviderEffectV1`. Neither digest changes for a `model_provider`,
  `host_fallback_eligible`, `name`, `description`, `tags`, `version`, or `extensions` edit, and a source-boundary test
  asserts those exclusions remain outside ordinary workflow execution. The effect digest never contains a
  bearer value selected through `api_key_env`, only the env-var name. A separate non-disclosure fixture uses a
  redaction-sensitive MCP environment value, proves changing it changes the digest, and proves its raw bytes appear
  in no snapshot, history row, projection, diagnostic, or log artifact.
- **W1's exact constructible state, red first:** freeze an API agent against endpoint A with env `X`, then hot-reload
  only `base_url` to endpoint B (and separately only `api_key_env` to `Y`) before the queued node binds. Every
  selection field stays byte-identical. The repaired path refuses with `configuration_drift{effect}` before resolve,
  checkout, configuration, and prompt, and records zero provider effects. A selection-only digest passes this reload,
  so the old identity fails the fixture red.
- **The inherited fallback state, red first:** a queued preflight-enabled node freezes primary `M` with fallback
  `F1`; a hot reload rewrites only `fallback_models` to `F2`. If the reload wins before M's bind, the repaired path
  refuses on selection drift before resolve. If M binds first and then fails before prompt acceptance, its exact use
  is invalidated and dropped; F1 takes a new bind. A reload that wins before that bind yields zero-effect selection
  drift, while a reload after it leaves F1 consistently on the old bound identity. The path never observes or calls
  F2. A separate invariant fixture injects an out-of-set candidate and proves `provider_selection_out_of_set`
  refuses it before provider effect.
- Paused downstream dispatch plus `high -> max`, `max -> high`, model, or mode reload **before the bind** refuses
  before that provider attempt; the same reload after the bind leaves the attempt consistently on its old bound
  values. An unrelated reload leaving both frozen digests byte-identical proceeds. The same matrix covers retries
  and resume.
- Toggling only `preflight` between admission and dispatch refuses; the old triple-only identity accepted it.
- A reload landing **before** the bind refuses at the bind on a drifted digest and records zero
  checkout/configure/prompt effects. A **spawn-frozen** reload landing between the bind and a provider effect retires
  the old mapping but the held lease pins it, so the attempt completes on its bound old entry/backend and never
  touches the replacement. A **config-only** reload landing in that same window is likewise inert: the attempt
  continues on its bound entry, and a regression asserts the configured model/effort/mode equal the bound values,
  not the reloaded ones. The race harness forces both linearization orders and admits only zero-effect drift or one
  call whose complete effect and selection identity is the old frozen identity.
- Exactly one registry bind occurs per provider attempt. A counter-backed regression asserts one `bind_entry_use`
  for each preflight candidate and a separate bind for the real turn, retry, or resumed attempt, and **zero** unbound
  `resolve`, `entry_snapshot`, or `configured_effective` calls after each bind; only `resolve_bound` on that attempt's
  handle may follow.
- The bind takes its lease **before** reading the entry: a regression drives a retirement concurrent with a bind and
  asserts the bind either returns the old slot pinned before retirement, or observes retirement and retries the new
  mapping before reading its entry. It never returns an unpinned entry or combines an entry and backend from
  different slots.
- A deterministic interleaving publishes the replacement state but pauses before setting the old slot's retired
  flag. The bind must detect the slot-map mismatch and retry; an `is_retired()`-only implementation fails this red by
  returning the no-longer-mapped old slot. A config-only same-slot swap on either side of the single entry load binds
  one complete old or new entry Arc, never a mixed field set.
- Durable identity and the use token stay separate: a regression asserts `EntryUseTokenV1` appears in no digest,
  fingerprint, snapshot, history row, or projection, and that two bind calls yielding byte-identical entries produce
  equal digests but distinct tokens.
- An A→B→A reload completed before a bind is semantically admissible when both current digests again equal frozen A;
  the new A slot receives a distinct use token. An A→B→A replacement during a bind cannot create slot ABA because
  the state revalidation compares exact slot Arc identity, not only bytes or agent id.
- Exact-bound invalidation: a node that invalidates after an intervening reload retires only when the exact slot and
  entry Arc it used remain mapped; a regression asserts both a newly mapped slot and a config-swapped same slot
  serving a sibling are untouched. Id-keyed and slot-only invalidation fail this red.
- The run preflight cache is keyed by agent plus both frozen digests. Two nodes with the exact identity single-flight
  and reuse one decision; a node whose effect or selection digest differs runs its own preflight and cannot consume
  the prior decision.
- A registry that does not implement the additive bind methods refuses the bound path with a typed error and never
  falls back to an unbound `resolve`.
- The pre-acceptance fallback walk consumes only frozen candidates in frozen order; a `preflight = false` node has a
  one-element candidate set and refuses any fallback.
- Resume rebinds from the persisted frozen effect and selection and never re-derives either from current
  configuration; a resumed attempt under changed configuration refuses before effects, including a change confined to
  effect-only fields such as `base_url`.
- Inline, dispatcher, each preflight candidate, the real post-preflight turn, retry, resume, and fresh/resumed
  batch-child paths each bind exactly once per provider attempt: batch fixed-grace refusal, profile freeze, effect
  and selection freeze, Max validation, V2 resume, and fingerprints match the non-batch surfaces and record zero
  provider/session effects on refusal.
- Conflicting replay of a different effect digest, selection digest, or identity fingerprint for the same
  `(attempt_id, node_id)` refuses as a persistence conflict; byte-identical replay is idempotent.
- No test path introduces a provider retry, fallback, or replacement attempt that current main does not already make.

#### Terminal encoding bounds

- A 512-byte sanitized cause of only `"` characters, of only `\` characters, and of a mix encodes at or below
  `MAX_NODE_TERMINAL_JSON_BYTES` with every other field simultaneously at its maximum — the previous 1,024-byte
  reserve fails this fixture red.
- Adversarial UTF-8: astral scalars, combining sequences, a lone `\t`, and a byte-boundary-splitting truncation all
  encode within bound, and the serializer emits no `\uXXXX` escape for any of them.
- Control-character sanitization drops every control scalar except `\t` before construction, so no `\u00XX` escape can
  appear in a constructed terminal.
- The longest closed `primary`, `cleanup.disposition`, and `failure_class` tokens, a 64-byte static code, a full
  64-hex dependency digest, and a maximum-length control-event id together stay within bound.
- A constant test asserts `derived_worst_case <= constant` for the node terminal, policy trigger, and frozen controls,
  and asserts the canonical skeleton length is at most its declared ceiling.
- The fail-closed control: an injected over-bound value produces the bounded-evidence fallback, which itself encodes
  at or below `MAX_NODE_TERMINAL_JSON_BYTES`, and no over-bound row is ever written or admitted.
- **W5 evidence preservation, red first:** inject the overflow required by the fault-injection fixture and require the
  result to encode within 2,048 bytes while preserving the **original** `failure_class` and static `code`, a
  **nonempty** deepest-cause suffix, `evidence_overflow = true`, `cause_truncated` set when the cause was shortened,
  sticky prompt acceptance, degraded ancestry, and trigger identity. The previous fallback dropped `deepest_cause`
  and overwrote the code with `terminal_encoding_overflow`, so it fails this fixture red on both counts.
- Overflow is indicated separately, not by class or code substitution: two distinct failure classes sharing one
  static code both overflow and remain distinguishable, and `evidence_overflow` is the only differing signal against
  their non-overflowing counterparts.
- The retained suffix is budget-driven, not constant-driven: as the mandatory fields grow toward their ceilings the
  retained suffix shrinks monotonically and the encoding stays within bound; with an empty cause the mandatory shape
  encodes at or below 880 bytes.
- The derived worst case including `evidence_overflow` is 1,978 bytes and the checked
  `derived_worst_case <= MAX_NODE_TERMINAL_JSON_BYTES` assertion still holds with margin.
- Round-trip: checkpoint, history row, journal `NodeFinished`, `TaskStatusDto`, MCP, A2A, and the offline artifact all
  carry the exact same canonical bytes for the same terminal.
- No formula, column cap, or reserve anywhere still uses 1,024 for a node terminal or 512 for a policy trigger, and
  `MAX_NODE_TERMINAL_JSON_BYTES` is never substituted for the 8-KiB attempt-terminal `MAX_TERMINAL_JSON_BYTES`.

### Scheduler

- Bounded-independent failed root plus running sibling: no sibling cancel, trigger absent, sibling completes, typed marker reaches synthesis, terminal is completed-degraded.
- All healthy: completed, no degraded ancestry.
- Fail-fast: trigger persists before cancellation, running sibling cancels exactly once, pending nodes never start, every future drains.
- Simultaneous failures with reversed delivery select the same lowest `NodeId`; the selected node's first and only
  checkpoint atomically includes the trigger, and all failures persist exactly once.
- Workflow cancel already observable suppresses a new policy action.

#### Trigger-barrier results

- **Healthy offline, red first:** an offline fail-fast run with a healthy reserved writable configured or platform
  ledger, one failing node, and a running sibling returns `OfflineHistoryCommitted`; the trigger and its triggering
  terminal are durable **before** the sibling's cancellation token fires, and the attempt carries no
  `telemetry_unavailable` marker. The current sole offline result fails this fixture red on both counts.
- **Genuinely unavailable:** an admission-time unavailable optional ledger, and separately a commit that fails with
  each of the thirteen fail-open `LedgerUnavailableReason` codes — `Open`, `Permission`, `ReadOnlyDatabase`,
  `ReadOnlyLock`, `ReadOnlyParent`, `AdvisoryLockUnsupported`, `AdvisoryLockIo`, `Locked`, `Migration`, `Schema`,
  `Corruption`, `Io`, `CapacityProtected` — return `OfflineTelemetryUnavailable { reason }`, still authorize policy
  action, record the bounded reason with no raw database text, try no second ledger, and leave the workflow outcome
  unchanged. `Collision` is explicitly excluded from this table and is covered by the fail-closed case below.
- **Collision is fail-closed, red first:** seed a conflicting durable trigger or triggering terminal under the same
  identity so the commit fails with `Collision`. The barrier returns `PrimaryFailed`, globally cancels and drains,
  takes **no** targeted policy action, signals no sibling cancellation token, and sets no `telemetry_unavailable`
  marker. The previous matrix returned `OfflineTelemetryUnavailable` and authorized targeted cancellation for this
  same input, so it fails this fixture red.
- The classifier is exhaustive and table-tested: every one of the fourteen current `LedgerUnavailableReason` variants
  is asserted to be exactly one of fail-open or fail-closed, with no wildcard arm, and adding a variant fails
  compilation until classified.
- A source-level producer audit and targeted fixtures cover reservation identity, parent lineage, process lease,
  replay, and terminal-write conflicts; every one maps to `Collision` and the fail-closed barrier result. A fixture
  that injects an availability failure proves it maps to its specific fail-open reason instead of `Collision`.
- A healthy admitted offline ledger never returns the fail-open marker without attempting the commit; a counter-backed
  regression asserts the attempt occurred.
- **Primary failure:** a failed durable primary transaction returns `PrimaryFailed`, globally cancels and drains, and
  takes no targeted policy action; no partial checkpoint or orphan trigger survives.
- A durable trigger or triggering-terminal conflict returns `PrimaryFailed`, not `OfflineTelemetryUnavailable`.
- Served execution backed by an in-memory task store takes the offline path, not the served path.
- Crash ordering under failpoints: a crash after the barrier acknowledgement and before cancellation reconciles to
  `interrupted` with the trigger retained; a crash before the acknowledgement leaves no trigger and no canceled
  sibling.
- Byte-identical trigger replay on `(attempt_id, control_event_id)` is idempotent; any other value is
  `policy_trigger_conflict`.
- All four results preserve the complete structured node map, and each is distinguishable in every projection.

#### Grace, cancellation, synthesis, and cleanup

- Fixed grace with fake time: before-expiry completion, exact-boundary completion wins, expiry cancels remaining nodes, later failure does not renew, duplicate expiry is a no-op.
- Paused production time advanced beyond two hours causes no warning, snapshot, grace expiry, outer timeout, preservation, or cancellation.
- Manual per-node cancel affects only the selected child; duplicate and late cancellation are no-ops.
- Strict mode skips downstream prompt with the verified sorted dependency-set reference.
- Degraded mode admits the same input and propagates taint through a successful intermediate and empty terminal.
- Existing provider/command timeout maps to `TimedOut` without producing an outer `Deadline`.
- Cancellation storm retains all in-flight futures until exact cleanup results return.
- Held cleanup prevents terminal publication; immediately ready cleanup still precedes terminal; cleanup failure records `Failed`, never `Complete`.

### Persistence, migration, and projection

- Memory and SQLite round-trip frozen controls, frozen provider effects and selections with both digests, trigger,
  every node terminal, ancestry, and cleanup duration.
- Exact replay succeeds; conflicting replay refuses.
- Accounting-V2 migration is idempotent and the exact mixed V1/V2 row equation rederives under boundary capacity,
  retention, rollback-required, concurrent admission, and crash/failpoint fixtures.

#### Arbitrary-node-ID physical accounting

- **Schema shape, red first:** `sqlite_master.sql` for `workflow_attempt_node_terminals` contains `WITHOUT ROWID`;
  `PRAGMA index_list` reports **exactly one** entry with `unique=1`, `origin='pk'`, and `partial=0` and no entry of
  any other origin; `PRAGMA index_xinfo` reports exactly the key columns `attempt_id`, then `node_id`; and
  `sqlite_schema` holds **zero** `type='index'` rows for the table, proving the primary key is the table itself with
  no separately rooted B-tree. The previous empty-`index_list` assertion rejects exactly this intended schema, so it
  fails this fixture red. Adding a secondary index or changing key order then proves rejection.
- **Near-cap long-ID regression, red first:** an accepted node ID spanning many pages against a platform ledger close
  to its ceiling. Admission either commits with the measured post-materialization `page_count * page_size` at or
  below the 56-MiB main-database budget, or rolls back entirely and refuses before effects with bounded
  `capacity_protected`. The ordinary-rowid schema plus the `2 * page_size * expected_node_count` term fails this
  fixture red by admitting an attempt whose real file exceeds the budget.
- **No static page prediction survives:** a regression asserts that admission consults measured `page_count` and that
  no `node_row_physical`, `NODE_ROW_STRUCTURE_PAGES`, or `ceil(payload / page_size)` term participates in the
  decision. The specific falsifying case is covered directly — a 512-byte page database with a 1 MiB node ID, where
  the deleted formula charged 2,055 pages while the node ID and terminal alone need at least 2,068 overflow pages —
  and the repaired path decides it by measurement rather than arithmetic, in both the admit and refuse directions.
- Postconditions are authoritative and checked at boundaries: short IDs, exact-page-boundary IDs, single-overflow
  IDs, and multi-page IDs each satisfy P1–P5, with reserved page bytes, autovacuum on and off, a legacy
  WAL-to-supported-rollback transition, each supported rollback journal mode, `cache_spill=OFF`, exact hard
  `max_page_count`, live sidecars, and near-cap admission and refusal. A forced `SQLITE_FULL` at the hard page limit
  is a bounded pre-effect `capacity_protected` refusal.
- **Rollback evidence:** a refused reservation leaves no placeholder, summary row, attachment, debit, or partial
  allocation, and the allocation accounting is byte-identical to its pre-admission state. A failpoint injected
  between materialization and postcondition evaluation produces the same result.
- Replacing a full-size placeholder with a real terminal adds zero pages, never exceeds the reserve, and draws only
  on the provisioned reusable pool; the integer columns are unchanged by the replacement.
- Retention of a long-ID attempt credits exactly the stored attempt charge and cascades its attachment and node rows
  by exact key, returning the allocation to its pre-admission accounting.
- A refusal is a capacity refusal, never a `NodeId` length cap: the same graph admits on a ledger with headroom, and
  no path truncates, hashes, or rejects an ID for being long by itself.
- Mixed V1/V2 allocations sum legacy zero-node charges with V2 node charges under one checked compare-and-debit, and
  the migration rolls back inside the physical gate while leaving `migrating` intact rather than exceeding the
  ceiling. The migration rebuilds the accounting table rather than attempting to store version 2 under the current
  `CHECK(accounting_version=1)` constraint.
- Configured-store logical accounting charges the same exact key bytes and the same per-row overhead as the platform
  physical derivation.
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
- Primary terminal commits before optional history enrichment on a served durable attempt.
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

Require one fresh cumulative hard-read-only correctness review. The **implementation** review cap is one full review
plus at most one targeted closure round for closed enumerable findings; if the second round exposes an open class,
stop and escalate rather than extending the cap. This implementation cap is unchanged and is separate from the
design-review budget recorded in the dogfood evidence, which the operator has extended per round; this revision is a
further targeted repair of the same closed enumerable population and consumes no implementation budget. Neither
budget authorizes implementation before the pending design closure review returns.

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

## 16. Closure-blocker repair and review disposition

Sol closure review 2 accepted the encoded-terminal reserve as closed, rejected five blockers, and deferred one WRONG.
W6 was roadmap-only and was already applied to the program cursor at this revision's frozen base. This revision
repairs the four remaining technical blockers and folds the deferred W5 correction. None crosses into R2f1b, and
every non-goal in §14 is preserved.

| Blocker | Repaired mechanism |
|---|---|
| W1 — provider identity did not cover the provider call: a tuple-identical hot reload of `base_url`, `api_key_env`, `kind`, command/args, sandbox, auth, MCP, or session location changed the call while passing a selection-only digest | §3 adds `FrozenProviderEffectV1`, a durable canonical digest over every ordinary-workflow effect field not already carried by the unchanged selection digest; exhaustive `AgentEntry` destructuring explicitly routes all fields to provider effect, selection, carried identifier, or excluded compatibility/R2d/presentation metadata, so a new field cannot be silently omitted. `identity_fingerprint` covers both digests. §5 replaces check-then-use with a lease-first, same-entry, bound-use contract: one bind per provider attempt takes the lease **before** reading the entry, validates both digests against that exact immutable value, and consumes that same value for candidate selection, configuration, and dispatch. A reload that wins before the bind yields zero-effect drift; a reload after the bind may retire the old mapping, but the lease pins that old slot and the attempt finishes consistently on it without re-reading the registry. Each preflight candidate and the real post-preflight turn takes its own bound use; exact-bound invalidation drops only the failed candidate's slot, the next candidate revalidates both digests, and the run cache is keyed by agent plus both digests. The binding covers inline, dispatcher, retry, resume, and fresh/resumed batch paths, refuses any candidate outside `frozen_candidates` before provider effect, and makes retry invalidation exact-bound. Current main exposes **no** registry generation or revision API, so the prior `RegistryEntryGenerationV1` was not implementable; §5 specifies the minimal additive `bind_entry_use`/`resolve_bound`/`invalidate_bound` replacement, each with a source-compatible default, and keeps durable digests strictly distinct from an opaque process-local `EntryUseTokenV1` that is never persisted or compared across processes. No provider retry or fallback beyond today's candidate walk is added. |
| W2 — the required `WITHOUT ROWID` schema test always rejects the intended schema, because SQLite reports the main PK B-tree through `PRAGMA index_list` with `origin='pk'` | §9 replaces the false empty-`index_list` assertion with the correct metadata invariant: exactly one `index_list` entry with `unique=1`, `origin='pk'`, and `partial=0`; exact `index_xinfo` key order `(attempt_id, node_id)`; no entry of any other origin; and zero `type='index'` rows in `sqlite_schema` for the table, proving the primary key is the table itself with no separately rooted B-tree. §10 and §12 carry the same invariant through migration verification and a red-first schema-shape regression that also proves a secondary index or changed key order is rejected. |
| W3 — the arbitrary-ID page formula was not a conservative physical bound: at a 512-byte page a 1 MiB node ID needs at least 2,068 overflow pages where the formula charged 2,055 total | §9 removes the formula rather than re-deriving it, together with `NODE_ROW_STRUCTURE_PAGES`, the `ceil(payload / page_size)` term, and the false `4n` lemma, because no static logical-to-page prediction is sound for an uncapped `NodeId` under SQLite's local-payload and overflow rules. R2f1a instead **materializes** every full-size placeholder inside the pre-effect reservation transaction and decides admission from measured postconditions — `page_count` against the existing 56-MiB main-database budget, the provisioned reusable-page pool, exact placeholder presence, charge equality, and the revalidated aggregate physical regime. The write is bounded *before* the post-check by the existing hard `max_page_count`, supported rollback-journal policy, `cache_spill=OFF`, descriptor-bound live-sidecar check, 68-MiB transaction reserve plus framing margin, and `history_growth_fits`; `SQLITE_FULL` maps to `capacity_protected`. Failure rolls the whole transaction back before effects, and a post-commit aggregate mismatch quarantines the ledger. §12 replaces every static-prediction test with measured-postcondition, boundary, hard-limit, sidecar, transition, and rollback regressions. The `NodeId` contract is unchanged — no cap, no truncation, no hashing — and the 128-MiB invariant is unchanged. |
| W4 — `Collision` was simultaneously fail-open and fail-closed, since the normative rule required `PrimaryFailed` while the acceptance matrix said every bounded reason falls open | §6 adds an exhaustive classifier over the fourteen current `LedgerUnavailableReason` variants: all thirteen availability and capacity reasons are fail-open to `OfflineTelemetryUnavailable { reason }`; `Collision` **alone** is `PrimaryFailed` and cannot authorize targeted cancellation because current producers reserve it for identity, lineage, lease-ownership, reservation, or terminal replay conflict/ambiguity rather than optional-ledger unavailability. The classifier is a total `match` with no wildcard arm, so a new variant fails compilation until classified, and a producer audit prevents a future availability use from silently inheriting fail-closed semantics. §12 replaces the contradictory "each bounded reason" regression with the thirteen-variant fail-open table plus red-first reservation, lineage, lease, replay, and terminal-conflict cases. |
| W5 (deferred) — the overflow fallback deliberately discarded the deepest cause and overwrote the failure code | §3 replaces `minimal_over_bound` with a bounded-evidence fallback that preserves the primary failure evidence — original `failure_class` and static `code` — retains the deepest UTF-8 suffix that fits the remaining measured budget, and indicates overflow **separately** through an additive `evidence_overflow` flag. The 2,048-byte proven bound is retained: the derived worst case becomes 1,978 bytes including the additive indicator, still under the checked `derived_worst_case <= constant` invariant, and the fail-closed control for a still-over-bound value is unchanged. §12 adds the red-first evidence-preservation regression. |

The complete likelihood, impact, fix, regression, and BLOCKER/DEFER analysis for each finding is retained in the
linked [review record](../reviews/2026-08-01-r2f1a-sol-closure-review-2.md). Every dependent type, digest, freeze
order, schema and accounting clause, migration, resume and replay rule, projection, ownership stage, test, and
compatibility clause in this document has been reconciled to the mechanisms above; no superseded formula, reserve,
assertion, or barrier matrix remains stated as an alternative.

This document is a design checkpoint awaiting its closure review. No implementation, test result, review approval,
release, deployment, or live operator effect is claimed, and no execution id, attempt id, artifact hash, or verdict
exists for that pending review.

R2F1A FOCUSED BOUNDARY: AWAITING SOL CLOSURE REVIEW
