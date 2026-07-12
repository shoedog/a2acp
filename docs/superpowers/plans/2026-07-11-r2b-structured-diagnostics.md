# R2b — Structured lifecycle diagnostics implementation plan

- **Status:** NOT STARTED
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

## R2b0 — close the two remaining ownership/checklist ambiguities

- **Branch:** `agent/reliability-r2b0-contract`
- **Type:** docs-only contract patch
- **Exit:** reviewed design revision with no unresolved production owner

### Required design edits

1. Assign direct prompt ownership explicitly:

   - add an additive `Translator::run_observed` path in
     `crates/bridge-core/src/translator.rs`; the legacy `run` delegates with a no-op observer;
   - `crates/bridge-a2a-inbound/src/server.rs::spawn_local_producer` owns the task-bound observer for
     normal streaming `message/send`;
   - the synchronous local dispatch arm in `server.rs` owns the same task-bound observer;
   - `server.rs::local_kiro_source` receives a source/task-bound observer for fan-out local work;
   - `crates/bridge-coordinator/src/coordinator.rs::collect_turn` owns the observer for coordinator warm
     turns because it mints the prompt task id;
   - non-task catalog probes such as `AcpBackend::describe_options` use an in-memory/no-op observer and
     never invent task ownership.

2. Enumerate every current raw ACP SDK logging site that R2b must replace:

   - the three effort walk-down warnings in `AcpBackend::apply_effort_with_fallback`;
   - initialize handshake failure in `AcpBackend::connect`;
   - session mint failure in `AcpBackend::ensure_session`;
   - the second `session/new` failure in `AcpBackend::describe_options`.

   All six sites must use bridge-owned phase/code/numeric metadata only. SDK `Debug`, `Display`, message,
   and data never become tracing fields before diagnostic redaction.

3. Add the R2b0 disposition to the v6 review trail and update the program cursor's next action to R2b1.

### R2b0 verification

- Search the whole repo for `translator.run(` and `inspect_err`/`error = ?e`; the contract must account
  for every production hit or explicitly classify it as no-op/non-task.
- Run repository hygiene and the full workspace suite even though the patch is docs-only.
- Fresh adversarial design review; verdict must be `READY`/`APPROVE` before R2b1.

## R2b1 — diagnostic foundation and rollback-safe persistence surface

- **Branch:** `agent/reliability-r2b1-diagnostic-foundation`
- **Behavioral boundary:** types and persistence/projection compatibility; no production failure site
  constructs `AgentFailure` yet

### Production changes

1. Add `crates/bridge-core/src/diagnostics.rs` and export it from `bridge-core`:

   - `DiagnosticPhase`, `PhaseStatus`, `DiagnosticFailureClass`, `FailureDisposition`;
   - typed `DiagnosticOperation`, `DiagnosticCode`, `AuthenticationEvidence`, `StderrScope`, and
     `StderrRedaction`;
   - private-field `PhaseTransition`, `PersistedPhaseTransition`, `DiagnosticEvent`, and
     `FailureDiagnostic`;
   - validating builders, schema version 1, append-only serialized vocabularies, all bounds from the
     design, and custom safe `Debug`.

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
- `Display`, `Debug`, `client_message`, and wire serialization cannot expose diagnostic secrets;
- prior-schema `Progress { text }` fixture reads a new `diagnostic: None` payload and a prior reader
  ignores the optional new field;
- diagnostic progress persists, produces no live/reattach frame, never panics, and is followed by normal
  node/terminal projection;
- exhaustive class/disposition/metrics tables fail to compile or test when a variant is unhandled;
- no production constructor of `AgentFailure` exists yet outside fixtures/builders.

### Merge gate

R2b1 may merge independently only if legacy runtime behavior is unchanged and rollback fixtures pass.
Do not begin error-site migration on the same branch.

## R2b2 — ACP/Fable lifecycle evidence, journaling, and no replay

- **Branch:** `agent/reliability-r2b2-acp-lifecycle`
- **Priority:** first end-to-end value; directly addresses the observed Fable failure opacity

### Observation plumbing

1. Add the bridge-owned `DiagnosticObserver` port and no-op implementation.
2. Add `AgentRegistry::resolve_observed`; the default delegates to `resolve`.
3. Thread one attempt observer through concrete registry resolution and `AcpConfig` into:

   - process spawn/transport connection;
   - initialize;
   - authenticate/skip evidence;
   - `session/new`;
   - mode/model/effort configuration;
   - prompt start, prompt stream, prompt finish;
   - teardown.

4. Implement the R2b0 ownership map:

   - `Translator::run_observed` plus all three inbound translator owners;
   - coordinator `collect_turn`;
   - warm workflow executor's `NodeTurn.backend.prompt_observed` path;
   - additive `TurnRunner::run_turn_observed`, overridden by `ResilientWarm` for the first and rebuilt
     backend;
   - detached rich sink persistence with total optional projection.

### ACP failure migration

- Flip `prompt_may_have_been_accepted` immediately before installing the SDK prompt future.
- Start `prompt_stream` after future installation and before first poll/update.
- Migrate the generic prompt SDK/transport/kill-switch error and the distinct watchdog
  `AgentTimedOut` arm to structured fatal failures.
- Preserve initialize/auth/session/config failures with their actual phase and deepest sanitized cause.
- Remove all six R2b0 raw SDK trace sites and replace them with metadata-only logging.
- Extend `Supervised` with a bounded sequence-numbered process-scoped stderr ring. Text persistence is
  disabled by default; successful turns never persist stderr text.

### Retry and warm-session safety

- Workflow E6 and `bridge-controller::resilient::classify_death` consume typed disposition
  exhaustively.
- A post-barrier failure is never transient or replayed, including a same-poll send failure.
- `WarmNodeCleanup` expires every `AgentFailure` for every class/disposition. A later relaxation needs
  a new proof and review.
- Legacy `AgentCrashed` arms retain their prior behavior until separately migrated.

### Required tests

- full started/completed/failed transition grammar; no failed phase without a preceding start;
- initialize timeout cannot become authenticate; model rejection cannot become prompt;
- `pre_authenticated`, configured method, selected method, and no-methods skip survive store round trip;
- generic prompt failure and watchdog timeout are post-barrier/fatal under both E6 and `ResilientWarm`;
- production `WarmWorkflowNodeDispatcher` expires, never finishes, structured transport/process/timeout
  failures;
- direct inbound streaming, synchronous inbound, fan-out local source, coordinator warm turn, cold
  workflow, warm workflow, and rebuilt warm backend all emit the expected observer path;
- SDK secrets injected into all six former raw log sites are absent from captured traces;
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

- Override `ApiBackend::prompt_observed`.
- Set the acceptance barrier before installing each attempt's first HTTP send; keep it set through tool
  rounds.
- Bound non-success bodies at 64 KiB before JSON parsing.
- Implement the design's exact HTTP/ACP token/status/conflict table. Bare 429/503/529, incompatible
  status, fuzzy text, stderr prose, and conflicting fields remain `unknown`.
- Accept only the normative structured retry/reset fields and single `Retry-After`; hints are bounded,
  advisory, and never change disposition.

### Container and dispatch work

- Override `ContainerRwBackend::prompt_observed` and thread the observer through `open_inner` and
  `ContainerSpawn` into the newly created ACP backend.
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
- full task-journal, CLI/operator artifact, trace, and A2A wire secret regressions.

## R2b completion gate

R2b is complete only when R2b0–R2b3 are merged and the R2 design's entire R2b test list is satisfied.
Record the final commit/PRs, exact suite totals, review verdicts, and unrun live gates in the central
roadmap. Then change the single next action to R2c.
