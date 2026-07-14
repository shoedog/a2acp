# R2b — Structured lifecycle diagnostics implementation plan

- **Status:** R2b0 MERGED at `11ebc402`; R2b1 MERGED at `7b788c1f`; R2b2 IN PROGRESS (2a `4ed12f1`; 2b `f40096df`; 2c `40790720`; 2d `14402f8`, closure review 12 `APPROVE`, pushed; exact **1,090 / 0 / 0**; full host workspace **1,806 / 0 / 12 ignored**; hygiene **37/7**; final full-R2b2 review pending); R2b3 NOT STARTED
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
   - preserve the exhaustively constructible public `WorkflowRunContext` and add explicit
     `WorkflowDiagnosticContext` entrypoints for per-node observer authority: detached tasks provide a
     journal-backed factory after task creation, while legacy/direct A2A entrypoints provide in-memory
     regardless of `task_id` or rich-sink presence;
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
   `TurnRunner`. Preserve `WorkflowRunContext` source compatibility and add explicit diagnostic-context
   entrypoints: direct/correlation-only paths always use in-memory observation, while detached tasks use a
   journal factory created only after the durable task row exists. Prove rich events are neither lost nor
   duplicated and journal failure is fatal before completion.
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

R2b2a implementation evidence (`4ed12f1035c16fa5dbd55169e59ca4c277373da4`):

- the first fresh Sol/xhigh review found one `WRONG/MAJOR`: cancellation or failure of the awaited
  journal write could leave volatile transition grammar advanced without its durable start; it also found
  one `SMELL/MINOR`: no secret-bearing `Debug` regression for the in-memory observer;
- the fold validates against a cloned grammar state, holds the per-observer ordering lock across the
  write, commits only after persistence, and adds deterministic write-error/cancellation and exact safe
  `Debug` tests;
- the fresh closure review marked both findings `FIXED`, found no new `WRONG` or `SMELL`, and returned
  `APPROVE`;
- exact post-fold gates: workspace check, warnings-denied all-target clippy, **1,640 passed / 0 failed /
  12 ignored**, release build, and repository hygiene (37 tracked artifacts / 7 example configs).

R2b2b implementation evidence (`f40096dfcfb43a37236ce5626fd362a16645f0fe`):

- lifecycle observation now spans ACP spawn, initialize, authentication, session creation, configuration,
  prompt start/stream/finish, and operation-owned cancellation, forget, and release. Process-scoped
  retirement deliberately retains no operation observer; it closes the connection fence before teardown.
  Post-barrier failures are fatal and never replayed. Bounded escalation and process-scoped retirement
  close the connection fence; warm expiry after other terminal failures remains R2b2d;
- process evidence is byte-bounded before retention, invalid UTF-8 is lossy and bounded, known credentials
  are re-sanitized when an existing child is adopted, and failure diagnostics retain only typed/redacted
  stderr metadata; all production ACP trace calls pass through one scalar-only typed funnel guarded by an
  AST regression;
- the first fresh Sol/xhigh review returned `REVISE` for eight items: empty initial redaction, observer
  failure abandoning accepted work, cancellable first-session initialization, missing observed
  cancel/forget teardown, unbounded stderr record reads, omitted pre-prompt stderr, a bypassable trace
  guard, and watchdog loss of a near-deadline SDK cause;
- the first closure review marked four `FIXED` and four `PARTIAL`, then found three new `WRONG/MAJOR`
  cases (short-secret/static-code collision, adopted-child redactor loss, and cancellation during slow
  completion observation) plus one `SMELL/MINOR` for missing byte-boundary/invalid-UTF-8 tests;
- the second fold separates trusted static diagnostic codes from dynamic fields without weakening
  redaction, installs and retroactively applies the adopted-process redactor, re-checks the unavailable
  fence after turn-lock acquisition, suppresses false cancellation escalation after the prompt route is
  gone, exercises observed cancel/forget persistence ordering, makes same-poll SDK completion win
  deterministically, and closes the trace-funnel expression/trait bypasses;
- the second closure review marked six items `FIXED` and two `PARTIALLY-FIXED`, then returned `REVISE` for
  one `WRONG/BLOCKER` (connection fencing was not atomic across sessions or retirement), two
  `WRONG/MAJOR` findings (pre-dispatch failures claimed possible acceptance; trace attributes/local
  wrapper macros bypassed the guard), and one `SMELL/MAJOR` (the public static-code builder still accepted
  runtime `String` codes). It also required a production-path `from_child` redactor test and found the
  cancel watcher could snapshot a live route just before slow completion persistence removed it;
