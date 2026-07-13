# R2b — Structured lifecycle diagnostics implementation plan

- **Status:** R2b0 MERGED at `11ebc402`; R2b1 MERGED at `7b788c1f`; R2b2 IN PROGRESS (2a active); R2b3 NOT STARTED
- **Prerequisite:** R2a merged at `24aff09c`
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Completion shape:** four independently reviewed and merged sub-slices, R2b0 through R2b3

R2b is the highest-risk part of the reliability program. It changes error ownership, replay decisions,
warm-session reuse, task-journal payloads, projection, and multiple production dispatch paths. Do not
implement it as one flag-day branch.

## Fixed scope boundary

R2b owns:

- versioned bridge-owned diagnostic types and validation/redaction;
- additive `BridgeError::AgentFailure` with the same static public category as `AgentCrashed`;
- rollback-compatible diagnostic-bearing `Progress` journal records;
- lifecycle observation from resolve through teardown;
- migration of the named ACP and API failure boundaries;
- prompt-acceptance barriers and no-replay mappings;
- warm workflow/session retirement after structured failures;
- provider-limit/overload recognition from the closed structured-evidence table;
- bounded stderr metadata and explicitly opted-in best-effort-redacted text.

R2b does **not** own:

- the billable `smoke` CLI (R2c);
- host fallback planning or execution (R2d/R2e);
- typed eligibility for a host downgrade (R2d);
- scheduled canaries or compatibility promotion (R3);
- dependency/image pinning (R4);
- M4 retention.

## R2b0 — close the remaining ownership, composition, and trace-audit ambiguities

- **Branch:** `agent/reliability-r2b0-contract`
- **Type:** docs-only contract patch
- **Exit:** reviewed design revision with no unresolved production owner

### Required design edits

1. Assign direct prompt ownership before resolution begins:

   - add an additive `Translator::run_observed` path in
     `crates/bridge-core/src/translator.rs`; the legacy `run` delegates with a no-op observer;
   - the inbound streaming and synchronous handlers create a bounded in-memory observer before
     `warm_local_dispatch`/`resolve_configure_bind`, pass it through `resolve_observed`, and transfer it to
     `spawn_local_producer` or `Translator::run_observed`;
   - each `server.rs::local_kiro_source` receives an in-memory source observer created before
     `resolve_for_fanout`;
   - `crates/bridge-coordinator/src/coordinator.rs::{prompt,continue_turn}` create an in-memory observer
     before session-manager checkout and transfer it through resolution to `collect_turn`;
   - direct A2A/coordinator correlation ids do not imply a `TaskRecord`; those paths never write a task
     journal or create a hidden task row;
   - `WorkflowRunContext` selects an explicit per-node observer factory: detached tasks provide a
     journal-backed factory after task creation, while direct A2A workflows provide in-memory regardless
     of `task_id` or rich-sink presence;
   - non-task catalog probes such as `AcpBackend::describe_options` use an in-memory/no-op observer and
     never invent task ownership.

2. Preserve the existing rich-event API while adding diagnostic composition:

   - retain `AgentBackend::{prompt,prompt_observed}` unchanged;
   - add `BackendObservers { rich, diagnostic }` and additive `prompt_with_observers` with a
     source-compatible default that preserves rich observation when supplied;
   - require ACP, API, container, worktree, and resilient production decorators to forward both channels;
   - allow a journal-backed observer only after the owner has created a real `TaskRecord`; journal failure
     is fatal there, while direct in-memory observers have no persistence-failure mode;
   - preserve `SpawnFn`/`Registry::new`, add `ObservedSpawnFn`/`Registry::new_observed`, and pass the
     initialization observer separately to `AcpBackend::spawn_observed`; never store it in `AcpConfig`, a
     cached backend, or session config;
   - preserve required `ContainerSpawn::spawn` and add defaulted `spawn_observed`; production overrides
     it and never stores the observer in `WarmInner`;
   - add observer-aware cancel/forget/release and node-cleanup compatibility methods that return `Result`;
     background registry retirement and detached cancel escalation remain process-scoped and never retain
     an operation observer;
   - add a joinable, result-bearing `ReapController` for observed container cleanup while preserving the
     fire-and-forget reaper for `Drop`, and require `WorktreeBackend` observed cleanup to propagate
     non-`NotFound` inner/provider/sidecar errors.

