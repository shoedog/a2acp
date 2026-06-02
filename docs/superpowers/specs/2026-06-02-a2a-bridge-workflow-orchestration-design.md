# A2A Bridge — Workflow-DAG Orchestration (W1) + `code-review` instance Design

**Goal:** Add a **greenfield workflow-DAG orchestration** capability: define a *workflow* as a named DAG of agent-task nodes (fan-out ∥ · pipeline → · fan-in/rollup ∥→), execute it by reusing the existing registry + `AgentBackend::prompt`, and ship one concrete instance — the **`code-review` workflow** (fan-out to [codex, claude] → a `synth` rollup node). This is the first increment ("W1") of the **self-hosting** direction (ADR-0008 re-trigger #5): using the Rust bridge to drive its own code/plan/spec reviews, ultimately replacing the `~/code/a2a-local-bridge` PoC. W1 self-hosts the dual-review loop currently run by hand.

**Architecture:** A new crate `crates/bridge-workflow` holds the `WorkflowGraph` types + a `WorkflowExecutor`. The executor takes `Arc<dyn AgentRegistry>` and runs each node via the existing `backend.prompt` — it adds **composition over the spine, not a new agent path**. Workflows are defined in the bridge's TOML (`[[workflows]]`, prompt-templates as files), **parsed + validated ONCE at boot** (load-once; hot-reload deferred — §6/rev2). Triggers are **streaming-only**: an **A2A skill** (`RouteTarget::Workflow(id)`, a `spawn_workflow_producer` mirroring fan-out) and a thin **`a2a-bridge run-workflow`** CLI; both call the same executor. Per the conductor decision this is **chain-of-brains orchestration** (each node a full agent, output→input), explicitly NOT the conductor's proxy-chain.

**Tech stack:** Rust. Reuses `bridge-core` (ports: `AgentRegistry`, `AgentBackend`, `BackendStream`, `effective_config`/`configure_session`), `bridge-registry`, `RouteTarget`/`SkillRoute`, the inbound producer + `poll_cancel_requested` pattern, the Agent-Card builder. New deps: none beyond the workspace (`serde`, `serde_json`, `futures`, `tokio`, `async-trait`, `toml`). `bridge-workflow` depends only on `bridge-core`.

**Spec status:** brainstormed; design approved 2026-06-02; scope confirmed = **W1 only**. **Revision 2 — dual review (Codex `gpt-5.5` + Claude `opus-4.8`) folded.** Decisive corrections: the executor runs each node via a **node-turn runner that calls `configure_session` and concatenates `Update::Text`** (NOT a `Translator::run` reuse — the translator's artifact is `last_text`, which would drop content); **cancellation is explicit** (the producer polls `poll_cancel_requested` and calls `backend.cancel` per in-flight node — dropping the stream is insufficient for a JSON-RPC `CancelTask`); **workflows load once at boot** (hot-reload deferred, removing a TOCTOU class); triggers are **streaming-only** (unary rejects); the `RouteTarget::Workflow` ripple + Agent-Card advertisement are fully enumerated. See §11.

**Firewall:** `~/code/a2a-local-bridge` (the Python PoC this self-hosting program will eventually replace) is **black-box only** — its schema/methodology must NOT influence this design. Everything here derives from the bridge's own ports/primitives. The `code-review` instance's *shape* (two reviewers + a synthesizer) comes from the user's **dual-review methodology** + the `review-agent-roles` memory (Codex → blockers/correctness, Claude → architecture), not from the PoC. Reviewers confirmed: no PoC schema leakage. `~/Code/agent-knowledge` is separately readable.

---

## 1. Why — the gap, and the framing

The bridge routes each A2A task to exactly one of `RouteTarget::{ Local(id), Delegate, Fanout }` (`route.rs`). **`Fanout` is hardcoded to `(default agent, configured peer)` and has no fan-in/synthesis** — it emits both results as labeled artifacts. So the dual-review loop (fan-out to codex+claude → a human/controller synthesizes) has **no self-hosted equivalent**. W1 closes that: a general workflow-DAG layer where fan-out is N-way and **fan-in/rollup is a first-class node**.

Per ADR-0008 + `docs/conductor-pattern-review.md`: this is the **leading re-trigger (#5)**, built **greenfield** by extending `fan-out`/`RouteTarget`/registry — a *chain of brains* (each node a full agent doing real work, output→input), NOT the conductor's *chain of middleware* (proxy-chains). The only borrowed conductor pattern is **`skill = request-shaper`** (each node shapes its request from the workflow input + upstream outputs), implemented as in-process template rendering.

## 2. The workflow model (`bridge-workflow` domain types)

A workflow is a **named DAG**; edges are **implicit from each node's `inputs`** — there are no special node types for fan-out/pipeline/fan-in.

```rust
// WorkflowId + NodeId live in `bridge-core::ids` (RouteTarget::Workflow(WorkflowId) is a
// bridge-core type and core cannot depend on bridge-workflow). The graph types live in
// crates/bridge-workflow/src/graph.rs.
use bridge_core::ids::{AgentId, WorkflowId, NodeId};

pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,        // validated: acyclic, single-terminal, agents resolvable
}
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,                  // resolved via the registry
    pub prompt_template: String,         // loaded from prompt_file at boot-parse time
    pub inputs: Vec<NodeId>,             // [] = consume the workflow input; [a,b,..] = fan-in on a,b
}
```

- **`WorkflowId`/`NodeId` get REAL char-class validation** (not just non-empty): `[a-z0-9_-]+`. This is **load-bearing** — `NodeId` is interpolated into `{{<node-id>}}` template tokens, so whitespace/`{{`/`}}` must be rejected (rev2: the existing `id_newtype` macro validates non-empty only; these two ids need the stricter rule, e.g. a dedicated constructor).
- `inputs = []` → the node receives the **workflow input**. `inputs = ["codex","claude"]` → **fan-in** (depends on both). A chain of single-input nodes = a **pipeline**. Topology falls out of `inputs`; no `kind` field.
- **`{{input}}` (the workflow input) is available to EVERY node** (rev2); `{{<upstream-id>}}` is available for each id in that node's `inputs`. (So `synth` may reference `{{input}}`, `{{codex}}`, `{{claude}}`.)
- **Terminal node** = the node no other node lists in `inputs`. W1 requires **exactly one** terminal (validated; multi-terminal is a non-goal).
- Each `agent` MUST exist in the registry (validated at boot).

TOML (parsed once at boot in `bin/a2a-bridge`; prompt bodies loaded from files):

```toml
[[workflows]]
id = "code-review"
  [[workflows.nodes]]
  id = "codex";  agent = "codex";  prompt_file = "prompts/review-codex.md";  inputs = []
  [[workflows.nodes]]
  id = "claude"; agent = "claude"; prompt_file = "prompts/review-claude.md"; inputs = []
  [[workflows.nodes]]
  id = "synth";  agent = "claude"; prompt_file = "prompts/review-synth.md";  inputs = ["codex", "claude"]
```

## 3. The executor (`bridge-workflow/src/executor.rs`)

```rust
pub struct WorkflowExecutor { registry: Arc<dyn AgentRegistry> }

impl WorkflowExecutor {
    /// Run `graph` with `input` and a run-unique `run_id`; stream node-level events + outcome.
    /// `graph` is `Arc` so the returned stream is `'static` (movable into the producer).
    pub fn run(&self, graph: Arc<WorkflowGraph>, input: String, run_id: String) -> WorkflowStream;
    // WorkflowStream = Stream<Item = Result<WorkflowEvent, BridgeError>> + Send + 'static
}

pub enum WorkflowEvent {
    NodeStarted  { node: NodeId },
    NodeFinished { node: NodeId, ok: bool },
    Terminal     { outcome: WorkflowOutcome, output: String },   // emitted once, last
}
pub enum WorkflowOutcome { Completed, Failed, Canceled }
```

**Execution:**
1. Validate once at boot (§2). The stream re-checks defensively.
2. **Schedule topologically:** a node is ready when all its `inputs` finished. **Independent ready nodes run concurrently** (fan-out: codex + claude in parallel via `futures` join); dependent nodes wait (synth after both).
3. **Per node — the "node-turn runner" (rev2, reuses the inbound dispatch semantics, NOT `Translator::run`):**
   a. **Render** `prompt_template` with **single-pass** substitution (rev2): one left-to-right scan replacing each `{{token}}` with its value (`input` → workflow input; `<upstream-id>` → that node's collected output); an already-substituted value is NEVER re-scanned (so an upstream output containing `{{claude}}` cannot corrupt a later pass). Unknown `{{x}}` left verbatim + a warn.
   b. `registry.resolve(&node.agent)` → `Resolved { backend, entry, lease }`. Compute `effective_config(&entry, None)` and **`backend.configure_session(&session, &eff)`** before prompting (rev2 — ACP nodes otherwise mint with empty config and lose model/effort/mode; codex+claude are ACP).
   c. `session = SessionId("workflow-{wf}-{node}-{run_id}")` where **`run_id` is run-unique** (rev2: the A2A task id on the A2A path; a fresh uuid on the CLI path) — so concurrent runs of the same workflow never collide on a warm ACP backend.
   d. `backend.prompt(&session, vec![Part{text: rendered}])` → **drain the `BackendStream` to a `String` by CONCATENATING every `Update::Text`** (rev2: this is correct and intentional — `Translator::run`'s artifact is `last_text`, the last delta only, which would drop content). `Update::Permission` is **ignored — safe** (rev2: live AcpBackend resolves `session/request_permission` internally via its injected `PolicyEngine` and only ever streams Text/Done; the api backend decides tool calls silently; neither emits `Update::Permission` on the prompt stream). `Update::Done{stop_reason}`: `== STOP_REASON_CANCELLED` → node canceled; an error mid-stream → node failed.
   e. `backend.forget_session(&session)` after (rev2 — symmetry with the inbound `BindingGuard`, prevents stash leaks); drop the lease.
   f. Store `output[node]`; emit `NodeFinished{ok}`. On failure, `output[node] = "[node <id> failed: <reason>]"` (§6).
4. Emit `Terminal { outcome, output: terminal_node_output }`. `outcome = Failed` iff the **terminal** node failed (or was canceled → `Canceled`); a non-terminal failure that the terminal survives is still `Completed` (graceful degradation, §6).

**Nodes inherit each backend's injected `PolicyEngine`** (threaded at spawn via `with_policy`). W1 has **no workflow-level permission policy** (read-only reviews; a non-goal — §10).

The executor is a **library** — both the A2A producer and the CLI call `run()`.

## 4. Triggers (streaming-only) + the `RouteTarget::Workflow` ripple

### 4.1 A2A skill → `RouteTarget::Workflow`
`RouteTarget` gains `Workflow(WorkflowId)` (`bridge-core::domain`). **Full ripple enumeration (rev2 — every site the variant touches):**
- `crates/bridge-a2a-inbound/src/server.rs`: **`stream_message` exhaustive match (`:454`)** → add a `Workflow` arm calling `spawn_workflow_producer`. **`unary_message` exhaustive match (`:1041`)** → add a `Workflow` arm that **rejects** (W1 workflows are **streaming-only**; return `BridgeError::InvalidRequest{field:"skill"}` — "workflow requires the streaming method"; mirror the existing `Fanout => unreachable!()` precedent but as a real reject). **`local_agent_id` wildcard (`:271`)** → must explicitly handle `Workflow` (it currently maps non-`Local` to default — a silent-wrong site). **The unary `if let RouteTarget::Fanout` pre-dispatch (`:1035`)** compiles but must be checked for the workflow case.
- **`cancel_task`** (`:1294`): add a workflow arm (§6 cancel).
- `bin/a2a-bridge/src/route.rs` `SkillRoute` (§4.3) + its tests.
- **Agent Card (`card.rs:84`)** — rev2: the card advertises a fixed set of A2A skills; **add one `AgentSkill` per workflow id** so workflows are A2A-**discoverable** (else a caller can't find `code-review`).

`spawn_workflow_producer` mirrors `spawn_fanout_producer`: build `input: String` from the task's parts, `run_id = task.as_str()`, `executor.run(graph, input, run_id)`; map `WorkflowEvent::{NodeStarted,NodeFinished}` → A2A Status events (labeled by node id); `Terminal{Completed,output}` → final Artifact + terminal **Completed**; `Terminal{Failed,..}` → terminal **`TaskState::Failed`** (rev2 — a failure marker must NOT look like success); `Terminal{Canceled,..}` → **Canceled**. The producer **`select!`s on `poll_cancel_requested`** (rev2, §6).

### 4.2 CLI `run-workflow`
`a2a-bridge run-workflow <workflow-id> --input <file> [--out <file>]`: load config, build `input` from the file, generate a **fresh uuid `run_id`**, call the **same** `executor.run`, print node-level progress to stderr and the terminal output to stdout/`--out`. Thin wrapper; no new orchestration.

### 4.3 `SkillRoute` (boot-fixed workflow set)
With **load-once** (§6) the workflow-id set is fixed at boot, so `SkillRoute` is constructed with it (no stale/live-read issue). Precedence: `skill=="delegate"`→`Delegate`; `"fan-out"`→`Fanout`; **else if `skill` is a known workflow id → `Workflow(id)`**; else → `Local(meta.agent.unwrap_or(default))`.

## 5. The `code-review` instance

Config = §2's `code-review` workflow. Three prompt files under `prompts/` (committed): `review-codex.md` (Codex lens: blockers/correctness/regressions/test-gaps; `{{input}}` = diff/spec), `review-claude.md` (Claude lens: architecture/seams/design; `{{input}}`), `review-synth.md` (synthesizer: `{{codex}}` + `{{claude}}` → one merged, de-duplicated review weighted by the `review-agent-roles` complementarity; may also reference `{{input}}`). An A2A `skill="code-review"` streaming task with the diff as the message → fan-out to both reviewers → `synth` merges → one review artifact. This **self-hosts** the dual-review loop.

## 6. Config (load-once), errors, cancellation

**Load-once at boot (rev2 — replaces hot-reload).** `[[workflows]]` is parsed + validated in the **initial** config pass (alongside `[[agents]]`), prompt files loaded, graphs validated (acyclic / single-terminal / agents-resolvable), built into a `HashMap<WorkflowId, Arc<WorkflowGraph>>` held for the process lifetime. **Bad workflow config / missing prompt file → fail loud at startup** (consistent with `Registry::new`). **Workflow hot-reload is a non-goal for W1** (§10) — this removes the `RegistrySnapshot`-carries-no-workflows atomicity/TOCTOU problem entirely; the agent registry stays hot-reloadable as today, workflows do not.

**Node failure** (agent crash / non-2xx / timeout / drain error): the node's output becomes an **error marker** (`"[node <id> failed: <reason>]"`); downstream nodes that depend on it receive that marker as `{{node}}`, and `NodeFinished{ok:false}` fires. The run **continues** → a single reviewer failing still yields a `synth` from the surviving review (**graceful degradation**; hard-fail-the-run rejected as too brittle for fan-out). If the **terminal** node itself fails → `Terminal{outcome:Failed}` → the A2A task ends **`Failed`** (rev2).

**Cancellation (rev2 — the real blocker).** Inbound `CancelTask` is JSON-RPC (not an SSE disconnect), so `tx.closed()` never fires and merely *dropping* the executor stream is insufficient. Two coordinated mechanisms (mirroring fan-out): (a) `cancel_task` always latches `store.request_cancel(&task)` (it already does, `:1292`) and gains a **workflow arm** that returns after latching (the producer does the node cancels — no default-backend no-op); (b) **`spawn_workflow_producer` `select!`s on `poll_cancel_requested`**; on cancel it calls **`backend.cancel(&session)` for every in-flight node** (the executor exposes the active `(backend, session)` handles, or the producer drives nodes and owns them), then ends the stream with `Terminal{Canceled}`. Dropping the executor future additionally aborts the in-flight `reqwest`/closes the ACP `done_sender` as a backstop.

## 7. Output / streaming

The workflow is **one A2A streaming task**. It streams **node-level** progress (`NodeStarted`/`NodeFinished` → A2A Status events, labeled by node id) and ends with the terminal: a final **Artifact** = the terminal node's output, plus an explicit **terminal state event** `Completed | Failed | Canceled` (rev2 — mirror `spawn_local_producer`'s terminal emission; the producer MUST emit one). Internal per-node token streams are **collected to text** (concatenated, §3d), not forwarded token-by-token (W1 simplicity; labeled live streaming is a later enhancement).

## 8. Crate structure & reuse

**New — `crates/bridge-workflow`** (`graph.rs` types + validation, `executor.rs`, `template.rs` single-pass `{{var}}`). Depends only on `bridge-core`. **New ids `WorkflowId`/`NodeId`** in `bridge-core::ids` (char-validated, §2). **`RouteTarget::Workflow(WorkflowId)`** in `bridge-core::domain`. The executor takes `Arc<dyn AgentRegistry>` and returns a `'static` stream over `Arc<WorkflowGraph>`.

**Modified — `bin/a2a-bridge`:** `config.rs` parses `[[workflows]]` + loads prompt files at boot into the workflow map; `route.rs` `SkillRoute` constructed with the workflow-id set; `main.rs` wires the executor + map; the `run-workflow` subcommand. **`crates/bridge-a2a-inbound`:** `spawn_workflow_producer`, the `RouteTarget::Workflow` arms (§4.1), the `cancel_task` workflow arm, the Agent-Card workflow skills.

**Reused, unchanged:** `AgentRegistry`/`resolve`/lease, `AgentBackend::{prompt,cancel,configure_session,forget_session}`, `effective_config`, `RouteTarget`/`SkillRoute`, the inbound producer→`tx` + `poll_cancel_requested` pattern, the Agent-Card builder. **Deliberately NOT reused:** `Translator::run` (its `last_text` artifact would drop node content — §3d).

## 9. Testing & Definition of Done

**Unit (`bridge-workflow`, fake registry + scripted fake backends):**
- DoD-1 — **fan-out parallel:** two `inputs=[]` nodes prompted concurrently.
- DoD-2 — **pipeline:** a→b→c; `{{a}}`/`{{b}}` substituted; order enforced.
- DoD-3 — **fan-in:** `synth`'s rendered prompt contains BOTH upstream outputs (exact substitution asserted).
- DoD-4 — **single-pass template:** an upstream output literally containing `{{claude}}` is NOT re-expanded when a later `{{claude}}` is substituted (the rev2 corruption case); `{{input}}` available to every node; unknown var verbatim.
- DoD-5 — **validation:** cyclic / multi-terminal / unknown-`agent` graphs each rejected at construction; a `NodeId` with whitespace/`{{` rejected.
- DoD-6 — **node-failure degradation:** a failing fan-out leg → its **exact marker string appears in `synth`'s recorded prompt**; the run completes `Completed`; `NodeFinished{ok:false}` emitted. (Falsifiable — asserts the marker reaches synth, not just that the run finished.)
- DoD-7 — **cancellation (real):** the producer, on `poll_cancel_requested`, calls **`backend.cancel(&session)` on each in-flight node** (assert the fake backend recorded the cancel calls — NOT merely that a stream dropped) and ends `Canceled`.
- DoD-8 — **configure_session:** each node calls `configure_session` with the agent's effective config before `prompt` (assert on the fake).

**Config + wiring:**
- DoD-9 — `[[workflows]]` parses + loads prompt files; bad DAG / missing prompt file **fails loud at boot**; `SkillRoute` maps `skill="code-review"`→`Workflow`; the **Agent Card advertises a `code-review` skill**.
- DoD-10 — **A2A streaming e2e** (fake/wiremock agents): an `skill="code-review"` streaming task → node-level Status events + the synth output as final Artifact + terminal **Completed**; **assert synth's prompt recorded BOTH reviews**. Plus: a **unary** `skill="code-review"` send → clean **`InvalidRequest`** (streaming-only). Plus: a terminal-node failure → A2A **`Failed`**.
- DoD-11 — **CLI smoke:** `run-workflow code-review --input <fixture>` over fake agents → prints synth output.

**Gated live:** DoD-12 — `#[ignore]` run of `code-review` against real codex + claude (manual), incl. a cancel mid-run.

**Coverage:** new `bridge-workflow` HARD CI floor (propose 90); workspace 85 / bridge-core 90 / bridge-acp 90 / bridge-api 90 unchanged. `clippy -D warnings`, `fmt`, full test green.

## 10. Scope boundary

**BUILDS:** `bridge-workflow` (graph + executor + single-pass template + validation); `WorkflowId`/`NodeId` (char-validated) + `RouteTarget::Workflow`; `[[workflows]]` **boot-load** + prompt-file loading; `SkillRoute` mapping + `spawn_workflow_producer` (streaming) + the unary reject + the `cancel_task` arm + the Agent-Card workflow skills; the `run-workflow` CLI; the `code-review` instance + 3 prompt files; the CI floor; ADR-0009.

**NON-GOALS (later increments / YAGNI):** **workflow hot-reload** (load-once at boot for W1 — rev2); structured/typed review output (W1 output is **text** — W2); durable task store + submit/history (W3); research/dev workflow instances + the log-triage pipeline (config-only follow-ons once the primitive ships); **unary** workflow invocation (streaming-only); **multi-terminal** workflows; **workflow-level permission policy** (nodes inherit the backend's injected policy — read-only reviews); labeled live token streaming; conditional/dynamic edges, retries, loops (DAG only); per-node model/effort overrides in workflow config (use the agent's registry config); replacing the `a2a-local-bridge` PoC wholesale.

## 11. Review

**Revision 2 folds the dual review** (Codex `gpt-5.5` `review_required`; Claude `opus-4.8` `input_required`), launched detached against rev1 commit `3045dee`.

- **Cancellation (both, BLOCKER).** A workflow task's `cancel_task` hit the local branch (cancels `session-{task}` on the default backend, missing node sessions); and a JSON-RPC `CancelTask` never fires `tx.closed()`, so dropping the stream doesn't cancel. **Folded:** `cancel_task` workflow arm + producer `poll_cancel_requested` + explicit `backend.cancel` per in-flight node (§6); DoD-7 asserts the real cancel calls.
- **Translator-bypass split (Claude refined Codex).** **Permission ignore is SAFE** (backends resolve `session/request_permission` internally; never emit `Update::Permission`) — documented (§3d). **Output concat is CORRECT** (translator's artifact is `last_text` → would drop content; do NOT reuse it) — documented (§3d, §8). **`configure_session` WAS missing** (ACP nodes lose model/effort/mode) — **folded** (§3b, DoD-8) + `forget_session` cleanup.
- **`RouteTarget::Workflow` ripple + unary (both, BLOCKER).** Rev1's "§1 enumeration" enumerated nothing; the unary exhaustive match (`:1041`) was unhandled. **Folded:** full site list (§4.1) — stream/unary matches, `local_agent_id` wildcard, `if-let`, `cancel_task`, `SkillRoute`, **Agent Card**; workflows are **streaming-only**, unary rejects.
- **Hot-reload incoherent (both, MAJOR).** `watch()` yields `RegistrySnapshot` which carries no workflows → torn reloads. **Folded:** **load-once at boot** (§6) — removes the whole class; hot-reload is a non-goal.
- **Terminal-failure outcome (both, MAJOR).** `Done{output}` had no outcome → a failure marker looked like success. **Folded:** `WorkflowEvent::Terminal{outcome,output}` → A2A `Completed|Failed|Canceled` (§3/§4.1/§7).
- **Design gaps (folded):** single-pass templating (Claude's `{{claude}}`-corruption case, §3a/DoD-4); run-unique session id = task id / CLI uuid (§3c); real char-validation for `WorkflowId`/`NodeId` (§2); `Arc<WorkflowGraph>` `'static` signature (§3/§8); `{{input}}` available to every node + fan-in clarified (§2).
- **Scope (Claude).** Load-once + streaming-only shrink W1 and remove a blocker; the CLI stays (thin, shared executor). Firewall: both confirmed clean.

Reviewers' questions resolved: (1) **load-once**; (2) **streaming-only**; (3) **run_id = task id / CLI uuid**; (4) **`{{input}}` available to all nodes**; (5) **inherit backend policy** (no workflow-level policy in W1).