- the third fold orders the final availability check plus SDK request installation against escalation
  and retirement with one connection-wide no-await gate, gives each live route an atomic terminal claim,
  reports every pre-barrier prompt failure as not accepted, rejects tracing attributes and local
  tracing-expanding macros, structurally separates trusted `&'static str` codes from dynamic inputs, and
  drives the actual `from_child` path with stderr both before and after adoption;
- the third closure review marked three items `FIXED` and three `PARTIALLY-FIXED`, then returned `REVISE`
  for three new `WRONG/MAJOR` cases: adopted-child initialize failures lost available stderr metadata,
  pre-dispatch observed cancel/release failures falsely claimed possible acceptance, and `{cwd}`-expanded
  MCP env credentials were not in the redaction set. It also found the dispatch gate lacked a
  mutation-sensitive mutual-exclusion test, nested `AcpTraceEvent`/`extern crate` aliases still bypassed
  the trace guard, two pre-dispatch branches lacked direct coverage, and two handoff status claims needed
  correction;
- the fourth fold drives failing `from_child` initialization through the same process-scoped evidence,
  carries accepted-work state in typed teardown failures, removes the unnecessary connection dependency
  from already-minted session reuse, mutation-locks both sides of the dispatch gate, rejects nested funnel
  lookalikes and extern-crate aliases, and seeds host/container redactors with raw plus effective MCP env
  values after `{cwd}` substitution. Retirement ownership and aggregate R2b2 status are now described
  consistently;
- the fourth closure review marked five items `FIXED` and three `PARTIALLY-FIXED`, then returned `REVISE`
  because a root-level funnel lookalike in a sibling source file remained trusted, teardown acceptance was
  derived from pre-barrier route liveness, and the ACP lifecycle redactor did not know MCP secrets expanded
  with a per-session cwd. It also found one `WRONG/MINOR` handoff overclaim and one `SMELL/MINOR` gap in the
  central `AgentFailure` constructor guard;
- the fifth fold qualifies the typed trace funnel by its owning source file, stores a distinct accepted bit
  only after SDK request installation under the dispatch gate, derives session/session-operation lifecycle
  redactors from the effective cwd, narrows the handoff to the fences actually closed, and mutation-locks
  `BridgeError::agent_failure` to `AcpLifecycle::failure`;
- the fifth closure review marked four items `FIXED` and the effective-cwd redaction item
  `PARTIALLY-FIXED`, then returned `REVISE`: config replacement could split the redactor snapshot from
  the mint snapshot, and a failed deferred cancel delivery after mint remained transient legacy
  `AgentCrashed` after the observed cancellation had been reported complete;
- the sixth fold makes one immutable session snapshot own cwd/model/mode/effort plus lifecycle redaction,
  retains every attempted and minted cwd for later operation redaction, reports a pre-mint observed cancel
  as latched rather than delivered, and maps a failed deferred delivery to structured fatal pre-dispatch
  failure. Deterministic regressions cover config replacement, minted-cwd teardown, latched observation,
  and the deferred failure mapping;
- the sixth closure review marked both inherited items `PARTIALLY-FIXED`: tests bypassed the production
  `prompt_inner` snapshot and deferred-send call sites. It also found one `WRONG/MAJOR` unbounded attempted
  cwd vector retained across failed retries/forget and one `SMELL/MINOR` because deferred R2f incident work
  crossed the R2b2b branch boundary;
- the seventh fold replaces the vector with one RAII-cleared active-attempt cwd plus the immutable minted
  cwd, exercises config replacement after `prompt_inner` snapshot capture, injects failure at the actual
  initializer send call, and covers active-attempt redaction plus failed-retry cleanup. The R2f material is
  preserved separately at `ae4a569` on `agent/reliability-r2f-incident-evidence` and removed from this tree;
- the seventh closure review marked all four inherited items `FIXED`, then returned `REVISE` for two new
  `WRONG/MAJOR` cases: cancellation delivery could sample `accepted` before a concurrent prompt crossed
  the barrier, and the process-scoped stderr ring never learned MCP credentials expanded with a
  per-session cwd;
- the eighth fold performs route lookup plus synchronous cancel delivery under the same no-await dispatch
  gate as prompt installation, closes the fence and samples acceptance before releasing that gate on
  failure, and mutation-locks both orderings including the route-not-yet-published case. Before each real
  `session/new`, it atomically installs a process redactor containing the raw template plus every active or
  minted live-session cwd expansion; deterministic two-session coverage proves the first credential is not
  dropped when the second session mints, while the process-ring regression proves replacement sanitizes
  retained and future exact values;