3. Close every current ACP trace surface, not only raw error values:

   - route all 16 current `bridge-acp` `tracing!` calls through one typed `AcpTraceEvent` funnel;
   - allow only bridge-owned enums/codes plus booleans, bounded counts, hard-coded ranks, and numeric RPC
     codes—no string/id/path/model/effort/auth/error field;
   - remove production `resolved_log_line` logging and forbid direct production `tracing!` outside the
     funnel with a source regression;
   - inject one bridge-known credential through every current dynamic source, including success-path
     model, effort, auth, agent, and session values as well as SDK message/data.

   Every captured trace must remain free of the credential and any SDK/derived `Debug`/`Display` value.

4. Apply security/survivability rules to every owner:

   - construct every serializable transition with `DiagnosticRedactor`; arbitrary auth ids and API-key
     environment names use tagged `RedactedDiagnosticId` and retain no partial value after redaction;
   - route workflow cleanup, coordinator collection, inbound streaming, and inbound synchronous warm
     completion through one exhaustive classifier that expires every `AgentFailure` before any guard can
     return the session to `Idle`;
   - make the common completion guard set its generation/operation-bound `Expire` drop action
     synchronously at error observation, then hand the atomically removed handle to an `ExpiryClaim` whose
     state transfers into one observer-free `CleanupFlight` before the first cleanup await; drop starts or
     detaches from that flight but never invokes release twice.

5. Add the R2b0 review dispositions to the design trail. Keep the program cursor on review/merge R2b0;
   advance it to R2b1 only after the approved contract is present on `origin/main`.

### R2b0 verification

- Search the whole repo for `translator.run(`, `prompt_observed`, task-row creation, cleanup ports, and
  every `tracing!` call in `bridge-acp`; the contract must account for every production hit and observer
  lifetime or explicitly classify it as in-memory/no-op/process-scoped.
- Run repository hygiene and the full workspace suite even though the patch is docs-only.
- Fresh adversarial design review; verdict must be `READY`/`APPROVE` before R2b1.

## R2b1 — diagnostic foundation and rollback-safe persistence surface

- **Branch:** `agent/reliability-r2b1-diagnostic-foundation`
- **Behavioral boundary:** types and persistence/projection compatibility; no production failure site
  constructs `AgentFailure` yet

### Production changes

1. Add `crates/bridge-core/src/diagnostics.rs` and export it from `bridge-core`:

   - `DiagnosticPhase`, `PhaseStatus`, `DiagnosticFailureClass`, `FailureDisposition`;
   - typed `DiagnosticOperation`, `DiagnosticCode`, `AuthenticationEvidence`,
     `RedactedDiagnosticId`, `StderrScope`, and `StderrRedaction`;
   - private-field `PhaseTransition`, `PersistedPhaseTransition`, `DiagnosticEvent`, and
     `FailureDiagnostic`;
   - validating builders for failures and every persisted transition, all taking `DiagnosticRedactor`;
     schema version 1, append-only serialized vocabularies, all bounds from the design, and custom safe
     `Debug`.

2. Add `BridgeError::AgentFailure { diagnostic }` in `crates/bridge-core/src/error.rs`:

   - static `Display` and `client_message()` return `agent crashed` without diagnostic text;
   - `is_transient()` delegates only to the typed disposition;
   - legacy `AgentCrashed` behavior is unchanged in this sub-slice.

3. Extend `OrchEventKind::Progress` in `crates/bridge-core/src/orch.rs` with
   `diagnostic: Option<DiagnosticEvent>` using serde defaults and omission for `None`.

4. Make projection total in `crates/bridge-coordinator/src/detached.rs`:

   - replace `frame_from_orch(...) -> Frame` plus `unreachable!` with an exhaustive
     `project_orch_frame(...) -> Option<Frame>`;
   - diagnostic progress persists but returns `None` for live and reattach projection;
   - ordinary progress remains byte-for-byte compatible.

5. Update every `Progress` constructor and pattern match in core, store, coordinator, inbound tests,
   and SQLite fixtures to preserve `diagnostic: None` behavior.

### Pre-change-failing tests

- diagnostic code rejects empty, oversized, uppercase, whitespace, slash, and secret-shaped tokens;
- cause truncation retains two outermost plus six deepest causes;
- per-field and adjacency-normalized credential redaction covers unsplit, two-field, and three-field
  values plus multibyte boundaries;
- mixed-case HTTP/HTTPS queries and fragments are removed during construction and deserialization;
- reset timestamps accept the 30-day boundary and reject the next millisecond, extreme futures, and
  malformed-wire futures; missing/invalid reference time rejects reset metadata; retained stderr never
  exceeds either its observed count or the 32-line cap;
- auth method ids and API-key environment names equal to or containing a known credential serialize only
  tagged `redacted` state in journal/artifact JSON; unchanged safe ids round-trip as tagged values;
