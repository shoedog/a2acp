# A2A Bridge — Workflow-DAG Orchestration (W1) + `code-review` instance Design

**Goal:** Add a **greenfield workflow-DAG orchestration** capability: define a *workflow* as a named DAG of agent-task nodes (fan-out ∥ · pipeline → · fan-in/rollup ∥→), execute it by reusing the existing registry + `AgentBackend::prompt`, and ship one concrete instance — the **`code-review` workflow** (fan-out to [codex, claude] → a `synth` rollup node). This is the first increment ("W1") of the **self-hosting** direction (ADR-0008 re-trigger #5): using the Rust bridge to drive its own code/plan/spec reviews, ultimately replacing the `~/code/a2a-local-bridge` PoC. W1 self-hosts the dual-review loop currently run by hand.

**Architecture:** A new crate `crates/bridge-workflow` holds the `WorkflowGraph` types + a `WorkflowExecutor`. The executor takes `Arc<dyn AgentRegistry>` and runs each node via the existing `backend.prompt` — it adds **composition over the spine, not a new agent path**. Workflows are defined in the bridge's TOML (`[[workflows]]`, prompt-templates referenced as files) and loaded on the **same `ConfigSource`/hot-reload** machinery as `[[agents]]`. Two triggers, one executor: an **A2A skill** (`RouteTarget::Workflow(id)`, a `spawn_workflow_producer` mirroring fan-out) and a thin **`a2a-bridge run-workflow`** CLI. Per the conductor decision this is **chain-of-brains orchestration** (each node a full agent, output→input), explicitly NOT the conductor's proxy-chain.

**Tech stack:** Rust. Reuses `bridge-core` (ports: `AgentRegistry`, `AgentBackend`, `BackendStream`), `bridge-registry`, the binary's `ConfigSource`/reconcile, `RouteTarget`/`SkillRoute`, the inbound producer pattern. New deps: none beyond the workspace (`serde`, `futures`, `tokio`, `async-trait`, `toml`). `bridge-workflow` depends only on `bridge-core`.

**Spec status:** brainstormed; design approved 2026-06-02; scope confirmed = **W1 only** (primitive + review instance); grounded in the current orchestration code (`bin/a2a-bridge/src/route.rs`, `crates/bridge-a2a-inbound/src/server.rs` fan-out/delegate producers, `bridge-registry`, `bin/a2a-bridge/src/config.rs`). **Dual review (Codex + Claude) pending — folds into Revision 2 before the plan.**

**Firewall:** `~/code/a2a-local-bridge` (the Python PoC this self-hosting program will eventually replace) is **black-box only** — its schema/methodology must NOT influence this design. Everything here derives from the bridge's own ports/primitives. The `code-review` instance's *shape* (two reviewers + a synthesizer) comes from the user's **dual-review methodology** + the `review-agent-roles` memory (Codex → blockers/correctness, Claude → architecture), not from the PoC. `~/Code/agent-knowledge` is separately readable.

---

## 1. Why — the gap, and the framing

The bridge routes each A2A task to exactly one of `RouteTarget::{ Local(id), Delegate, Fanout }` (`route.rs`). **`Fanout` is hardcoded to `(default agent, configured peer)` and has no fan-in/synthesis** — it emits both results as labeled artifacts. So the dual-review loop (fan-out to codex+claude → a human/controller synthesizes) has **no self-hosted equivalent**. W1 closes that: a general workflow-DAG layer where fan-out is N-way and **fan-in/rollup is a first-class node**.

Per ADR-0008 + `docs/conductor-pattern-review.md`: this is the **leading re-trigger (#5)**, built **greenfield** by extending `fan-out`/`RouteTarget`/registry — a *chain of brains* (each node a full agent doing real work, output→input), NOT the conductor's *chain of middleware* (proxy-chains). The only borrowed conductor pattern is **`skill = request-shaper`** (each node shapes its request from the workflow input + upstream outputs), implemented as in-process template rendering.

## 2. The workflow model (`bridge-workflow` domain types)

A workflow is a **named DAG**; edges are **implicit from each node's `inputs`** — there are no special node types for fan-out/pipeline/fan-in.

```rust
// WorkflowId + NodeId live in `bridge-core::ids` (validated like AgentId: non-empty, [a-z0-9-_]),
// because `RouteTarget::Workflow(WorkflowId)` is a bridge-core type and core cannot depend on
// bridge-workflow. The graph types below live in `crates/bridge-workflow/src/graph.rs`.
use bridge_core::ids::{AgentId, WorkflowId, NodeId};

pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,        // topologically runnable; validated acyclic
}
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,                  // resolved via the registry
    pub prompt_template: String,         // loaded from prompt_file at config-parse time
    pub inputs: Vec<NodeId>,             // [] = consume the workflow input; [a,b,..] = fan-in on a,b
}
```

TOML shape (parsed in `bin/a2a-bridge`, prompt bodies loaded from files so long review prompts stay out of TOML):

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

- `inputs = []` → the node receives the **workflow input** (rendered as `{{input}}`).
- `inputs = ["codex","claude"]` → **fan-in** (depends on both; rendered as `{{codex}}` / `{{claude}}`).
- A chain of single-input nodes = a **pipeline**. Any DAG of the two is allowed.
- **Topology falls out of `inputs`** — fan-out (multiple nodes with `inputs=[]`), pipeline (single-input chain), fan-in/rollup (multi-input node). No `kind`/special node.
- **Terminal nodes** (no other node lists them in `inputs`) produce the workflow output. (W1 requires **exactly one** terminal — validated; multi-terminal is a later non-goal.)
- Each `agent` MUST exist in the registry (validated at config load).

## 3. The executor (`bridge-workflow/src/executor.rs`)

```rust
pub struct WorkflowExecutor { registry: Arc<dyn AgentRegistry> }

impl WorkflowExecutor {
    /// Run `graph` with `input`; stream node-level events; return the terminal output.
    pub fn run(&self, graph: &WorkflowGraph, input: String) -> WorkflowStream; // Stream<Item=Result<WorkflowEvent, BridgeError>>
}

pub enum WorkflowEvent {
    NodeStarted { node: NodeId },
    NodeFinished { node: NodeId, ok: bool },
    Done { output: String },   // the terminal node's output
}
```

**Execution:**
1. Validate the graph (acyclic; all `agent` ids resolvable; exactly one terminal). Done once at config load, re-checked defensively.
2. Topologically schedule: a node is **ready** when all its `inputs` have finished. **Independent ready nodes run concurrently** (fan-out: codex + claude in parallel); dependent nodes wait (synth after both). Use `futures` join over ready sets.
3. **Per node:**
   a. **Render** `prompt_template`: substitute `{{input}}` → the workflow input, and `{{<node-id>}}` → that upstream node's collected output. Simple literal `{{var}}` replacement (no templating engine — YAGNI; unknown `{{x}}` left verbatim + warns).
   b. `registry.resolve(&node.agent)` → `Resolved { backend, lease, .. }`; mint a **fresh `SessionId`** for this node-run (`workflow-{wf}-{node}-{nonce}` — nonce passed in, not generated in core, to keep determinism).
   c. `backend.prompt(&session, vec![Part{ text: rendered }])` → **drain the `BackendStream` to a `String`** (concatenate `Update::Text`; stop at `Update::Done`; `Update::Permission` is auto-handled/ignored — the api backend never emits it, and acp permission is policy-resolved). Drop the lease.
   d. Store `output[node] = text`; emit `NodeFinished`.
4. Emit `Done { output: terminal_output }`.

The executor is a **library** — the A2A producer and the CLI both call `run()`.

## 4. Triggers

### 4.1 A2A skill → `RouteTarget::Workflow`
- `RouteTarget` gains a variant: `Workflow(WorkflowId)` (`bridge-core::domain`). A one-arm-per-existing-variant match-update across the inbound server's `RouteTarget` consumers (the §1 enumeration) handles it.
- `SkillRoute::route` (`route.rs`) precedence — given the set of known workflow ids: `skill=="delegate"` → `Delegate`; `skill=="fan-out"` → `Fanout`; **else if `skill` is a known workflow id → `Workflow(id)`**; else → `Local(meta.agent.unwrap_or(default))`. (`SkillRoute` is constructed with the workflow-id set so the lookup is local, no error path needed — an unknown skill that isn't a workflow id simply falls through to `Local`.) The `delegate`/`fan-out`/`Local` arms are unchanged.
- The inbound server gains **`spawn_workflow_producer`** (mirrors `spawn_fanout_producer`): it takes the routed task's input parts → `input: String`, runs `executor.run(graph, input)`, maps `WorkflowEvent` → A2A task updates (`NodeStarted`/`NodeFinished` → Status events; `Done` → the final Artifact). The workflow run is **one A2A task** with the normal task lifecycle (cancel via the existing task-cancel path → executor cancellation).

### 4.2 CLI `run-workflow`
- `a2a-bridge run-workflow <workflow-id> --input <file> [--out <file>]`: loads config, builds `input` from the file, calls the **same** `executor.run`, prints node-level progress to stderr and the final output to stdout/`--out`. A thin wrapper — no new orchestration logic.

## 5. The `code-review` instance

Config = §2's `code-review` workflow. Three prompt files under `prompts/` (committed):
- `review-codex.md` — Codex-lensed review (blockers, correctness, regressions, test gaps), `{{input}}` = the diff/spec under review.
- `review-claude.md` — Claude-lensed review (architecture, seams, design), `{{input}}` = same.
- `review-synth.md` — synthesizer: `{{codex}}` + `{{claude}}` → one merged, de-duplicated review weighted by the `review-agent-roles` complementarity (Codex blockers/correctness, Claude architecture).

This **self-hosts** the dual-review loop: an A2A task `skill="code-review"` with the diff as the message → fan-out to both reviewers → `synth` merges → one review artifact. (The bridge dogfooding its own review loop is the W1 payoff.)

## 6. Errors, cancellation, validation

- **Acyclic + resolvable + single-terminal** validated at config load → **fail loud on startup** (consistent with `Registry::new`). A bad workflow keeps the last-good config (the existing `ConfigSource` behavior).
- **Node failure** (agent crash / non-2xx / timeout / drain error): the node's output becomes an **error marker string** (`"[node <id> failed: <reason>]"`); downstream nodes that depend on it receive that marker as `{{node}}`, and the `NodeFinished{ok:false}` event fires. The run **continues** — so a single reviewer failing still yields a `synth` from the surviving review (**graceful degradation**, the resilient choice for fan-out reviews). *(Rejected alternative: hard-fail the whole run on any node failure — too brittle for fan-out.)* If the **terminal** node itself fails, the workflow ends with `Done` carrying the error marker (the caller sees the failure).
- **Cancellation:** cancelling the workflow A2A task (existing cancel path) drops the executor stream → in-flight node `backend.prompt` streams are cancelled (each node holds a `cancel`-capable backend).
- **Bounded:** the DAG is acyclic (no loops); each node is one prompt turn; per-node timeout = the backend's request timeout. No retries in W1.

## 7. Output / streaming

The workflow is **one A2A task**. It streams **node-level** progress (`NodeStarted`/`NodeFinished` → A2A Status events, labeled by node id) and a **final Artifact** = the terminal node's output text. Internal per-node token streams are **collected to text**, not forwarded token-by-token (W1 simplicity). *(Labeled live token streaming across concurrent nodes = a later enhancement, like fan-out's labeled artifacts.)*

## 8. Crate structure & reuse

**New — `crates/bridge-workflow`** (modules: `graph.rs` types + validation, `executor.rs`, `template.rs` `{{var}}` rendering). Depends only on `bridge-core`. **New ids** `WorkflowId`/`NodeId` in `bridge-core::ids` (validated like `AgentId`). **`RouteTarget::Workflow(WorkflowId)`** in `bridge-core::domain`.

**Modified — `bin/a2a-bridge`:** `config.rs` parses `[[workflows]]` (+ loads each `prompt_file`'s contents at parse time) into a **`Workflows` map** (`HashMap<WorkflowId, WorkflowGraph>`) built in the binary alongside the registry and **rebuilt on each reconcile** (same `watch()` stream as `[[agents]]`); `route.rs` `SkillRoute` is constructed with the workflow-id set; `main.rs` wires the executor + the `Workflows` map (shared via `ArcSwap`/`Arc`, read by both `SkillRoute` and `spawn_workflow_producer`); the `run-workflow` subcommand. (Workflows do NOT ride on `bridge-core::RegistrySnapshot` — they stay a binary-level concern, since `bridge-workflow`/`WorkflowGraph` is not a `bridge-core` type.) **`crates/bridge-a2a-inbound`:** `spawn_workflow_producer` + the `RouteTarget::Workflow` arms.

**Reused, unchanged:** `AgentRegistry`/`resolve`/lease, `AgentBackend::prompt`/`BackendStream`, `ConfigSource`/reconcile/hot-reload, the inbound producer→`tx` streaming pattern, the task-cancel path.

## 9. Testing & Definition of Done

**Unit (`bridge-workflow`, fake registry + scripted fake backends):**
- DoD-1 — **fan-out parallel:** two `inputs=[]` nodes run concurrently (assert both prompted before either's downstream).
- DoD-2 — **pipeline:** a→b→c, each `{{prev}}` substituted; order enforced.
- DoD-3 — **fan-in:** `synth` receives BOTH upstream outputs in its rendered prompt (exact substitution asserted).
- DoD-4 — **template rendering:** `{{input}}`/`{{node}}` substitution; unknown var left verbatim.
- DoD-5 — **validation:** a cyclic graph and a multi-terminal graph and an unknown-`agent` graph are each rejected at construction.
- DoD-6 — **node-failure degradation:** a failing fan-out leg → its marker reaches `synth`; the run completes; `NodeFinished{ok:false}` emitted.
- DoD-7 — **cancellation:** cancelling the run mid-node ends promptly.

**Config + wiring:**
- DoD-8 — `[[workflows]]` parses + loads prompt files; bad DAG fails loud; `SkillRoute` maps `skill="code-review"` → `Workflow`.
- DoD-9 — **A2A e2e:** an `skill="code-review"` task through the inbound server with **fake/wiremock agents** → asserts node-level Status events + the synth output as the final artifact (the synth prompt contains both reviews).
- DoD-10 — **CLI smoke:** `run-workflow code-review --input <fixture>` over fake agents → prints the synth output.

**Gated live:** DoD-11 — `#[ignore]` run of `code-review` against real codex + claude (manual).

**Coverage:** new `bridge-workflow` HARD CI floor (propose 90, matching the other crates); workspace 85 / bridge-core 90 / bridge-acp 90 / bridge-api 90 unchanged. `clippy -D warnings`, `fmt`, full test green.

## 10. Scope boundary

**BUILDS:** `bridge-workflow` (graph + executor + template + validation); `WorkflowId`/`NodeId`/`RouteTarget::Workflow`; `[[workflows]]` config + hot-reload + prompt-file loading; `SkillRoute` workflow mapping + `spawn_workflow_producer`; the `run-workflow` CLI; the `code-review` instance + 3 prompt files; the CI floor; ADR-0009.

**NON-GOALS (later increments / YAGNI):** structured/typed review output — W1 output is **text** (W2); durable task store + submit/history surface (W3); research/dev workflow instances (W4); the log-triage pipeline instance (a config-only follow-on once the primitive ships); **labeled live token streaming** across nodes; **multi-terminal** workflows; **conditional/dynamic edges, retries, loops** (DAG only); per-node model/effort overrides in the workflow config (use the agent's registry config); replacing the `a2a-local-bridge` PoC wholesale (the multi-increment program this opens).

## 11. Review

_Dual review (Codex `gpt-5.5` + Claude `opus-4.8`) pending — launched detached via the `~/code/a2a-local-bridge` tooling. Findings fold into Revision 2 before the implementation plan._