- the eighth closure review marked both inherited items `PARTIALLY-FIXED`: an SDK-terminal route is removed
  before slow completion observation finishes, so cancellation failure could lose already-accepted
  evidence, and caller cancellation while process-redactor installation waited on the sessions lock could
  land before the active-cwd RAII guard existed;
- the ninth fold moves acceptance evidence from routing lifetime to one operation-scoped per-session slot
  owned by the turn driver and cleared before its turn lock releases. It also constructs the active-cwd
  guard immediately after publication, before the first redactor await, then moves that guard into the
  shielded initializer. Deterministic regressions inject cancel-send failure after route removal during
  slow completion observation and abort initialization while the sessions lock blocks redactor installation;
- the ninth closure review marked both inherited items `FIXED`, then returned `REVISE` for one new
  `WRONG/MAJOR`: a credential-bearing session/config attempt could fail before `minted_cwd` publication,
  and a later finite redactor replacement could forget that delivered value before delayed process stderr;
- the tenth fold adds a monotonic process-ring metadata-only mode: entering it retroactively replaces all
  retained text, future lines retain only count metadata, and later redactor replacement cannot re-enable
  text. A mint evidence guard arms immediately before `session/new` installation and commits only after
  minted cwd/id publication; uncertain error/abort/unwind enters metadata-only. Session removal does the
  same under the sessions lock because ACP has no close acknowledgement. Regressions cover failed mint to
  successful next mint, normal release, and literal stderr captured before and after policy replacement;
- the tenth closure review marked the inherited item `FIXED`, found no new `WRONG` or `SMELL` across the
  complete 12-file tree, and returned `APPROVE`. It confirmed metadata-only monotonicity, mint/release
  boundary coverage, bounded sequence/count retention, and the R2b2c/R2b2d/R2f exclusions;
- current focused gates pass: bridge-acp library **183 passed / 0 failed**, bridge-container **24 / 0**,
  the host and core MCP redaction regressions, R2b1 diagnostics **20 / 0**, process lifecycle **13 / 0**,
  and warnings-denied all-target Clippy for every changed crate. The approved exact code tree also passes
  workspace check, workspace/all-target warnings-denied Clippy, **1,700 passed / 0 failed / 12 ignored**
  across 46 test executables, the release binary build, format/diff checks, and repository hygiene
  (**37** tracked artifacts / **7** example configs). The ignored set remains authenticated Kiro/two-bridge
  and local Ollama coverage. R2b2b is committed and pushed at
  `f40096dfcfb43a37236ce5626fd362a16645f0fe`, but remains unmerged until R2b2a-d and the full branch review
  gate complete.

R2b2c implementation handoff (committed and pushed at
`407907202982d732c2395be0f6319f6029622f82`, based on
`f40096dfcfb43a37236ce5626fd362a16645f0fe`):

- `Translator::run_observed`, `SessionManager::{checkout_turn_observed,checkout_child_turn_observed}`,
  `WorkflowNodeDispatcher::checkout_observed`, and `TurnRunner::run_turn_observed` are additive seams.
  Concrete legacy owner entrypoints use grammar-valid no-op observers, while public trait compatibility
  defaults delegate the observed call back to the legacy implementation;
- direct inbound streaming, synchronous, and unary/streaming fan-out owners construct bounded in-memory
  observers before cold resolution and carry the same `Arc` into prompt ownership. Coordinator
  prompt/continue and the production implement turns do the same. The worktree decorator forwards the
  exact composite observer pair;
- the additive `WorkflowDiagnosticContext` wrapper owns an explicit `DiagnosticObserverFactory` while
  legacy `WorkflowRunContext` literals remain source-compatible. Cold execution creates one observer per
  node attempt before resolution; warm execution creates it before child checkout and passes the same
  value to `prompt_with_observers`. Legacy/direct A2A entrypoints select an in-memory factory regardless of
  a correlation `task_id`;
- detached execution constructs `TaskJournalDiagnosticObserverFactory` only after the operation id and
  existing durable task row are proven, then overwrites caller context with that explicit authority. A
  missing row prevents any backend prompt. A diagnostic journal write failure propagates before prompt and
  leaves a single durable failed terminal rather than a completion;
- mutation-sensitive coverage proves exact observer identity at resolution/checkout/prompt for inbound,
  coordinator, workflow, warm-child, and rebuilt implement paths; one observer per cold retry attempt;
  rich-event preservation and flush counts in cold, retry, warm, and worktree-decorator paths; direct warm
  correlation authority; and journal-backed cold plus warm workflow authority;