- `Display`, `Debug`, `client_message`, and wire serialization cannot expose diagnostic secrets;
- prior-schema `Progress { text }` fixture reads a new `diagnostic: None` payload and a prior reader
  ignores the optional new field;
- diagnostic progress persists, produces no live/reattach frame, never panics, and is followed by normal
  node/terminal projection through the actual detached sink/hub and reattach snapshot fold;
- exhaustive class/disposition/metrics tables fail to compile or test when a variant is unhandled;
- the exact class/phase/barrier matrix rejects post-barrier fallback and every fatal-class retry pairing
  during both construction and deserialization;
- no production constructor of `AgentFailure` exists yet outside fixtures/builders.
  The AST guard scans `error.rs` too and permits exactly one constructor in the central builder.

### Merge gate

R2b1 may merge independently only if legacy runtime behavior is unchanged and rollback fixtures pass.
Do not begin error-site migration on the same branch.

## R2b2 — ACP/Fable lifecycle evidence, journaling, and no replay

- **Branch:** `agent/reliability-r2b2-acp-lifecycle`
- **Priority:** first end-to-end value; directly addresses the observed Fable failure opacity

### Internal execution sequence

R2b2 remains one merge boundary, but implementation and review use four ordered, independently
revertible commits so a lost session can resume from the first incomplete item:

1. **R2b2a — observer, persistence, and registry compatibility.** Add `DiagnosticObserver`, bounded
   in-memory/no-op/task-journal implementations, explicit factories, `BackendObservers`, additive backend
   cleanup methods, `resolve_observed`, and legacy/observed registry spawn constructors. Prove legacy
   backend/registry implementations compile unchanged, journal authority requires an existing task row,
   cached resolution emits only `backend.reused`, and constructor observers are not retained.
2. **R2b2b — ACP lifecycle and safe evidence.** Thread the initialization and prompt observers through
   spawn, initialize, authentication, session creation, config application, prompt start/stream/finish,
   and operation-owned teardown. Migrate generic prompt and watchdog failures, add the process-scoped
   stderr ring, and replace all 16 direct ACP trace sites with the typed metadata-only funnel. This commit
   owns phase grammar, accepted-work barrier, no-replay, cause retention, auth evidence, and trace-secret
   regressions.
3. **R2b2c — production owner and workflow authority.** Thread one attempt observer through inbound
   streaming/synchronous/fan-out, coordinator prompt/continue, cold and warm workflow paths, and
   `TurnRunner`. Add the explicit `WorkflowRunContext` factory: direct/correlation-only paths always use
   in-memory observation, while detached tasks use a journal factory created only after the durable task
   row exists. Prove rich events are neither lost nor duplicated and journal failure is fatal before
   completion.
4. **R2b2d — warm expiry and cleanup single-flight.** Add the shared survivability classifier,
   `WarmCompletionGuard`, claim-identified tombstones, `ExpiryClaim`, observer-free `CleanupFlight`, and
   joined worktree cleanup. Exercise all cancellation windows, stale generation/claim protection,
   cleanup failure, lease lifetime, and forced-retirement races before any owner adopts the new path.

R2b2a–R2b2c use ordinary fresh full-branch xhigh review. R2b2d is concurrency-qualified and may use Max
because its correctness evidence is one tightly connected cancellation/idempotency proof. Max is not the
default for the other commits; use it there only after High/xhigh fails to resolve a concrete issue.

After each internal commit, update the central roadmap with the exact commit, focused tests, and next
item. Do not mark R2b2 `MERGED`, advance to R2b3, or expose partially migrated production errors until all
four items pass the full workspace/release/hygiene gates and one final bridge-mediated review.

### Observation plumbing

1. Add the bridge-owned `DiagnosticObserver` port plus no-op, bounded in-memory, and task-journal-backed
   implementations, and an explicit per-node/attempt factory. The journal-backed form requires an
   existing `TaskRecord` and operation id; `task_id`/rich-sink presence never selects it.
2. Add `BackendObservers` and additive `AgentBackend::prompt_with_observers`. Preserve existing
   `prompt_observed(..., RichEventSink)` behavior and require production decorators to forward both rich
   and diagnostic channels.
3. Add `AgentRegistry::resolve_observed`; the default delegates to `resolve`. Keep legacy
   `SpawnFn`/`Registry::new`, add `ObservedSpawnFn`/`Registry::new_observed`, and use the observed form in
   production. The concrete registry drops the initialization observer before cache publication;
   `AcpConfig` and cached backends never retain it.
4. Thread one attempt observer through concrete registry resolution and prompt ownership into:

   - process spawn/transport connection;
   - initialize;
   - authenticate/skip evidence;
   - `session/new`;
   - mode/model/effort configuration;
   - prompt start, prompt stream, prompt finish;
   - synchronous operation-owned teardown.

