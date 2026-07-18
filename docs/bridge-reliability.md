# Bridge reliability program

- **Status:** active P0 as of 2026-07-11
- **Compatibility baseline:** [`compatibility.md`](compatibility.md)
- **Execution and handoff cursor:**
  [`reliability-execution-roadmap.md`](reliability-execution-roadmap.md)
- **Operator workflow:** [`../skills/a2a-bridge-operator/SKILL.md`](../skills/a2a-bridge-operator/SKILL.md)

## Objective

An upstream package, protocol crate, embedded CLI, model, credential, or container change must either
keep a supported bridge path green or fail in a named phase with enough evidence to identify the
boundary. A feature developer should not have to rediscover executable provenance, adapter versions,
authentication behavior, or host/container differences during unrelated work.

## Reliability contract

For each advertised supported agent path, the project owns this chain:

```text
bridge release
  -> executable provenance
  -> ACP adapter and protocol version
  -> embedded/external agent CLI
  -> authentication mode
  -> initialize and advertised capabilities
  -> session/new
  -> model/effort/mode application
  -> prompt stream
  -> terminal result
  -> host/container parity
```

Each failure must retain the last completed phase, the failed phase, and the underlying error chain.
`AgentCrashed` remains the public category, not the complete diagnostic.

## Execution mode and degradation policy

Container health and content trust are separate decisions. Follow ADR-0032:

| Work class | Normal mode | Container degradation |
|---|---|---|
| Trusted own-repo read-only review/design | Tier 0/1 host is first-class; Tier 2 container is opt-in defense-in-depth | Operator may explicitly rerun through an eligible host entry after confirming trust. No silent downgrade. |
| Third-party or otherwise untrusted read-only work | Tier 2 container required | Fail closed; no host fallback. |
| Any write-capable `implement` work | Tier 3 quarantine container required, even for own repos | Fail closed; no host-write fallback under the current ADR. |

Fallback must be based on a classified container-infrastructure failure, not generic `AgentCrashed`,
model rejection, authentication failure, or prompt failure. Never automatically replay a prompt that
may have been accepted; the retry could duplicate cost or side effects. The current bridge has explicit
host and container entries but no automatic fallback policy. R2's first fallback surface is a local,
non-billable operator plan/recommendation only; caller-supplied A2A metadata cannot assert trust or start
a host attempt. In-process fallback is deferred until policy can bind a non-forgeable trust attestation
to authenticated caller context. The local planner accepts only complete smoke-v2 evidence bound to the
current config bytes, derives host scope from a plan-time identity snapshot of the read-only source mount
rather than artifact cwd, and emits a guarded distinct fixed-PONG smoke that the operator must invoke
separately. The later smoke refuses if that source-mount object changed. The planner performs no external
post-failure runtime probe and never executes the emitted plan.

`doctor` remains a read-only configuration/metadata check and is not a new-container startability proof.
Production container spawn separately observes the exact generated name inside the handshake deadline. A
runtime-confirmed pre-start object fails in `Spawn` as `container.runtime.start_timeout` /
`ContainerRuntime` with `ContainerFallbackCandidate` and a false replay barrier; started or unknown state
cannot be relabeled as that failure. Bridge-owned production spawn arms one cancellation-safe cleanup owner
immediately after process creation; success transfers it to the backend, ordinary error joins exact-client
termination then exact named-container removal, and one independent joined thread/runtime owns that order
through pre-publication cancellation or shutdown during ordinary-error settlement.
Public legacy reap callbacks retain their detached fire-and-forget contract. This active boundary
runs only after an operator-selected container path is actually spawned; it does not make doctor mutating or
billable.

Provider capacity is a separate axis. For trusted own-repo full-branch reviews, use Fable xhigh only when
its usage window has headroom; when Claude is known to be near its usage limit, select the explicit
`gpt-5.6-sol` reviewer at `xhigh` before starting. Reserve max for the tightly connected correctness and
concurrency cases in the operator skill, or after High/xhigh fails to resolve the issue. If a Fable turn
already reached prompt start, a Sol review
is a new operator-selected attempt, not an automatic retry: preserve the first attempt as possibly
accepted and record both costs/provenance. A structured provider-limit/reset signal may recommend that
choice but never executes it. Tier 2/3 rules still apply independently.

Claude Haiku is available only as a low-cost dogfood lane for small, tightly specified Anthropic-model or
Claude Code compatibility checks. It is not a substitute for a broad implementation or for
Sonnet/Opus/Fable/Sol-caliber review.

## Work slices

### R0 — front door and compatibility baseline (this documentation slice)

- Add one docs index and put the operator skill at its top.
- Add a checked-in compatibility matrix with `PASS`, `FAIL`, `UNKNOWN`, and `STALE` states.
- Record the corrected Codex container root cause and the still-open Fable incident separately.
- Make the roadmap explicitly pause M4 after Slice 3a.