- the first fresh bridge-mediated Sol/xhigh review inspected every changed path, found no untracked files,
  and returned `REVISE` for one `WRONG/MAJOR` and one `SMELL/MAJOR`: cancellation during a warm prompt-open
  could discard an already-recorded rich event without flushing, and the production edit/fix callsites
  were not mutation-locked against legacy `run_turn`. The fold flushes once before canceled completion and
  routes both callsites through one observed-only helper tested with a runner that panics on legacy use;
- self-audit closed two additional owner/error-precedence gaps. The non-task ACP catalog probe now carries
  one bounded in-memory observer across `spawn_observed` and `describe_options_observed`; discovery emits
  typed session-create start/completed/failed transitions and an observer write failure prevents the ACP
  request. A canceled `Done` remains the primary warm outcome if rich flush also fails instead of being
  overwritten by the secondary store error. Deterministic tests cover both cases;
- focused new tests pass. The review fold passes workspace check and workspace/all-target Clippy with
  warnings denied. Closure review 2 marked both inherited findings `FIXED`, verified the two self-audit
  folds, and returned `REVISE` for one new `WRONG/MAJOR`: the cold prompt-open branch still constructed its
  rich sink inside the cancellation race and could discard an already-recorded event. The fold hoists the
  cold sink outside the race, flushes once before cancellation cleanup, and adds a deterministic backend
  that records before signaling cancellation; its exact one-event/one-flush regression passes. A complete
  affected-crate run passed **912 tests** while three unchanged `bridge-core::process` tests failed only at their
  pre-teardown liveness preconditions under parallel executable load; the entire process module then passed
  **13 / 0** serially, including those three. No baseline was changed;
- closure review 3 marked all three inherited findings `FIXED` and both self-audit folds verified, then
  returned `REVISE` for two `WRONG/MAJOR`, one `SMELL/MAJOR`, and one `WRONG/MINOR`: a new required public
  context field broke exhaustive downstream literals; detached checkpoint failure could drop a pending
  sibling rich event; warm inbound/catalog production seams lacked mutation locks; and the roadmap cursor
  was stale. The fold introduces additive `WorkflowDiagnosticContext` entrypoints plus an external
  exhaustive-literal compile regression, preserves legacy entrypoints with bounded in-memory authority,
  cancels and fully drains detached siblings after the first sink error, and proves the real two-root
  checkpoint race persists the sibling rich event before terminal failure. New exact-identity regressions
  exercise both warm inbound branches and the production catalog owner. Focused tests pass. Sol/xhigh
  closure review 4 adjudicated all seven inherited findings `FIXED`, verified both self-audit folds, and
  found only one `WRONG/MINOR`: this plan's top status still claimed 2b review was pending. The header now
  records 2b committed at `f40096df`, 2c at its current fold, and 2d not started. Closure review 5 marked
  that finding `FIXED`, found no code/test defect, and reported one `WRONG/MINOR` exact-cursor mismatch:
  the roadmap table used `f40096d` while its other cursors used `f40096df`. The table now uses the exact
  prefix. Closure review 6 marked that finding `FIXED`, found no code/test defect, and reported one
  `WRONG/MINOR`: the roadmap's older Current handoff sentence still said the first 2c review was next.
  Closure review 7 marked that inherited finding `FIXED`, read the complete 16-file base diff, found no new
  code/test defect or cursor contradiction, and returned `APPROVE`. The exact tree passes format/diff
  checks, workspace check, workspace/all-target warnings-denied Clippy, **1,725 passed / 0 failed / 12
  ignored** across 47 test binaries under serial execution, the release binary build, and repository
  hygiene (**37** tracked artifacts / **7** example configs). This full serial run clears the three
  unchanged process-fixture precondition failures seen only under the earlier parallel affected-crate run.
  No live/billable gate was run. Commit/push 2c, then begin 2d;
- R2b2d exclusively owns observed cleanup methods, structured-failure warm expiry, completion guards,
  tombstones/claims, cleanup flight, and worktree release/retire single-flight. Its local implementation
  and evidence are recorded below. R2b3 still owns API and container adapter diagnostic implementation.
  R2f still owns phase-aware stagnation/takeover.

R2b2d implementation handoff (review-approved branch commit based on
`407907202982d732c2395be0f6319f6029622f82`):