5. Implement the R2b0 ownership map:

   - `Translator::run_observed` plus all three inbound owners, with observers created before resolve;
   - coordinator `prompt`/`continue_turn` through checkout and `collect_turn`;
   - direct and detached workflow factories plus warm executor's
     `NodeTurn.backend.prompt_with_observers` path;
   - additive `TurnRunner::run_turn_observed`, overridden by `ResilientWarm` for the first and rebuilt
     backend;
   - detached rich sink persistence with total optional projection.

6. Use bounded in-memory observation for direct inbound/coordinator operations, direct A2A workflows, and
   smoke. Use the journal-backed observer only for a real durable task; an external/correlation `TaskId`
   never authorizes `TaskStore::record_event_sequenced`.
7. Add observer-aware `cancel`, `forget_session`, `release_session`, and `NodeTurnCleanup::on_exit`
   compatibility methods returning `Result`, plus observed SessionManager cancel/expire/release paths.
   Finalize the diagnostic after synchronous cleanup. Registry retirement, `Drop`, and detached cancel
   escalation remain process-scoped and never write late task transitions.
8. Override observed cleanup in `WorktreeBackend`: propagate inner/provider/sidecar errors except
   `NotFound`; keep legacy cleanup best-effort. Route legacy/observed forget/release and forced retirement
   through one sealed per-session cleanup coordinator with monotonic `forget < release` strength; retire
   joins all cells before `inner.retire` and never independently drains worktree metadata.
9. Add shared `WarmCompletionGuard`/`ExpiryClaim` ownership. `observe_exit` sets `Expire` synchronously
   before any await; `begin_expire_current` replaces only the matching generation/operation with a unique
   resource-free `Expiring` tombstone and returns its resources in the claim. Before cleanup's first await,
   move backend/session, worktree metadata, lease, and child cleanup into one spawned `CleanupFlight`. The
   task returns a `CleanupReport` but never captures the operation observer/task store; observed completion
   only joins and records that report. Cancellation before flight start makes claim drop start it, while
   cancellation after start detaches the waiter and never starts a second release. Checkout rejects the
   tombstone; only successful matching claim-id completion clears it, while cleanup failure leaves
   `CleanupFailed` non-reusable.

### ACP failure migration

- Flip `prompt_may_have_been_accepted` immediately before installing the SDK prompt future.
- Start `prompt_stream` after future installation and before first poll/update.
- Migrate the generic prompt SDK/transport/kill-switch error and the distinct watchdog
  `AgentTimedOut` arm to structured fatal failures.
- Preserve initialize/auth/session/config failures with their actual phase and deepest sanitized cause.
- Route all 16 current ACP trace calls through the typed metadata-only funnel and remove free-form
  success/error fields.
- Extend `Supervised` with a bounded sequence-numbered process-scoped stderr ring. Text persistence is
  disabled by default; successful turns never persist stderr text.

### Retry and warm-session safety

- Workflow E6 and `bridge-controller::resilient::classify_death` consume typed disposition
  exhaustively.
- A post-barrier failure is never transient or replayed, including a same-poll send failure.
- One exhaustive warm-session classifier expires every `AgentFailure` for every class/disposition in
  `WarmNodeCleanup`, coordinator prompt/continue, inbound streaming, and inbound synchronous dispatch.
  Each owner synchronously sets the guard action before any await and uses the shared claim handoff. A
  later relaxation needs a new proof and review.
- Legacy `AgentCrashed` arms retain their prior behavior until separately migrated.

### Required tests

- full started/completed/failed transition grammar; no failed phase without a preceding start;
- initialize timeout cannot become authenticate; model rejection cannot become prompt;
- `pre_authenticated`, configured method, selected method, and no-methods skip survive store round trip;
- generic prompt failure and watchdog timeout are post-barrier/fatal under both E6 and `ResilientWarm`;
- production `WarmWorkflowNodeDispatcher`, coordinator prompt/continue, inbound streaming, and inbound
  synchronous dispatch expire, never finish, structured transport/process/timeout failures;
- deterministic cancellation before expiry lock acquisition, after tombstone/claim but before flight
  start, and after
  the cleanup task's first release side effect while provider removal is blocked never returns `Idle`,
  invokes release once, completes worktree/sidecar removal, drops the lease once, and adds no
  post-finalization journal event;
- checkout after claim and after first cleanup side effect sees `Expiring`, performs zero new
  resolve/configure/mint calls, and becomes eligible only after successful matching claim-id cleanup;
