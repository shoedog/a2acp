# R2f1a focused implementation boundary — profiles, fan-out policy, and per-node control

- **Status:** PARKED — SOL/XHIGH CLOSURE REVIEW 5 REJECT / ONE CLOSED-ENUMERABLE BLOCKER / CAP EXHAUSTED /
  IMPLEMENTATION UNAUTHORIZED
- **Frozen base:** `3f35ee6e07e9af314bb548b9d3ab694f3bba5fb1`
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Normative authority:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)
- **Parent plan:** [`2026-07-11-r2f-phase-aware-liveness.md`](2026-07-11-r2f-phase-aware-liveness.md)
- **Sol input:** `374ee10f8c4db570277c81803ad65e84520bb3f2aa0294a6e75057e1468ae9d6`
- **Fable input:** `d612788847a9142172cb38080bc77568e23c89116f44153ec0376b17327ce8c0`
- **Synthesis:** `644c2df21579bcb3dc9e07f347911f1516ebf61d6c0b9493433d117d83070a84`

This document records the repaired proposed source boundary. It narrows, but does not replace, the approved owner
design and parent plan. Its contents are not implementation authority. Closure review 4 rejected the prior exact
clean repair commit on two closed-enumerable blockers. The owner then authorized one bounded repair of exactly that
population, deterministic documentation gates, and one cumulative Sol/xhigh closure review. A rejection parks this
checkpoint; it does not authorize another repair, review, implementation, live gate, release, deployment, or
operator effect.

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

The candidate at commit `2bcbd524a39ebe7edb5928655681fbe7acad29e5` claimed to repair W1–W4, fold deferred
W5, and reconcile the authoritative roadmap. It remained a design repair only: no source, test, configuration,
prompt, generated artifact, implementation, release, deployment, or live operator effect was claimed.

The one authorized Sol/xhigh closure review ran as execution `exec-f8111c2406f9cf397beecd38ea6fc18b`, attempt
`attempt-60c2819ae1c8fd396fe3325a7e839d84`, against that clean commit, focused-artifact SHA-256
`36a10dcb9e74e768cd857d5d182432741def4893b3290b443f4d4e5790e0cbbe`, and roadmap SHA-256
`87f4ce2640c3f6a56fb722a652076759cf879aa2c72c7a5e9b75b48d54cc2e86`. Its 17,525-byte terminal artifact
had SHA-256 `154417a11a82c989eaa7682f718e23fc16d9b124e899e8267b53a6a663a6016b`; the checked-in
[`closure review 3 record`](../reviews/2026-08-01-r2f1a-sol-closure-review-3.md) differs only by the
repository-standard final newline. The review returned `REJECT` with four closed-enumerable blocker `WRONG`
findings and no `SMELL`. The declared round allowed one Sonnet/xhigh mining turn, one Opus/xhigh repair, and one
Sol/xhigh closure review. That cap is exhausted, so the checkpoint is parked rather than silently repaired again.

The owner then authorized one further bounded repair of that exact closed population, deterministic documentation
gates, and exactly one Sol/xhigh cumulative closure review. The repair decisions are explicit: API session model
state becomes tri-valued; MCP environment values gain typed secret references and secret-silent keyed commitments
without pretending to infer credential entropy; configured-store accounting uses measured allocation-owned pages
plus pre-debited journal/WAL mutation tickets; and the injected overflow comparison permits only the exact
dependency/cause changes the fallback requires. This cap authorizes no second repair, replay, fallback review,
implementation, Rust test claim, live compatibility case, release, deployment, or operator mutation. A rejecting
closure review parks the artifact again.

The pre-freeze docs gate ran `cargo run -p a2a-bridge -- validate --repo-hygiene` successfully in the scratch clone
and reported **39 tracked artifacts / 7 validated example configs**. `git diff --check` and direct existence checks
for the changed documents' roadmap, owner-design, parent-plan, prior-review, and ADR targets also passed. The hygiene
command compiled the dev binary in the scratch clone; no Rust test suite, compatibility case, smoke, or provider
behavior was exercised by these deterministic checks.

The exact docs-only repair was frozen at commit `3440829aa920de8bf6782a7181d3c664cc56f87b`, tree
`55577cfb7e933c044e2388f436b758a778a9a890`, with focused-artifact SHA-256
`6af2531fbbe042e73f892571fb08713dd09640517fd05be46b1b712822afcb85` and roadmap SHA-256
`f8ef242c1b00d02f09aa9273c63b5cc13675d3f54ab2899f31d4d32b7b0367d6`. The one authorized Sol/xhigh
closure review ran through host `codex-acp 1.1.7` / nested Codex `0.145.0` on exact advertised
`gpt-5.6-sol[xhigh]` / `xhigh` / `read-only` as execution `exec-5a7db8aac53fcc0092dc7c937b3f931a`, attempt
`attempt-8f62c3a1dd9b15c88301c9e8d9182e3e`. The first model-catalog probe was inadmissible because the managed
sandbox could not initialize the existing Codex state; it recorded `prompt_may_have_been_accepted: false`. The exact
same host probe then passed, and the review was dispatched once with no replay or fallback.

The 17,689-byte raw review artifact has SHA-256
`868010ca02b8fc8403dd673910469553e727b84c3c57ac88fb9b3e7a34c1a5f4`; the checked-in
[`closure review 4 record`](../reviews/2026-08-01-r2f1a-sol-closure-review-4.md) differs only by the
repository-standard final newline. It returned `REJECT` with two blocker `WRONG` findings and no `SMELL`: effective
request cwd and `{cwd}`-resolved MCP delivery bytes are not committed into provider identity, and SQLite FULL
auto-vacuum can relocate an unrelated tail page and rewrite more pointer-map pages than `D(R)=3R+2` reserves. The
review classified both as closed-enumerable with bounded fixes, but the declared cap permits neither fix here.

The owner subsequently authorized one new bounded round: repair exactly those two findings, run deterministic
documentation gates, and dispatch exactly one cumulative Sol/xhigh closure review. The repair freezes one resolved
effective session cwd per node into snapshot V2 and provider identity, derives the exact MCP delivery bytes from
that value once, and makes those same bound bytes the only session/MCP delivery source. It also rejects configured-
history `auto_vacuum=FULL` before any durable or provider effect and permits `INCREMENTAL` only while the bridge-
owned connection cannot execute incremental vacuum. This authorization does not extend to implementation, a second
repair/review loop, Rust behavioral evidence, a compatibility case, release, deployment, or operator mutation.

The docs-only repair was frozen at clean commit `f5096575814d40e0b5e506e03bb7c03c21a780e6`, tree
`3652ad65239bce1150d86b213c2deab421f8e4b3`, with focused-artifact SHA-256
`8388b24c5781aa68e9e742ad065283f25b749ba29dfba6da0e7334737e2fa96d` and roadmap SHA-256
`5e5463adbea772f6845c00a1caa147530d8ae45e783765c249a9513dd1c5b040`. The one authorized review ran through host
`codex-acp 1.1.7` / nested Codex `0.145.0` on exact advertised `gpt-5.6-sol[xhigh]` / `xhigh` / `read-only` as
execution `exec-1882a11ffa68f9dad47ae79115e16939`, attempt
`attempt-454556a4b041f292d63612df44d7c6cb`. The first model-catalog probe was an inadmissible managed-sandbox state-
initialization failure with `prompt_may_have_been_accepted: false`; the identical host probe passed, and the review
was dispatched once with no replay or fallback.

The 15,717-byte raw artifact has SHA-256
`f6bf2f57c72d378410b9819f5ffc9f1452b61394205222cfdc69932414dcc607`; the checked-in
[`closure review 5 record`](../reviews/2026-08-01-r2f1a-sol-closure-review-5.md) adds only the repository-standard
final newline and has SHA-256 `47718d2cbb5f56b06d2a9e6c6f3bc54afbdb1e229a2741eca3c06d9251e82fff`. It returned `REJECT` with one blocker
`WRONG` and no `SMELL`: when worktrees are enabled, `WorktreeBackend` derives an attempt/session-specific inner cwd
after the proposed provider-effect bind, so the frozen cwd/MCP bytes, the actual worktree delivery, isolation, and
resume identity cannot all be true. W1/W1-B remain `PARTIAL`; W1-A and W2-W6 are `FIXED`. The defect is
closed-enumerable around that shipped decorator, but the declared cap is exhausted.

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
| Provider-effect and selection freeze | Freeze two distinct canonical digests per node whose combined identity covers every ordinary-workflow provider input: a **provider-effect digest** over the spawn, checkout, credential, session, watchdog, effective session cwd, and exact resolved MCP-delivery fields, and a **selection digest** over the agent, preflight flag, primary model, exact ordered fallback candidate list, effort, and mode. Freezing only the configured entry or selection tuple leaves the call mutable: a tuple-identical hot reload of `base_url`/`api_key_env`, or request cwd A versus B under one `{cwd}` template, can change the provider call while passing the old digest. Fields used only by compatibility resolution, guarded R2d fallback, or presentation are explicitly excluded by source boundary. |
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
   fits the remaining encoded budget, setting `cause_truncated` to its prior truth value OR whether that shortened
   the cause;
5. return the exact byte string. A value still over bound after step 4 is an invariant violation: the node
   terminalizes through the typed over-bound path and no over-bound row is ever written or admitted.

**Bounded-evidence fallback.** The earlier fallback discarded `deepest_cause` and *replaced* the static code with
`terminal_encoding_overflow`, which destroyed the most useful failure evidence exactly when an invariant had already
been violated, and contradicted the owner design's requirement that failed roots and strict/degraded results retain
the deepest bounded cause. Overflow is therefore indicated **separately** from the failure it describes:

- `failure_class` and `code` always keep their original values; no code is overwritten, and `evidence_overflow` is
  the only **dedicated overflow-classification field**. It is not the only serialized field permitted to change:
  fitting the bounded representation may drop `dependency_set` and shorten the cause as specified below. Two
  distinct failures that overflow remain distinguishable.
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
    /// Absolute, lexically normalized path bytes actually supplied to the session.
    pub effective_session_cwd: SessionCwd,
    /// Commits the exact ordered, post-substitution MCP bytes for the selected delivery channel.
    pub mcp_delivery_digest: Sha256HexV1,
    pub effect_digest: Sha256HexV1,
    /// Present exactly when the entry carries one or more MCP environment values.
    pub secret_commitment_key_id: Option<Sha256HexV1>,
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
    /// Normalized per-request override after allowed-root validation; absence is distinct.
    pub requested_session_cwd: Option<SessionCwd>,
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
mint, watchdog behavior, or MCP prompt transport and is not already carried by the selection digest. It additionally
binds the resolved per-node `effective_session_cwd` and exact ordered post-substitution MCP delivery commitments;
those are request/run effects and cannot be reconstructed from `AgentEntry` alone:

| Effect group | Fields |
|---|---|
| backend construction and transport | `kind`, `cmd`, `args`, `base_url` |
| credentials and authentication | `api_key_env` (the variable **name** only, never its value), `auth_method`, `pre_authenticated` |
| checkout, isolation, and session location | raw `cwd`, raw `session_cwd`, resolved `effective_session_cwd`, `sandbox`, `watchdog` |
| tool surface offered to the agent | `mcp` with exact server/argument/environment order, public source descriptors, keyed template commitments, keyed exact-delivery commitments, and the managed-depth marker, plus `mcp_delivery` |

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
participates. The exhaustive builder also receives a required `ProviderFreezeContextV1` containing the immutable
operator-launch cwd and the already validated request override; this is how it classifies effects that are not
fields of `AgentEntry`. MCP commands, arguments, and environment values alter the delivered tool surface, but they
enter durable identity only through the secret-silent keyed commitments below. No raw MCP command, argument,
environment value, key path, or key byte is persisted, projected, or logged by this identity mechanism.

**One effective cwd, selected once.** `effective_session_cwd` is resolved during pre-effect run-spec construction,
separately for each node's bound entry, with exactly this precedence:

```text
validated requested_session_cwd
  ?? entry.session_cwd
  ?? entry.cwd
  ?? operator_launch_cwd
```

The request value is already an absolute, lexically normalized `SessionCwd`. A configured absolute value is parsed
the same way; a configured relative value (including `.`) is joined to the absolute launch cwd captured once when
the operator/one-shot command starts and then lexically normalized. The resolver does not consult the filesystem,
follow a symlink, re-read `current_dir`, or claim R2d-style object identity. Invalid/overflowing values refuse before
task, history, registry, session, or provider effects. `requested_session_cwd` preserves absence versus presence in
`WorkflowRunSpecV1`; every node identity preserves its resolved effective value because entries may have different
fallbacks when the request is absent.

Existing sandbox/worktree containment code may still canonicalize a separately held host source object to validate
or mount it, but that security result cannot silently replace the frozen delivery text. Where a container needs a
canonical host mount source, composition separates that source from the container destination/session cwd and uses
the frozen effective path for the latter. No post-freeze `canonicalize`, spawn helper, or backend default may change
the bytes supplied to `SessionSpec.cwd` or `{cwd}` substitution. This deliberately gives ordinary workflow identity
textual path semantics; R2d remains the separate object-identity boundary.

`SessionSpec.cwd` is always `Some(node.effect.effective_session_cwd)` on the V2 path. Neither the executor nor a
backend may fall back again to entry configuration. Resume with no new request envelope uses the persisted value.
If a resume surface does supply a new cwd field, its explicit absence/presence and normalized bytes must equal the
persisted request field or the resume refuses with a pre-effect persistence conflict. V1 keeps the legacy
`TaskRecord.session_cwd` path only for V1 compatibility.

V2 also removes the current `run-workflow`/served/batch technique of stamping a request cwd into cloned
`AgentEntry.session_cwd`. The configured raw field remains immutable and participates only at its declared fallback
precedence. The request travels solely in `ProviderFreezeContextV1`, is persisted separately, and produces each
node's effective value. Otherwise resume would compare a transiently rewritten entry against the unstamped current
registry and could not distinguish operator configuration from a per-request override.

**Typed MCP environment values and secret-silent commitment.** The current `EnvToml { name, value }` and
`McpServerSpec.env: Vec<(String, String)>` make a literal value both delivery data and potential credential material.
R2f1a replaces that ambiguous internal shape with one closed source enum while retaining declaration order:

```rust
pub enum McpEnvValueSourceV1 {
    PublicLiteral(String),
    SecretFromEnv { variable: String, resolved: SecretString },
}

pub struct McpEnvBindingV1 {
    pub name: String,
    pub source: McpEnvValueSourceV1,
}
```

Each `[[agents.mcp.env]]` sets exactly one of `value` or `value_from_env`. `value` is explicitly a public literal and
retains today's `{cwd}` substitution. `value_from_env` is a nonempty environment-variable **name**, permits no
template syntax, and is resolved exactly once while constructing the immutable `AgentEntry`; missing, non-Unicode,
or empty input refuses the snapshot. `SecretString` has secret-silent `Debug`/display/serialization behavior. The
same immutable source bytes held by the bound entry are used for commitment and delivery: referenced secrets are
delivered directly, while every public MCP argument/environment literal is substituted once with the frozen
`effective_session_cwd`. No later `std::env` read or second template expansion may change a referenced or public
value between validation and provider use. Reload re-resolves into a new entry, so a changed referenced value is
ordinary provider-effect drift. Existing literal syntax remains valid, but the type and operator docs no longer call
it a credential channel.

The resolver produces one cloneable but non-serializable `BoundMcpDeliveryV1` beside the durable commitments. Its
custom `Debug` reports only channel, digest, and counts. It contains the exact ordered
server/command/argument/environment bytes after `{cwd}` substitution and after the reserved managed-depth
environment marker is normalized, plus the exact channel representation: already-rendered ACP servers,
Codex-native argv suffix, or Kiro-native agent name and JSON bytes. It is created from the exact immutable entry and
frozen cwd used to recompute `effect_digest`; those bytes, not the templates, are passed to the selected adapter.

An additive `AgentBackend::configure_bound_session(session, &BoundSessionSpecV1)` carries the ordinary
`SessionSpec` plus `Arc<BoundMcpDeliveryV1>`. Its default is a typed `BoundSessionUnsupported` refusal, so existing
backends and exhaustive `SessionSpec` struct literals remain source-compatible while no V2 caller can fall back
silently. V1 alone continues to call `configure_session` and may use legacy static `AcpConfig.mcp`. Production ACP,
container, native, API, and worktree backends implement the additive method for V2; API's explicitly empty delivery
still requires the method and cannot bypass effect verification. ACP stashes the bound value beside cwd/config, and
mint converts its already-rendered server tuple directly to wire types. It performs no substitution and does not
consult static MCP config on the V2 path. The container backend's per-session `prepare_inner` consumes the same
bound Codex-native argv suffix. Host native spawn receives the same value through `resolve_bound`. Thus every
currently distinct delivery seam has one explicit V2 input and no backend has to reconstruct it.

ACP delivery remains per session. Native delivery is process-start input, so a native backend is reusable only for
the exact `mcp_delivery_digest`: the bound registry subslot is keyed by the complete provider-effect digest (which
includes effective cwd and delivery bytes), and `resolve_bound` either returns that exact subslot or cold-spawns it
from `BoundMcpDeliveryV1`. A backend spawned for cwd A cannot serve cwd B, and a config-only slot reuse cannot share a
native MCP child across different effect keys. Retirement/invalidation drains every keyed backend owned by the
slot, while exact-bound invalidation names only the keyed backend used by the failing attempt. This is a bounded
adapter-process partition, not a provider retry or fallback. It preserves served cross-repository native-MCP use
without delivering a static operator cwd under a per-request session identity.

That registry subslot rule applies to an `AgentKind::Acp` backend whose Codex/Kiro native MCP is fixed when the
adapter process starts, including a read-only sandbox child. `ContainerRwBackend` remains one outer session manager:
its actual native child is already minted per `BoundSessionSpecV1`, and `prepare_inner` consumes the bound argv for
that child. API/worktree/ACP-protocol delivery likewise stays session-scoped. This classification is exhaustive over
`AgentKind × McpDelivery` and fails compilation when either enum gains a variant.

Kiro's current stable `~/.kiro/agents/a2a-mcp-<agent>.json` would defeat that partition: spawning effect B can
overwrite the path still named by effect A. V2 first hashes a versioned Kiro-render domain, every constant rendered
field, and the canonical post-substitution public server tuple (excluding only the name derived next), derives
`a2a-mcp-v2-<full-hex-tuple-digest>` from it, renders that exact name into the final JSON, and commits both the
name/argv and final JSON bytes in `mcp_delivery_digest`. Any future renderer change must bump that domain. The bridge creates
the owner-only content-addressed file atomically without overwrite; an existing regular file is reusable only after
exact byte equality, otherwise spawn refuses. The file is made owner-read-only after file and parent sync and is not
automatically unlinked by one backend, so a concurrent process with the same delivery cannot lose its named config.
Kiro secret references remain forbidden, so this immutable cache contains only schema-declared public delivery
bytes. A separately reviewed custody command may later reap unreachable V2 files; normal retirement never mutates a
possibly shared name.