- `warm_session_survivability` exhaustively expires every structured failure class/disposition while
  preserving each owner's legacy policy. Inbound streaming/unary, coordinator prompt/continue, and warm
  workflow cleanup synchronously arm one sticky completion guard before formatting, channel sends,
  persistence, or cleanup awaits. Ready backend results beat simultaneous workflow cancellation or stream
  receiver close after prompt ownership; force-reset abort remains first.
- `SessionTable` separates live handles from claim-id `Expiring`/`CleanupFailed` tombstones. Exact
  generation/operation claims synchronously transfer the backend session, lease, outstanding operation
  abort tokens, and child-cleanup ownership into detached cleanup. Claim drop starts an unstarted flight;
  waiter cancellation detaches a started one; any whole-worker panic or surfaced task `JoinError` uses a
  retained exact-claim settlement capability to become a bounded cleanup failure; only the matching
  successful claim clears its tombstone. Explicit release, reap, cancel-error, and all registered
  children are claimed before their first cleanup await. A failed flight drops the warm handle and lease
  but retains one adjacent backend/session retry capability; release/clear atomically reclaim it under a
  new claim id, failure restores it, and waiter cancellation detaches from the retry flight.
- Cancel settlement uses an owned future that is polled once inline. Immediate success preserves existing
  SessionCancel/busy-token ordering; a pending future is moved intact into a task, so waiter cancellation
  cannot strand `Cancelling` or issue a second backend cancel. A panic expires and releases the handle.
- `NodeTurnCleanup::on_exit_observed` is additive and source-compatible. Workflow owners settle observed
  cleanup before `TurnFinished`/`NodeFinished`; cleanup failure becomes primary only without an earlier
  backend/rich failure. Additive synchronous `arm_exit` makes structured expiry sticky at the prompt-open
  or stream-error match site before a cancellable rich-sink flush. At prompt-open and stream-drain ownership
  boundaries, a simultaneously ready concrete backend result precedes workflow cancellation; a pending
  backend future remains cancelable. On expiry, `WarmCompletionGuard` consumes its claim into the detached
  observer-free flight before teardown-start persistence and joins that report before returning even when
  the start observation fails. The shared operation observer remains outside detached cleanup.
- `WorktreeBackend` owns sealed per-session shared flights. A synchronous cell/report selection starts or
  joins before the first await; equal callers receive one report, a stronger `release` installs a detached
  monotonic upgrade, completed components are not repeated, and an explicit post-failure call retries only
  incomplete work. Every successful configure retains its cell until cleanup, and admission, retirement
  seal, and successful eviction linearize under the cell-map lock. Cleanup cells retain `WtEntry` until
  provider and sidecar completion, including ownerless reservation, partial add, sidecar-write, and inner-
  configure failures. Reservation publication arms cleanup-on-drop before provider/sidecar/inner awaits;
  returned failure or cancellation marks the same cell and synchronously starts detached Release after
  balancing admission. Failed compensation re-arms release in that flight slot with bounded backoff, while
  explicit release or retirement can replace it by exact flight id. A takeover retains the maximum existing/
  requested strength, so Forget cannot erase failed Release. Degraded cleanup rejects new allocation before
  provider add, and a 64-admission circuit breaker bounds the already-in-flight wave. Retirement seals all
  configure paths, waits admitted calls, joins cleanup cells, then
  calls `inner.retire`. Observed callers record started/completed/failed locally; observer persistence
  failure is fatal but does not suppress cleanup.
- Max review 1 returned `REVISE` for four `WRONG` R2b2d schedules: workflow drop during rich flush used the
  guard's default cancel action; release could pass configure between admission and reservation publication;
  `HostGitWorktree::remove` discarded both command statuses; and successful cells accumulated by distinct
  session id. Each schedule was reproduced red before its fold. The implementation now arms the warm error
  synchronously, shares a pre-await per-session configure/cleanup lifecycle cell, waits every admitted git,
  non-git, and no-cwd configure before teardown, reports incomplete real git cleanup while treating an
  already-absent target plus successful prune as idempotent success, and token-evicts the exact successful
  flight before its report becomes visible. Failure retains component state; an older completion cannot
  evict a stronger replacement. The review's inherited claimed-state/sweep-cancellation concern was tagged
  `SMELL` and remains in R2f.
- Deterministic regressions first failed for: multi-child waiter cancellation (**1** release, expected
  **3**); provider gate receiver dropped with the report waiter; inner release preceding a reserving
  configure; concurrent waiters seeing different failure/retry reports; canceled stronger upgrade; warm
  cancel gate receiver dropped with the completion waiter; and a ready backend error losing to simultaneous
  cancellation/receiver close. Each now passes with exact count/state/order assertions plus stale-claim,
  cleanup-failure/panic, post-seal, non-git/no-cwd, and observer-lifetime edges.