- cleanup failure leaves `CleanupFailed` non-reusable, and a stale flight cannot clear a newer tombstone;
- dropping the cleanup-report waiter proves the flight retains no observer/task-store reference; a
  retained waiter records the same bounded success/failure report once;
- a stale generation/operation expiry fallback cannot affect a newer handle for the same context;
- direct inbound streaming, synchronous inbound, fan-out local source, and coordinator prompt observe a
  cold resolution failure before translator/collection begins;
- those direct paths never attempt a task-journal write, while a real task-journal observer remains
  durable-first and fatal on write failure;
- a direct A2A warm workflow with a correlation `task_id` and no task row uses only an in-memory observer;
  a detached workflow with an existing row uses the journal factory in both cold and warm branches;
- cold workflow, warm workflow, and rebuilt warm backend emit the expected diagnostic path without
  losing or duplicating existing rich tool events;
- a backend/mock that implements only legacy `prompt`/`prompt_observed` remains source-compatible and
  preserves rich-event behavior through the composite default;
- constructor/prompt observer weak references drop before cached reuse; concurrent resolve waiters emit
  `backend.reused`, and later retirement/escalation produces no late task event;
- observed cleanup returns and records primary/secondary teardown failure correctly, while legacy cleanup
  defaults remain source-compatible;
- legacy and observed registry spawn constructors are both covered; the observed initialization reference
  is gone before cache reuse;
- `WorktreeBackend` observed cleanup propagates inner/provider/sidecar failures (`NotFound` excepted) and
  legacy cleanup remains best-effort;
- forced registry retirement racing observed worktree release joins the same per-session cell: one inner
  release, provider removal, and sidecar removal; `inner.retire` begins only after that cell finishes;
- transition auth/env identifiers colliding with the known credential are redacted in task-journal and
  smoke/artifact output, not only traces;
- one known credential injected through every dynamic input to all 16 current ACP trace events is absent
  from captured traces, and the source guard rejects a direct trace call outside the typed funnel;
- old process stderr is excluded by attempt cursor; concurrent lines remain labeled process-scoped;
- journal failure remains fatal and occurs before node/terminal completion.

### Live dogfood

After local gates and review, run one explicitly approved bridge-mediated read-only review. Preserve the
artifact. A failure is useful only if it demonstrates phase/cause retention; do not replay it
automatically.

## R2b3 — API/provider mapping and remaining container/dispatch observation

- **Branch:** `agent/reliability-r2b3-api-container`
- **Prerequisite:** R2b2 merged

### API work

- Override `ApiBackend::prompt_with_observers` while preserving rich-event forwarding.
- Set the acceptance barrier before installing each attempt's first HTTP send; keep it set through tool
  rounds.
- Bound non-success bodies at 64 KiB before JSON parsing.
- Implement the design's exact HTTP/ACP token/status/conflict table. Bare 429/503/529, incompatible
  status, fuzzy text, stderr prose, and conflicting fields remain `unknown`.
- Accept only the normative structured retry/reset fields and single `Retry-After`; hints are bounded,
  advisory, and never change disposition.

### Container and dispatch work

- Preserve `ContainerSpawn::spawn`, override defaulted `spawn_observed` in production, and have
  `ContainerRwBackend::prompt_with_observers` thread both channels through `open_inner` without storing
  the operation observer in `WarmInner`.
- Add the shared-state joinable `ReapController`: observed release awaits and returns one bounded reap
  result; `Drop`/registry retirement use detached start/join and never write late task diagnostics.
- Cover cold per-turn container creation, warm cache miss, and warm reuse (`backend.reused`).
- Complete any production prompt owner named by R2b0 that is not yet observed.
- Do not implement R2d fallback eligibility or host execution here.

### Required tests

- table-driven first/later send, status, body, SSE chunk/frame, non-streaming parse, timeout, and later
  tool-round failures;
- every post-installation failure is fatal and neither E6 nor warm respawn replays it;
- every recognized code/status pair and every conflict/unknown boundary;
- duplicate/malformed/out-of-range retry/reset metadata is omitted with a bridge-owned diagnostic code;
- provider-limit remains fatal and never routes to Sol/Fable/container fallback automatically;
- cold, cache-miss, and reuse container observer sequences;
- observed reap success, spawn failure, timeout, nonzero exit, concurrent joiners, and detached drop;
- full task-journal, CLI/operator artifact, trace, and A2A wire secret regressions.

## R2b completion gate

R2b is complete only when R2b0–R2b3 are merged and the R2 design's entire R2b test list is satisfied.
Record the final commit/PRs, exact suite totals, review verdicts, and unrun live gates in the central
roadmap. Then change the single next action to R2c.