Exit: a new agent can find the correct runbook and current incident status without searching handoffs.

### R1 — isolate and disposition Fable (**complete 2026-07-11**)

- Reproduce the same minimal prompt through host `claude-agent-acp` 0.44.0 and 0.55.0.
- Run a non-Fable Claude control through each adapter.
- Replay the same sequence through a direct ACP harness and through the bridge.
- Repeat the known combination in the reader image.
- Capture frames and the deepest adapter error at the prompt boundary.
- Fix the narrow failing boundary, pin a last-known-good adapter, or explicitly mark Fable unsupported.

Exit: Fable has a reproducible root cause and a tested disposition; no environment is inferred from
another environment's result.

Disposition: both 0.44.0 and 0.55.0 pass matched Fable/Sonnet host ACP and bridge controls when run
outside the managed no-egress sandbox. Reader-image 0.55.0 passes with isolated credentials plus the
pinned minimal Fable settings mount. The historical failure was DNS-disabled execution, not adapter
drift; see the [R1 evidence](superpowers/2026-07-11-fable-r1-disposition.md). `doctor` now reports a
missing Fable opt-in and missing reader settings prerequisite before a paid turn.

### R2 — provenance and phase-specific diagnostics

- **R2a complete 2026-07-11:** read-only `doctor --json` now reports resolved
  executable/package/CLI/image/auth/model provenance as additive four-key rows. It remains non-billable
  and does not start an agent or container.
- Add an explicit, billable `smoke` path for one agent/model/effort with a bounded minimal prompt.
- Emit structured phase transitions for spawn, initialize, auth, session creation, config application,
  prompt start, stream, and finish.
- Preserve adapter stderr/error sources through the bridge error mapping and task journal.
- Distinguish structured provider usage-window/rate-limit failures from overload and unknown transport
  loss; preserve a structured reset/retry hint without automatically sleeping, replaying, or rerouting.
- Include whether authentication was skipped through `pre_authenticated` or applied through an
  advertised method.
- Classify container infrastructure failures separately from agent/model/prompt failures.
- Design an explicit, default-off trusted-host fallback policy with a named host target, audit event,
  and no replay after prompt acceptance. R2d emits only a local operator plan; authenticated in-process
  execution/auditing is a separately gated R2e. Do not weaken Tier 2/3 fail-closed behavior.

Exit: an operator can tell which boundary failed from one bounded run and its JSON artifact.

Deferred R2f follow-up: phase-aware liveness and safe takeover after useful edits, plus shared-operator
session/backend health. The verification trigger is `INC-VERIFY-STALL-2026-07-11`, where a Luna run
completed edits quickly and then parked in verification for nearly three hours. The shared-service triggers
are `INC-SHARED-WARM-CRASH-2026-07-16` and `INC-SHARED-SESSION-CAPACITY-2026-07-17`, where the long-lived
operator failed before observable prompt start while a fresh one-shot control succeeded. R2f must distinguish
a silent healthy verifier from provider/adapter/child/waiter failure and distinguish poisoned transport from
session-capacity debt before adding action. It owns capability-gated close and non-disruptive generation
drain/rotation that preserves running turns and warm sessions; see the [R2f
plan](superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md).

### R3 — compatibility harness and canaries

- Check in a versioned smoke manifest covering each supported agent, model class, auth mode, and host
  or container path.
- Record the complete resolved package/runtime manifest and immutable image digest for container runs;
  a top-level npm pin does not freeze ranged transitive dependencies.
- Maintain two lanes:
  - **pinned:** exact production package/image versions; release-blocking;
  - **floating-current:** newly resolved packages/models; advisory until promoted deliberately.
- After the manifest/runner, pinned/floating, and owner-bound scheduling core is merged, add OpenRouter
  and OpenCode as independent explicit provider increments before R4. Credentials remain environment-only;
  each integration must pass local fake/corpus gates before a separately authorized live smoke, and
  neither becomes an automatic fallback target.
- Run only characterized, fixed minimal cases under a narrow private provider-effect standing grant, using the lowest-cost
  eligible model for a provider-generic adapter/capability probe. A model/effort/mode/alias/capability-
  specific PR must exercise the exact affected claimed-support identity; a cheap substitute cannot green
  that gate. New provider-minimal probes live in a separate non-support scheduled-case registry through
  characterization; an R3d-only strict scheduled-execution source permits only the exact registry-bound
  model variance without weakening R3c's support-baseline resolver. They remain advisory unless R4 promotes
  them deliberately. Scheduling uses fresh one-shot processes and never manipulates the long-lived operator.