- Debugging the affected-crate gate followed the hypothesis/probe log. A silent `workflow_producer` process
  localized to `session_cancel_keeps_context_busy_until_producer_exits`; the suspected child-lock cycle was
  ruled out. Temporary phase markers proved SessionCancel returned but the second request was admitted and
  blocked on its fake prompt. Root cause was the unconditional spawn yield in cancel settlement. Inline
  first-poll plus transfer-on-pending restored the existing ordering while retaining detached ownership;
  the full producer binary passes **50 / 0** and the markers were removed.
- Closure review 1 marked the workflow-arm and pre-reservation-admission findings `FIXED`, then returned
  `REVISE`: absent-target success did not prove exact Git registration absence, and retirement could miss a
  post-eviction release generation. The fold makes Git cleanup success require target metadata absence,
  successful prune, and byte-exact absence from `worktree list --porcelain -z`; metadata/list/prune failure
  fails closed, and git subprocess cancellation kills its child. Sealed retirement retains completed cells
  for late known owners, while unknown post-seal cleanup cannot create unbounded cells. A gated retirement-
  first test was red at two inner releases and is green at one.
- Self-audit additionally reproduced release parked forever after its configure owner was canceled during
  provider add. `Reserving` now stores configure-owner identity plus cleanup metadata; lifecycle admission
  tracks active identities, wakes peer configures on owner drop, and lets release, a peer, or retirement take
  the orphan only after all admitted configuration settles. The prior reservation test acknowledgement was
  moved to that real wait boundary. Two exact stale worktree test processes from earlier yielded commands
  were terminated; a new complete run emitted **38 / 0 / 0** and no stale process remains.
- Closure review 2 adjudicated Git final-state proof and the repaired harness `FIXED`, configure cancellation
  `PARTIAL`, and the retirement fold `NOT FIXED`, then returned `REVISE` with four present-slice failures:
  a known release could fall through between seal and retirement's cell snapshot; ownerless reservation
  cleanup lost `WtEntry` after provider failure; prompt-open cancellation preceded an already-ready
  structured error; and `CleanupFailed` made the backend's failed cleanup cell unreachable to release/clear.
  It also identified stale gate wording and retained-registration command injection as a documented coverage
  limitation, while leaving the generic reset/reconcile/compact/cancel sweep in R2f.
- Each closure-2 failure was reproduced deterministically before its fold. Configure success now retains a
  bounded cell and seal/admission/eviction share one lock boundary; failed cleanup retains exact worktree
  metadata through retry; prompt-open polls the concrete backend result first; and `CleanupFailed` retains a
  minimal retry owner consumed by explicit release/clear. Additional edges prove partial add, sidecar-write,
  and inner-configure cleanup retry; repeated manager retry failure; and retry-waiter cancellation.
- Closure review 3 marked ownerless metadata, prompt-open precedence, retry capability, and docs `FIXED`,
  but returned `REVISE` with three worktree blockers. A later failed/canceled configure could remove a cell
  retained by an earlier successful no-cwd configure; configure rejection after cleanup start decremented an
  unincremented global counter from zero to `u64::MAX`; and observed cleanup awaited `teardown started`
  persistence before selecting its flight. All three schedules were reproduced red.
- Cell lifecycle now persists configured ownership independently of transient admissions; rejected admission
  leaves the global counter untouched; and observed cleanup synchronously starts/joins the observer-free
  flight before its first diagnostic await. Canceled and returned-error admissions retain the known cell;
  retirement completes after rejected configure; and canceling a pending observer write drops the observer
  while provider cleanup continues. The complete worktree package passes **47 / 0 / 0**.
- Closure review 4 marked all three closure-3 blockers `FIXED`, then returned `REVISE` for the analogous
  outer warm-guard boundary: it claimed the session but awaited teardown-start persistence before spawning
  cleanup, and an immediate observation error returned before detached cleanup settled. Pending and
  immediate-error schedules reproduced both failures red. `ExpiryClaim::into_flight` now synchronously
  transfers ownership; `WarmCompletionGuard` starts that flight before its diagnostic await and joins it
  before returning observation failure.
- Closure review 5 marked the observer-ordering fold `FIXED`, then returned `REVISE` because a public lease
  destructor panic after checked backend release escaped the release-only unwind boundary. The resulting
  `JoinError` lacked generation/operation/claim identity and left an unrecoverable `Expiring` tombstone.
  Deterministic lease-panic and task-abort tests first reproduced the stuck state. `CleanupFlight` now keeps
  an exact-claim settlement capability in the whole-worker recovery and joiner; either failure installs one
  bounded backend/session retry owner only if the same claim still owns the tombstone. Both former-red
  schedules pass through explicit retry.
