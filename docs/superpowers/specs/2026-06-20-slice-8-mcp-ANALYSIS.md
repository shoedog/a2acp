# Slice 8 — MCP surface + D1 typed params — ARCHITECTURE ANALYSIS (converged)

> Status: **CONVERGED across dual-lens (codex-xhigh + Opus).** Both lenses returned
> `sound-with-changes`; both INDEPENDENTLY disproved the controller's D-B keystone (the sqlite
> single-owner lock), and both confirmed D-A/D-D/D-E with sharpening corrections. This doc has been
> revised to the converged position; §2A records what each lens changed. The decomposition (the A–E
> roadmap) is settled — do NOT re-litigate it; this resolves the Slice-8 IMPLEMENTATION ARCHITECTURE.

## 2A. Dual-lens convergence (what the lenses changed)
- **D-B (keystone) — BOTH lenses disproved the controller's "separate `mcp` process shares the serve's
  sqlite."** `SqliteStore::open` takes an EXCLUSIVE cross-process `<path>.lock` (`bridge-store/src/
  sqlite.rs:37-64`, tested `:1179-1188`); `serve` opens it on boot + `sweep_interrupted` + resumes
  working tasks (`main.rs:3892-3923/4006-4009`). A second Coordinator process CANNOT open the same
  store. → **MVP = a STANDALONE single-owner `a2a-bridge mcp` that takes the same store lock and fails
  clearly if a serve holds it.** Durable/composite = a later `mcp --http` mode hosting A2A over the SAME
  in-process Coordinator (≡ "IPC front-end"). No live cross-process A2A/MCP visibility in S8; cross-
  process warm sharing is non-MVP. (Was H3 "a risk to check" → it is a DISPROVEN precondition.)
- **D-A — both confirmed the new `bridge-coordinator` crate + move SessionManager, BUT the move is
  BIGGER than stated.** The detached-workflow service path also needs `workflow_sink`
  (`DetachedProgressSink`/`DetachedRichSinkFactory`), `TaskProgressHub`, the progress-frame DTOs
  (`workflow_sink.rs:78-190`, `reattach.rs:37-95/146-165`), the `now_ms` clock helper
  (`workflow_sink.rs:60-67`, called by `SessionManager::record_usage :530-542`), task-id minting
  (`a2a::new_task_id()` `server.rs:2157-2160` — Coordinator must not dep the A2A crate to mint an id),
  `summarize_collect` (`server.rs:429-472`, the compact driver), and the `workflow_cancels` token map
  (`server.rs:170-201/3242-3267`) — all trapped under the A2A crate today. These move/rehome WITH
  SessionManager (or become injected). Concrete struct (not trait) confirmed.
- **D-C — "kills per-role TOML" is OVERCLAIMED.** Local dispatch already plumbs overrides
  (`server.rs:3836-3889`, `domain.rs:158-212`; CLI populates them `main.rs:2873-2903`) — but WORKFLOW
  nodes IGNORE overrides: the cold executor calls `effective_config(&entry, None)` (`executor.rs:210`)
  and warm workflow child checkout passes `None` (`server.rs:694-703`). → D1 must EITHER add workflow/
  node override plumbing OR scope the "kills per-role TOML" claim to local `prompt`/`continue`. **MVP:
  scope to local; defer workflow-node D1.**
- **D-D — both confirmed: TWO tools, do NOT collapse.** `run_workflow` = detached → `task_id`;
  `prompt`/`continue` = warm context turns (the non-streaming `unary_message` collect ALREADY exists
  `server.rs:2815-2908`). Collapsing `continue` into `run`-over-context recreates the durable-task-id vs
  warm-context-id ambiguity the service API removes. Warm WORKFLOW continuation is streaming-only
  (dispatcher `server.rs:2111-2123`) → DEFER for MCP.
- **D-E — both confirmed async tokio stdio; reuse `bridge-acp/framing.rs` async NDJSON reader (size-
  limited `framing.rs:7-52`) + a serialized writer; detached-run+status avoids progress-notifications.**
  MCP `initialize` ECHOES `protocolVersion` (`lsp-mcp transport.rs:18-29`) — do NOT copy the A2A
  version-REJECT policy (`card.rs:175-185`).
- **Hazard upgraded to BLOCKER-for-the-gate:** `bridge_observ::init()` sets NO writer →
  `tracing_subscriber::fmt` defaults to **stdout** (`bridge-observ/src/lib.rs:5-10`); CLI paths also
  print to stdout (`main.rs:2694-2700/2928/3019`). The `mcp` subcommand MUST install a stderr/file
  writer (or skip `init()`), else every log corrupts the NDJSON stream and silently breaks MCP.

## 0. Slice scope (the converged ruling K — do not reopen)

From `specs/2026-06-17-orchestration-slicing.md` (row **8**) + `specs/2026-06-17-orchestration-architecture.md`
(S4 Surfaces, lines 128–131, P-6):

- **K = MCP/D1 surface.** "Extract a stable Rust service API over the Coordinator; stdio MCP adapter
  (lsp-mcp newline framing) exposing run/continue/clear/compact/status/release; D1 typed params; CLI =
  thin client of the same API."
- **A2A + CLI + MCP are co-equal thin adapters over ONE Rust service API (the Coordinator).** NOT "CLI
  thins over MCP." **Build the Rust service API first; A2A/CLI/MCP call it.** D1 params = typed operation
  fields (kills per-role TOML). Reuse the `lsp-mcp` stdio-MCP pattern.
- **Dep:** K→C (SessionManager) + stable ops. "Surface adapter only; built AFTER ops settle (the
  non-divergence point)." Slices 0–7 shipped → the ops (run/continue/status/clear/compact/release/
  cancel/task-get/list/cancel/subscribe) are all settled. **K is unblocked.**
- **DoD / Gate:** "an MCP client drives run+continue+status+clear (no Bash shell-out); A2A/CLI/MCP yield
  identical state."
- **Defers (out of K):** Turn-Channel ops (S9 — true mid-turn inject, queued questions, permission
  round-trip); generalized fan-out (M); the prompt-template/task-spec library tail (N).

## 1. Ground truth (file:line — from a 3-lens code sweep)

### 1a. The lsp-mcp stdio MCP server pattern (the reuse template)
`crates/lsp-mcp/src/mcp/{transport.rs,mod.rs,error.rs}` + `src/main.rs`. **Hand-rolled, no MCP SDK**
(serde_json only).
- **Framing:** NDJSON, `read_line_frame`/`write_line_frame` (`transport.rs:79–102`) — read until `\n`,
  trim `\r\n`, skip blanks, `Ok(None)` on EOF. SYNC `std::io` (blocking). (Distinct from the
  `Content-Length` LSP codec at `lsp/codec.rs`; same idea as `bridge-acp/src/framing.rs` but sync + no
  max-size.)
- **Serve loop:** `serve(session)` (`mod.rs:250–278`) — blocking, single-threaded, one request at a
  time; JSON-parse error → silent `continue`.
- **Protocol surface:** `Lifecycle::handle_meta` (`transport.rs:14–67`) — `initialize` (echo
  `protocolVersion`, `capabilities.tools:{}`, `serverInfo`), `notifications/initialized` (set flag, no
  reply), `tools/list`, `tools/call` (gated on initialized → else `-32600`), `ping`. Unknown →
  `-32601`.
- **Tool schema:** `tool_schemas()` (`mod.rs:30–117`) — `{name, description, inputSchema:{type:object,
  properties, required}}`.
- **Result/error envelope:** `ok(id,body)` → `result.content[0]={type:text,text}` (`mod.rs:119–130`);
  `iserror(id,text)` → `result.isError=true` (`mod.rs:134–143`) — an MCP tool error is `isError`, NOT a
  JSON-RPC error. JSON-RPC `err()` (`error.rs`) is only for protocol-level (`-32600/-32601`).
- **NOT transferable:** sync blocking I/O; single-request loop; per-process/per-cwd lifetime; inline
  90s readiness check. The orchestration adapter is async (tokio) + multiplexed + long-lived.

### 1b. Current surfaces + the divergence (the problem K fixes)
- **A2A** (`bridge-a2a-inbound/src/server.rs`): ~11 JSON-RPC methods (`message/send` `:2816`,
  `message/stream` `:849`, `SubscribeToTask` `:967`, `CancelTask` `:3201`, `GetTask` `:3591`,
  `ListTasks` `:3639`, `SessionStatus` `:3405`, `SessionRelease` `:3453`, `SessionCancel` `:3560`,
  `SessionClear` `:3478`, `SessionCompact` `:3516`; dispatch `:831`). **All business logic is INLINE in
  the handlers** — gate→route→(`SessionManager` / `TaskStore` / `Translator::run` / detached spawn).
  There is no service-layer object the handlers thin over.
- **CLI** (`bin/a2a-bridge/src/main.rs`): split personality —
  - **thin HTTP clients** via `rpc_call`: `submit` `:2868`, `task get/list/cancel` `:2945+`,
    `task watch` `:3026`, `session status/release/cancel/clear/compact` `:3001`.
  - **in-process one-shots** that REBUILD Registry+Executor locally: `run-workflow` (local) `:2489`,
    `implement` `:1720`. The **SpawnFn is DUPLICATED**: serve `:3732–3798` vs run-workflow `:462–531`
    (the exploration flags them "identical → extraction candidate").
  - **hybrid:** `run-workflow --serve` `:2366` is a thin streaming client.
- **Divergence:** the SAME logical op (run a workflow) is in-process-synchronous on the CLI vs
  detached-to-background on A2A (`spawn_detached_workflow` `server.rs:3022`); `continue` (warm) exists
  ONLY on A2A (streaming follow-up with the same task id) with NO CLI/MCP equivalent; every
  session/task op's real logic lives inline in `server.rs`, reachable from the CLI only over HTTP. **No
  shared Rust Coordinator exists today.**

### 1c. The candidate Coordinator (the objects that already ARE the service)
- **SessionManager** — `crates/bridge-a2a-inbound/src/session_manager.rs` (NB: lives in the A2A surface
  crate — a layering smell K should weigh). Public: `checkout_turn` `:230`, `checkout_child_turn` `:465`,
  `finish_turn` `:498`, `cancel` `:586`, `release(_with_children)` `:550/574`, `reset_session` `:747`
  /`clear_with_children` `:707` (→ `ResetOutcome{generation}`), `compact_session<F,Fut>` `:872`,
  `status` `:509` (→ `SessionStatusInfo`), `record_usage` `:534`, `reap_idle` `:1024`.
- **WorkflowExecutor** — `crates/bridge-workflow/src/executor.rs`: `run` `:314`, `run_with_context`
  `:326`, `run_from(_with_context)` `:367/388`, `run_with_context_and_dispatcher` `:337` (the
  `WorkflowNodeDispatcher` seam `:48`). Returns `WorkflowStream` of `WorkflowEvent{NodeStarted,
  NodeFinished, Terminal}` `:76–89`.
- **TaskStore** — `crates/bridge-core/src/task_store.rs` (trait; sqlite + `MemoryTaskStore`): `create`
  `:95`, `get` `:105`, `list` `:107`, `set_terminal(_sequenced)` `:97/178`, `cancel_if_working` `:115`,
  `sweep_interrupted` `:109`, checkpoints `:121/130`, sequenced progress `:152/166`, journal
  `:192/202`, `progress_snapshot` `:230`, `claim_resume_attempt` `:138`, `working_tasks` `:145`.
- **DTOs** — `crates/bridge-core/src/orch.rs`: `OrchEvent`/`OrchResult` (versioned `ORCH_V=1` `:7`,
  Ser+De, `#[serde(flatten)] kind`), `OrchEventKind`, `UsageSnapshot`, `TerminalStatus`,
  `AgentSessionCaps`; `TaskProgressSnapshot` in `task_store.rs`.