`SecretFromEnv` is admitted only when the resolved delivery is `McpDelivery::Acp`. Current `CodexNative` renders MCP
values in process arguments and `KiroNative` writes them into native settings; those channels remain public-literal
only and reject a secret reference until they have a separately reviewed secret-safe transport. For ACP, every
referenced source variable is removed from the adapter child's ambient environment, the value is delivered only in
the typed ACP MCP-server field, and its redaction material is installed before any diagnostic-capable send. This is
delivery custody, not a claim that a deliberately hostile adapter cannot inspect the MCP configuration it receives.

Every MCP environment value is keyed before it enters durable identity, including a declared-public literal. This
defense-in-depth rule closes the offline oracle even when an old or mistaken config put a low-entropy credential in
`value`; the bridge does not try to infer entropy or secrecy from names or bytes. For each environment binding,
compute `HMAC-SHA256(K, domain || schema || agent || server_ordinal || env_ordinal || env_name || source_kind ||
source_name_if_any || template_bytes || exact_delivery_bytes)` with injective length prefixes and distinct template
and delivery domains. Public MCP names, commands, and arguments—including their template and exact delivered bytes—
enter the ephemeral canonical SHA input directly; their schema declares them public, and no raw copy is persisted.
The canonical provider-effect encoding contains the public server/env order and source descriptor plus each 32-byte
environment template/delivery MAC, never a configured or resolved environment value. The exact
`effective_session_cwd` is independently encoded even when no MCP field contains `{cwd}`. `mcp_delivery_digest`
covers the complete ordered delivery including public command/argument bytes, environment MACs, and the managed-
depth marker; `effect_digest` remains SHA-256 over that digest plus the resulting secret-silent canonical effect
encoding. The bridge-generated fixed managed-depth marker is public domain-separated input and does not by itself
require `K`.

`K` is an operator-provisioned, exactly 32-byte random provider-effect key named by
`[security].provider_effect_key_file`. The bridge opens that existing regular file descriptor-relatively/no-follow
under the same owner-private custody rules used for history state, requires one link and owner-only access on Unix,
requires an absolute canonical path in a separately held platform secret-state directory, and rejects containment
under a repository, session/allowed-cwd root, evidence/output root, configured SQLite artifact, or any projected
container mount. Raw and canonical containment checks prevent a symlink/`..` alias from bypassing that separation.
The process loads the key once and never passes it to an adapter child, container, snapshot, history row, export, or
diagnostic. Syntax-only validation performs no key-file I/O; doctor and effect freezing report a bounded typed
failure. An entry with no MCP environment values needs no key and persists `secret_commitment_key_id = None`.
Otherwise the key is mandatory before snapshot-V2 persistence or any registry/session/provider effect, and the
persisted key id is
`HMAC-SHA256(K, "a2a-bridge/provider-effect-key-id/v1")`. The 32-byte key requirement is a custody requirement for this
bridge-generated verifier key; it does not claim that an arbitrary MCP credential has measurable entropy.

The supported provisioning path is `a2a-bridge provider-effect-key create --out <absolute-new-path>`. It obtains all
32 bytes from the operating-system CSPRNG, creates the destination with no-follow/exclusive-create and owner-only
mode, refuses an existing path or link, writes and syncs the file, syncs its containing directory, and emits no key
bytes. `init` may invoke the same primitive but never overwrites or rotates a key. A deterministic fault-injection
test covers every write/sync boundary and leaves either no entry or one complete owner-private key. Runtime cannot
statistically prove entropy from an imported 32-byte string; accepting an externally provisioned file is therefore
an explicit operator assertion that it was CSPRNG-generated, while the bridge-created path supplies the enforceable
default. Neither path attempts to infer the entropy of an MCP value.

Resume resolves the current key and every typed value once, requires the current key id to equal the persisted id,
recomputes every MAC, and compares the resulting effect digest before any effect. Missing/replaced key material,
missing referenced environment input, or a changed value refuses as typed provider-identity drift; it never creates a
replacement key, silently accepts a new epoch, or falls back to an unkeyed digest. Key rotation for working attempts
is a separate administrative migration and is not added here. Terminal historical rows remain readable without the
key because they are evidence, not authority to resume.

**Selection digest.** `selection_digest` is unchanged: SHA-256 over a canonical, injective encoding of the whole
selection tuple, each component emitted as a length-prefixed byte string in fixed order — agent id, preflight flag,
model presence plus bytes, fallback count plus each fallback in declaration order, effort token, mode presence plus
bytes. Length prefixing is required so a concatenation such as `["a|b"]` cannot collide with `["a", "b"]`. It remains
separate from `effect_digest` because the two answer different questions: the selection digest is what the workflow
froze and what a candidate must belong to; the effect digest is what the registry entry would actually do. A node
whose selection is satisfied by an entry that has been re-pointed at another endpoint must fail on the effect digest,
and a projection must be able to say which of the two drifted.

`identity_fingerprint` is SHA-256 over the node reference, resolved `effective_session_cwd`, `mcp_delivery_digest`,
`effect_digest`, and `selection_digest`, so a request-cwd, delivered-tool, selection, or other provider-effect change
always changes the node identity. All three digests are durable semantic
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
7. Validate and freeze the optional request cwd, capture one absolute operator-launch cwd, and inspect each effective
   configured provider entry — every ordinary-workflow effect field in §3 plus the selection tuple of agent,
   preflight flag, model, ordered fallback list, effort, and mode — without resolving, checking out, configuring, or
   spawning a backend.
8. Resolve each node's effective session cwd with the closed precedence above; resolve its exact ordered MCP delivery
   once from that cwd and the immutable entry; reject any invalid source/key/delivery before effects.
9. Validate Max qualification.
10. Validate retry counts and critical-path retry backoff.
11. Freeze each node's complete `FrozenProviderEffectV1` and `FrozenProviderSelectionV1`, compute the canonical
    MCP-delivery, provider-effect, and selection digests, and the graph-bound execution-identity fingerprint over
    all of them, then construct the controls fingerprint and control-inclusive workload fingerprint from those
    frozen values.
12. Select the one authoritative ledger, reject unsupported configured-history physical modes, and record its
    `LedgerAdmissionV1` disposition in the frozen run spec, so no
    later barrier has to guess whether an offline history ledger was healthy.
13. Refuse inactive production behavior, including `fixed_grace` in R2f1a.
14. Only then create task/history rows, mutate context-cancel maps, construct/lookup a registry, create sessions, or contact a provider.

All arithmetic uses checked operations. `work_cutoff_ms + 70_000` must fit the persisted monotonic representation. Retry `max_attempts` becomes explicitly `1..=1024`; zero no longer means one. Zero backoff remains legal. Compute the maximum cumulative retry backoff on a DAG path, not the sum across parallel branches; it must be less than the frozen work cutoff.

At admission, persist:

- the complete frozen controls;
- the optional normalized requested session cwd;
- their fingerprint;
- the control-inclusive workload fingerprint;
- the complete per-node frozen execution identities, including effective session cwd, MCP delivery digest, each `FrozenProviderEffectV1` and
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
    effect: &BoundProviderEffectV1,
    observer: Arc<dyn DiagnosticObserver>,
) -> Result<Arc<dyn AgentBackend>, BridgeError>;

/// Additive on `AgentRegistry`; retires only the exact keyed backend used by this effect while
/// its exact slot and entry Arc are still mapped for the id. Default no-op.
async fn invalidate_bound(&self, bound: &BoundEntryUseV1, effect_digest: &Sha256HexV1) {}

pub struct BoundEntryUseV1 {
    pub entry: Arc<AgentEntry>,   // the exact immutable value validated AND used
    pub lease: Box<dyn Lease>,    // pins the slot through replacement and normal drain
    pub use_token: EntryUseTokenV1,
}

/// Non-serializable effect material handed to `resolve_bound`/`configure_session`.
/// Its digest fields must equal the persisted frozen identity before any use.
pub struct BoundProviderEffectV1 {
    pub effective_session_cwd: SessionCwd,
    pub mcp_delivery_digest: Sha256HexV1,
    pub effect_digest: Sha256HexV1,
    pub delivery: BoundMcpDeliveryV1,
}

/// In-memory only; the delivery has secret-silent Debug and no durable serializer.
pub struct BoundSessionSpecV1 {
    pub session: SessionSpec,
    pub provider_effect: Arc<BoundProviderEffectV1>,
}

/// Additive AgentBackend method. The default returns BoundSessionUnsupported.
async fn configure_bound_session(
    &self,
    _session: &SessionId,
    _spec: &BoundSessionSpecV1,
) -> Result<(), BridgeError> {
    Err(BridgeError::BoundSessionUnsupported)
}

/// Opaque, process-local, non-durable. Identifies the exact bound slot and entry object
/// within this process only.
pub struct EntryUseTokenV1(/* private */);