- Closure review 6 marked the claim-aware worker/joiner fold `FIXED` but found a separate
  `WRONG/BLOCKER`: partial Worktree configuration plus failed compensation retained exact metadata after
  warm, direct inbound, fan-out, and workflow callers dropped their only owner. Same-session configure was
  rejected and distinct failures could accumulate. A deterministic production-owner regression reproduced
  the second allocation. Failed configuration now marks its cell before admission drops; the reporter
  re-arms exact release with exponential backoff and yields by flight id to explicit release/retirement.
  Degraded cleanup rejects new allocation before provider add, while a 64-admission circuit breaker bounds
  the pre-marker wave. The retry performs exactly one additional provider removal and does not repeat the
  completed inner release. The review's `SMELL/MAJOR` test gap is also closed: a worker-only lease-panic test
  discards the joiner capability before allowing the panic. The complete worktree package passes
  **49 / 0 / 0** and coordinator passes **228 / 0 / 0**.
- Closure review 7 marked worker-only panic recovery `FIXED`, but returned `REVISE` for two Worktree
  blockers. Cancellation after reservation/provider side effects dropped an unmarked admission before any
  reporter existed, so sequential orphans bypassed the capacity bound. A completed failed Release slot also
  allowed weaker Forget takeover, which could clear the marker and stop Release recovery. Both schedules
  reproduced red. Reservation publication now arms a destructor fallback; admission drop marks the cell,
  balances counts, and synchronously starts observer-free Release. Failed-slot replacement keeps maximum
  strength. The cancellation regression proves cleanup without manual release, distinct-allocation rejection,
  exact provider retry, and no repeated inner release; the takeover regression requires two Releases and zero
  Forgets. The autonomous-retry test now directly asserts exactly two provider removals and one inner release,
  closing review 7's proof gap. The complete worktree package passes **51 / 0 / 0**.
- Closure review 8 marked the standalone cancellation/strength folds `FIXED`, but returned `REVISE` for a
  cross-flight race: pending Forget superseded by destructor-owned Release could report success and clear the
  failed-config marker before checking flight identity. A later Release failure then lost admission closure
  and automatic retry. The combined schedule reproduced red. Success finalization now requires exact current
  flight ownership; when failed-config cleanup is pending, the current slot must also be Release strength.
  Stale/weaker success reports to its own waiter but cannot clear/evict shared state. The regression requires
  pending Forget, canceled configure, failed Release, distinct-allocation rejection, two Releases, one Forget,
  one provider removal, and automatic exact-cell recovery. Worktree passes **52 / 0 / 0**.
- Closure review 9 marked closure 8's exact-flight finalization and Release satisfaction `FIXED`, then returned
  `REVISE` for a cross-owner structured-failure/`SessionCancel` race. Failure observation could arm expiry
  locally, cancel could settle the same operation to `Idle`, and the later exact claim would no-op. Both
  in-flight and already-completed cancel settlement orders reproduced red. Every warm turn now shares one
  opaque exact-operation expiry intent between `WarmCompletionGuard` and its retained turn record.
  `observe_exit` publishes it synchronously; cancel settlement converts it to expiry, `Cancelling` claims
  accept an exact deferred-expiry handoff, and only the exact armed retained operation may claim a settled
  `Idle` handle. The existing stale-operation test keeps a newer turn untouched. Coordinator passes
  **230 / 0 / 0**.
- Review 9's major test-proof smell is folded with a deterministic state that keeps the exact current slot at
  Forget while a failed-config marker is present. Forget success preserves the marker/cell; a temporary
  predicate mutation accepting Forget makes the regression fail. A marker-free Forget control proves exact
  success eviction. Worktree passes **54 / 0 / 0**.
- Closure review 10 marked the Worktree fold and mutation proof `FIXED`, then returned `REVISE` for two
  production schedules. Cancel could restore A to `Idle`, A could arm expiry afterward, and B could check out
  before A acquired the table; both existing-context and ordinary checkout reproduced the poisoned reuse.
  `WarmExpiryIntent` now atomically chooses `armed` or `successor_reserved`. Failure-first checkout claims one
  detached expiry and returns `SessionExpired`; successor-first makes a late stale arm inert. Both former-red
  paths and a direct two-order atomic control pass. Coordinator passes **233 / 0 / 0**.