- **Params today** — `domain.rs`: TOML `[[agents]]` defaults (`AgentEntry` `:121`) + per-request
  `AgentOverride{model,effort,mode}` `:159` (parsed from A2A metadata `a2a-bridge.*`) + `SessionCwd`;
  `EffectiveConfig` `:169` = override-over-default; `TaskMeta{skill,agent,overrides}` `:223`.
  **D1's raw material already exists** — the gap is exposing it as TYPED operation params, not plumbing.

## 2. Architecture decisions (controller positions — for the lenses to pressure-test)

### D-A — The Coordinator boundary (pivotal). **Position: a concrete `Coordinator` façade struct in a NEW `bridge-coordinator` crate; NOT a trait; NOT a god-object.**
The three service objects (SessionManager, WorkflowExecutor, TaskStore) ALREADY exist and are sound; K
does NOT rewrite them. The Coordinator is a thin façade that OWNS `Arc<SessionManager>`,
`Arc<WorkflowExecutor>`, `Arc<dyn TaskStore>`, `Arc<dyn AgentRegistry>` and exposes the *union of
operations the surfaces need*, with the per-op orchestration that today lives INLINE in `server.rs`
(create-task-then-spawn-detached, route decision, summarize-collect driver) lifted INTO methods.
- **Why a concrete struct, not a trait:** there is exactly ONE implementation; a trait adds a vtable +
  an `async_trait` tax for zero polymorphism. (Mirrors the existing concrete `SessionManager`.)