- Require a trusted local affected-case canary on the exact current GitHub test-merge result SHA
  (`merge_commit_sha` / `refs/pull/<n>/merge`) before merging a compatibility-impacting same-repo PR. GitHub
  performs deterministic coordination/status only and never directly invokes credentialed work. Only an
  executed pass or locally proved no-impact state can satisfy the test-merge compatibility conclusion; due-
  but-not-run remains blocking. Every characterization/reuse/consumption key binds one canonical exact test-
  merge/base/head, manifest/case/pin, candidate, requested/expected-effective provider/model/config/prompt,
  environment, and budget fingerprint; observed effective identity must match before reuse. The required
  context is never posted to the PR head, so a regenerated test-merge SHA cannot fall back to old head
  evidence. A published success is deliberately valid for the lifetime of that immutable test-merge SHA;
  R3d does not claim GitHub can recheck local freshness or authority at the later merge click. Retain a
  coalesced main run as the integration backstop. Terminal publication updates one persisted, App-owned
  `in_progress` check run through a durable outbox; crash recovery observes that exact remote id before any
  same-object update and never replays provider work or creates a second terminal check. A first transient
  failure remains `in_progress`; only a typed immutable failure, the authorized confirmation's pass, or its
  second identical complete failure terminalizes that same check. For multiple due cases, success waits for
  every pass while any terminal case failure fails fast without spending unstarted confirmations. Repeated
  non-waste unknowns only update audit and notification-dedup state; repetition alone never suppresses them.
- Defer representative review scheduling until a separate owner-reviewed design defines a fixed Tier-2
  read-only fixture, prompt, scoring, caps, provider rotation, and evidence boundary. Review content is not
  compatibility or promotion evidence.
- Persist the unchanged strict compatibility aggregate plus a separate versioned scheduling sidecar in
  bounded private hot storage and the explicitly owner-authorized private iCloud cold tier. Case/class/pin
  retention uses explicit extending precedence; cold files receive no-follow, single-link, owner-only
  protections, local scan/seal before the first iCloud-visible byte, independent storage consent, and upload/
  offload-aware revalidation before hot eviction and during retention. Raw evidence is never published to
  GitHub. Fresh floating observations may reuse content-addressed bytes only when exact immutable identity is
  unchanged.
- Keep automatic safety holds, exact-fingerprint waste suppression, and explicit operator quarantine
  distinct. `candidate_fail` alone is not permanent: one typed transient confirmation is allowed before
  suppression. Admission separately deduplicates equivalent work across R3d-aware trigger sources. Retained
  pre-R3d compatibility commands do not participate in that lease and are prohibited while a provider-effect
  standing grant is active; exact process detection holds and conservatively reconciles observed legacy attempts without
  touching the legacy production `serve` process. Every future scheduled compatibility case is
  characterized before eligibility, and no skipped or unknown case is normalized into green.

Exit: upstream drift is found by the canary rather than an unrelated feature branch.

### R4 — dependency and release policy

- Group updates by compatibility boundary: ACP Rust SDK, each ACP adapter, each agent CLI, container
  base/toolchain, and A2A SDK. Do not mix unrelated floating upgrades.
- Make production images reproducible by recording or locking the full dependency resolution; do not
  treat a top-level adapter version or a `latest` download URL as a complete pin.
- Intake upstream changes weekly; promote pins in a deliberate compatibility window, normally monthly,
  with urgent patch releases for broken supported paths.
- Require the full unit/corpus/workspace suite plus host and container live smokes before promotion.
- Build and re-run smokes from the release artifact and published image candidate.
- Update the compatibility matrix and changelog in the same PR as the pin.
- Keep the previous working binary/image/package pins and a documented rollback command.

Exit: a release cannot claim an agent path that was not tested from its distributable artifact.

## Current execution queue

Volatile slice status, review evidence, sequencing, and the next action live only in the
[reliability execution roadmap](reliability-execution-roadmap.md). This behavior overview does not copy
changing candidate hashes or gate totals. Keep provider/model fallback operator-selected and separately
recorded; it never enters the container-degradation eligibility gate.

## Guardrails

- Raw advertised capability wins; aliases are compatibility shorthands, not a substitute for probing.
- Production images stay pinned. Floating results never rewrite a pin automatically.
- Direct CLI success does not prove ACP success; ACP success does not prove bridge success; host success
  does not prove container success.
- A live prompt consumes credentials, quota, time, and sometimes money. Make it explicit and bounded.
- Do not call a model unavailable until direct and adapter controls separate access from transport.
- Do not call an adapter incompatible until the exact package/version and executable path are known.
- Protocol/crate upgrades require captured-wire or corpus coverage plus a real live gate.

## Program exit gates

Bridge reliability can stop being the sole P0 when:

1. Fable is fixed or explicitly unsupported with current evidence. **Satisfied by R1 on 2026-07-11.**
2. Every supported agent has a dated pinned host/container result or an explicit environment non-goal.
3. Minimal phase-specific smokes run on schedule and on compatibility-changing PRs.
4. Failures retain their phase and original cause.
5. Releases consume the compatibility matrix and re-test distributable artifacts.
6. Last-known-good pins and rollback instructions are exercised.