- Repeated biased data priority also let 128 always-ready usage items postpone already-ready workflow cancel
  or inbound disconnect through the whole prefix. The next item still wins once, so queued structured errors
  retain precedence; every benign workflow item and inbound usage item checks control before another data
  poll. Inbound closed-select, usage, and failed-send paths share one canceled finalizer. Both former-red tests
  now consume one item, and the prior error-precedence/finalization controls remain green. Workflow passes
  **76 / 0 / 0** and inbound passes **263 / 0 / 0**.
- Closure review 11 adjudicated both production folds and every inherited implementation/test surface
  `FIXED`. Its only `WRONG/MINOR` was that the design header, roadmap top/table, and this plan header did not
  all state closure 10, exact **1,090 / 0 / 0**, and the full-workspace/hygiene pending boundary. Those
  authoritative summaries now agree. The retained Git fixture and bounded-yield polling remain minor smells;
  review 11 found no open code or test defect.
- Closure review 12 used a fresh Sol/xhigh read-only instance for the documentation-only fold. It confirmed
  that no production/test path changed after review 11, adjudicated the design/roadmap/plan summaries
  consistent, retained only the two accepted minor coverage debts, found no new `WRONG`, and returned
  `APPROVE`.
- The post-fold exact six-package gate passes **1,090 / 0 / 0 ignored**. Format and diff checks, workspace/
  all-target check, warnings-denied workspace/all-target Clippy, and the workspace release build are clean on
  the same tree. The managed-sandbox full run stopped at **268 / 14** because 12 Wiremock tests received OS
  port `PermissionDenied` and two file-watch tests timed out. The identical host serial command passes
  **1,806 / 0 / 12 ignored** across 64 terminal result groups. Repository hygiene passes at **37** tracked
  artifacts / **7** validated example configs. The Git command-fixture limitation and bounded yield polling
  remain documented minor test-coverage follow-ups. No docs-link checker was found and no live/billable gate
  ran. R2b2d is pushed at `14402f895a5eda2852684a8fbd35f83452e2645f`. Run the final full-R2b2
  review; do not merge before that approval.

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
   `CleanupFailed` non-reusable and retains one adjacent bounded backend/session retry capability. Explicit
   release/clear consumes that capability into a new claim-id flight; failure restores it, and success clears
   it without retaining the warm handle, lease, observer, or task store.

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
- explicit release and clear retry a recovered `CleanupFailed` backend/session capability; repeated failure
  restores it, and cancellation of the retry waiter does not cancel the owned flight;
- a ready prompt-open structured failure beats cancellation made ready after the eager precheck, while a
  pending prompt-open future remains immediately cancelable;
- configure success publishes a retained per-session cleanup cell before retirement seal; a known release
  between seal and retirement snapshot joins that cell/report, while unknown post-seal ids remain bounded;
- a later failed and a later canceled same-session configure cannot erase an earlier successful no-cwd
  configured cell; configure rejected after cleanup start leaves the global admission count exactly zero
  and retirement completes after the shared flight;
- ownerless reservation, partial provider-add, sidecar-write, and inner-configure cleanup failures retain
  canonical source/path until provider and sidecar completion, and retry only incomplete components;
- failed configuration plus failed compensation retains a backend-owned retry in the same exact flight slot,
  blocks a distinct allocation before provider add, bounds the admitted wave at 64, and automatically clears
  the cell without an external session owner once the provider recovers;
- cancellation after reservation/provider publication starts that same owned Release from admission drop,
  blocks distinct allocation, and resumes only incomplete provider cleanup after recovery;
- a Forget takeover of completed failed Release retains Release strength and cannot clear the marker until
  the stronger inner action succeeds;
- a pending Forget superseded by cancellation-owned Release cannot clear the failed-config marker; Release
  failure keeps distinct admission closed and automatic retry finishes only the missing stronger component;
- dropping the cleanup-report waiter proves the flight retains no observer/task-store reference; a
  retained waiter records the same bounded success/failure report once;
- canceling a pending `teardown started` observer write drops the observer but cannot suppress the already-
  claimed observer-free provider/sidecar cleanup flight;
- outer warm completion starts backend release while teardown-start persistence is pending, and an immediate
  start-observation error cannot return before its gated cleanup report settles;
- a lease-destructor panic after checked backend release and an explicitly aborted cleanup task each settle
  only the exact tombstone to `CleanupFailed`, retain one retry capability, and clear on explicit release;
- dropping the joiner's settlement capability before a lease-destructor panic still lets the raw worker task
  catch the panic, publish `CleanupFailed`, and clear on explicit release;
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