struct FrozenEntryUseV1 {
    bound: BoundEntryUseV1,
    effect: FrozenProviderEffectV1,         // the frozen value, carried alongside for comparison
    selection: FrozenProviderSelectionV1,   // the frozen value, carried alongside for use
    bound_effect: BoundProviderEffectV1,    // exact committed bytes; secret-silent and never persisted
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

`bind_frozen_entry` takes the lease **before** reading the entry, obtains exactly one `Arc<AgentEntry>`, resolves the
exact MCP delivery from that value and the node's already frozen effective cwd, recomputes the MCP-delivery,
provider-effect, and selection digests, and compares all three plus the identity fingerprint with the node's frozen
identity. The `BoundMcpDeliveryV1` returned is the exact byte source later handed to the backend. Because current
`apply` publishes the replacement map before marking
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
  `SessionSpec` effective cwd, `AgentOverride` model, effort, and mode, MCP delivery, and the
  `configure_session`/`configure_turn` arguments all read `use.effect`, `use.bound_effect`, and `use.selection`.
  `resolve_bound` on the already-bound handle/effect is the **only** further
  registry call the attempt may make; no code path may call `resolve`, `entry_snapshot`, `configured_effective`, or
  any other entry-reading accessor again. Because the attempt never re-reads `slot.entry`, a config-only reload
  landing after the bind cannot reach it: the validated value is the used value.
- The API backend represents its per-session model as
  `Unconfigured | ExplicitNone | ExplicitSome(String)`, not `Option<String>`. A newly minted legacy session is
  `Unconfigured` and may use the backend's spawn-time default; every successful `configure_session` changes it to
  `ExplicitNone` or `ExplicitSome` from the bound `SessionSpec`. `resolve_model` falls back to the spawn default only
  for `Unconfigured`. An explicit bound `None` therefore suppresses a stale default captured when the warm API slot
  was first spawned, while `Some(B)` selects B. This rule is API-local and does not force a new registry slot or
  weaken the config-only warm-reuse contract.
- Each candidate's backend resolution goes through `resolve_bound`, which resolves the backend of the exact bound
  slot rather than re-looking-up by agent id, and initializes that backend from the same bound entry/effective-cwd/
  delivery tuple if needed. ACP delivery is session-local. CodexNative/KiroNative backend reuse is keyed by the
  complete effect digest; two effective cwd or native-delivery digests can never share one spawned child.
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
- The run preflight cache key is `(agent, effect_digest, selection_digest)`, not agent alone. Because the effect
  digest contains effective cwd and exact MCP delivery, a successful decision
  under one frozen provider identity can single-flight and replay only within that exact identity; a changed digest
  cannot reuse it.
- Resume rebinds against the frozen identity persisted in snapshot V2, including requested/effective cwd, MCP
  delivery, and provider-effect digests. It re-resolves only to verify and obtain the exact bound delivery bytes; it
  never replaces the frozen effect, cwd, or selection from current configuration.
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
  fails until the measurement roster and mutation-ticket proof are adjudicated in the same change.

#### Placeholder materialization

At admission, create one placeholder node row per canonical graph node with its **full** key and a
`MAX_NODE_TERMINAL_JSON_BYTES` terminal reserve. The placeholder is materialized at exactly its reserve size: its
`terminal_json` is canonical filler of exactly `MAX_NODE_TERMINAL_JSON_BYTES` bytes, and its `terminal_reserve`
integer column is written at admission at its **final** value and never rewritten. Replacing the
filler with a real terminal therefore writes an equal-or-smaller payload — a shorter TEXT also has an equal-or-smaller
serial-type varint — into an already-allocated cell, so replacement cannot grow the row, split a page, or add an
overflow page. The reserve is consumed at admission or not at all.

#### Configured-store measured accounting

Configured shared stores cannot use the fixed `exact key bytes + 256` charge. A 256-byte constant does not bound
SQLite record headers, B-tree structure, overflow pointers, or WAL frames for an uncapped `NodeId`. Accounting V2
therefore deletes `NODE_ROW_OVERHEAD_BYTES`, every per-attempt page prediction, and the invariant equating aggregate
charge to a sum of logical row lengths.

The bundled SQLite is compiled with `SQLITE_ENABLE_DBSTAT_VTAB`. In the serialized pre-effect transaction, after
materializing the exact rows, query `dbstat('main')` and checked-sum `pgsize` for one explicit
`HISTORY_ALLOCATION_TABLES_V2` roster and every SQLite index whose `sqlite_schema.tbl_name` belongs to that roster:

- `workflow_attempt_summaries`;
- `workflow_history_attachment`;
- `workflow_history_rewrite_reserve`;
- `workflow_attempt_node_terminals`;
- `workflow_history_mutation_reserve`; and
- `workflow_history_allocation`.

Admission retains the existing atomic authority/history transaction. That transaction explicitly inserts or updates
`attempt_identities` on success; a served refusal may instead update `attempt_identities` and
`task_attempt_locators` without admitting a history row. Those permanent authority/primary roots and their journal
frames remain outside the configured workflow-history allocation, exactly like unrelated task/turn data. The ticket
is root-attributed rather than whole-transaction-attributed: it conservatively reserves every frame whose database
page belongs to a roster root, while explicit authority/locator frames are primary-store custody. This preserves the
accepted R2f0a property that a large permanent identity or unrelated primary table cannot brick history admission.
A served refusal that writes no roster root creates no history ticket or history debt.

The schema gate rejects an unclassified table/index in the allocation namespace, any trigger on an allocation or
explicitly co-mutated authority/locator root, and any foreign-key action that a history-root mutation could drive
outside the history roster. The attachment delete cascade terminates inside the roster. Adding a history index
changes the measured charge automatically; adding a trigger or an escaping cascade refuses until its dirty roots and
mutation direction are adjudicated. `tasks`, `sessions`, `turn_log`, task journals/checkpoints, attempt identities,
task-attempt locators, harvest tables, and every other primary-store object are deliberately outside the roster:
unrelated primary data does not consume the 128-MiB history allocation, and it cannot hide a history page because
every history-owned root is enumerated from `sqlite_schema`. `dbstat` counts leaf, interior, and overflow pages in
the current transaction, so the full arbitrary ID and placeholder are charged by their real table-local page
allocation rather than a payload formula.

`workflow_history_mutation_reserve` is `WITHOUT ROWID` and keys a ticket by bounded `attempt_id`, closed numeric
`mutation_kind`, and checked `u32` ordinal; node tickets use the graph-bound sorted ordinal and never duplicate an
arbitrary `NodeId`. Its reserve is an exact eight-byte big-endian BLOB and its state is a closed integer, so rebasing a
ticket cannot enlarge its record. The admission transaction materializes every ticket row before the final `H`
measurement, then writes only equal-width bounds/states. Schema-shape tests reject a secondary index, variable-width
reserve, full node-id key, or unclassified state/kind.

Accounting V2 likewise rebuilds every dynamic numeric cell in `workflow_history_allocation`—the five byte
components, aggregate charge, slot count, and terminal count—as an exact eight-byte big-endian BLOB decoded through
checked `u64`; allocation kind/state are closed one-byte integer codes. Static limits are schema constants, not
rewritten counters. The singleton has no secondary index. Replacing a component value or flipping a closed state
therefore cannot change record width after measurement; a variable-width INTEGER/TEXT component is a schema refusal.

Every possible post-admission history write consumes one persisted closed mutation ticket: one ticket per node
terminal replacement, at most one trigger barrier, one attempt terminalization, and each separately bounded
enrichment/reconciliation mutation named by the schema. There is no wildcard ticket, and an unreserved mutation
refuses before SQL. Let `H` be the freshly measured count of allocation-owned pages after materialization and let
`frame_bytes = page_size + 24`. Inside a future history-only mutation the implementation remeasures and requires
`H_post <= 2 * H + 2`, so the pre/post roster-root union has size at most `R = H + H_post <= 3 * H + 2`.

`dbstat` deliberately omits shared structural pages, so `R + 2` is not a proof: at 512-byte pages an auto-vacuum
database with one 1-MiB value has 2,138 pages while `dbstat` reports only 2,116, 2,115 of them for the owning table.
Before relying on any structural multiplier, configured-history admission and migration therefore enforce a closed
auto-vacuum policy:

- `PRAGMA main.auto_vacuum=NONE` is admitted;
- `INCREMENTAL` is admitted only with bridge-owned `incremental_vacuum` prohibited while configured history V2 is
  active; and
- `FULL` and every unknown value are typed unsupported-configuration refusals before task/history/session/provider
  effects. They do not enter the optional-ledger `Schema` fail-open classifier.

For an existing configured database, opener/freeze first uses a no-create, read-only, no-follow connection to read
`main.auto_vacuum` before opening the read-write store, applying write pragmas, creating schema, or creating a
primary task row. FULL/unknown therefore refuses without changing database-file bytes or any bridge business row;
SQLite's read-only lock/shm housekeeping is not misrepresented as byte-for-byte directory custody. An absent path
may proceed to ordinary fresh creation, whose SQLite default is then checked rather than assumed and before schema.

The authoritative check runs again on the serialized configured-store connection after `BEGIN IMMEDIATE` and before
the first history/authority SQL statement, and migration performs it before setting `migrating` or executing DDL.
Because `BEGIN IMMEDIATE` excludes a concurrent mode-changing writer, the in-transaction value is the value
governing commit. Every configured-history write rechecks it; read-only/open-time success is not cached as authority.
The bridge installs one connection authorizer (and keeps a source-level exhaustive production call-site guard) that
rejects `PRAGMA incremental_vacuum` while V2 configured history is active. A future maintenance implementation must
add a separately reviewed whole-relocation ticket or remain rejected. No bridge path issues `VACUUM` or changes
`auto_vacuum`; a database changed while closed is rejected at its next open/admission.

With FULL relocation excluded and incremental vacuum unexecutable, SQLite does not relocate an unrelated tail page
at history commit. The closed mutation SQL also uses no temporary table, DDL, `VACUUM`, or insert-then-delete
allocation churn; every roster page it allocates or frees appears in the pre/post union. For any such root union,
reserve `D(R) = 3 * R + 2` dirty pages: `R` for every pre/post roster page, at most `R` distinct pointer-map pages
whose entries describe those roster pages, at most `R + 1` distinct freelist trunk/leaf pages, and one database-
header page. This proof applies only to admitted NONE/INCREMENTAL-without-vacuum modes; it makes no claim about FULL.
The future history-only WAL ticket is consequently `W(D(3 * H + 2)) = 32 + (9 * H + 8) * frame_bytes`, with checked
arithmetic. A larger root transition or any SQL plan outside the closed no-churn shape rolls back as
`capacity_protected`; it is never learned after commit.

Admission uses its own exact closed history ticket because arbitrary IDs can make its page growth much larger than a
post-admission lifecycle mutation. Measure roster pages as `H0` before the transaction's first write and `H1` after
materializing the complete history result and pre-materializing every fixed-width ticket/allocation cell, but before
installing their computed equal-width values and committing. Its history root-union bound is
`R_admission = H0 + H1`; its dirty-page bound is
`D_admission = D(R_admission) = 3 * (H0 + H1) + 2`, and its WAL ticket is
`W(D_admission) = 32 + D_admission * frame_bytes`. The explicit authority/locator writes in the same transaction do
not enter `H0`/`H1`; the structural multiplier covers shared pointer-map, freelist, and header pages attributable to
roster growth. Every ticket, component, count, and state cell is pre-materialized before the final measurement, so
installing the checked values and consumed state cannot grow a root after `H1`; an out-of-range value refuses.

History transactions run with connection-local `cache_spill=OFF` while holding the store mutex, so one transaction
cannot emit repeated spill frames for the same dirty page. An RAII guard captures the exact prior spill setting,
disables it before `BEGIN IMMEDIATE`, and restores it on commit, rollback, error, cancellation, and unwind;
restoration failure quarantines configured-history writes instead of silently changing primary-store policy.
Whenever measured `H` changes, admission/mutation recomputes **all** unconsumed tickets under the same serialized
transaction before accepting the new state, so a later attempt cannot make an older ticket stale.

The closed kinds are `admission`, `prompt_acceptance`, `node_terminal`, `trigger_barrier`, `attempt_terminal`,
`cleanup_settlement`, `final_activity`, and `boot_reconciliation`. Admission persists their exact legal population:
its own commit ticket, one prompt transition, one node transition per canonical node, at most one trigger, one of attempt-terminal or boot-
reconciliation, at most one cleanup settlement, and one final activity snapshot; byte-identical replay consumes no
new ticket. The admission transaction runs with spill disabled, measures before commit, and moves its ticket directly
to WAL debt when it commits. `pin_change`, one bounded `retention_batch`, and a migration step are operator/store
mutations rather than provider-lifecycle promises. One maintenance ticket sized from current `H` is permanently
included in the allocation so retention can still make progress when ordinary admission has consumed every other
byte; retention consumes and re-establishes that ticket atomically. Pin changes and migration obtain their own exact
one-operation ticket under the serialized capacity check immediately before SQL and may refuse when no headroom
exists. Repeated pin toggles therefore cannot evade the cap, while they also do not require an unbounded reservation
at workflow admission. A total enum/match and persisted ticket identity make a new mutation kind a compile/schema
failure until its reserve ownership is specified.

On a WAL commit, the exact ticket moves atomically from `future_wal_reserve_bytes` to sticky `wal_debt_bytes`; total
charge does not fall merely because the mutation finished. The persisted debt is an upper bound tagged with the
observed WAL epoch. It may be treated as cleared only after `busy=0` plus a complete checkpoint/reset proves that
epoch has no surviving frame. No standalone “set debt to zero” write is allowed, because that write would itself
append a frame. Instead, the next serialized history mutation uses the reset proof in its preflight calculation and
atomically replaces the stale upper bound with that mutation's new ticket/epoch at commit; until then read-only
reporting may show the conservative old bound. A pinned reader or unrelated primary WAL traffic may delay reset proof
but cannot erase or reduce history debt; unrelated frames themselves remain outside the history allocation.

For `DELETE`, `TRUNCATE`, or `PERSIST`, let `S = 65,536`, bundled SQLite's hard maximum sector size, and conservatively
reserve `J(D) = (D + 1) * S + D * (page_size + 8)` for a transaction with dirty-page bound `D`: at most one padded
journal header before each page record plus one initial header, and one page-number/checksum record around each page.
The connection must have only `main` as a writable durable database, so no master journal or attached-database write
can escape that formula. Serialized writers keep the maximum applicable future `J(D)` as
`transient_journal_reserve_bytes`; the live admission transaction temporarily substitutes `J(D_admission)` if that
is larger and proves the component sum before commit. Only roster-page records are attributed to that reserve;
explicit authority/locator records remain primary-store custody. The reserve does not become debt. In rollback mode
`future_wal_reserve_bytes = wal_debt_bytes = 0`; in WAL mode `transient_journal_reserve_bytes = 0`.
`MEMORY`, `OFF`, an unknown journal mode, FULL/unknown auto-vacuum, an executable incremental-vacuum path, a writable
attached database, missing `dbstat`, or inability to restore the prior connection-local spill setting refuses
configured-history admission before provider effects.

The V2 configured invariant is component-exact:

```text
allocation.history_page_bytes = measured pgsize sum over every allocation-owned table/index root
allocation.future_wal_reserve_bytes = sum(all unconsumed WAL mutation tickets)
allocation.wal_debt_bytes = sum(consumed tickets since the last proven complete WAL reset)
allocation.maintenance_reserve_bytes = one current-H retention ticket
allocation.transient_journal_reserve_bytes = max(unconsumed rollback-journal ticket, default 0)
allocation.charged_bytes = checked sum of those five components
allocation.charged_bytes <= MAX_CHARGED_BYTES
allocation.slots_used = count(retained attempt summaries)
allocation.terminal_rows = count(terminal attempt summaries)
```

The component rows are mode-exact: a WAL allocation persists zero transient-journal reserve, while a supported
rollback-journal allocation persists zero future-WAL reserve and zero WAL debt. Any nonzero forbidden component is
corruption, not a conservative overcharge.

`workflow_history_mutation_reserve` stores the closed ticket identity, bound, and state. A ticket transition and the
history mutation it covers are one transaction. Retention cascades exact rows, then remeasures pages and recomputes
remaining tickets; it never “credits” a predicted per-attempt byte count. Migration sets `migrating`, creates and
verifies the V2 schema, materializes any required bounded reserves, derives all five components from authoritative
rows plus current journal state, and flips to `ready` in one transaction. Restart repeats safely, and any component,
ticket, roster, or count mismatch is corruption rather than a rebaseline. Legacy V1 attempt rows have no node
terminal or future-mutation ticket; they remain readable and are included in the measured table pages.

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
2. derive a checked `payload_precheck_bytes` from the exact stored string/blob lengths and bounded reserves. This is
   only an early growth screen, never a page prediction or configured-store debit;
3. gate with the existing `history_growth_fits(conn, payload_precheck_bytes)` before writing any row;
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
P4  the exact summary/attachment/node-row counts and stored reserve lengths match the admitted graph
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
4. **Retention.** Collection removes the oldest unpinned terminal summaries first and cascades their attachment and
   node rows by exact key. Platform custody remains governed by the measured whole-file gate; configured custody
   remeasures the allocation-owned roots and tickets under the component invariant above.
5. **Mixed V1/V2.** Legacy V1 attempts contribute no node rows or future-mutation tickets but remain part of measured
   history-owned table pages; V2 adds its exact placeholders and closed ticket population.

Configured stores use the measured table-local component regime above, not the platform whole-file page gate and not
a logical byte equation. Platform and configured decisions therefore share materialize-before-effect ordering while
retaining their deliberately different owner-approved custody boundaries.

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
    "requested_session_cwd": "/trusted/repo",
    "node_execution_identities": [
      {
        "node": { "sorted_ordinal": 0, "id_sha256": "..." },
        "effect": {
          "agent": "...",
          "effective_session_cwd": "/trusted/repo",
          "mcp_delivery_digest": "...",
          "effect_digest": "...",
          "secret_commitment_key_id": null
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
- V2 verifies requested-cwd absence/presence and bytes, uses each persisted effective cwd verbatim for
  `SessionSpec`, and rebinds exact MCP delivery bytes to its persisted delivery/effect digests. It never consults
  `TaskRecord.session_cwd`, entry cwd fallbacks, process current directory, or a template renderer after the bind.
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
8. Workflow-history accounting V2 is transactional, idempotent, schema-admitted, and rederives exact state from
   authoritative rows. It creates the node-terminal table as
   `WITHOUT ROWID` with exactly one `unique=1`, `origin='pk'`, `partial=0` metadata entry, exact
   `(attempt_id, node_id)` key order, and no separately rooted index; verifies that shape before flipping to `ready`;
   creates the closed mutation-ticket table; and rebuilds the accounting table whose current
   `CHECK(accounting_version=1)` constraint cannot admit version 2 in place. Configured migration remeasures the
   exact allocation-owned `dbstat` roots and derives future WAL reserve, sticky debt, and transient journal reserve;
   platform migration remains inside the hard page/journal/sidecar gate and measures P1–P5. Configured migration
   first reads `main.auto_vacuum` and refuses FULL/unknown before setting `migrating` or executing DDL; NONE and
   INCREMENTAL-with-vacuum-prohibited proceed under the rechecked transaction policy. Either rolls back before
   effects while leaving `migrating` intact on a capacity failure. No static page formula participates. Legacy rows
   receive no invented node evidence or future ticket, but their real history-owned pages remain charged.
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
14. Existing `value` MCP environment syntax remains source-compatible and is explicitly public. New
    `value_from_env` is mutually exclusive and secret-bearing. Every literal or referenced MCP environment value is
    nevertheless HMAC-committed before durable identity. A V2 working snapshot records the commitment key id;
    resume under a missing or different key refuses before effects rather than migrating the digest silently.
15. V2 snapshots add requested cwd plus each node's exact effective cwd and MCP-delivery digest. A V2 executor never
    consumes the legacy mutable `WorkflowRunContext.session_cwd`; V1 alone retains that fallback. A native-MCP
    backend is effect-keyed so cwd A and B cannot share spawn-time delivery bytes.
16. Configured-history V2 supports `auto_vacuum=NONE` and `INCREMENTAL` without incremental vacuum. FULL/unknown is
    a pre-effect unsupported configuration, not optional telemetry unavailability. Platform-ledger whole-file
    custody is unchanged; this restriction is specific to the configured root-attributed `D(R)` proof.

## 11. Compile-correct build and ownership order

### Stage 1 — serial foundation

One owner changes:

- `crates/bridge-core/src/execution_policy.rs`
- `crates/bridge-core/src/lib.rs`
- `crates/bridge-core/src/ports.rs`
- `crates/bridge-workflow/src/graph.rs`
- new `crates/bridge-workflow/src/fanout.rs`
- run-spec serialization helpers

Land the pure types, resolver, typed MCP environment sources and secret-silent debug boundary, exact constants, the
canonical terminal/trigger/controls serializer with its derived
worst-case assertions and bounded-evidence fallback, the frozen provider-effect and provider-selection digests with
their exhaustive `AgentEntry` plus freeze-context classification, the pure effective-cwd/MCP-delivery resolver, the
exhaustive `LedgerUnavailableReason` classifier, fingerprinting, and controller transition tests first. Stage 1 also
lands the additive `AgentRegistry` bind methods and their defaults in
`crates/bridge-core/src/ports.rs`, since Stage-2 and Stage-3 owners both depend on that signature. The workspace must
compile before parallel work begins.

### Stage 2 — parallel siblings from the same frozen Stage-1 base

- **Configuration owner:** `bin/a2a-bridge/src/config.rs`, `[security].provider_effect_key_file`, typed MCP
  source resolution, key-custody validation, and config-only tests.
- **Controller owner:** `crates/bridge-workflow/src/fanout.rs` and fake/manual state-machine tests, without touching executor integration.
- **Persistence owner:** `crates/bridge-core/src/task_store.rs`, `workflow_history.rs`, `orch.rs`, and
  `crates/bridge-store/src/sqlite.rs`, plus the root `Cargo.toml` solely to add rusqlite's existing `hooks` feature.
  This owner installs the configured FULL/unknown auto-vacuum refusal and `incremental_vacuum` authorizer, owns
  migration ordering, and supplies the NONE/INCREMENTAL physical-accounting fixtures. No other owner edits the
  workspace rusqlite declaration.

Each sibling owns disjoint paths and has an independently runnable test target. Configuration and controller do not
modify manifests; persistence has only the feature exception above. None modifies the roadmap, generated files,
executor integration, or serving adapters.

### Stage 3 — single integration owner

After integrating all Stage-2 siblings, one owner changes the shared seams:

- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-registry/src/registry.rs`
- `crates/bridge-coordinator/src/detached.rs`
- `crates/bridge-coordinator/src/batch.rs`
- `crates/bridge-coordinator/src/coordinator.rs`
- `crates/bridge-coordinator/src/params.rs`
- `crates/bridge-coordinator/src/session_manager.rs`
- `crates/bridge-api/src/backend.rs`
- `bin/a2a-bridge/src/main.rs`
- `crates/bridge-a2a-inbound/src/server.rs`
- `crates/bridge-mcp/src/server.rs`

This stage owns every fresh/resumed batch and non-batch entrypoint, freezing, the lease-first `bind_frozen_entry`
check-to-use path for every provider attempt, event widening, terminal ordering, the four-result trigger barrier,
resume, projections, CLI/A2A/MCP overrides, and compile fixes. It also owns removing every remaining registry read
inside an attempt, so no path can re-derive a model, effort, mode, preflight flag, fallback candidate, or any other
effect-bearing field after the bind, and converting retry invalidation to the exact-bound form. The concrete
`bind_entry_use`/`resolve_bound`/`invalidate_bound` implementations in `crates/bridge-registry/src/registry.rs` are
part of this stage because they must land with their only production callers. It owns the native-MCP effect-keyed
backend subslots plus `main.rs`/container/ACP composition changes that consume `BoundMcpDeliveryV1` without a second
cwd resolution or substitution. This stage also owns the API-local
`Unconfigured | ExplicitNone | ExplicitSome` session-model transition, so bound `None` cannot fall through to a
stale spawn default. These files must not be split among
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
  in no snapshot, history row, projection, diagnostic, debug string, or log artifact.
- **W1/W1-B effective-cwd state, red first:** submit the same immutable served workflow and, separately, fresh and
  resumed batch children against normalized cwd A then cwd B with public MCP argument and environment templates such
  as `{cwd}/tools` and `ROOT={cwd}/tools`. The reviewed design gave both attempts one effect/identity and one
  preflight-cache key while delivering different ACP bytes. V2 requires distinct MCP-delivery/effect/identity/
  workload fingerprints, a cache miss, and a pre-effect replay conflict for a reused `(attempt_id,node_id)`. Capture
  the actual `SessionSpec.cwd` and ACP `session/new` server bytes and require byte equality with the one bound
  delivery that produced the persisted MACs; there is no second substitution call. A literal with no `{cwd}` keeps
  identical delivery bytes but still changes effect identity because session cwd changed. A same-cwd request with
  a different raw spelling that normalizes to the same `SessionCwd` is the negative control and remains identical.
- Cwd precedence is table-tested for request override, `entry.session_cwd`, `entry.cwd`, and launch-cwd fallback.
  Relative static values resolve against the captured launch cwd even if process cwd changes later. Absence versus
  presence is preserved in the run spec; entries with different static fallbacks produce different per-node
  effective values. A request outside `allowed_cwd_root`, or a malformed/overflowing request or static value,
  refuses before task/history/registry/provider counters; this does not invent a general allowed-root restriction
  for unsandboxed static entry paths that current configuration admits.
- Served, `run-workflow`, fresh batch, and resumed batch source guards prove that no V2 path rewrites
  `AgentEntry.session_cwd` with request data. A configured fallback edit is provider-effect drift; a request override
  remains the separate persisted request/effective field and takes precedence without mutating registry state.
- ACP, CodexNative, and KiroNative each receive the same exact bound server bytes committed by
  `mcp_delivery_digest`. Native cwd A/B attempts resolve different backend effect keys and process identities; a
  paused A child cannot be reused for B. Exact-bound invalidation of A leaves B live. Managed-depth marker order and
  value are part of the commitment. A source-level guard rejects calls to the old substitution/render helpers below
  `bind_frozen_entry`.
- ACP V2 mint must receive `configure_bound_session(BoundSessionSpecV1)`, ignore `AcpConfig.mcp`, and serialize the
  already-rendered tuple without calling `substituted_for_managed_agent`; using legacy `configure_session` is a typed
  pre-prompt refusal, not a static-config fallback. Container Codex and host Codex consume the identical committed
  argv suffix.
  Kiro A/B deliveries derive distinct full-digest agent names and immutable JSON files; hold A's child open while B
  spawns and prove A's file bytes/name and later tool surface remain unchanged. Existing-name wrong bytes, a link,
  wrong owner/mode, interrupted create/sync, or a truncated file refuses before child spawn. Equal content reuses the
  same name without rewriting it.
- A symlink-spelled cwd fixture proves the sandbox may canonicalize a separate host source for containment/mounting
  while ACP `session/new`, container destination/cwd, and every MCP channel retain the one frozen lexical path. A
  source guard rejects post-freeze delivery calls to `canonicalize` as well as the old template renderers. This is
  text identity only and does not assert the symlink still names the same object at action time.
- MCP env parsing requires exactly one of `value`/`value_from_env`; empty names, both/neither sources, missing or
  non-Unicode referenced values, template syntax in a reference name, and any MCP-env-bearing entry without the exact
  32-byte owner-private key refuse before registry/provider effects. A public literal retains `{cwd}` substitution;
  a referenced secret is resolved once and the same bound bytes reach HMAC and ACP delivery. Secret references on
  CodexNative/KiroNative refuse; ACP child-spawn tests prove the source variable is removed from ambient env and the
  value is present only in the redacted typed MCP field.
- **W1-B oracle, red first:** persist the old deterministic digest for a literal selected from `0000..9999` while
  every other canonical field is known, and recover the value by enumeration. Under V2, repeat against both
  `value = "0042"` and `value_from_env = "TEST_MCP_SECRET"`: enumeration without the separately held key cannot
  validate a candidate, while changing either resolved value changes the keyed commitment and effect digest. The
  artifact exposes only key id, MAC-derived effect digest, and public source descriptor. Missing/replaced key or
  changed referenced value on resume refuses before checkout/configure/prompt; an unchanged key/value resumes.
  Tests prove the key reaches no child, the source variable is absent from the adapter's ambient environment, and
  the raw value reaches no projection or serialization/diagnostic surface.
- **W1's exact constructible state, red first:** freeze an API agent against endpoint A with env `X`, then hot-reload
  only `base_url` to endpoint B (and separately only `api_key_env` to `Y`) before the queued node binds. Every
  selection field stays byte-identical. The repaired path refuses with `configuration_drift{effect}` before resolve,
  checkout, configuration, and prompt, and records zero provider effects. A selection-only digest passes this reload,
  so the old identity fails the fixture red.
- **W1-A warm API model, red first:** warm one API backend whose spawn config is `model=M`, hot-reload the reused
  slot to `model=None`, bind/configure a new attempt, and assert the outgoing request contains no `M`. The old
  `Option<String>` state fails by falling back to M. Negative controls prove `M -> Some(B)` sends B and a legacy
  session that was never configured may still use the spawn default. The assertions inspect the request body and
  frozen identity together, so a fresh-slot workaround or provenance-only change cannot green the wrong call.
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
- The run preflight cache is keyed by agent plus both frozen digests. Two nodes with the exact cwd/delivery identity single-flight
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
- **W5 evidence preservation, red first:** the production proof makes natural overflow unreachable for every valid
  current-schema value, so a test-only/internal encoder limit is parameterized below 2,048 but above the measured
  mandatory shape plus one retained UTF-8 scalar. The same production algorithm must then take the fallback and fit
  the injected bound. The pre-fallback and fallback terminals are destructured without `..` and may differ only as
  follows: `evidence_overflow` changes `false -> true`; `dependency_set` changes `Some -> None` (and `None` remains
  `None`); `deepest_cause` is byte-identical or a **nonempty valid-UTF-8 deepest suffix**; and
  `cause_truncated` equals `input.cause_truncated || cause_was_shortened`. `primary`, both cleanup fields,
  `failure_class`, static `code`, prompt acceptance, degraded ancestry, trigger identity, schema version, and every
  other field are byte-identical. Any forbidden mutation, wildcard comparison, empty/non-suffix cause, or output
  above the injected bound fails the fixture; a fallback still over bound refuses before persistence.
- Overflow is indicated separately, not by class or code substitution: two distinct failure classes sharing one
  static code both overflow and remain distinguishable. Each output is compared with its own pre-fallback terminal
  under the exact allowed-difference set above; `evidence_overflow` is the only dedicated classifier, not the only
  serialized difference.
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

- Memory and SQLite round-trip frozen controls, requested cwd, every node's effective cwd and exact MCP-delivery
  digest, frozen provider effects and selections with all digests, the provider-effect key ID and per-binding
  template/delivery MACs, trigger, every node terminal, ancestry, and cleanup duration.
- Provider-effect key creation obtains 32 bytes from the OS CSPRNG, creates one owner-private file atomically, emits
  no bytes, refuses an existing file/link, and at every injected write/sync failure leaves either no destination or
  one complete valid key. Doctor refuses wrong length, wrong ownership/mode, multiple links, no-follow failure, a
  relative path, and raw/canonical containment under a repo, session/output/evidence root, SQLite artifact, or
  projected mount. Alias fixtures prove the separately held key never enters a bundle or child projection.
- Exact replay succeeds; conflicting replay refuses.
- Accounting-V2 migration is idempotent and rederives the exact configured page/reserve/debt components or platform
  physical state under boundary capacity, retention, rollback-required, concurrent admission, and crash/failpoint
  fixtures.

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
- **Configured long-ID/WAL regression, red first:** in otherwise empty 512-byte-page configured stores using
  `auto_vacuum=NONE` and, separately, `INCREMENTAL` without vacuum, materialize a 1-MiB-ID placeholder and assert
  `dbstat` charges every allocation-owned leaf/interior/overflow page plus the exact closed mutation-ticket
  population. Assert that `D(R) = 3 * R + 2` covers the roster-associated pointer-map, freelist, and header pages;
  this fails the former `R + 2` proof red without claiming FULL support. Seed the old logical allocation to
  `MAX_CHARGED_BYTES - old_proposed_charge`; the old `exact ID + 256` equation admits while measured history pages
  plus WAL reserve exceed the cap, and V2 rolls the whole admission back. Repeat with many short IDs and a reader
  pinning WAL: admission itself creates charged debt, each later committed history mutation transfers its full ticket
  to sticky debt, a busy checkpoint releases
  zero debt, and admission refuses before the component sum crosses 128 MiB. After a proven complete reset, only the
  next committed history mutation may replace the stale-epoch debt with its own ticket; a bookkeeping-only clear is
  forbidden. A near-cap store consumes its permanently reserved retention ticket and successfully removes one
  eligible attempt without exceeding the cap. Unrelated primary tables/frames neither add to measured history pages
  nor erase the debt. The same fixtures cover each supported rollback-journal mode, `cache_spill=OFF` restoration,
  and refusal for MEMORY/OFF/unknown journal mode or unavailable `dbstat`.
- **W3 FULL relocation, red first:** against the exact bundled SQLite, construct a 512-byte configured database with
  `auto_vacuum=FULL`, a small history roster, and a fragmented unrelated primary B-tree whose tail non-root interior
  page points to children/overflow pages in more than `R` pointer-map regions. In a control copy without the repaired
  gate, free a lower history page and inspect committed WAL/journal page numbers; require relocation of the tail page
  and more than `R` distinct child pointer-map writes. This is dependency-behavior evidence for why the old formula
  is false, not an acceptance path. The production opener, fresh admission, retention, and V1-to-V2 migration each
  refuse the FULL original with static code `configured_history_auto_vacuum_full_unsupported` before any schema,
  history, authority, task, session, or provider mutation, and byte/digest snapshots prove the database is unchanged.
  `NONE` and `INCREMENTAL` without vacuum are the positive controls. An attempted `PRAGMA incremental_vacuum`
  through every bridge-owned SQL/maintenance surface while V2 configured history is active is authorizer-refused
  before a page change; a source guard proves no unclassified production issuer exists. Reopen after changing the
  database to FULL while the bridge is stopped refuses again rather than relying on cached open-time state.
- **Root attribution and mixed-admission regression:** grow `attempt_identities`, `task_attempt_locators`, and their
  accepted index roots without changing history; the history charge and future tickets remain byte-identical and a
  small admissible history reservation still fits. Then admit one served attempt (authority update) and one direct
  attempt (authority insert): the history ticket is exactly `W(D(H0 + H1))` in WAL mode or temporarily reserves
  `J(D(H0 + H1))` in rollback mode, while rollback/failpoint leaves neither authority nor history half committed.
  A served optional-ledger refusal that changes only authority/locator roots creates no history ticket or debt.
  Adding a history-root index incorporates it automatically; adding a trigger on a history/explicitly co-mutated
  root, an escaping cascade, or a writable attached database refuses before effects. These fixtures distinguish
  deliberate primary-root exclusion from the old undercharge of actual history leaf/interior/overflow pages.
- **Journal-mode exclusivity:** WAL fixtures persist zero transient-journal reserve; each supported rollback mode
  persists zero future-WAL reserve and zero WAL debt. Seed each forbidden nonzero component and assert corruption,
  not automatic repair. Use page sizes at both extremes and the bundled 65,536-byte sector bound to exercise checked
  `W(D)` and `J(D)` arithmetic overflow/refusal.
- **Fixed-width accounting schema:** every V2 dynamic component/count and mutation bound is an exact eight-byte BLOB,
  kind/state are closed one-byte codes, and neither singleton table has a secondary index. Boundary values that
  cross SQLite INTEGER serial-width thresholds leave `H1` unchanged after installation; a variable-width legacy or
  malformed V2 cell refuses migration/open rather than silently growing after measurement.
- A configured mutation with an injected post-measurement page expansion above `2 * H + 2`, an unreserved mutation
  kind, a temporary/DDL/allocation-churn SQL plan, a ticket/component mismatch, or checked-arithmetic overflow rolls
  back before commit. Adding a history-owned index enters the root roster and increases the measurement; adding an
  unclassified allocation-namespace object fails schema admission. These controls make the ticket bound executable
  rather than a comment.
- Postconditions are authoritative and checked at boundaries: short IDs, exact-page-boundary IDs, single-overflow
  IDs, and multi-page IDs each satisfy P1–P5, with reserved page bytes, auto-vacuum NONE and INCREMENTAL-without-
  vacuum, explicit FULL refusal, a legacy
  WAL-to-supported-rollback transition, each supported rollback journal mode, `cache_spill=OFF`, exact hard
  `max_page_count`, live sidecars, and near-cap admission and refusal. A forced `SQLITE_FULL` at the hard page limit
  is a bounded pre-effect `capacity_protected` refusal.
- **Rollback evidence:** a refused reservation leaves no placeholder, summary row, attachment, debit, or partial
  allocation, and the allocation accounting is byte-identical to its pre-admission state. A failpoint injected
  between materialization and postcondition evaluation produces the same result.
- Replacing a full-size placeholder with a real terminal adds zero pages, never exceeds the reserve, and draws only
  on the provisioned reusable pool; `terminal_reserve` is unchanged by the replacement.
- Retention of a long-ID attempt cascades its attachment, node, and mutation-ticket rows by exact key, remeasures the
  remaining allocation-owned pages, and recomputes every outstanding ticket. It never credits a stored prediction.
- A refusal is a capacity refusal, never a `NodeId` length cap: the same graph admits on a ledger with headroom, and
  no path truncates, hashes, or rejects an ID for being long by itself.
- Mixed V1/V2 allocations measure legacy summary/attachment pages without inventing node rows or tickets; V2 adds
  exact placeholders and its closed mutation population. Migration rolls back while leaving `migrating` intact
  rather than exceeding either custody regime, and rebuilds the accounting table rather than attempting to store
  version 2 under the current `CHECK(accounting_version=1)` constraint.
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
budget authorizes implementation while this design remains parked without cumulative approval.

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

The complete likelihood, impact, fix, regression, and BLOCKER/DEFER analysis for the prior population is retained in
the linked [closure review 2 record](../reviews/2026-08-01-r2f1a-sol-closure-review-2.md). Closure review 3 then
source-validated the following residual population in this candidate:

| Residual blocker | Real-world condition, incorrect result, and bounded fix |
|---|---|
| WRONG W1-A — stale warm-API model | After an API backend is warm under model `M`, a reload removing the model reuses the slot; `configure_session(None)` cannot suppress the spawn-time default, so the provider still receives `M` while the frozen entry says no model. Plausible on supported warm reloads. Use tri-state session-model state or normatively force a fresh slot on every API model change. |
| WRONG W1-B — credential-verifier digest | A low-entropy literal MCP environment credential is included in a persisted deterministic unkeyed digest, allowing an artifact reader who knows the other fields to enumerate the credential offline. Rare but a hard credential-custody failure. Use a domain-separated HMAC with separately held stable key and fail-closed resume, or use indirect credential names plus a nonsecret rotation identity. |
| WRONG W3 — configured-store undercount | Configured shared stores charge exact key bytes plus fixed 256-byte overhead and no WAL reserve; arbitrary IDs can therefore consume more SQLite overflow/B-tree/WAL bytes than charged and cross the 128-MiB history allocation after admission. Rare near the boundary but violates hard custody. Materialize and debit measured table-local pages plus a conservative remaining-WAL reserve inside the serialized pre-effect transaction. |
| WRONG W5 — mutually unsatisfiable fault-injection criterion | The specified fallback drops `dependency_set` and may change cause fields, while its mandatory forced-overflow test permits only `evidence_overflow` to differ, so no implementation can satisfy both requirements. The production overflow remains theoretical under the current size proof; the certain impact is a failed implementation acceptance gate. State that the flag is the only dedicated classifier and permit exactly the named dependency/cause fields to differ. |

The full mechanism, source locations, likelihood, exposure, impact, fix cost, and fail-first evidence are retained in
the linked [closure review 3 record](../reviews/2026-08-01-r2f1a-sol-closure-review-3.md). The owner-authorized repair
of that closed population is:

| Residual blocker | Repaired mechanism and exact acceptance evidence |
|---|---|
| WRONG W1-A — stale warm-API model | §5 gives API sessions the tri-state `Unconfigured | ExplicitNone | ExplicitSome`. Only `Unconfigured` may use the spawn default; a bound/configured `None` suppresses it. §11 assigns `bridge-api` to the integration owner. §12 requires the former-red warm `M -> None` request-body regression plus `M -> B` and never-configured legacy controls. |
| WRONG W1-B — credential-verifier digest | §3 makes MCP env sources typed (`value` public literal or mutually exclusive `value_from_env`), resolves a referenced value once into the bound entry, and HMAC-commits **every** MCP env value under a separately held 32-byte provider-effect key before durable identity. The artifact carries only the key id, public descriptor, and MAC-derived effect digest. Missing/rotated key or changed value refuses resume before effects. The supported key creator uses the OS CSPRNG and atomic owner-private custody; imported key entropy remains an explicit operator assertion because runtime cannot infer it statistically. §12 proves the old four-digit digest is enumerable, the repaired artifact supplies no verifier without the key, delivery uses the committed bytes, and key/value bytes are absent from projections and diagnostics. No MCP-credential entropy inference is claimed. |
| WRONG W3 — configured-store undercount | §9 deletes the fixed 256-byte row overhead and measures every allocation-owned table/index page through bundled `dbstat` after exact materialization. A persisted closed ticket population reserves every remaining lifecycle mutation; WAL commits move the full reserve to sticky debt until a complete reset, and rollback journals retain one transient maximum. Root attribution keeps unrelated primary/authority pages outside the configured history allocation while accounting for every history leaf/interior/overflow page and its journal frame, including inside a mixed atomic admission. Because `dbstat` omits pointer-map/freelist/header pages, the ticket applies the proved `D(R) = 3R + 2` structural bound to the pre/post history-root union and forbids transient allocation churn. Each transaction remeasures, rebases outstanding tickets, and rolls back before commit if the component sum exceeds 128 MiB or page growth exceeds the ticket proof. §12 covers 512-byte/1-MiB ID, auto-vacuum structure, many-short-ID, pinned-reader, root isolation, index/trigger classification, unsupported journal modes, arithmetic, migration, retention, and failpoints without capping or hashing `NodeId`. |
| WRONG W5 — impossible injected comparison | §3 and §12 now say `evidence_overflow` is the only dedicated classifier, not the only changed field. A test-only smaller internal limit forces the otherwise unreachable fallback and permits exactly flag `false -> true`, dependency `Some -> None`, a deepest nonempty UTF-8 suffix, and monotonic `cause_truncated`; every other field is byte-identical and explicitly destructured. Forbidden changes or a still-over-bound output fail before persistence. |

The newly declared cap permits deterministic documentation gates followed by exactly one fresh cumulative Sol/xhigh
closure review of the clean repair commit. It permits no second repair/review loop. Until that review approves, no
implementation, Rust test result, release, deployment, or live operator effect is authorized or claimed; a rejection
parks this document again.

Closure review 4 marked W1-A, W2, W4, W5, and W6 `FIXED`, retained W1/W1-B and W3 as `PARTIAL`, and rejected the
following exact residual population:

| Residual blocker | Real-world condition, impact, and bounded proposed fix |
|---|---|
| WRONG W1/W1-B — effective request cwd and delivered MCP bytes are not committed | A normal served or batch workflow runs the same entry against cwd A versus B with `ROOT={cwd}/tools`; the frozen configured literal and provider digest are identical, but ACP receives different session/MCP bytes. Likelihood is common for cross-repository use and the impact is wrong tool/repository provenance under falsely equal identity. Freeze the resolved effective cwd in the run spec and execution identity, use it for both session mint and substitution, and commit the exact public delivery bytes (or the template plus that exact frozen resolution input) into cache/replay/resume identity. Add served plus fresh/resumed batch A/B fail-first controls and a no-template negative control. |
| WRONG W3 — FULL auto-vacuum escapes `D(R)=3R+2` | Rare but constructible boundary state: retention frees a history page while FULL auto-vacuum relocates an unrelated fragmented tail interior page; SQLite rewrites pointer-map entries for its distributed children/overflow pointers, which is not bounded by history-root union `R`. Near 128 MiB this can underreserve WAL/journal custody. Fail closed on `auto_vacuum=FULL` for configured-history admission/migration; permit `NONE`, and `INCREMENTAL` only while vacuum operations are prohibited or separately ticketed. Retaining FULL support requires a new relocation proof or exact dirty-page instrumentation. |

The complete likelihood, exposure, impact, repair cost, and fail-first evidence is retained in the linked closure-4
record. The owner then opened one further closed-population round. This revision repairs exactly those two items:

| Closure-4 blocker | Repaired mechanism and exact acceptance evidence |
|---|---|
| WRONG W1/W1-B — effective request cwd and delivered MCP bytes were not committed | §§3/5 add normalized `requested_session_cwd` to `WorkflowRunSpecV1` and resolved `effective_session_cwd` plus `mcp_delivery_digest` to every provider effect/identity. One closed resolver applies request → entry session cwd → entry cwd → captured launch cwd. The same bound effect supplies non-optional `SessionSpec.cwd`, exact post-substitution ACP/native MCP bytes, redaction, cache, replay, and resume through additive `BoundSessionSpecV1`; V2 never uses static ACP templates. ACP conversion does no second substitution; process-start native backends are keyed by complete effect, container children consume the per-session bound argv, and Kiro uses immutable content-addressed config names so cwd A cannot overwrite or reuse B's delivery. §12's served and fresh/resumed batch A/B controls require distinct identities/cache entries, committed-to-delivered byte equality, pre-effect replay conflict, same-normalized-cwd equality, no transient entry stamping, symlink-spelling stability, all three delivery channels, and a no-template delivery negative control. |
| WRONG W3 — FULL auto-vacuum escaped `D(R)=3R+2` | §9 limits the configured root-attributed proof to `NONE` and `INCREMENTAL` without vacuum. The serialized connection rechecks mode after `BEGIN IMMEDIATE` and before the first mutation; migration checks before DDL. FULL/unknown is a typed unsupported-configuration refusal outside optional-ledger fail-open, and an authorizer plus exhaustive source guard prohibits bridge-owned incremental vacuum while V2 is active. §12 retains the exact bundled-SQLite fragmented FULL fixture as fail-first proof of the old formula, then requires open/admission/retention/migration to refuse byte-clean before authority/task/provider effects, with NONE/INCREMENTAL positives and an incremental-vacuum negative. |

This repair remains design-only and has not run a Rust behavior test. Its pre-freeze deterministic gates passed:
`git diff --check`, direct existence checks for every changed-document target, and
`cargo run -p a2a-bridge -- validate --repo-hygiene` (**39 tracked artifacts / 7 validated example configs**).
That command reused the scratch clone's dev build; it is not provider or Rust behavior evidence. Closure review 5
accepted the two intended local mechanisms but found the post-bind worktree transformation above. Its complete
likelihood, exposure, impact, bounded corrections, cost, and fail-first matrix are retained in the linked review.
The round cap is exhausted and permits no repair, review replay/fallback, implementation, compatibility/live case,
release, deployment, or operator mutation. This exact artifact and evidence are parked pending separate owner
direction.

R2F1A FOCUSED BOUNDARY: PARKED / SOL CLOSURE REVIEW 5 REJECT / CAP EXHAUSTED / IMPLEMENTATION UNAUTHORIZED
