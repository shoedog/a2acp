# R2f — Phase-aware liveness and safe takeover plan

- **Status:** DESIGN APPROVED by clean-room Sol/xhigh closure review 5; D1-D11 settled; R2f0a **MERGED** by
  [PR #48](https://github.com/shoedog/a2acp/pull/48) at merge
  `2685ffb78ef21c987b3f63f7aba1ddc096b01189`, final PR head
  `630b9cc9d7ae86c323b183763b3d4e83bdbfc792`, after integrated native verification and independent concurrent
  Sol/xhigh and Fable/xhigh `APPROVE` reviews. PR Build/Lint/Coverage, macOS store, Windows unsupported-target,
  and CLA checks are green. R2f0b is **IN REVIEW** with its implementation candidate complete;
  deterministic/native verification and independent review remain pending.
- **Prerequisite:** R2b structured diagnostics merged; may proceed independently of R2c–R2e afterward
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Focused owner design:** [`../specs/2026-07-20-r2f-owner-design.md`](../specs/2026-07-20-r2f-owner-design.md)
- **R2f0b focused boundary:** [`2026-07-30-r2f0b-focused-boundary.md`](2026-07-30-r2f0b-focused-boundary.md)
- **Queued process-deployment increment:** [`2026-07-20-r2g-stable-ingress.md`](2026-07-20-r2g-stable-ingress.md)
- **Short-bound validation spike:**
  [`../spikes/2026-07-20-r2f-short-bound-validation.md`](../spikes/2026-07-20-r2f-short-bound-validation.md)
- **Adversarial review 1:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-review-1.md`](../reviews/2026-07-20-r2f-owner-design-sol-review-1.md)
- **Closure review 1:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md)
- **Failed closure re-review 2:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md)
- **Closure review 3:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-3.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-3.md)
- **Closure review 4:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-4.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-4.md)
- **Closure review 5 — APPROVE:**
  [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-5.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-5.md)
- **Final cumulative Sol/xhigh review — APPROVE:**
  [`../reviews/2026-07-24-r2f0a-final-cumulative-sol-review.md`](../reviews/2026-07-24-r2f0a-final-cumulative-sol-review.md)
- **Final cumulative Fable/xhigh review — APPROVE:**
  [`../reviews/2026-07-24-r2f0a-final-cumulative-fable-review.md`](../reviews/2026-07-24-r2f0a-final-cumulative-fable-review.md)
- **Final native macOS verification:**
  [`../reviews/2026-07-24-r2f0a-native-verification.md`](../reviews/2026-07-24-r2f0a-native-verification.md)
- **Integrated final Sol/xhigh review — APPROVE:**
  [`../reviews/2026-07-25-r2f0a-integrated-final-sol-review.md`](../reviews/2026-07-25-r2f0a-integrated-final-sol-review.md)
- **Integrated final Fable/xhigh review — APPROVE:**
  [`../reviews/2026-07-25-r2f0a-integrated-final-fable-review.md`](../reviews/2026-07-25-r2f0a-integrated-final-fable-review.md)
- **Integrated final native macOS verification:**
  [`../reviews/2026-07-25-r2f0a-integrated-native-verification.md`](../reviews/2026-07-25-r2f0a-integrated-native-verification.md)
- **Incident ids:** `INC-VERIFY-STALL-2026-07-11`, `INC-SHARED-WARM-CRASH-2026-07-16`,
  `INC-SHARED-SESSION-CAPACITY-2026-07-17`, `INC-SHARED-RESTART-RECOVERY-2026-07-19`,
  `INC-UNARY-NULL-FINAL-2026-07-20`,
  [GitHub #22](https://github.com/shoedog/a2acp/issues/22),
  [GitHub #24](https://github.com/shoedog/a2acp/issues/24), and
  [GitHub #47](https://github.com/shoedog/a2acp/issues/47)

## Incident evidence and limits

The operator reported a Luna run in `~/code/stockTrading` with **2h54m total elapsed time**. Useful file
edits completed in about the first **25 minutes**; the last file edit was at **17:22**. The run then made
no observed editing progress for nearly three hours while parked in verification. The operator killed
only that run's process tree, took over the remaining verification, and found the retained work clean.

This is an operator report, not a bridge reproduction. The current record does not prove whether the
stall was in the provider, ACP adapter, agent runtime, verification command, child-process wait, or
orchestration waiter. File modification time alone is not a safe liveness signal: a legitimate long test
can make no edits, while a wedged verifier can keep a process alive. R2f must collect the observations that
separate those alternatives before changing timeout behavior.

## Shared-operator evidence and limits

The long-lived production operator has repeatedly returned immediate Codex `AgentCrashed` before observable prompt
start while its bridge, ACP adapter, and Codex app-server processes remained alive. Fresh isolated one-shot
operators completed the same package/model/effort/mode and review shape. The old app server had observed 15
distinct session thread ids and no close notifications; bridge release removed its local session entry
without sending a capability-gated ACP close, while codex-acp retained sessions until close. On 2026-07-19 the same
boundary recurred against R3d2 exact `3e4508a`: no task/session/turn row or prompt/usage evidence was created, the
roughly two-day-old warm process tree remained alive, and the same release binary completed the review through one
fresh one-shot bridge without touching the production generation.

The incident stream later recurred against operator release `983398427c9f0486`: card/catalog and Codex
doctor/provenance checks were healthy and there were zero unfinished tasks and zero durable sessions, yet two
explicit unary raw-`gpt-5.6-sol`/xhigh/read-only submits failed before task, turn-log, prompt-start, or usage
creation. The operator reports that stopping and restarting the served bridge ultimately restored the affected
path, while one controlled exact unary reproduction after an earlier restart still failed pre-prompt. That makes
pre/post-restart process, transport, ACP-child, and session state required evidence; it does not establish a
session-count threshold, poisoned transport, or restart as the root cause or durable remedy.

This rules out a general package/model/auth/cwd incompatibility for those incidents, but does not distinguish
a capacity ceiling/session leak from a poisoned long-lived transport. Fifteen is evidence, not a threshold.
The earlier isolated comparison stopped no running turn, warm session, backend, image, or production operator and
replayed no failed request. The later stop/start was an independent operator recovery action, not an R3d gate.
R2f owns this investigation and every lifecycle remedy. R3d only records that its fresh one-shot executions did
not evaluate shared-operator health.

The second R2f owner-design closure attempt exposed a separate accepted-work finalization failure on 2026-07-20.
The exact served bridge and read-only ACP/app-server PIDs stayed alive. The prompt is present in the Codex journal,
four assistant commentary messages prove active review work, and turn `019f8223-4447-7922-b967-71c4db938ce3`
emitted `task_complete` after 185.355 seconds with `last_agent_message: null`. The unary client nevertheless returned
`AgentCrashed`, and no task/turn row was written. The attempt produced no final review and was not retried. Preserve
this as `INC-UNARY-NULL-FINAL-2026-07-20`: accepted-work, progress, final-message, and transport disposition must be
separate evidence, and null-final completion must not be mislabeled as proved process death. The durable evidence is
the [failed review-attempt report](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md).

[GitHub #47](https://github.com/shoedog/a2acp/issues/47) records the served-unary companion failure: a Codex
prompt failed after one hour, the server logged terminal `AgentCrashed`, and the separate `submit` client remained
silent and sleeping until the operator interrupted that client. The incident predates R2f0a, so its missing durable
execution/attempt locator is addressed by the merged ledger boundary, but no post-merge reproduction proves the
terminal-delivery, deepest-cause, progress, metric, or bounded-silence criteria fixed. Those remain explicit R2f0b,
R2f1b, and R2f4 acceptance work; the earlier successful turn on the same route also prevents treating the incident as
proof that Codex or the model was categorically unavailable.

## Workflow-node wedge evidence and current-main disposition

Issues #22 and #24 are related symptoms with different proved boundaries. They must not be collapsed into one
provider claim:

- **#22 is confirmed at the workflow scheduler boundary.** A failed fan-out root does not cancel or bound an
  already-running silent sibling. The executor drains `FuturesUnordered` until every in-flight node returns; only
  an externally canceled workflow token stops new scheduling. Local `run-workflow` therefore cannot reach its
  terminal event or output artifact while that sibling remains pending.
- **#24 is partially confirmed.** When ACP delivers a structured provider-limit error, the current adapter retains
  its typed class and bounded hints and a one-node workflow can terminalize. A silent ACP stream remains unbounded
  when the agent has no configured watchdog, while the existing opt-in watchdog bounds the same fake-backend
  silence. This constructs the reported bridge symptom without proving whether the original quota-limited Kiro
  adapter retried silently, lost a terminal frame, or received no terminal provider result.
- **Failure visibility and recovery remain incomplete on both paths.** The executor builds a node marker from the
  error's debug projection, but the local CLI discards `NodeFinished.output` and prints only `node <id> failed`.
  Its generated `run_id` is neither printed nor persisted. `run-workflow --serve` now mints a durable task id, but
  the client prints only task state and still exposes no takeover artifact; that is partial identification, not
  closure of the local/offline incident.

### 2026-07-20 deterministic revalidation

The revalidation used exact `origin/main` `0d628271a910168230491e8610a31f92f7063cbc` and no provider, quota, or
billable turn. The hypothesis was that changes since the earlier `7efd689` reproduction had not altered the
scheduler or disabled-watchdog seams. The falsifier was a failure-triggered sibling bound/cancel, a terminal event,
or a now-exposed local attempt/recovery record. The competing hypothesis was that newer serve/task work had closed
the observability half even if offline execution remained exposed.

Results:

- a temporary two-root characterization passed **1/0**: both roots started, the failing root finished false, the
  next event remained pending past 150 ms, sibling `cancel()` remained exactly zero, and no terminal event arrived;
  the temporary test was removed after recording the result so it does not bless the defect;
- `no_watchdog_config_behaves_identically` passed **1/0** and retained a silent in-flight turn with no cancel;
- `watchdog_cancels_a_hung_turn_as_timed_out` passed **1/0** and bounded the configured control;
- `observed_prompt_structured_provider_limit_preserves_bounded_hints` passed **1/0**, so a delivered structured
  limit error is not currently erased at the ACP boundary;
- the runtime executor diff from `7efd689` to `0d628271` contains no failure-triggered sibling policy, and the
  checked-in workflow config still has no per-agent watchdog.

The primary hypothesis is supported. The alternative is only partly supported: serve-side task persistence exists,
but the caller still receives neither a printed identifier nor a takeover locator. GitHub #22 was subsequently
closed as an intake record, but its failed-root/silent-sibling acceptance behavior remains unfinished and owned by
R2f. GitHub #24 remains open; no current evidence attributes its original incident specifically to Kiro ACP or its
provider.

### Closure contracts for #22, #24, and #47

- Before implementation, choose an explicit per-workflow failure policy for fan-out roots: immediate peer cancel,
  bounded grace/drain, or continued independent work under a phase-aware hard bound. Preserve intentional graceful
  degradation where a failed leg may still feed synthesis; do not silently turn every node failure into fail-fast.
- The behavior recorded by closed intake #22 is complete only when a failed root plus nonterminating sibling
  reaches a bounded terminal state, reports every
  sibling's final/cleanup state, retains the failed node's deepest bounded sanitized cause, and exposes a usable
  attempt/recovery identifier. A silent but healthy negative control must not be terminated merely for being quiet.
- #24 closes only after the generic silent-stream and deepest-cause mechanisms are fixed and the original
  Kiro-specific alternative has an evidence-backed disposition. That disposition may use a captured deterministic
  protocol replay or a separately authorized live quota-limited turn; never manufacture quota exhaustion or spend a
  provider turn merely to make the issue closable.
- #47 closes only when a deterministic served-unary regression proves the server's terminal prompt failure reaches
  `submit` within a bounded delivery interval, the client exits nonzero, the deepest sanitized cause and failed
  outcome remain visible, and provider-execution timeout stays distinguishable from client/result-delivery stall.
- Offline and served execution must converge on the same identifier, phase/progress, completed/pending-node, and
  takeover-artifact contract. A durable id that the invoking operator cannot discover is insufficient.

## R2f0a — Execution/attempt identity, run ledger, and stats

**Integrated closure checkpoint:** R2f0a implementation, correction, native verification, and final cumulative
reviews are complete at exact integrated code checkpoint
`7b01ab4bae167d3640050dfda5de7e1478728497` on `agent/r2f0a-identity-ledger`, tree
`7d0b14aa1d39ca36fdc68a9ad69df4fc8442e64e`. This supersedes the historical folded checkpoint
`9761b3b78c89cca079ddb1d9376514fceb77e0df` and approved candidate
`d7f20d37a9fda493c0b8dc18339489bfe1a059a3` / tree `1803a888cf77fdee378367404179cc9ba4085ee6`; the
[July 24 native](../reviews/2026-07-24-r2f0a-native-verification.md),
[Sol/xhigh](../reviews/2026-07-24-r2f0a-final-cumulative-sol-review.md), and
[Fable/xhigh](../reviews/2026-07-24-r2f0a-final-cumulative-fable-review.md) artifacts remain historical predecessor
evidence.

The integrated four-commit correction stack preserves its approved provenance:

- `4a6fcb90` imported approved API/handoff candidate `0cb10903`;
- `f145535a` imported approved recovery candidate `7b8fa376`;
- `4359dc9c` folded approved test candidates `6d34edcb`, `0b77ed87`, and `04b5792e`;
- `7b01ab4b` folded approved lineage/Platform/test candidates `a1481ed`, `dea817be`, and `24fd4b8a`.

The [integrated native macOS verification](../reviews/2026-07-25-r2f0a-integrated-native-verification.md)
(source SHA-256 `a67e1362217a3263b09a42b9e86136cd3cd8a1e044f921538eef5fc2fe91203d`) records passing fmt, locked
all-target/all-feature check, warnings-denied Clippy, debug build, release build, exact alias regression, repository
hygiene, and final diff/clean checks. The complete workspace emitted **73** result groups with **2,785 passed / 0
failed / 12 ignored / 0 measured / 0 filtered**. The 12 ignored tests are repository-declared live/external-provider
or multi-bridge cases; no command-line skips were used.

Fail-first truth is preserved: the first final native attempt reached **2,541 passed / 1 failed / 12 ignored** before
a test-only `/var` versus `/private/var` canonical-path expectation failed. The six-line test-only correction
canonicalized the expected path; its exact regression passed **1 / 0 / 0** with **211 filtered**, and the full suite
then passed. This was a test-oracle correction, not a production defect.

Independent concurrent fresh exact-head reviews also closed green. The
[integrated Sol/xhigh review](../reviews/2026-07-25-r2f0a-integrated-final-sol-review.md) (source SHA-256
`8f9cc3efa961492915ef59bf4563682cfb57caa76a53662813b8bc0f87da037d`) adjudicated all seven current
mechanisms and seven inherited families `RESOLVED`, with zero `WRONG`, zero `SMELL`, and `APPROVE`. The
[integrated Fable/xhigh review](../reviews/2026-07-25-r2f0a-integrated-final-fable-review.md) (source SHA-256
`623faf2ea4170b014c3b8f027cd555b387bf5fb0bb4f7aa0056c8d9304a1d6e0`) reported zero `WRONG`, one
nonblocking `SMELL`, and `APPROVE`. Its new follow-up is to add a legacy one-method `RouteTarget::Workflow` arm to
the existing fail-closed route coverage and document the compatibility delta for hypothetical third-party one-method
routers. Shipping `SkillRoute` uses the explicit pre-default hook, and no incorrect production behavior was
demonstrated. The three earlier nonblocking Fable follow-ups remain: root-only foreign-owner CI coverage;
foreign-owner coverage for both selection wrappers; and any foreign-owned rollback-journal policy change only through
a separate owner decision. None expands or blocks R2f0a.

No ignored live/provider test was forced. The locked-egress Linux verifier could not fetch one missing `a2a-lf`
dependency for the final six-line macOS test-only correction, so that attempt is not Linux proof. PR #48 later
merged the integrated work at `2685ffb78ef21c987b3f63f7aba1ddc096b01189`; its final head
`630b9cc9d7ae86c323b183763b3d4e83bdbfc792` passed Build/Lint/Coverage, macOS store, Windows unsupported-target,
and CLA checks. The merge does not by itself prove a release, deployment, live canary, production-server update, or
post-merge operator build. R2f0b is now in progress from its frozen focused boundary, with IN REVIEW — implementation candidate complete; deterministic/native verification and review pending; R2f overall, the behavior retained from closed intake #22, open #24/#47, R2g, and R4 remain incomplete.

- Mint and expose distinct `execution_id` and `attempt_id` before registry/session/provider effects. `execution_id`
  remains stable across served resume and operator takeover; every resume/takeover gets a new attempt id, ordinal,
  parent link, and monotonic clock. Print the offline ids before execution, print the served task/execution locator
  before SSE, return them through MCP, and retain them on transport loss.
- For direct unary, require the caller to mint validated high-entropy execution/attempt ids and carry both in the
  request. `submit` prints them before network I/O. Missing, invalid, or colliding ids refuse before effects; the
  server never substitutes `task-1`, silently reuses an attempt, or treats a duplicate locator as prompt replay.
- Give direct served unary submissions a mandatory bounded safety row in the already selected ledger before any
  registry/session/provider/prompt effect, even when they do not use a workflow task envelope. Initial ledger open
  or core-reservation failure returns typed pre-effect `durable_evidence_unavailable` and sends no prompt. Once the
  core row exists, optional summary enrichment may fail open without erasing accepted-work/progress/terminal
  evidence or the caller-visible recovery ids.
- Before first effect, select exactly one configured-store or platform-state ledger and reserve one bounded attempt
  slot plus conservative byte/WAL charge. Every no-store surface, including served/in-memory task execution, selects
  the platform ledger. Enforce 180 days, 100,000 terminal rows, and 128 MiB for workflow-history allocation.
  Protected capacity or permission/lock/migration/corruption/I/O/open failure produces a bounded
  `telemetry_unavailable` reason in status/terminal output while an otherwise admissible workflow proceeds; it never
  falls through to a second ledger, changes primary outcome, or exceeds the cap. Direct unary is the explicit
  exception above: its minimal safety reservation is admission-critical, while only later enrichment is fail-open.
- Commit primary task terminality first and independently for workflow/task surfaces. For direct unary, terminalize
  the mandatory core row's producer/final/process fields as its primary durable state. Add optional bounded
  enrichment only afterward. Boot marks a surviving nonterminal reservation interrupted before creating a resume
  attempt. Store failure cannot roll back primary terminal state or double count reconstructed metrics.
- Record workflow/task class, execution surface, policy version, workload fingerprint, start/completion clocks,
  work/end-to-end/cancel/cleanup/finalization durations, outcome, degraded/prompt/cleanup state, phase totals, node
  disposition counts, and completeness flags. Do not retain prompt/output/process command text or use ids as metric
  labels.
- Export multi-hour buckets and a read-only report with count, min, mean, median, p90/p95/p99, max, partitions, and
  excluded populations. Calibration consumes only healthy non-degraded successes; applying a recommendation remains
  a separate reviewed policy edit.

## R2f0b — Meaningful progress, terminal evidence, and recorder

**Status:** IN PROGRESS — the [focused implementation boundary](2026-07-30-r2f0b-focused-boundary.md) is frozen
from exact main `1a8cfc0`; IN REVIEW — implementation candidate complete; deterministic/native verification and review pending.

- Add an append-only activity/progress/absolute-clock recorder with bounded low-cardinality phase/reason codes and no
  timeout behavior change. Capture phase transitions, agent updates, tool start/end, owned-child spawn/exit and
  bounded output, file digest change, verification gate start/exit, and completed-gate-set growth. Empty/duplicate
  events are activity at most, never progress.
- Use fake monotonic clocks and distinguish provider/adapter, tool, verification, waiter, cleanup, and terminal-store
  phases. Wall time identifies records/retention only.
- Record producer terminal event, final assistant message presence, and exact process-liveness observation as
  independent fields. `task_complete` plus a null/absent final message becomes typed `protocol_incomplete_final`,
  never `AgentCrashed`, success, or pre-prompt refusal; accepted prompt state stays sticky and no retry is authorized.
- Implement the owner design's negotiated `a2a_bridge.turn_evidence.v1` ACP extension. Advertise support explicitly,
  carry opaque attempt correlation in prompt `_meta`, and emit one exact generation/session/adapter-turn/attempt-bound
  `a2a_bridge/turn_evidence` envelope before the prompt RPC resolves or rejects. For codex-acp, producer disposition
  comes from native Codex turn terminal evidence; final `nonempty` comes only from a same-turn nonempty assistant item
  tagged `phase=final_answer` (or an equivalent native terminal field), and `absent` requires authoritative producer
  completion plus ordered notification drain. Commentary, message id, stop reason, generic error, and process
  liveness never synthesize either fact.
- Preserve duplicate-identical envelopes idempotently. Unsupported, advertised-but-missing, malformed, late,
  mismatched, and duplicate-conflicting evidence produces bounded `protocol_terminal_evidence_*` or
  `protocol_terminal_unknown`, leaves producer/final unknown where necessary, retains independent process state and
  sticky acceptance, and never authorizes success, `AgentCrashed`, or retry. The Codex incident lane cannot close
  until the selected adapter advertises the version and passes conformance against its real mapping.
- Reproduce #47's served-unary terminal-delivery split with a deterministic server/client boundary: after the server
  records a terminal prompt failure, `submit` must surface the deepest bounded sanitized cause and exit nonzero
  within the approved delivery bound. Distinguish provider execution timeout from result-delivery/client-stream
  stall, and retain the execution/attempt locator, phase, progress, and failed outcome throughout.
- Reproduce blocked child, exited-child/wedged waiter, silent healthy verification, failed fan-out plus silent sibling,
  delivered provider limit, silent provider stream, and active non-tool model updates. Preserve the
  hypothesis/probe/result log and do not assign the historical incident a provider/adapter root cause.

## R2f1a — Profile validation, fan-out policy, and per-node control

- Check in `legacy_bounded_v1` and `review_high_xhigh_v1` exactly as D4.1 defines them: 30-minute queue cap,
  30-minute no-progress snapshots, 31-second pre-dispatch control bound, two-hour work cutoff, cancellation observable
  by six seconds inside 60-second cleanup, and terminal observable by 2:01:10. Explicit unknown profiles/classes fail
  before effects; true omission alone maps to legacy/`other`; Max requires reason and larger finite work cutoff.
- Validate one frozen `bounded_independent`, `fail_fast`, or `fixed_grace` policy before prompt. Give every running
  node its own cancellation source while retaining one workflow-wide source. `fixed_grace`/`fail_fast` use their
  separately recorded failure trigger; only `bounded_independent` promises a failed sibling never shortens clocks.
- Preserve deepest bounded causes, structured per-node terminal/cleanup state, and `completed_degraded` synthesis.
  Exercise manual/fake policy triggers only in this slice; no real automatic deadline ships yet.

## R2f1b — Worktree preservation, warning, deadline, and cleanup ownership

- **Prerequisite before enabling any deadline:** add result-bearing `preserve_after_cancel`. Under the sweep/run-end
  coordination boundary and before cancellation/process effects, parent-sync a protective `preservation_prepared`
  intent, atomically replace it with a durable identity-bound preserved-worktree claim, then transfer the volatile
  lease. Run-end and boot sweeps exclude either durable state even for matching run id/free flock; corrupt or
  ambiguous evidence fails safe without deletion. Resume atomically exchanges the claim for a live lease; only exact
  retain/archive/delete disposition may release it. Never call provider remove, `git worktree remove --force`, reset,
  clean, checkout, or delete during cancellation/takeover.
- Retain each `OwnedProcessTree`-equivalent capability from spawn in exactly one resource flight: generation-scoped
  for a multiplexed ACP/shared container, exact-resource-scoped for a proved dedicated child/container. Per-node
  session/worktree flights reference it. Close generation admission and journal intent before signaling; automatic
  cleanup, manual takeover, release escalation, and retirement that request a resource action join it and publish one
  result to every collateral owner; ordinary session-only close remains on its per-session flight.
  Missing/ambiguous capability refuses or returns partial; numeric PID/name artifacts never recreate authority.
- A 30-minute no-progress crossing snapshots evidence but does not cancel. Before the two-hour work cutoff, only
  mechanical orphan/impossibility may auto-cancel; silence plus absent observed progress is insufficient. At two
  hours, request cancel, publish initiating turn disposition by six seconds or transfer its exact owner, settle or
  type partial/unknown cleanup by 60 seconds, and publish primary terminal/reporting by 2:01:10.
- Report every collateral session on a shared-process escalation. Complete the behavior retained from #22 only
  when a failed root plus nonterminating
  sibling reaches bounded terminal state with all node/cleanup dispositions and preserved worktree state.
- This merge boundary cannot turn timers on unless preservation, retained-capability single-flight, unrelated-process
  survival, and worktree-diff survival tests are already green.

## R2f2 — Local scoped takeover artifact and resume

- Add only local OS-owner CLI authority for manual takeover and explicit generation retirement. Remote controllers
  retain status/recovery reads and ordinary cancellation; no new destructive remote scope exists.
- Select exact execution/attempt/node/generation and join its retained generation/resource process flight. Stop enumerated children
  before the anchored root, record every disposition, and return typed refusal/partial rather than using a late
  PID/PGID/name signal or claiming success with survivors.
- Emit an owner-private bounded artifact with provenance, last progress, phase, capability identity, termination and
  collateral result, preserved worktree diff/hash, completed/pending gates, and exact recovery locator. No credential,
  unbounded output, or arbitrary command line is retained.
- Operator-selected resume reuses the recovery-owned worktree from the first unfinished gate under a new attempt id;
  it never replays a possibly accepted prompt or starts a provider automatically. Explicit final disposition alone
  may remove the preserved worktree.

## R2f3a — ACP close, durable debt, and capacity

- Separate `forget`, `close_session`, and `retire_generation`. Final cold release no longer treats config-only forget
  as remote session cleanup. Negotiate close from the exact initialized generation capability.
- Implement the durable `(generation_id, session_id)` state machine from owner design §6.1: write-before-effect
  prepared/dispatched boundaries, one idempotency key and serialized flight, capacity claim, acknowledged,
  unsupported, retry-due, exhausted, and generation-exit resolution.
- Retry only definitely-not-accepted or contract-idempotent effects. A recovered `close_prepared` has crossed no
  dispatch barrier, so it dispatches still-unspent ordinal 0 at the first one-minute safe-recovery bound. Only after a
  recorded ordinal-0 dispatch failure do retry ordinals 1/2/3 run after 1/5/30 minutes from the preceding safe failure.
  Unsafe/ambiguous dispatch never auto-replays. Boot reconstructs wall timestamps with rollback-safe monotonic delays;
  concurrent release, retry, drain, and retirement join one flight; generation exit resolves only its exact claims.
- Source session capacity from a truthful finite adapter capability and/or checked-in
  `session_capacity_limit`, using the lower when both exist. Durably reserve a creation-attempt claim before
  `session/new`, bind it to the accepted session id, release it on definitely-not-accepted creation, and retain
  accepted-or-unknown/no-id as generation-owned `creation_unknown` without replay. Bound claims release only on close
  ack or exact generation exit. Unknown capacity reports exact claims but never fabricates fullness, automatic
  replacement, or repair; a known-full generation refuses only a new cold session before provider effect.
- Cover close capability present/absent, ack/failure, crash before/after dispatch, unspent-ordinal recovery,
  duplicate/concurrent release, safe/unsafe retry, initial-attempt-plus-third-retry exhaustion,
  advertised/configured/minimum/unknown capacity, create-acceptance boundaries, known-full refusal, boot recovery,
  and generation exit.

## R2f3b — Backend health axes and non-disruptive generation drain

- Implement `ephemeral`, actionless `observe`, and production-only `enforce` with separate state roots/namespaces.
  Promotion creates a fresh production generation and never imports development strikes.
- Store and serialize the D8 ownership lifecycle (`active`, `draining`, `dead`, `retired`) independently from health
  (`healthy`, `suspect`, `auth_required`, `degraded_external`, `isolated_unknown`, `quarantined`, `probation`). Enforce
  routing precedence, illegal transitions, persisted evidence, successor relation, and rollback-safe cooldowns.
- Exclude auth/config/model/provider-limit/quota/cancel from strikes. Two distinct ambiguous pre-dispatch failures in
  15 minutes permit one prompt-free same-config successor comparison. Shared external failure self-clears through
  bounded control; instance-local differential evidence quarantines; inconclusive remains isolated unknown.
- Default selection favors a healthy active successor. Probation requires exact local authorization and one turn;
  quarantine blocks warm turns. Mechanical exact-generation exit alone is dead. No provider prompt is retried.
- Planned drain routes new sessions to the successor while running/warm ownership remains predecessor-affine until
  TTL/clear/release. Drain age only warns. `urgent_security` freezes future warm turns after the current turn. Retire
  only after close/debt/process settlement; indefinite ownership stays visible.

## R2f3c — Bridge-process handoff contract

- Expose stable release/process identity, readiness distinct from accepting work, lifecycle/health/drain state,
  task/execution/attempt/context/session/generation affinity, exact running/warm/producer/debt counts, and bounded
  refusal/recovery locators required by R2g.
- Refuse missing/conflicting affinity without replay or guessing. Terminal drain cannot become true while any
  process-local or debt ownership remains.
- Do not start another served binary, share the exclusively locked SQLite store, proxy traffic, or claim
  non-disruptive binary replacement. Stable ingress, cross-process store/SSE ownership, promotion, and rollback remain
  the dedicated [`R2g plan`](2026-07-20-r2g-stable-ingress.md) immediately after R2f.

## R2f4 — Provider-free matrix, dogfood, and closure

- Run exhaustive fake-clock/profile/fan-out/state-transition/debt/crash/cleanup tests, including wall rollback,
  duplicate wake, healthy silence, active output, orphaned waiter, exact PID reuse defense, two-node/one-generation
  escalation, collateral result fan-out, run-end/boot/corrupt-claim worktree survival, telemetry capacity and
  open/permission/lock/migration/corruption refusal, and primary-terminal/store-failure ordering.
- Prove no automatic retry/fallback/second billable attempt; offline/served/resume/transport-loss/cancel/takeover all
  expose both ids and a recovery locator; degraded and fail-fast paths retain every node state/deepest cause.
- Reproduce `INC-UNARY-NULL-FINAL-2026-07-20` provider-free: accepted work plus progress plus producer
  `task_complete`/null final yields `protocol_incomplete_final`, a durable direct-unary attempt/turn record, live
  process disposition, no fabricated final text, and no retry.
- Prove the adapter evidence boundary separately: negotiated capability and exact correlation; commentary-only then
  null-final; genuine per-turn failure after commentary; nonempty final-answer; unsupported, missing, malformed,
  late, duplicate-identical, duplicate-conflicting, reordered, and transport-loss cases. A fake that injects fields
  proves the state machine only; Codex closure additionally requires captured adapter conformance or separately
  authorized live evidence from the actual selected lane.
- Prove direct unary prints its caller-minted ids before network I/O and that missing/invalid/colliding ids plus
  configured/platform-ledger initial-open and core-reservation failures all refuse before registry/session/provider/
  prompt effects; optional post-reservation enrichment failure preserves the core record and primary outcome.
- Run the provider-free matrix before any live gate. Then separately authorize one disposable verifier wedge and
  targeted takeover. A provider-specific #24 disposition requires captured protocol evidence or separate live
  authorization; do not manufacture quota exhaustion.
- Run fresh Sol/xhigh adversarial implementation/full-branch review. Use a hard/complex second lens only if the
  primary review identifies a qualifying unresolved problem. Report full serial workspace totals and every live/
  production/deployment path not exercised.

## Completion

R2f is complete only after the verification stall, the behavior retained from closed intake #22, open #24/#47,
and shared-operator alternatives have evidence-backed
dispositions; the phase-aware watchdog distinguishes the negative controls; the selected fan-out failure policy is
bounded without breaking intentional graceful degradation; scoped termination preserves useful work; session/close
and generation ownership are capability- and concurrency-safe; non-disruptive rotation preserves running turns and
warm sessions; takeover is exercised end to end; and a fresh adversarial review approves the safety boundary. Until
then the operator runbook permits evidence capture and targeted manual termination only; it must not claim automatic
recovery, session-capacity repair, provider-specific root cause, or safe rotation.
