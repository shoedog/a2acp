# R2f focused owner design — bounded liveness, telemetry, and non-disruptive ownership (v1)

- **Status:** DESIGN APPROVED by clean-room Sol/xhigh closure review 5; D1-D11 settled; R2f0a implementation,
  correction, native verification, and final cumulative reviews complete at exact integrated code checkpoint
  `7b01ab4bae167d3640050dfda5de7e1478728497` on `agent/r2f0a-identity-ledger`, tree
  `7d0b14aa1d39ca36fdc68a9ad69df4fc8442e64e`; integrated native evidence is green and independent concurrent
  Sol/xhigh and Fable/xhigh exact-head reviews both returned `APPROVE`; after this docs-only fold receives its own
  verification/review, ready for PR/CI and merge; R2f0b is next only after merge and is not started
- **Base:** `345941db91a7d898884bfe79e573433484ccafcc`
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Execution plan:** [`../plans/2026-07-11-r2f-phase-aware-liveness.md`](../plans/2026-07-11-r2f-phase-aware-liveness.md)
- **Review 1:** [`../reviews/2026-07-20-r2f-owner-design-sol-review-1.md`](../reviews/2026-07-20-r2f-owner-design-sol-review-1.md)
- **Closure review 1:** [`../reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md)
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
- **Incidents:** `INC-VERIFY-STALL-2026-07-11`, `INC-SHARED-WARM-CRASH-2026-07-16`,
  `INC-SHARED-SESSION-CAPACITY-2026-07-17`, `INC-SHARED-RESTART-RECOVERY-2026-07-19`,
  `INC-UNARY-NULL-FINAL-2026-07-20`, GitHub #22, and GitHub #24

This document records owner policy and does not claim that current `main` can detect a
stagnant workflow, persist workflow durations, close ACP sessions, take over a process tree, or rotate a live
backend generation safely. The implementation plan remains the slice breakdown; this document is the normative
decision surface now that every owner item is settled. Review 1 and closure reviews 1, 3, and 4 are folded; closure
review 5 approved the corrected design. R2f0a implementation, correction, native verification, and final cumulative
reviews are complete at exact integrated checkpoint `7b01ab4bae167d3640050dfda5de7e1478728497`, tree
`7d0b14aa1d39ca36fdc68a9ad69df4fc8442e64e`. The integrated native evidence is green and both independent
exact-head reviews returned `APPROVE`; after this docs-only fold receives its own verification/review, PR/CI and
merge remain pending. The failed null-final attempt remains incident
evidence only, and R2f0b is not started.

## 1. Current-main facts

The design is constrained by these verified production seams:

1. `WorkflowExecutor::run_from_with_context_inner` owns one `FuturesUnordered` pool and clones one workflow-wide
   `CancellationToken` into every node. A failed node is inserted into `done`; already-running siblings continue,
   and downstream nodes may consume its failure marker. The executor waits on `inflight.next().await` until every
   sibling returns. Dropping those futures is unsafe because `run_node` owns cancel/session cleanup.
2. Workflow configuration contains nodes, retry policy, and panel weights, but no task class, fan-out failure
   policy, progress policy, cleanup bound, or workflow budget. Existing example `timeout_secs` values are container
   process settings, not measured workflow policy.
3. Offline `run-workflow` invents a hidden `cli-<pid>-<nanos>` run id, discards failed `NodeFinished.output`, and
   persists no workflow envelope. Served execution has a durable task id, but the serve client does not print its
   minted id and boot resume changes the internal run id to `<task>-resume-<attempt>`.
4. `ObsEvent` declares task and node lifecycle variants. Production paths emit node/turn events, but not task
   lifecycle events. `TurnLogObserver` and `PrometheusObserver` ignore task/node lifecycle in any case. The durable
   schema therefore has exact turn latency but no completed-workflow duration row.
5. The current Prometheus turn-duration histogram ends at 300 seconds. Its sum/count can produce a turn mean, but
   its buckets cannot estimate long-review percentiles, and Prometheus does not retain an exact minimum or maximum.
6. The production operator store inspected on 2026-07-20 retained 12 completed turns from 2026-07-17 through
   2026-07-19 and no workflow task envelopes. Nine successful Sol/xhigh turns ranged from 5.11 to 24.90 minutes
   with a 15.53-minute mean. This small turn sample neither includes nor disproves the operator's healthy workflows
   over 30 minutes and cannot calibrate a workflow deadline.
7. Cold workflow attempts normally call `forget_session_observed` after their final attempt. For `AcpBackend`,
   forget removes config/turn metadata but deliberately leaves the live agent session map. Retry cleanup may call
   release, but successful and final-failure cold paths do not.
8. ACP initialize capability projection already records `session_capabilities.close`. The pinned
   `agent-client-protocol` 1.0.1 dependency exposes stable `CloseSessionRequest`, but `AcpBackend` release currently
   cancels and removes bridge-local session state without sending `session/close`.
9. Registry slots already provide side-by-side object identity and lease draining, but no stable generation id or
   health state is exposed through `Resolved`. Ordinary replacement force-retires after a 30-second grace even if a
   warm lease remains; retry invalidation uses a one-hour backstop. `SessionManager` rejects a retired lease rather
   than permitting an existing warm context to finish a planned drain.
10. ACP cancel escalation can terminate the entire shared adapter process when one session does not settle within
    cancel grace. A claimed node-level cancel can therefore affect other sessions on the same backend generation.
    R2f must report that ownership/collateral boundary rather than promise isolation it does not have.
11. On 2026-07-20, the second Sol/xhigh closure attempt proved a distinct accepted-work terminal gap. The exact
    read-only ACP/app-server generation remained alive; the prompt appeared in its Codex journal; four assistant
    commentary messages and continued inspection followed; then Codex emitted `task_complete` after 185.355 seconds
    with `last_agent_message: null`. The unary client returned `AgentCrashed`, and the configured bridge store gained
    no task/turn row. This is not a review verdict and was not retried. R2f must preserve accepted-work/finalization
    evidence and distinguish null-final completion from process death rather than collapsing both into one error.
12. The bridge's current ACP update vocabulary exposes undifferentiated text, permission, usage, and a terminal stop
    reason. It discards ACP agent-message ids and `_meta`. The operator codex-acp 1.1.2 internally observes native
    Codex `turn/completed` and emits assistant response chunks tagged with `codex.phase`, but it does not project an
    authoritative producer-terminal/final-presence envelope through ACP; its successful prompt metadata contains
    quota only. Commentary followed by null-final producer completion and commentary followed by a genuine per-turn
    error are therefore indistinguishable at the current bridge seam even when the shared process stays live.

## 2. Global invariants

1. No workflow mode is unbounded. Queue admission, execution, node work, retry/backoff, cancellation, cleanup, and
   artifact finalization have explicit finite ownership.
2. Silence, elapsed time, process existence, CPU use, and file modification time are evidence, not proof of a
   wedge. A quiet live child may be healthy model reasoning or a silent test.
3. Under `bounded_independent`, a failed fan-out leg does not shorten or reset a sibling's frozen clocks.
   `fail_fast` and `fixed_grace` are pre-prompt frozen policy exceptions with their own failure-triggered cancel
   condition; they never rewrite the sibling's recorded node deadline.
4. A timeout or takeover never silently starts a retry, fallback provider, replacement reviewer, or other billable
   attempt. Prompt acceptance uncertainty remains sticky.
5. Workflow termination never drops a node future while it is the only cleanup owner. Before the executor stops
   awaiting a node, cleanup ownership must transfer to a separately joined or independently durable owner.
6. Process action is bound to execution id, attempt id, node id, backend generation, process group, PID start
   identity, and managed-container identity where present. Name-only kills are forbidden.
7. A shared-process escalation reports every sibling session potentially affected. It cannot be labeled targeted
   merely because the initiating cancellation named one node.
8. Worktree contents survive cancellation and takeover. R2f never resets, cleans, checks out over, or deletes the
   operator's useful repository state.
9. Wall clocks identify records and retention age. Monotonic clocks drive warning, stagnation, and execution
   deadlines. A restart creates a new attempt rather than pretending monotonic continuity.
10. High-cardinality execution, attempt, task, session, and generation ids never become Prometheus labels.
11. Telemetry failure cannot rewrite the primary workflow outcome or trigger cancellation. It is surfaced as a
    bounded explicit observability failure and must not be silently called complete evidence.
12. Backend/session health never authorizes a provider prompt. A fresh compatibility or review turn remains a new,
    separately selected attempt.
13. Producer completion, final-message presence, and process liveness are separate evidence. Missing final output
    cannot be relabeled as process death, success, or pre-prompt rejection.
14. Direct unary work cannot cross registry, session, provider, or prompt effects until caller-minted
    execution/attempt ids are printed or otherwise returned to that caller and one mandatory minimal durable safety
    record is reserved. If identity validation, uniqueness, ledger open, or reservation fails, direct unary refuses
    before effects. Optional summary enrichment remains fail-open after that reservation.
15. Producer terminality and final-message presence come only from an exact-turn, negotiated, versioned adapter
    evidence contract. Commentary/text, message ids alone, ACP stop reason, a generic SDK/transport result, and
    process liveness cannot fabricate either fact. Unsupported, missing, malformed, late, or conflicting evidence is
    recorded explicitly as unknown/incomplete, preserves accepted-prompt uncertainty, and never authorizes success,
    `AgentCrashed`, or retry.

## 3. Approved owner decisions

### D1 — fan-out failure policy

Every workflow declares one bounded policy; omission uses a bounded compatibility profile, never today's implicit
unbounded drain.

`bounded_independent` is the built-in review/design default:

- a failed root is recorded immediately with its deepest bounded sanitized cause;
- already-running siblings continue while they remain within their own phase and absolute budgets;
- the failure neither resets nor shortens sibling clocks;
- configured synthesis may consume completed outputs and typed failure markers;
- workflow completion retains every node's terminal and cleanup disposition;
- successful synthesis after a failed/timed-out input is `completed_degraded`, not an ordinary healthy success;
- a workflow absolute deadline cancels remaining nodes, transfers/joins cleanup ownership, then reaches a bounded
  terminal report even when cleanup must be recorded as partial or unknown.

Two explicit alternatives remain available:

- `fail_fast`: a required node failure requests cancellation of every still-running sibling, schedules no new
  downstream node, and performs bounded cleanup;
- `fixed_grace`: already-running siblings receive one configured non-renewable grace interval after the triggering
  failure, then receive cancellation and bounded cleanup.

`fail_fast` and `fixed_grace` are selected and frozen before the first provider prompt. Their failure-triggered
cancellation is an explicit exception to invariant 3's `bounded_independent` clock-preservation rule. The original
node deadline remains recorded for diagnosis; the separately named policy condition explains the earlier cancel.

There is no `continue_forever` or absent-timeout policy. Partial synthesis is independent of failure timing: a
strict workflow may require all inputs under any timing mode, while a review workflow may deliberately synthesize a
degraded result.

### D2 — three liveness clocks

Each attempt tracks three distinct clocks:

1. **Activity:** the last bridge-visible event, including a bounded protocol frame, heartbeat, repeated status, or
   process observation. Activity helps diagnosis but does not necessarily extend a progress threshold.
2. **Meaningful progress:** the last phase-specific state advance. Examples include a lifecycle phase transition,
   nonempty agent/thought delta, increasing usage, a tool state transition, owned-child spawn/exit, bounded new
   child output, file-state digest change, verification gate start/exit, or completed-gate-set growth. Empty,
   duplicate, or non-advancing events do not count.
3. **Absolute execution:** monotonic time since this attempt entered execution. Activity and progress never reset
   this clock.

The state vocabulary distinguishes:

- `active_progressing`: meaningful progress remains inside the phase threshold;
- `quiet_live_child`: no recent progress, but a directly owned child or adapter connection is still live without a
  mechanically proved orphan;
- `stagnant_suspected`: the phase threshold elapsed and a warning snapshot exists, but automatic early
  termination is not justified;
- `orphaned_waiter`: the owned child exited/closed and the bridge waiter remains pending, or another deterministic
  ownership contradiction proves that no producer can complete it;
- `absolute_deadline`: the frozen workflow work cutoff elapsed and bounded cancellation/cleanup must begin;
- `protocol_incomplete_final`: accepted work emitted a producer terminal event without a nonempty final assistant
  response; prompt acceptance remains sticky, exact progress is retained, process liveness is reported separately,
  and no retry/fallback is authorized;
- `cleanup_pending`, `cleanup_partial`, and `cleanup_complete`: post-cancel ownership results.

Crossing a stagnation threshold snapshots evidence. Before the absolute deadline, automatic cancellation is
permitted only for a mechanically proved orphan or another deterministic impossible-to-complete ownership state.
`quiet_live_child` and `stagnant_suspected` remain visible and may be taken over manually, but are not killed merely
for silence. The absolute deadline always bounds them eventually.

### D3 — workflow budget construction

Budget is frozen and persisted before the first provider prompt. Precedence is:

1. an explicit workflow/task invocation profile;
2. a checked-in workflow task-class profile;
3. the bounded compatibility profile.

Invalid or internally unbounded policy fails validation before provider/session effects. Telemetry never changes a
running budget and never silently edits a future default.

Queue/admission has its own cap. It does not consume work time. R2f distinguishes the **work cutoff** from the
**observable terminal envelope**. The work budget covers the longest DAG path, not the sum of parallel legs:

```text
work cutoff >= max critical-path node work budgets
              + retry backoff on that path

observable terminal bound = work cutoff
                          + bounded cancellation/cleanup tail
                          + terminal persistence/reporting tail
```

The sum of all node/provider budgets remains a separate cost/admission concern. Parallel elapsed-time budgeting
uses the maximum; cost budgeting uses the sum. Reaching the work cutoff starts cancellation; it does not claim the
workflow is already terminal. Cleanup and terminal reporting consume only their named tail bounds and never extend
the frozen work cutoff.

### D4 — provisional review profile and calibration

The owner-approved provisional `high`/`xhigh` review profile is:

- workflow execution warning/snapshot: **30 minutes**;
- workflow work cutoff: **2 hours**;
- observable terminal bound after execution begins: **2 hours, 1 minute, 10 seconds** (the two-hour work cutoff,
  then at most 60 seconds of cleanup and 10 seconds of terminal persistence/reporting);
- queue/admission: **30 minutes** in the checked-in compatibility profiles;
- phase warnings: may occur earlier and do not extend the two-hour work cutoff or its 70-second terminal tail.

A workflow containing a `max` node must supply both a qualifying reason and an explicitly larger finite work
cutoff. Max inherits neither two hours nor an unbounded fallback. This matches the operator routing rule that Max
is reserved for tightly connected hard problems such as concurrency failures, deadlocks, data races, critical
correctness proofs, rare production failures, or a problem xhigh could not resolve.

These values are provisional owner policy, not a conclusion from the incomplete current metrics. Later changes are
manual, reviewed, versioned policy revisions.

Calibration uses completed, healthy, non-degraded successes partitioned by task class, workflow, policy version,
execution surface, and workload/config fingerprint. The operator report includes sample count, minimum, mean,
median, p90, p95, p99, and maximum. Failed, canceled, takeover, stagnation, deadline, and degraded runs remain
visible in separate populations; they never inflate the healthy baseline. Mean alone is never a budget input.

#### D4.1 — checked-in compatibility profiles

Policy omission is deterministic rather than an implementation choice:

| Profile | Selection | Queue cap | Phase/control bounds | Work and terminal bounds |
|---|---|---:|---|---|
| `legacy_bounded_v1` | Existing/custom workflow with no explicit task class/profile | 30 minutes | one pre-dispatch ACP control operation: 31 seconds observable; no-progress snapshot in provider/tool/verification work: 30 minutes; silence alone has no pre-cutoff kill threshold | 2-hour work cutoff; cancel is observable by 6 seconds inside the 60-second cleanup tail; terminal observable by 2:01:10 |
| `review_high_xhigh_v1` | Built-in review, spec/plan review, design, or explicit high/xhigh review class | 30 minutes | same provisional control and snapshot values | same provisional 2-hour work cutoff and 2:01:10 terminal bound |
| explicit non-Max | A validated checked-in or invocation profile | finite value required | every enabled phase has a finite control/snapshot value; a smaller command/provider bound wins | finite work cutoff plus the fixed cleanup/reporting tails |
| explicit Max | Any workflow containing a Max node | finite value required | 30-minute snapshots remain unless an explicit smaller value is supplied | qualifying reason and work cutoff greater than two hours are required; terminal bound is that cutoff plus 70 seconds |

An explicitly named unknown profile or task class fails validation before registry/session/provider effects. Only true
omission maps to `legacy_bounded_v1`, whose metrics task class is normalized to `other`. Retry count and backoff must
fit the remaining frozen work budget; existing per-command or watchdog limits may shorten a phase but never extend
the profile. These are provisional compatibility values and change only through a reviewed policy revision.

### D5 — takeover authority

R2f uses a split authority boundary:

- automatic cancellation is limited to the frozen workflow work cutoff or a mechanically proved orphan or other
  deterministic impossible-to-complete ownership state allowed by D2;
- manual process-tree takeover and explicit backend-generation retirement are local OS-owner CLI operations only;
- remote controllers may inspect status and recovery artifacts and may use the existing ordinary task-cancellation
  contract, but R2f adds no remote process-tree takeover or generation-retirement authority;
- suspected stagnation never triggers destructive action by itself, and no cancellation or takeover authorizes a
  replacement provider attempt, retry, or fallback.

The local command still requires exact attempt/process/generation identity, records collateral and partial cleanup,
and preserves the worktree. Authenticated remote destructive operations are deferred to a separate policy design
covering caller ownership, delegation, replay protection, idempotency, audit, collateral confirmation, and
revocation.

Process authority is a spawn-time capability, not a late PID lookup. The spawn registry places each
`OwnedProcessTree`-equivalent handle in exactly one resource-action flight. A multiplexed ACP process and any shared
container use a generation-scoped flight; a provably dedicated child/container uses an exact resource-scoped flight.
The retained capability contains the unreaped group leader/child handle, process-group identity, immutable start
evidence, backend generation, and exact managed-container identity. Retaining the leader prevents PID/PGID reuse
until the owner settles it.

Each per-node flight owns only its session/worktree state and references the resource flight; two node cells can
never independently signal the same generation resource. Before a shared-resource signal, the generation transition
cell closes admission and durably journals the initiating node, exact capability, and current collateral-owner set.
Every automatic cleanup, manual takeover, session-release escalation, or generation-retirement path that requests a
resource-level action then joins the same resource flight; an ordinary session-only close remains on its per-session
flight and cannot signal the process. The resource flight records each child/root/container disposition once and
publishes the single process result to every referencing node/session, including owners discovered before settlement
completes. All signaling occurs through the retained capability, with children settled before the anchored root where
the platform permits enumeration. An artifact containing only numeric PID/PGID/start data cannot recreate authority.
Missing/closed capability or identity ambiguity returns typed refusal/partial cleanup and never falls back to
process-name or late numeric signaling.

### D6 — workflow telemetry storage and retention

Each workflow or direct-unary attempt has at most one authoritative ledger selection, frozen before the first
effect:

- when a configured durable `[store]` exists, the attempt summary uses that store's workflow-history allocation and
  shares stable task/execution ids without making primary task terminalization atomic with optional summary
  enrichment;
- otherwise, every surface—including offline execution and served execution using an in-memory primary task
  store—uses an owner-private platform-state SQLite ledger, with the macOS default at
  `~/Library/Application Support/a2a-bridge/workflow-history.sqlite` and the equivalent platform state directory on
  other systems;
- the same attempt is not dual-written to both ledgers; reports may aggregate explicitly selected ledgers and
  retain their source identity without treating duplicate attempt ids as independent samples.

Selection never falls through after an open/reservation failure: a configured store remains the selected ledger,
and an absent configured store selects the platform ledger even if that path is unavailable. Permission, lock,
migration/schema, corruption, I/O, and protected-capacity failures map to bounded reason codes under
`telemetry_unavailable`; raw database text is not projected. If the primary execution surface itself cannot admit or
start a task—for example, its required configured task store cannot open—that ordinary pre-effect refusal wins and
no workflow attempt starts. For workflow/offline/task surfaces, optional summary-ledger failure never blocks an
otherwise admissible primary execution.

Direct unary is the deliberate safety exception required by invariant 14 and §4.2. It uses the same frozen ledger
selection, not a fallback or second ledger, but must reserve its bounded minimal safety row before registry/session/
provider/prompt effects. Initial open or reservation failure therefore returns a typed pre-effect
`durable_evidence_unavailable{reason=<bounded-code>}` refusal for direct unary. This core reservation is primary
accepted-work safety evidence rather than optional summary telemetry; only later enrichment is fail-open. Thus a
valid in-memory served primary store can still run workflows with an explicit telemetry-unavailable marker, but it
cannot accept direct unary work that would have no durable recovery record.

Terminal summaries have a rolling **180-day** retention period. Each workflow-history allocation is capped at
**100,000 terminal rows** and **128 MiB**, whichever boundary is reached first. A standalone platform ledger applies
the byte cap to its database plus live journal/WAL. A configured shared store maintains a conservative charged-byte
account for the workflow-history rows and their WAL reserve; unrelated task/turn tables do not consume or evade that
allocation. Age and pressure collection remove the oldest unpinned terminal summaries first. Active reservations
and explicitly pinned incident rows are never selected.

Before the first provider/session effect, telemetry admission selects exactly one ledger, collects eligible rows,
and reserves one fixed-size attempt slot plus its conservative terminal/WAL byte charge. The reservation is bounded
for every column and is large enough to terminalize a minimal row in place without new capacity. If an optional
workflow-summary reservation fails, that workflow still runs: its first status and terminal envelope carry
`telemetry_unavailable{reason=<bounded-code>}`, including `capacity_protected`, and a bounded low-cardinality
counter/log records the failure, but no second ledger is tried and the primary outcome is unchanged. A universal
workflow-summary row is therefore not claimed when optional admission fails. A direct-unary core reservation is
mandatory under the preceding paragraph and refuses before effects instead of entering this fail-open path.

For workflow/task surfaces, primary task terminal state commits independently and first. A reserved summary then
terminalizes idempotently with the primary outcome and a completeness flag. For direct unary, the mandatory core row
is its primary durable attempt/turn state and terminalizes the required producer/final/process fields independently.
Optional timing/partition enrichment is a separate bounded update in either case. If the store becomes unavailable,
the caller receives an explicit telemetry failure and any surviving reserved row remains `interrupted`/`incomplete`
for boot reconciliation. Telemetry failure never rolls back or rewrites primary terminal state, never starts a
retry, and never exceeds the hard allocation merely to make the summary look complete.

The live SQLite ledger is local state, not an iCloud document. R2f does not place an open database or WAL under
`~/Documents`; a later compact, closed snapshot/export may use the already-approved iCloud cold archive through a
separate evidence-publication design.

### D7 — runtime health enforcement scope

Backend-health persistence and action use one explicit runtime mode:

- `ephemeral` is the default for local workflows, tests, smokes, provider integration, dependency updates, and
  disposable servers. It records ordinary attempt diagnostics but carries no health strikes across process lifetime
  and performs no quarantine or successor-routing action.
- `observe` is for candidate and development served builds. It persists bounded health observations and projects
  what the policy would have done, but cannot quarantine, replace, retire, or reroute a backend.
- `enforce` is opt-in and limited to the explicitly designated production operator-served bridge. Only this mode may
  persist actionable health state, isolate a generation, or activate a successor.

Development/candidate servers use separate state roots and generation namespaces. Their failures never count
against production. Promotion creates a fresh production generation without inheriting candidate strikes. Requests
from other authorized repositories participate only when deliberately sent through the enforcing production
operator, and request-specific prompt/config/auth/model failures remain excluded from backend-health strikes.

These modes scope the health controller only. D6 workflow-attempt telemetry still records in every mode, including
failed development work, so reliability analysis does not become production-only.

### D8 — health classification, recovery, and successor comparison

A backend `generation` is one concrete bridge-owned backend instance: its immutable spawn identity, adapter
process/container where applicable, ACP transport, and the sessions it owns. It is not a model generation, bridge
release, workflow, or attempt. Replacement creates a new stable `generation_id`; old and new generations may
coexist while exact ownership drains.

The enforcing health controller classifies outcomes as follows:

- authentication, configuration, model, provider-limit, quota, and ordinary cancellation failures are
  request/external state and never add a generation-health strike;
- authentication state clears on a successful login/authentication observation;
- shared or transient network/provider degradation enters a bounded cooldown and may self-clear after a successful
  prompt-free half-open control check;
- the first ambiguous generation-relevant **pre-dispatch** failure marks that generation `suspect`;
- two such failures from distinct logical executions within 15 minutes, with no intervening successful prompt start,
  temporarily isolate the generation and permit exactly one same-config prompt-free successor comparison.

Pre-dispatch means resolve, spawn, initialize, authenticate, session create, or config apply before the bridge sends
the model prompt. A failure at prompt start or later is never automatically replayed and does not become an
ambiguous pre-dispatch strike merely because no output arrived.

The differential comparison has closed outcomes:

- successor control succeeds while the original still fails the equivalent control: sticky instance-local
  `quarantined`; the successor may serve future separately authorized requests;
- both fail with the same authentication/external condition: shared `auth_required`/`degraded_external`, eligible
  for self-clear rather than quarantine;
- the original succeeds: clear suspicion and return it to `active`;
- evidence is inconclusive: retain an isolated/degraded unknown state and say so; do not claim quarantine.

Generation state has two orthogonal axes; `quarantined` is not overloaded as a lifecycle state:

`auth_required` and `degraded_external` represent shared-condition overlays keyed by the exact auth/provider/network
scope, not instance-local strikes. Matching generations reference the same durable overlay; one successful equivalent
authentication/control observation clears it for that scope. The remaining health states are generation-local.

| Ownership lifecycle | Meaning |
|---|---|
| `active` | May be considered for new ownership when the health axis permits. |
| `draining` | Accepts no new session; exact existing ownership remains until settled. |
| `dead` | Mechanical generation-bound proof says the instance cannot serve another request; it may settle debt and advance only to `retired`, never recover service. |
| `retired` | Process settlement and every close/debt disposition are complete; terminal and non-routable. |

| Health/admission | New prompt routing | Prompt-free control | Exit condition |
|---|---|---|---|
| `healthy` | allowed only with lifecycle `active`; an existing warm turn is allowed while `draining` | allowed | eligible failure or lifecycle action |
| `suspect` | no automatic replay; a separately authorized new execution may still target it while `active` | allowed | successful control/prompt start clears; second distinct eligible failure isolates |
| `auth_required` | blocked | authentication observation only | successful login/auth observation clears |
| `degraded_external` | blocked | bounded half-open after cooldown | successful equivalent control clears; repeated shared failure reschedules bounded cooldown without a generation strike |
| `isolated_unknown` | blocked | the one owned differential comparison or local inspection | evidence resolves to healthy/shared degradation/quarantine, or remains explicit unknown |
| `quarantined` | blocked, including warm turns | exact local clear probe only | local clear enters probation or retirement/death settles the generation |
| `probation` | excluded from default selection; exactly one local-operator-authorized turn may target this generation | allowed | that turn's successful terminal result returns healthy; one eligible repeat failure re-quarantines |

Lifecycle restrictions win over health: `dead`/`retired` never route; `draining` never accepts a new session;
`urgent_security` is a flag on `draining` that freezes future warm turns after the current turn. When a healthy active
successor coexists with a probationary, suspect, isolated, or quarantined predecessor, default selection chooses the
successor. Exact-generation local authorization is required to target probation and cannot be inferred from a
context/model/repository match.

The transition owner is a per-generation serialized cell. It persists lifecycle, health/admission state, evidence
ids, successor/predecessor relation, strike execution ids, cooldown wall timestamp, and last transition before
routing state becomes visible. Ephemeral mode discards this state at process end; observe mode reconstructs only a
non-acting projection; enforce mode reconstructs action state after revalidating exact process identity. A persisted
cooldown becomes a fresh full monotonic delay after wall-clock rollback and deducts only valid forward elapsed time.
Illegal transitions—health recovery from `dead`/`retired`, direct quarantine-to-healthy, probation through ordinary
routing, or draining-to-active after successor promotion without an explicit rollback operation—fail closed and
emit an operator-visible invariant violation.

Quarantine never expires on a timer. It blocks new sessions and turns without killing a running turn or discarding
warm state. Local OS-owner resolution selects exact generation plus quarantine evidence and either retires it or
requests `clear-quarantine`. Clearance reruns the failed control-plane phase without a prompt and records a reason;
when no adequate safe probe exists, an explicit local `--ack-ambiguous` override is required. A cleared generation
enters `probation`, does not automatically displace an active successor, re-quarantines after one repeated eligible
failure, and leaves probation only after a separately authorized successful turn.

`dead` requires generation-bound mechanical proof that the instance cannot serve another ACP request: retained
child exit/reap, exact managed-container exit, or irreversible sole-transport closure under an adapter contract that
cannot reconnect that instance. Silence, elapsed time, process age/existence, provider errors, and repeated generic
failures do not prove death. Dead is terminal; exactly one prompt-free successor may activate, but no prompt is
retried or replayed.

### D9 — planned warm-session drain

Routine planned replacement is non-disruptive and differs from health quarantine:

- the predecessor enters `draining` and accepts no new sessions;
- every running turn completes on its owning generation;
- an already-existing warm session may continue accepting turns on that same generation, with exact affinity and its
  configured idle TTL; activity uses the ordinary warm-session TTL semantics rather than a special drain extension;
- new sessions select the active successor;
- once the predecessor has no running or warm ownership, capability-gated close/debt settlement completes and the
  generation retires;
- elapsed drain age produces bounded status/warnings but never force-retires an active or warm owner.

Quarantine remains stricter: it retains warm state but permits no new turn until evidence-backed clearance. A local
OS owner may explicitly declare an `urgent_security` drain with a recorded reason; it lets the current turn finish,
then freezes further warm turns without killing the process or deleting context. Routine replacement cannot silently
escalate into that mode.

ACP sessions are not migrated between generations. R2f never reconstructs, exports, or replays their context onto a
successor. An indefinitely active predecessor remains visible as incomplete drain until normal expiry/clear or an
explicit owner action resolves it.

### D10 — bridge-process deployment boundary

R2f does not implement stable ingress or side-by-side bridge **binary** replacement. Its R2f3c slice supplies a
bounded handoff contract only: stable release/process identity, task/session/execution affinity identifiers,
readiness and drain projection, ownership-preserving refusal when affinity is missing or ambiguous, and the local
operator observations a future ingress needs. R2f cannot claim non-disruptive binary replacement complete.

The process boundary is a dedicated **R2g stable-ingress** increment immediately after R2f and before lower-priority
provider integrations. R2g requires its own focused owner design before source implementation because current `serve`
has three load-bearing single-process assumptions: one process binds the configured TCP port, one process holds the
exclusive SQLite store lock, and warm sessions/live SSE producers are in memory.

R2g must provide a stable local endpoint, side-by-side release ownership, exact predecessor affinity for existing
tasks/sessions/streams, successor routing for new work, storage/schema compatibility, rollback, drain, release GC,
and launchd/operator integration without dual-opening today's exclusively locked store. It may not migrate/replay
ACP sessions or infer affinity. Until R2g is implemented and gated, bridge binary updates still require a coordinated
pause; backend-generation drain inside one process does not remove that limitation.

## 4. Required identity and telemetry architecture

This section is implementation direction consistent with D1-D11.

### 4.1 Logical execution and attempt identity

Use two identities:

- `execution_id`: stable for the operator-visible logical workflow. Served execution binds it to the durable task
  identity; offline execution mints the same validated identifier class before any spawn/effect.
- `attempt_id`: unique for one monotonic execution attempt and linked to `execution_id`, ordinal, and optional parent
  attempt. Boot resume and operator takeover create new attempts; they do not rename history or reuse the old
  monotonic clock.

The current internal `run_id` becomes the attempt identity used in node/session names. Both ids are printed before
offline execution begins, included in the first served progress/status projection, returned by MCP submission, and
present in terminal/takeover artifacts. A served client's pre-minted task id must be printed before opening SSE so a
transport loss still leaves a recovery locator.

Direct unary uses one explicit fail-closed channel: the caller mints validated, high-entropy `execution_id` and
`attempt_id` values before sending the request, and the bridge `submit` CLI prints them before network I/O. The unary
request carries both ids; the server atomically validates syntax, uniqueness, and the mandatory §4.2 safety
reservation before any registry/session/provider/prompt effect. A missing/invalid id or collision returns a typed
pre-effect refusal plus the supplied locator and never substitutes `task-1`, reuses a prior attempt, or prompts. A
duplicate locator can be inspected through recovery/status but is not an idempotent prompt replay. Other controllers
must follow the same request contract; server-minted ids that become visible only in the final synchronous response
do not satisfy direct-unary recovery.

### 4.2 Durable terminal summary

Persist one idempotent row per attempt, keyed by `attempt_id`, with at least:

- execution/attempt/parent identity and ordinal;
- optional served task id;
- workflow id, bounded task class, execution surface, and policy version;
- workload fingerprint covering the configured node/agent/model/effort shape without prompt text;
- wall-clock start/completion and monotonic execution, queue, cancellation, cleanup, and finalization durations;
- bounded per-phase cumulative durations;
- outcome, terminal reason, producer-terminal disposition, final-message presence, process-liveness disposition,
  degraded flag, prompt-acceptance certainty, and cleanup disposition;
- node counts by completed/failed/canceled/deadline/cleanup-partial state;
- telemetry completeness and clock-source flags.

Do not store prompts, model output, raw process output, credentials, arbitrary repository paths, or full process
command lines in the summary table. Existing task journal/artifact retention remains the detailed evidence surface.

For a telemetry-admitted attempt, its reserved row is created before execution and receives at most one terminal
transition. A process restart marks a still-running reserved row interrupted before creating the resume attempt.
Replayed terminal writes are idempotent and cannot double count Prometheus reconstruction. A workflow whose optional
summary reservation was unavailable has no ledger row and is never silently backfilled into a second ledger; its
primary status/output retains the explicit failure marker described by D6.

For direct unary, the reserved row's bounded core is mandatory and exists before effects. It records the caller-
minted ids, execution surface, start wall time, prompt-acceptance state, producer-terminal disposition,
final-message presence, process-liveness disposition, terminal-evidence capability/version/source/completeness, and
telemetry completeness. Optional timing/partition enrichment may fail open, but failure to open/reserve the core row
refuses before effects. The prompt dispatch barrier updates acceptance conservatively and durably; transport loss or
later journal failure cannot erase the row, clear sticky uncertainty, or authorize replay. Producer terminal,
final-message presence, and process liveness terminalize as three independent fields, including
`protocol_incomplete_final` when applicable.

### 4.2.1 Authoritative adapter terminal evidence

R2f0b adds one explicit ACP extension contract, `a2a_bridge.turn_evidence.v1`; package version or adapter name never
implies support. The adapter advertises the exact version during initialization. The bridge includes its opaque
`attempt_id` correlation in prompt `_meta`, and a supporting adapter emits one logical ordered extension envelope on
`a2a_bridge/turn_evidence` before it resolves or rejects that prompt RPC. The bounded envelope is tied to
generation, session, adapter-native turn id, and attempt id and contains:

- producer disposition: `completed`, `interrupted`, `failed`, or `unknown`;
- final assistant presence: `nonempty`, `absent`, or `unknown`;
- the adapter-native terminal source and bounded evidence sequence/completeness flags.

For codex-acp, producer disposition must come directly from the native Codex turn terminal notification/result. A
nonempty final may come only from a nonempty assistant response item explicitly tagged `phase=final_answer` for that
same turn, or a native terminal field with equivalent semantics. Commentary/analysis chunks do not count. `absent`
may be emitted only after producer completion is authoritative and the adapter has drained the ordered notifications
for that turn; otherwise the value is `unknown`. The adapter emits the envelope before the ACP prompt terminal, and
the bridge durably applies it to the reserved core row before publishing terminal status. Repetition is idempotent;
a second non-identical envelope is `protocol_terminal_evidence_conflict`, not last-writer-wins.

An adapter that does not advertise v1 remains usable but reports `terminal_evidence_unsupported`; producer/final
fields stay `unknown`, process liveness remains separate, and an accepted prompt error becomes
`protocol_terminal_unknown`, never success, proved process death, or retry. If an adapter advertises v1 but its
envelope is absent, malformed, late, mismatched, or conflicting, the bridge records the corresponding bounded
`protocol_terminal_evidence_*` failure with sticky accepted-work state and no retry. R2f cannot claim the Codex
null-final incident closed until the selected Codex adapter advertises v1 and conformance proves the real mapping.
When authoritative evidence says `producer=completed` and `final=absent`, the terminal outcome is
`protocol_incomplete_final` even if the prompt RPC itself rejects; `producer=completed` plus a nonempty final is the
only corresponding completed-final state.

### 4.3 Metrics and reporting

Add bounded metrics such as:

- `bridge_workflows_total{workflow,task_class,surface,policy,outcome}`;
- `bridge_workflow_work_duration_seconds{workflow,task_class,surface,policy,outcome}` and
  `bridge_workflow_end_to_end_duration_seconds{workflow,task_class,surface,policy,outcome}` with buckets extending
  through multi-hour review durations and the cleanup/reporting tail;
- `bridge_workflows_in_flight{task_class,surface}`;
- `bridge_workflow_snapshots_total{phase,reason}`;
- `bridge_workflow_cleanup_total{disposition}`.

Configured workflow/task-class/policy vocabularies normalize unknown values to `other`. IDs and workload
fingerprints remain durable-row dimensions only.

Add a read-only operator report over completed rows. Human and JSON forms show the query window, sample count,
partitions, min/mean/median/p90/p95/p99/max, excluded-outcome counts, and whether the sample is sufficient for an
advisory recommendation. Any recommendation is output only; applying it requires a separate config/document change.

## 5. Scheduler and cleanup architecture

Replace the single shared node cancel path with an execution controller containing:

- one workflow cancellation source whose cancellation reaches every node;
- one node cancellation source per running node;
- frozen node/workflow deadlines;
- an append-only progress channel;
- a per-node flight containing session/worktree ownership and a reference to the exact generation/resource action
  flight;
- a terminal state map independent of terminal-node text.

`FuturesUnordered` may remain the completion pool, but the executor loop must also select on the next deadline and
progress/snapshot events. A node future cannot simply be dropped at deadline. It either returns after cancellation
and cleanup or transfers its exact cleanup guard to an independently owned flight before the workflow records
`cleanup_partial`/`cleanup_unknown` and terminalizes.

Deadline behavior cannot ship on top of today's worktree-wrapper `forget`/`release`, because both cleanup strengths
eventually call forced provider removal. R2f1b first adds a result-bearing `preserve_after_cancel` ownership path:
it cancels/settles or transfers the agent/session/process flight, converts the worktree lease into a durable
recovery-owned lease, and explicitly does **not** call provider remove, reset, clean, or checkout. Repeated automatic
and manual session/worktree cleanup joins the node flight; any shared-process escalation joins its referenced
generation resource flight. R2f2 adds the private takeover artifact, operator resume,
and eventual explicit worktree disposition UX, but preservation is a prerequisite to enabling the first automatic
deadline rather than a later safety improvement.

### 5.1 Preserved-worktree claim and sweep contract

`preserve_after_cancel` transfers the volatile process-lifetime lease into a durable
`PreservedWorktreeClaimV1`, keyed by an unguessable worktree identity and bound to execution/attempt/node, canonical
repository and worktree object identities, sidecar identity, preservation reason, creation wall time, and recovery
locator. Before cancellation or any process effect can release the live owner, the transfer runs under the same
worktree coordination boundary used by run-end cleanup: durably publish and parent-sync a bounded
`preservation_prepared` intent, atomically replace it with the complete claim, parent-sync again, then mark the live
lease transferred. Sweep selection treats both the prepared intent and complete claim as protective. A crash before
the prepared barrier leaves the still-live lease responsible; a crash after it leaves durable protective evidence,
never an unprotected free-flock gap.

Both run-end and boot/orphan sweeps must parse preservation state before selecting a sidecar. A valid prepared intent
or preserved claim is ineligible even when its original `run_id` matches or its process flock is free. When a
prepared marker, transfer journal, or claim is present, a missing complete claim, corruption, identity mismatch, or
partial publication fails safe to `preservation_unknown`: it is operator-visible and the worktree is not deleted. A
sidecar with no preservation intent remains subject to the existing bounded sweep policy. Sweep code never repairs
preservation ambiguity by force-removing the worktree.

Resume atomically exchanges the durable claim for a new live lease before work begins. Explicit local disposition
is the only release path: retain leaves the claim intact; archive records and syncs the completed archive before
clearing it; delete proves exact identity, removes the worktree, then records the terminal disposition before
clearing claim metadata. Warning/age reporting may be bounded, but no TTL automatically deletes useful contents.
The ownership transfer itself completes or reports typed partial/unknown inside the 60-second cleanup envelope;
later operator retention is durable state, not a still-running workflow cleanup future.

Overall outcome is computed from the policy plus every node disposition, not only the terminal node. Synthesis text
remains the user-facing result, while structured terminal metadata retains root failures and cleanup state.

If cancel escalation retires a shared backend process, the generation cell closes admission before the one
generation-scoped process flight signals it. Every active node/session bound to that generation joins or observes
that flight and is marked `collateral_generation_termination` until its own terminal result proves a narrower
outcome. No affected sibling is left `running` merely because another node initiated escalation, and no second node
may signal the same process independently.

## 6. ACP session ownership and health direction

R2f must separate three operations that current code partially conflates:

- `forget`: remove only request/config routing state while the agent session remains intentionally usable;
- `close_session`: serialize against the exact session turn, cancel/settle if required, send ACP `session/close`
  only when that initialized generation advertises it, and retain its acknowledged/failed/unsupported disposition;
- `retire_generation`: stop accepting new sessions, drain existing ownership, then terminate the adapter process and
  resolve any remaining remote-session debt as generation retirement rather than falsely calling it session close.

Final cold workflow attempts must no longer use config-only forget as their normal terminal cleanup. They request
result-bearing close/release. If close is unsupported or fails, bridge routing is removed only after a minimal
generation-owned debt record is retained. Concurrent release joins one per-session close flight; it cannot send
duplicate close or erase a prior failure.

### 6.1 Durable close/debt state machine

The durable key is `(generation_id, session_id)`. It stores the adapter capability snapshot, one stable local
idempotency key, close attempt ordinal, capacity disposition, last definitely completed protocol boundary, retry wall
timestamp, and bounded failure class. One per-key serialized flight owns every transition:

| State | Meaning and capacity disposition | Automatic next action |
|---|---|---|
| `open` | Local/remote session may own capacity. | On final release, reserve `close_prepared` before protocol effect. |
| `close_prepared` | Intent is durable; no close dispatch barrier was crossed and attempt ordinal 0 remains unspent. Capacity remains held. | Dispatch ordinal 0 immediately in the live flight; after crash recovery, dispatch that same still-unattempted ordinal 0 at the first 1-minute safe-recovery bound using the same local key. |
| `close_dispatched_unknown` | The request may have reached the adapter but no acknowledgement is durable. Capacity remains held. | Retry only if the exact adapter contract proves close idempotent; otherwise require exact generation exit; local action may request that retirement but cannot declare capacity released. |
| `close_retry_due` | A typed definitely-not-accepted or contract-idempotent failure after a dispatched ordinal is retryable. Capacity remains held. | Only after dispatched ordinal 0 fails, retry ordinals 1, 2, and 3 after 1 minute, 5 minutes, and 30 minutes from the preceding recorded safe failure respectively. |
| `close_exhausted` | The initial attempt plus all three safe automatic retries failed, or safe replay cannot be proved. Capacity remains held and operator attention is visible. | No timer loop; resume only on new evidence, operator action, or generation exit. |
| `close_acked` | Exact close acknowledgement is durable. | Release the capacity claim; terminal. |
| `close_unsupported` | Capability snapshot proves no close operation. Capacity remains generation-owned. | Resolve only by exact generation exit/retirement. |
| `resolved_by_generation_exit` | Mechanical process/transport settlement proves the remote session cannot survive. | Release capacity claim; terminal. |

The bridge persists `close_prepared` before send and `close_dispatched_unknown` at the accepted-work barrier; an
acknowledgement then transitions to `close_acked`. A crash at either boundary reconstructs the durable state without
assuming whether an unrecorded reply occurred. Boot schedules only safe retries using D11's wall-to-monotonic rule.
Recovering `close_prepared` schedules the unspent initial ordinal 0 rather than consuming a retry ordinal. Only a
durably recorded definitely-failed/idempotently-replayable dispatch advances the ordinal.
Concurrent release, retry, drain, and retirement join the same flight. Generation retirement cannot report complete
until every debt is `close_acked` or `resolved_by_generation_exit`; exact process/sole-transport exit atomically
resolves every remaining capacity claim for that generation. Unsupported, unknown, and exhausted debt is never
erased merely to admit another session or because an operator acknowledged seeing it.

### 6.2 Session-capacity source and admission

Session capacity is distinct from workflow/batch concurrency. Each generation records a capacity source and exact
live/debt claim count. A truthful finite adapter capability is authoritative; an optional checked-in
`session_capacity_limit` may impose a lower bridge admission cap. When both exist, the effective cap is their minimum;
configuration can never raise an adapter-advertised hard limit. A session reserves one claim atomically before
`session/new`, initially keyed by `(generation_id, creation_attempt_id)`. A durable accepted response binds it to the
returned `session_id`; a definitely-not-accepted result releases it. Accepted-or-unknown creation without a usable
session id becomes `creation_unknown`, remains generation-owned, and cannot replay `session/new`; only a truthful
adapter reconciliation contract or exact generation exit releases it. For a bound session, `close_acked` or exact
generation exit releases the claim. Unsupported, unknown, retrying, and exhausted close debt continues to consume it.

No count observed during an incident becomes a default: fifteen retained sessions is evidence, not a threshold. If
neither the initialized adapter nor checked-in configuration supplies a truthful finite limit, the generation reports
`capacity_limit=unknown` with its exact outstanding-claim count. Unknown capacity does not authorize a fabricated
`capacity_exhausted` refusal, an automatic replacement, or a claim that capacity was repaired; ordinary bounded
workflow/registry admission remains in force while close/debt cleanup proceeds. A known full cap refuses only a new
cold session before `session/new`; an already-owned warm session/turn retains its normal lifecycle rights.

Tests parameterize advertised-only, configured-only, minimum-of-both, known-full, definitely-not-accepted create,
accepted create, creation-unknown/no-replay, close release, generation-exit release, and unknown-limit cases.
Production `enforce` status must distinguish known remaining capacity from unknown; `observe` and `ephemeral` may
record the same evidence but cannot turn it into health action. Adding a mandatory production cap or choosing a
numeric compatibility default is a separately reviewed operator-policy change, not an implementation guess.

Every registry slot gains a stable backend-generation identity, the ownership-lifecycle axis and health/admission
axis defined in D8, a serialized transition cell, and exact successor/predecessor relation. Selection evaluates both
axes under that cell; no caller reconstructs policy from a flattened status string.

Planned drain and health quarantine are distinct. Planned drain permits an existing warm context to continue on its
owning generation until its idle TTL, explicit clear/release, or operator-selected migration boundary. Quarantine
does not kill a running turn, but it does not allow a new turn merely to see whether the backend recovered.

The current 30-second registry force-retirement cannot implement planned non-disruptive drain. A planned generation
stays visible until its leases settle; exceeding a drain-age warning produces health/operator evidence, not forced
termination. Explicit destructive retirement is a separately authorized action and reports surviving ownership.

## 7. Implementation slices

Keep each slice independently reviewable and deterministic-first:

1. **R2f0a — identity, run ledger, and stats:** distinct execution/attempt ids; caller-minted/direct-unary and early
   CLI/served exposure; mandatory pre-effect direct-unary safety reservation with typed refusal, plus fail-open
   optional workflow-summary reservation; durable direct-unary turn boundaries; independently ordered primary
   terminal state; terminal summary DTO/store; metrics with multi-hour buckets; read-only stats; no timeout behavior
   change.
2. **R2f0b — progress vocabulary, terminal-evidence adapter contract, and recorder:** monotonic phase/activity/
   progress events; negotiated `a2a_bridge.turn_evidence.v1`; exact-turn producer/final mapping; unsupported/missing/
   conflicting evidence outcomes; bounded snapshots; fake-clock and deterministic owned-child/null-final fixtures;
   separate producer/final/process dispositions; no automatic cancellation.
3. **R2f1a — fan-out policy and per-node control:** checked-in profile vocabulary/default migration, config/schema
   validation, bounded-independent/fail-fast/fixed-grace scheduling, degraded synthesis, structured node terminal map;
   still use manual/fake deadlines first.
4. **R2f1b — preservation, warning, and absolute deadlines:** land `preserve_after_cancel` and durable recovery lease
   before enabling timers; 30-minute snapshot, two-hour work cutoff, 2:01:10 terminal envelope, Max validation,
   deadline selection, retained process-tree capability/single-flight cleanup ownership, collateral generation
   reporting, and #22 deterministic closure.
5. **R2f2 — local scoped takeover:** private artifact, capability-bound child-first termination, typed refusal/partial
   result, preserved recovery-owned worktree, explicit final disposition, and resume-from-first-unfinished-gate
   handoff.
6. **R2f3a — ACP close and session debt:** capability-gated close, durable write-before-effect debt machine,
   single-flight/idempotency, safe-only retries/exhaustion, final cold release, capacity claims, boot recovery, and
   deterministic capacity fixtures.
7. **R2f3b — backend health and generation drain:** generation ids, lifecycle/health axes and transition table,
   pre-prompt health evidence, persisted cooldown reconstruction, exact probation routing, successor selection,
   warm-owner drain semantics, and no-force visibility.
8. **R2f3c — operator handoff contract:** expose stable release/process identity, exact affinity, readiness, and
   drain/refusal projections required by R2g; do not add stable ingress or claim side-by-side binary replacement.
9. **R2f4 — dogfood and closure:** provider-free failure matrix first; then separately authorized minimal live and
   disposable takeover gates; Sol/xhigh adversarial review before any hard/complex second lens.

No slice uses a provider turn to prove deterministic scheduling, timeout, cleanup, close, capacity, or transport
state. A provider-specific #24 disposition requires captured protocol evidence or separate authorization.

## 8. D11 — owner-approved cleanup and reporting bounds

The owner approved D11 on 2026-07-20 after the focused
[`short-bound validation spike`](../spikes/2026-07-20-r2f-short-bound-validation.md), with explicit observation
margins around the measured internal timers:

- external-degradation half-open checks are scheduled after 30 seconds, 2 minutes, and 10 minutes;
- one prompt-free ACP health/control operation has a **31-second observable hard bound**: its internal deadline fires
  at 30 seconds and the remaining second is reserved for scheduling, connection fencing, exact host-child settlement
  where available, and publication of the typed control outcome or `cleanup_pending` ownership transfer;
- cancellation has a **6-second observable hard bound**: five seconds remain the cooperative ACP cancellation grace,
  with the remaining second reserved for escalation, process-group reap, and publication of the initiating turn's
  terminal/cleanup-pending disposition;
- the universal outer cleanup bound is 60 seconds;
- terminal persistence/reporting has a 10-second bound;
- durable cleanup-debt retries begin after 1 minute, then 5 minutes and 30 minutes.

The 31/6-second observations do not claim that a named container reap, every collateral shared-session terminal, or
durable debt settlement is complete. By those bounds the dispatch fence is closed and cleanup has either completed or
transferred to the exact durable/single-flight owner; the universal 60-second cleanup envelope owns the remaining
process/container/collateral settlement and reports partial/unknown rather than blocking beyond it. The 1/5/30-minute
retry schedule applies only to effects proved safe to repeat by §6.1; ambiguous non-idempotent close dispatch moves
to exhausted/operator-owned debt instead of being replayed on a timer.

The 30-second first half-open delay and 60-second first debt retry remain explicit owner policy: fake-clock semantics
passed, but neither scheduler exists on current main. The 31/6-second observable bounds do not silently lengthen the
internal 30/5-second action timers; doing so would move the same scheduler overhead beyond the newly named bounds.
Every implementation path requires fail-first boundary and negative/edge coverage. Longer intervals require the
same fake-clock no-early/one-fire coverage.

## 9. Verification contract

Before implementation is called complete:

- every new/fixed behavior has a fail-first test and a negative or edge case per new path;
- fake clocks prove warning versus absolute deadlines and monotonic rollback immunity;
- silent healthy, active output, live-but-quiet, blocked, exited-child/wedged-waiter, and process-identity-reuse cases
  remain distinguishable;
- failed root plus nonterminating sibling reaches a bounded terminal without stranding cleanup;
- degraded synthesis and strict fail-fast both retain the deepest root cause and every node state;
- offline, served, resume, transport-loss, cancellation, and takeover expose usable identities;
- direct unary prints caller-minted execution/attempt ids before network I/O, rejects missing/invalid/colliding ids
  and mandatory-ledger open/reservation failure before effects, and never substitutes a server-only locator;
- direct unary accepted-work plus progress plus null-final producer completion yields a durable
  `protocol_incomplete_final` attempt/turn with separate live-process evidence and no retry; this uses an advertised
  `a2a_bridge.turn_evidence.v1` envelope from the selected Codex adapter rather than text/error inference;
- adapter conformance covers initialize negotiation, exact attempt/turn correlation, commentary-only then null-final,
  genuine per-turn error after commentary, final-answer nonempty, unsupported capability, advertised-but-missing,
  malformed, late, duplicate-identical, duplicate-conflicting, notification/response reordering, and transport loss;
- workflow stats reconstruct idempotently and report exact deterministic quantiles;
- protected telemetry capacity plus platform/configured-ledger permission, lock, migration, corruption, and initial
  open failures emit the correct bounded unavailable marker without fallback or primary-outcome change, while a
  reserved minimal row terminalizes without exceeding its charge and later store failure cannot roll back primary
  task terminality;
- capability-present/absent/failed/duplicate/concurrent close cases retain correct debt;
- crash-before-dispatch reuses unspent ordinal 0; crash-after-dispatch-before-ack, safe retry, unsafe retry refusal,
  initial attempt plus three-retry exhaustion, and generation-exit debt resolution preserve capacity ownership;
- advertised/configured/minimum/unknown session-capacity sources, known-full pre-session refusal, and exact claim
  release are distinguished without inferring a threshold from incident counts;
- two nodes escalating one shared generation join one process/resource flight and receive the same collateral result;
- a preserved worktree survives ordinary run-end selection, free-flock boot sweep, crash/restart, corrupt claim,
  resume exchange, and explicit retain/archive/delete disposition;
- planned drain routes new sessions to a successor while running and warm ownership survives on the predecessor;
- lifecycle/health transition and illegal-combination tables are exhaustively exercised, including probation routing
  with an active successor and wall-clock rollback during cooldown reconstruction;
- an ambiguous drain never claims completion, and unrelated processes survive scoped takeover;
- formatting, warnings-denied Clippy, locked release build, repository hygiene, dependency policy, and the full serial
  workspace suite pass with exact totals;
- live provider, production-operator, deployment, and compatibility behavior not exercised is named explicitly.

## 10. Review-1 correction disposition

The first fresh Sol/xhigh review returned five `WRONG` and four `SMELL` findings. This fold accepts all nine and
routes each correction to a normative contract rather than treating review prose as the implementation source:

| Review finding | Folded disposition |
|---|---|
| WRONG 1 — deadline could force-remove useful work | §5 makes `preserve_after_cancel` and a durable recovery-owned worktree lease a prerequisite to enabling any automatic deadline; R2f1b owns that prerequisite before R2f2 adds artifact/resume UX. |
| WRONG 2 — exactly-one ledger impossible at protected capacity | D6 uses pre-effect bounded reservation when available and an explicit `telemetry_unavailable{capacity_protected}` primary-status path when not; primary terminalization is independent and first. |
| WRONG 3 — two-hour budget contradicted its tail | D3/D4 separate the two-hour work cutoff from the 2:01:10 observable terminal envelope. |
| WRONG 4 — fixed grace shortened a supposedly immutable sibling clock | Invariant 3 and D1 scope clock preservation to `bounded_independent`; pre-prompt `fail_fast`/`fixed_grace` policy conditions are explicit earlier-cancel exceptions without rewriting the recorded deadline. |
| WRONG 5 — plan/roadmap did not match design | The linked plan and roadmap now use execution/attempt ids, approved D1-D11 state, and exact `0a/0b/1a/1b/2/3a/3b/3c/4` order. |
| SMELL 1 — PID reuse and competing cleanup authority | D5 requires a spawn-time retained process-tree capability, journal-before-signal, and one automatic/manual single-flight; late numeric/name action is forbidden. |
| SMELL 2 — registry and health states were not closed | D8 defines orthogonal lifecycle and health/admission axes, routing precedence, persisted transitions, illegal states, successor choice, and probation targeting. |
| SMELL 3 — close debt lacked a crash-safe machine | §6.1 defines the durable key, write-before-effect states, serialized flight, initial attempt plus three safe retries, exhaustion, boot recovery, capacity ownership, and exact-generation exit settlement. |
| SMELL 4 — compatibility policy was unnamed | D4.1 names deterministic legacy/review/explicit/Max profiles, exact finite defaults, task mapping, and fail-closed unknown-profile behavior. |

The six-second cancellation decision received one additional disposable provider-free stacked-timer check after
review 1: ten concurrent TERM-ignoring process groups completed in 5.508283416-5.508360916 seconds with every exact
group absent, leaving about 492 milliseconds to the observable bound. The spike records its scope and keeps Docker,
collateral-session, and durable-debt settlement inside their separately named outer bounds. Review-1 corrections were
therefore folded and adjudicated by closure review 1.

## 11. Closure-review-1 correction disposition

The first closure review adjudicated seven inherited items `FIXED`, retained two as `PARTIAL`, and found one new
`WRONG` plus four new `SMELL` items. This fold accepts every residual:

| Closure-review-1 item | Folded disposition |
|---|---|
| Inherited WRONG 5 partial — stale roadmap surfaces | The roadmap's top, detail, program table, next action, and current-handoff identity now agree on `345941db`, `agent/r2f-owner-design`, the completed folds, and the next closure review. |
| Inherited SMELL 3 partial / new WRONG — recovered `close_prepared` skipped ordinal 0 | §6.1 keeps ordinal 0 unspent until a dispatch barrier is crossed; boot dispatches that same ordinal at the first safe-recovery bound and advances only after a recorded dispatch failure. |
| New SMELL 1 — shared ACP process had only per-node flights | D5 and §5 assign a multiplexed process/container exactly one generation-scoped resource flight, close admission before signal, make every node action join it, and publish one result to all collateral owners. |
| New SMELL 2 — durable worktree claim did not bind sweeps | §5.1 defines atomic claim publication, run-end and boot-sweep exclusion, fail-safe corrupt/ambiguous handling, resume exchange, and explicit-only final disposition. |
| New SMELL 3 — telemetry selection omitted served/in-memory and open failure | D6 selects the platform ledger for every no-store surface and enumerates bounded no-fallback failures while preserving primary-surface admission semantics. |
| New SMELL 4 — capacity claims had no source | §6.2 defines advertised/configured/minimum/unknown sources, exact claim lifetime, known-full refusal, and truthful unknown behavior without inventing a threshold. |

The durable report is
[`closure review 1`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-1.md). The next authorized
[`closure re-review 2 attempt`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-2-failed.md) accepted work
but ended null-final with no review verdict and was not replayed.

## 12. Closure-review-3 correction disposition

The operator then authorized one distinct clean-room Sol/xhigh
[`closure review 3`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-3.md), not a retry or resume. It
adjudicated all six inherited families `FIXED`, kept the incident-derived direct-unary contract `PARTIAL`, found one
new `WRONG` and one new `SMELL`, and returned `R2F OWNER DESIGN: REVISE`.

| Closure-review-3 item | Folded disposition |
|---|---|
| Incident partial / WRONG — fail-open ledger could accept direct unary with no durable evidence | Invariant 14, D6, §4.1, and §4.2 make one minimal direct-unary core reservation mandatory in the already selected ledger before registry/session/provider/prompt effects. Open/reservation failure is a typed pre-effect refusal; optional workflow summaries and later unary enrichment retain their fail-open behavior. |
| SMELL — caller-visible pre-prompt unary identity channel unspecified | §4.1 selects caller-minted validated high-entropy execution/attempt ids, requires the CLI to print them before network I/O, carries them in the request, refuses missing/invalid/colliding ids before effects, and forbids the current server-only `task-1` substitute or duplicate-locator replay. |

The operator's one-turn authorization produced the distinct closure review 4 recorded below. It was not a retry or
resume and did not authorize an automatic re-review, fallback provider, or second billable attempt.

## 13. Closure-review-4 correction disposition

The authorized distinct clean-room Sol/xhigh
[`closure review 4`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-4.md) adjudicated both closure-review-3
corrections and all six older regression families `FIXED`, retained the incident terminal contract as `PARTIAL`,
found one new High `WRONG`, no `SMELL`, and returned `R2F OWNER DESIGN: REVISE`.

| Closure-review-4 item | Folded disposition |
|---|---|
| Incident partial / WRONG — ACP could not distinguish null-final producer completion from a genuine live-process per-turn failure | Current-main fact 12 and invariant 15 forbid inference from text, stop reason, generic error, or process liveness. §4.2.1 defines negotiated `a2a_bridge.turn_evidence.v1`, an exact-turn ordered extension envelope, authoritative Codex terminal/final sources, durable application ordering, typed unsupported/missing/malformed/late/conflicting behavior, and no retry. R2f0b and the verification contract require real Codex adapter conformance before incident closure. |

The operator's one-turn authorization produced the distinct closure review 5 approval recorded below. It was not a
retry or resume and did not authorize an automatic re-review, fallback provider, second billable attempt, or source
implementation.

## 14. Closure-review-5 approval

The authorized distinct clean-room Sol/xhigh
[`closure review 5`](../reviews/2026-07-20-r2f-owner-design-sol-closure-review-5.md) adjudicated the terminal-evidence
correction and every closure-review-4 regression family `FIXED`, found no new `WRONG` or `SMELL`, and returned
`R2F OWNER DESIGN: APPROVE`.

The review confirmed that ACP prompt/capability metadata and vendor notifications provide an implementable extension
seam; current codex-acp exposes the native turn-terminal, phase, turn-id, and ordered-drain sources required by the
planned mapping; every unsupported/missing/order/conflict path remains conservative; and the design does not claim
the actual Codex incident closed before adapter conformance. No provider/live gate, automatic re-review, retry,
fallback provider, or second billable attempt is authorized by the approval turn itself.

## 15. R2f0a integrated implementation closure

R2f0a implementation, correction, native verification, and final cumulative reviews are complete at exact integrated
checkpoint `7b01ab4bae167d3640050dfda5de7e1478728497` on `agent/r2f0a-identity-ledger`, tree
`7d0b14aa1d39ca36fdc68a9ad69df4fc8442e64e`. This supersedes the historical operator-folded checkpoint
`9761b3b78c89cca079ddb1d9376514fceb77e0df` and approved candidate
`d7f20d37a9fda493c0b8dc18339489bfe1a059a3` / tree `1803a888cf77fdee378367404179cc9ba4085ee6`. The
July 24 [native](../reviews/2026-07-24-r2f0a-native-verification.md),
[Sol/xhigh](../reviews/2026-07-24-r2f0a-final-cumulative-sol-review.md), and
[Fable/xhigh](../reviews/2026-07-24-r2f0a-final-cumulative-fable-review.md) records remain historical predecessor
evidence.

The exact integrated correction stack retains provenance rather than flattening it: `4a6fcb90` imported approved
API/handoff candidate `0cb10903`; `f145535a` imported approved recovery candidate `7b8fa376`; `4359dc9c` folded
approved test candidates `6d34edcb`, `0b77ed87`, and `04b5792e`; and `7b01ab4b` folded approved
lineage/Platform/test candidates `a1481ed`, `dea817be`, and `24fd4b8a`.

The [integrated native macOS verification](../reviews/2026-07-25-r2f0a-integrated-native-verification.md), copied
from source SHA-256 `a67e1362217a3263b09a42b9e86136cd3cd8a1e044f921538eef5fc2fe91203d`, records passing fmt,
locked all-target/all-feature check, warnings-denied Clippy, debug and release builds, the exact alias regression,
repository hygiene, and final diff/clean checks. The complete workspace emitted **73** result groups with **2,785
passed / 0 failed / 12 ignored / 0 measured / 0 filtered**. All 12 ignored tests are repository-declared
live/external-provider or multi-bridge cases; no command-line skips were used. The first final native attempt honestly
failed after **2,541 passed / 1 failed / 12 ignored** on a test-only `/var` versus `/private/var` canonical-path
expectation. The six-line test-only correction canonicalized the expected path, its exact test passed **1 / 0 / 0**
with **211 filtered**, and the full suite then passed. This was not a production defect.

The independent concurrent fresh exact-head
[Sol/xhigh review](../reviews/2026-07-25-r2f0a-integrated-final-sol-review.md), source SHA-256
`8f9cc3efa961492915ef59bf4563682cfb57caa76a53662813b8bc0f87da037d`, adjudicated all seven current
mechanisms and seven inherited families `RESOLVED`, with zero `WRONG`, zero `SMELL`, and `APPROVE`. The
[Fable/xhigh review](../reviews/2026-07-25-r2f0a-integrated-final-fable-review.md), source SHA-256
`623faf2ea4170b014c3b8f027cd555b387bf5fb0bb4f7aa0056c8d9304a1d6e0`, reported zero `WRONG`, one
nonblocking `SMELL`, and `APPROVE`. Its new nonblocking follow-up is to add a legacy one-method
`RouteTarget::Workflow` arm to the existing fail-closed route coverage and document the compatibility delta for
hypothetical third-party one-method routers. Shipping `SkillRoute` uses the explicit pre-default hook, and no
incorrect production behavior was demonstrated. The three predecessor Fable follow-ups also remain nonblocking:
root-only foreign-owner CI coverage; foreign-owner coverage for both selection wrappers; and any foreign-owned
rollback-journal policy change only through a separate owner decision.

No ignored live/provider test was forced. Locked-egress Linux could not fetch one missing `a2a-lf` dependency for
the final six-line macOS test-only correction, so the artifact is not Linux proof. No GitHub CI, push, PR, merge,
release, deployment, live canary, production-server change, or post-merge operator build is proved. After this
docs-only fold receives its own verification/review, R2f0a is ready for PR/CI and merge. R2f0b remains next only
after merge and is not started. This fold does not complete R2f overall, #22, #24, R2g, or R4.