- **Why a new crate:** SessionManager currently lives in `bridge-a2a-inbound` — the A2A *surface* crate.
  The MCP adapter and the CLI must call the Coordinator WITHOUT depending on the A2A HTTP server. A new
  `bridge-coordinator` crate (deps: bridge-core, bridge-workflow, bridge-acp/registry) is the clean
  home; `bridge-a2a-inbound` then depends on it and its handlers become thin. **MOVE SessionManager into
  `bridge-coordinator`** (it is not A2A-specific — it is keyed by the A2A `ContextId` concept but that
  type is in bridge-core). This is the largest mechanical change; weigh it against an in-place façade
  (D-A alt below).
- **D-A alt (smaller) — REJECTED by both lenses:** keep SessionManager in `bridge-a2a-inbound`, façade
  in place. Cheaper, but a standalone `mcp` binary would then drag in the axum HTTP server, and D-B'
  (composite `mcp --http`) needs the Coordinator crate-independent of any one surface. **CONVERGED: the
  new crate, moving SessionManager + the detached-workflow service internals (see §2A/H2) — bigger than
  "just SessionManager," but a pure suite-gated move.**

### D-B — MCP transport topology (THE keystone). **CONVERGED (both lenses): a STANDALONE single-owner `a2a-bridge mcp` process that builds the in-process Coordinator, TAKES the same store lock as `serve`, and FAILS clearly if a serve already owns it. NOT a separate process sharing a live serve's sqlite (impossible), NOT an HTTP thin-client of serve (violates the ruling).**
- **Why the controller's original ("separate `mcp` shares the serve's sqlite") is DEAD:**
  `SqliteStore::open` takes an exclusive cross-process `<path>.lock` (`sqlite.rs:37-64`, tested
  `:1179-1188`); `serve` opens it on boot + sweeps/resumes (`main.rs:3892-3923/4006-4009`). A second
  Coordinator process gets `StoreFailure` and cannot start. Also: live cancel tokens + `TaskProgressHub`
  are PROCESS memory (`server.rs:170-201`, `reattach.rs:146-165`), and boot-resume can spawn a second
  runner (`server.rs:2460-2680`) — duplicate ownership corrupts, not just collides (`create` IS
  non-clobbering `sqlite.rs:378-399`, ids are UUID `server.rs:2157-2160`; collision was never the risk).
- **The MVP topology (D-B''):** the coding agent spawns `a2a-bridge mcp --config X` (one stdio peer,
  like spawning `lsp-mcp`). It is the SOLE owner of the store for its lifetime; A2A/CLI either run in
  the SAME process's surfaces or against the store AFTER the mcp process exits (or a serve runs at a
  different time). "A2A/CLI/MCP yield identical state" = shared Coordinator CODE + durable-task
  equivalence + same-process warm equivalence. Cross-process warm-session sharing is explicitly NON-MVP
  (SessionManager is a process-local `HashMap` `session_manager.rs:116-124`).
- **The durable design (D-B', deferred fast-follow):** a composite `mcp --http` mode where the MCP-stdio
  process ALSO hosts the A2A HTTP router over the SAME in-process Coordinator → simultaneous A2A + CLI +
  MCP over one live state, INCLUDING warm sessions, with no second store-owner. This is the full honoring
  of "co-equal adapters over ONE Rust service API." Out of S8 scope; the Coordinator design must not
  preclude it (so the Coordinator owns the store + registry, and serve/mcp are both thin hosts).
- **Streaming:** MCP `tools/call` is request/response; a workflow run is long. **`run_workflow` is
  DETACHED** (returns a `task_id`, reusing `spawn_detached_workflow` `server.rs:2950-3048` + the W3a/W3b
  durable substrate); `status` (= session-status OR task-get) polls; `prompt`/`continue` return one
  collected text result (the `unary_message` collect shape `server.rs:2842`). MCP `notifications/progress`
  / `watch` are DEFERRED. This is why detached-`run_workflow` + `status` is the natural pair.

### D-C — D1 typed params. **Position: define a typed `RunParams`/`OpParams` struct on the Coordinator ops (`{workflow, input, context, agent?, model?, effort?, mode?, cwd?}`), populated identically from (a) MCP tool `arguments`, (b) CLI flags, (c) A2A metadata `a2a-bridge.*`. "Kills per-role TOML" = you no longer need a bespoke `examples/*.toml` per role; you pass `agent=codex, effort=xhigh` as op params.**
The raw material exists (`AgentOverride`/`EffectiveConfig`/`TaskMeta`); D1 is the TYPED SURFACE over it,
not new plumbing. **Cut:** do NOT remove the config file (it still supplies agent `cmd`/registry); D1
removes the need for ROLE-specialized copies. Scope D1 to the typed param struct + its three population
adapters; defer a full param schema/validation DSL.

### D-D — `run`/`continue` lifecycle. **CONVERGED (both lenses): TWO tools, do NOT collapse. `run_workflow` = detached multi-node submit → `task_id`. `prompt`/`continue` = warm single-turn against a `ContextId` (the `unary_message`/`checkout_turn` path, returning one collected text result).**
- `continue` reaches the warm session because it is just `prompt` on an EXISTING `ContextId` —
  `checkout_turn` returns `HandleBusy` if a turn is in flight (`session_manager.rs:262`), the correct
  serialization, no new machinery. The non-streaming warm shape ALREADY EXISTS: `unary_message`
  collects the streaming producer into one JSON response (`server.rs:2842`), and warm continue via
  `contextId` works on the Local route (unary only rejects `contextId` for `Workflow` routes,
  `server.rs:2826-2832`).
- **Why NOT one idempotent `run`-over-context:** detached tasks have durable `task_id`s, warm turns
  have `context_id`s — collapsing them recreates exactly the ambiguity the service API exists to remove
  (both lenses). Keep `run_workflow` (→task_id) and `prompt`/`continue` (→context turn) as distinct
  lifecycles.
- **DEFER:** warm WORKFLOW continuation over MCP — it is streaming-only today via the dispatcher
  (`server.rs:2111-2123`); not in the request/response MVP.

### D-E — async stdio MCP server. **Position: a tokio rewrite of the lsp-mcp loop — `tokio::io::stdin`/`stdout` + a line-framed NDJSON codec (port `framing.rs` style, add a write side), a `tokio::select!` read loop, `tools/call` dispatched to an async Coordinator method; serialize protocol writes through one writer task. Reuse the Lifecycle handshake + tool-schema + `isError` envelope VERBATIM.**
Concurrency: a coding agent drives one tool at a time over one stdio peer; but a `run` that detaches
returns immediately, so blocking is a non-issue for the MVP. **Cut:** concurrent in-flight tools/call
(spawn-per-call) — DEFER unless a lens shows the MVP needs it.

### D-F — non-divergence proof (the gate). **Position: the live-gate drives run+continue+status+clear through the MCP stdio adapter against a real agent, then asserts the SAME task/session state is visible via the A2A `task get` / `session status` against the shared sqlite — and a unit/integration test asserts all three surfaces (A2A handler, CLI, MCP dispatch) call the identical Coordinator method (no duplicated logic).**
The strongest structural proof is that after K, `server.rs` handlers, the CLI subcommands, and the MCP
dispatch are each a ~3-line adapter over one Coordinator method. A grep/test that the route logic exists
in exactly ONE place.

## 3. Hazards / risks (CONVERGED — lens-confirmed + extended)
- **H1 — scope blowup.** "Make A2A+CLI+MCP all thin over the Coordinator" is multi-week if big-banged.
  **MVP cut: extract ONLY the gate ops (prompt/continue/run_workflow/status/clear + cancel_task) into
  the Coordinator + migrate the MATCHING A2A handlers + CLI paths; leave the rest of `server.rs` calling
  the underlying objects, to migrate incrementally.**
- **H2 — the move is BIGGER than SessionManager (lens-confirmed).** The Coordinator extraction must
  also move/rehome (from `bridge-a2a-inbound`): `workflow_sink` `DetachedProgressSink`/
  `DetachedRichSinkFactory` + `TaskProgressHub` + progress-frame DTOs (`workflow_sink.rs:78-190`,
  `reattach.rs:37-95/146-165`), the `now_ms` clock (`workflow_sink.rs:60-67`, used by
  `record_usage :530-542` — becomes an injected/coordinator clock, NOT a surface import), task-id
  minting (`a2a::new_task_id()` `server.rs:2157` → move to core/service, Coordinator must not dep A2A),
  `summarize_collect` (`server.rs:429-472`, the compact driver, with the 32 KiB cap + EOF→`AgentCrashed`
  expire), and the `workflow_cancels` token map (`server.rs:170-201/3242-3267`). Pure move, suite-gated;
  this is the bulk of the slice's mechanical risk.
- **H3 — DISPROVEN, resolved (D-B).** Two Coordinator processes on one sqlite is impossible
  (`sqlite.rs:37-64` exclusive lock). MVP `mcp` is single-owner + takes the lock + fails if a serve
  holds it. No sharing-races to manage.
- **H4 — warm sessions process-local** (`session_manager.rs:116-124`). Cross-process warm visibility is
  NON-MVP (delivered later by D-B' composite `mcp --http`). Documented boundary.
- **H5 — tracing defaults to STDOUT → corrupts NDJSON (lens-upgraded to a gate BLOCKER).**
  `bridge_observ::init()` sets no writer (`bridge-observ/src/lib.rs:5-10`) → `fmt` defaults to stdout;
  CLI helpers also `println!` to stdout (`main.rs:2694-2700/2928/3019`). The `mcp` subcommand MUST
  install a stderr/file writer (or skip `init()`); stdout is RESERVED for framed replies. (lsp-mcp logs
  to stderr+file `mod.rs:190-199`.) Verify NO stdout writes on the Coordinator path.
- **H6 — detached MCP run needs a same-surface stop.** Cancel today uses in-memory tokens first
  (`server.rs:3242-3258`) then falls back to `cancel_if_working` (`:3260-3267`). If `mcp` exposes
  `run_workflow`, it MUST expose `cancel_task` (else a runaway detached job has no stop from MCP). MVP
  `cancel_task` = the durable `cancel_if_working` flip (no live in-flight interrupt) is acceptable +
  honest — OR move the token map onto the Coordinator (H2) for a live interrupt.
- **H7 — MCP EOF shutdown.** `mcp` stops on stdin-EOF (lsp-mcp `mod.rs:250-277`). SessionManager has no
  public `release_all` (only release-by-context `:550-582` + `reap_idle :1023-1034`) → add a Coordinator
  shutdown path that releases warm sessions + drops active detached tokens on EOF.
- **H8 — MCP `initialize` ECHOES `protocolVersion`** (`lsp-mcp transport.rs:18-29`) — do NOT copy the
  A2A version-REJECT policy (`card.rs:175-185`). Settled by the template; reuse verbatim.

## 4. MVP cut-line (CONVERGED) / cut-defer
**IN (the gate's spine):**
- `bridge-coordinator` crate: concrete `Coordinator` façade owning `Arc<SessionManager>`,
  `Arc<WorkflowExecutor>`, `Arc<dyn TaskStore>`, `Arc<dyn AgentRegistry>` + the detached-workflow
  service internals moved in (H2: workflow_sink/hub/sink-DTOs/now_ms/task-id-mint/summarize_collect/
  cancel-token-map). ONE registry-build path: collapse the serve inline SpawnFn (`main.rs:3732-3798`)
  onto the shared `make_spawn_fn` (`:462-531`).
- Coordinator methods: `prompt`/`continue` (warm turn, collected), `run_workflow` (detached → task_id),
  `status` (context_id OR task_id), `clear` (reset_session), `cancel_task` (cancel_if_working).
- Typed `OpParams` (D1) + 3 population adapters (MCP args / CLI flags / A2A metadata). Scope the
  "kills per-role TOML" claim to LOCAL prompt/continue (workflow-node overrides ignored today,
  `executor.rs:210` → deferred).
- Async stdio MCP adapter: `a2a-bridge mcp` STANDALONE single-owner (takes the store lock; fails if a
  serve holds it; STDERR/file tracing; reuse lsp-mcp framing + Lifecycle + version-ECHO + isError
  envelope; tokio NDJSON via `bridge-acp/framing.rs` reader + a serialized writer). Tools:
  `run`/`continue`/`run_workflow`/`status`/`clear`/`cancel_task`.
- Migrate the MATCHING A2A handlers + CLI session/prompt paths to call the Coordinator → non-divergence
  proof on these paths.
**DEFER (explicit):**
- D-B' composite `mcp --http` (simultaneous live A2A+CLI+MCP / cross-process warm sharing).
- MCP `notifications/progress` / `watch` streaming; warm WORKFLOW continuation over MCP.
- compact/release over MCP (3-line adds once summarize_collect/token-map are on the Coordinator).
- Full migration of ALL `server.rs` handlers + the rest of the CLI thinning (incremental).
- Workflow/per-node D1 override plumbing; D1 validation DSL; prompt-template/task-spec lib (N).

## 5. Resolved decisions (was: open questions for the lenses)
1. **D-B:** standalone single-owner `a2a-bridge mcp` (takes the store lock, fails if serve owns it);
   composite `mcp --http` is the deferred durable answer. NOT separate-process-shares-sqlite (impossible),
   NOT HTTP-thin-client (ruling). ✅ both lenses.
2. **D-A:** new `bridge-coordinator` crate; concrete struct; move SessionManager + the detached service
   internals (H2). NOT in-place façade. ✅ both lenses.
3. **D-D:** TWO tools — detached `run_workflow`→task_id + warm `prompt`/`continue`; warm-workflow-
   continuation deferred. ✅ both lenses.
4. **MVP cut:** run/continue/run_workflow/status/clear/cancel_task; SessionManager move IS in-scope (the
   layering + D-B' both need it). ✅
5. **Hazards:** H5 (stdout) is the silent gate-breaker; H2 (the bigger move), H6 (MCP cancel), H7 (EOF
   shutdown), H8 (version echo) all folded above.

## 6. Decomposition sketch (the plan will detail — TDD, bite-sized)
1. `bridge-coordinator` crate skeleton; pure MOVE of SessionManager + the detached-workflow service
   internals (workflow_sink/hub/sink-DTOs/now_ms→injected-clock/task-id-mint/summarize_collect/
   cancel-token-map) out of `bridge-a2a-inbound`; collapse the serve inline SpawnFn onto `make_spawn_fn`.
   Pure move, FULL-SUITE gated (no behavior change).
2. The `Coordinator` façade struct + typed `OpParams` (D1); unit tests on param population from the 3
   sources; assert all surfaces share ONE `Arc<dyn TaskStore>`.
3. Coordinator `prompt`/`continue`/`run_workflow`/`status`/`clear`/`cancel_task` methods (lift the inline
   `server.rs` orchestration).
4. Migrate the matching A2A handlers to call the Coordinator (byte-identical; suite-gated).
5. The async stdio MCP adapter (framing reader + serialized writer + Lifecycle + version-echo + tool
   schema + dispatch + stderr-only tracing) — unit-tested with a scripted stdio peer.
6. The `a2a-bridge mcp` subcommand (single-owner lock + EOF shutdown) + the CLI session/prompt paths
   thinned onto the Coordinator.
7. Whole-branch dual-lens review → live-gate (MCP drives run+continue+run_workflow+status+clear+cancel;
   identical state via A2A/CLI on the same store, single-owner) → merge.

---
*CONVERGED (dual-lens, both `sound-with-changes`). NEXT = the spec (`2026-06-20-slice-8-mcp.md`,
FIX-list) + dual spec-review, then the plan (PFIX-list) + dual plan-review. The lens raw outputs:
`/tmp/slice-8-arch-codex.out` (codex-xhigh) + the Opus lens (in-conversation).*
