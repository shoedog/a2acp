# RFC — Real Agents, Part II: Persistent Memory & Agent-to-Agent Delegation

**Repo:** `a2a-bridge` · **Status:** proposal, no code changes · **Extends:** the Agent/Provider-split RFC ([`rfc-agents-workflows.md`](rfc-agents-workflows.md) — ADR-0033/0034/0035 proposals: `WorkflowCatalog`, `ProviderEntry`/`AgentDef`/`compose_agent`, pin-by-value run immutability) · **Interrogates:** ADR-0008's line and its 5 re-trigger conditions.
**Author:** Fable-5 architecture pass, 2026-07-17.

**The framing up front.** The previous RFC drew a bright line — *"agents are configuration, workflows are data, execution is ports; the bridge never plans"* — and put both of this RFC's subjects on the far side of it. This pass maps each capability as a spectrum and finds the line actually falls *through the middle* of both: each has a declarative form that folds cleanly into the pinned-definition model, and a runtime form that is the conductor/orchestrator fork ADR-0008 rejected. The adversarial finding of this pass: **one runtime form already exists in the codebase as an unguarded composition of two shipped features** (§2.4), and the honest resolution is a documented stance, not pretending the door is closed.

---

## 1. Persistent agent memory

### 1.1 What the system assumes today (the thing memory breaks)

Every workflow node turn is **stateless by construction**: `configure_session` → `prompt` → drain → `forget_session` (ADR-0009's per-node contract; `AgentBackend`, `ports.rs:46-101`). The warm session (ADR-0024) is the *one* sanctioned exception, and it is bounded to a single `implement` run. Persistence is deliberately shaped around this:

- `turn_log` (`bridge-store/src/sqlite.rs:165-191`) is **content-free** — metrics, ids, tokens, cost; no message text. Its eval index is load-bearing: `idx_turn_log_eval ON turn_log(prompt_id, model, effort)` (`sqlite.rs:194`) — the eval harness's comparability key assumes `(prompt_id, model, effort)` determines the effective prompt.
- Content-bearing rows (`task_node_checkpoints`, `task_journal`) are **task-scoped and cascade-deleted** (`FOREIGN KEY … ON DELETE CASCADE`, `sqlite.rs:140-164`) — content lives exactly as long as the run needs it.
- A run's *entire* definition is pinned by value at submit (`workflow_spec_json`, `detached.rs:1420`); resume replays from the pin, never from live state.

Memory — state that outlives tasks — is therefore not "a new table." It is a **new retention class** (long-lived, content-bearing, outliving cascade-delete) and a **new input class** (prompt content not derivable from the pinned definition). Both must be designed, not bolted on.

### 1.2 The spectrum

**M0 — status quo.** Curated context lives in `[[prompts]]` files and role prompts; the operator is the memory. Exists; ships today.

**M1 — read-only memory-as-context (recommended).** Operator-curated (or offline-produced) content attached to an agent, injected at render time exactly like the role prompt:

```rust
// bridge-core/src/domain.rs — extends the last RFC's AgentDef
pub struct AgentDef {
    …,
    pub role_prompt: Option<PromptRef>,
    pub memory: Option<MemoryRef>,        // NEW
}
pub enum MemoryRef {
    File(String),                          // resolved at snapshot build, like prompt_file
    Store { scope: MemoryScope },          // read from MemoryStore at RUN START
}
pub enum MemoryScope {
    Agent,                                 // key = agent_id
    AgentRepo,                             // key = (agent_id, canonical repo path) —
                                           // same canonical-path keying ADR-0028 forced for prism
    TaskLineage,                           // key = (agent_id, root task id) — resumable chains
}
```

```rust
// bridge-core/src/ports.rs — new port; impl in bridge-store
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn read(&self, key: &MemoryKey) -> Result<Option<MemorySnapshot>, BridgeError>;
    /// Writes ONLY via the curated path (CLI verb / distillation workflow output) — never mid-turn.
    async fn write(&self, key: &MemoryKey, content: &str, provenance: &Provenance)
        -> Result<MemoryEpoch, BridgeError>;
}
pub struct MemorySnapshot { pub content: String, pub epoch: MemoryEpoch /* monotonic + hash */ }
```

```sql
CREATE TABLE IF NOT EXISTS agent_memory (
    agent_id   TEXT NOT NULL,
    scope_key  TEXT NOT NULL,     -- '' | canonical repo path | root task id
    epoch      INTEGER NOT NULL,
    content    TEXT NOT NULL,     -- size-capped (memory_max_bytes, journal_max_bytes precedent)
    content_sha TEXT NOT NULL,
    provenance TEXT NOT NULL,     -- 'operator' | 'workflow:<id>@<def_hash>:<task_id>'
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (agent_id, scope_key, epoch)
);
```

**Read path — pin at run start, not per turn.** This is the decision that keeps every existing invariant intact: memory is read **once at submit**, and pinned into the run snapshot alongside the graph — `{"v":1,"graph":…,"def_hash":…,"memory":{"<agent_id>":{"epoch":…,"content":…}}}` (additive serde-default field; `SUPPORTED_SNAPSHOT_VERSION` stays 1). Consequences, each falling out of pin-by-value:

- **Resume** replays with the *same* memory the run started with, even if the epoch advanced — exactly how the graph already behaves (`resume_one_working_task` reads only the snapshot, `detached.rs:1463`).
- **Warm sessions:** memory rides the existing `NodeTurn.seed` seam (`executor.rs:63`, "warm-only; prepended to the node prompt parts") — injected once at warm mint, stable for the session = stable for the run. No new staleness class beyond what a run already is.
- **Parallel batch runs** read snapshots independently; no read-consistency problem.
- **Determinism/eval:** a run is again a pure function of its pinned envelope. The eval hole is closed by **stamping**: add `memory_epoch`/`memory_sha` to `TurnContext` (`ports.rs:242`) and `turn_log` (additive column via the `migrate_tasks_columns` PRAGMA pattern, `sqlite.rs:204`). `(prompt_id, model, effort)` comparability becomes `(prompt_id, model, effort, memory_sha)` — the harness can hold memory fixed or ablate it deliberately. Without the stamp, M1 silently corrupts every eval; with it, memory becomes a *controlled variable*.

**M2 — bridge-written, declaratively distilled.** Memory writes happen only as the output of a **declared workflow** (e.g. a `distill-review-lessons` DAG whose terminal output is written to `agent_memory` with `provenance = workflow:<id>@<def_hash>:<task_id>`), triggered by the operator or an explicit post-run hook — never by an agent mid-turn. This stays inside "declared graphs": *what* writes memory, *from what inputs*, is a pinned definition; the loop `run → distill → next run reads` is visible, auditable, epoch-versioned, and revertible (`epoch` is append-only; rollback = re-point). It is also exactly ADR-0012's boundary discipline applied to memory: structure (a curated summary) is produced at a declared boundary by a dedicated step, not smeared through the system.

**M3 — read-write learning (the fork, memory edition).** The agent writes memory *during* turns — via an MCP memory tool or a new `Update` variant. Be honest about how much bigger this is than M2:

1. **A write channel** through the turn stream or a bridge-offered MCP server (new protocol surface, new `Update` handling in the executor's drain loop).
2. **Concurrency control**: parallel fan-out nodes and concurrent batch runs writing one key — last-write-wins corrupts, merging is an unsolved-in-general problem.
3. **Security — cross-tier memory laundering.** This is the disqualifying hazard today. The sandbox tier model (ADR-0032) exists because tier-2 agents read *adversarial* content. If a tier-2 reader can write memory that a tier-3 implementor later reads as trusted context, memory becomes a **persistence channel for prompt injection across the quarantine boundary** — defeating the exact threat model the tiers exist for. M3 requires a per-tier memory trust/taint model before it is even safe to design.
4. **Eval/reproducibility:** M3 makes the *within-run* prompt stream non-reproducible — no stamp fixes that; runs stop being replayable at all.
5. **Growth/compaction:** unbounded accretion needs summarization policy — which is itself an agent judgment call, i.e. M3 recursively needs M2's machinery anyway.

M3 is the "shared cross-agent context" row of the conductor review when scoped per-agent-shared-across-time, and literally re-trigger #3 when shared across agents. The review's verdict stands and is worth quoting: *"agent **independence** is the value (Codex and Claude review the same input independently) … **IGNORE for now.** Don't couple the reviewers"* (`docs/conductor-pattern-review.md:49`).

---

## 2. Delegation / agents-as-sub-workflows

### 2.1 D-a — Declarative sub-workflow nodes (nested DAGs): **fold in, by flattening**

Target shape, extending the last RFC's `WorkflowNode`:

```rust
// bridge-workflow/src/graph.rs
pub enum NodeTarget {
    Agent(AgentId),          // today's behavior — serde-compat via the existing `agent` field
    Workflow(WorkflowId),    // NEW: composite node = another catalog entry
    Peer(PeerId),            // §2.2
}
pub struct WorkflowNode {
    pub id: NodeId,
    pub target: NodeTarget,          // replaces `agent`; custom Deserialize accepts
                                     // `agent` XOR `workflow` XOR `peer` (exactly one, loud)
    pub prompt_template: String,     // for Workflow targets: renders the input handed to the child's roots
    pub inputs: Vec<NodeId>,
    pub retry: Option<RetryPolicy>,  // v1: ERROR on Workflow targets (child nodes own their retries)
    pub overrides: Option<AgentOverride>, // v1: ERROR on Workflow targets
}
```

**The load-bearing decision: expand at submit, don't recurse at runtime.** Two candidate semantics:

- **Flatten-at-submit (chosen).** `run_workflow` asks the catalog to resolve `Workflow` targets transitively and inline the child graph into the parent: child node ids namespaced (`sub.correctness`), the child's **single terminal** (guaranteed by `validate()`'s `NotSingleTerminal` rule, `graph.rs:98-105`) renamed to the composite node's id so parent `inputs: ["sub"]` resolve unchanged, child roots receive the composite node's rendered `prompt_template` as `{{input}}`. The result is **one flat `WorkflowGraph`**, pinned by value into `workflow_spec_json` exactly as today.
- **Child-task recursion (rejected for D-a).** The sub-run as its own durable `TaskRecord` with a parent pointer. Independently watchable — but it drags in nested resume, journal composition across tasks, cancel propagation, and batch-admission interactions, for zero declarative gain.

Flattening makes every hard question disappear into existing machinery:

| Concern | Answer under flattening |
|---|---|
| **Durability/resume** | Unchanged. `task_node_checkpoints (task_id, node_id)` (`sqlite.rs:140`) checkpoints every namespaced child node at full granularity; `resume_one_working_task` replays the flat pinned graph — a mid-child crash resumes *inside* the child. No nested snapshots exist to invent. |
| **Streaming/reattach (ADR-0015)** | Unchanged. One task, one `task_journal` seq cursor; `NodeStarted/Finished` events carry namespaced ids a UI can group by prefix. |
| **Cycle detection** | Two layers, both static: `catalog.apply` validates the **reference graph** (edges = `Workflow` targets) acyclic with the same Kahn algorithm `assert_acyclic` already uses (`graph.rs:121`) — a workflow can never reference itself transitively, so flattening terminates; then plain `validate()` runs on the flattened graph. |
| **Two-engines (ADR-0024)** | Untouched. Flattening happens above the executor; `resolve_impl_identity` still sees single-node graphs on the warm path; the constraint's wording — "the warm edit/fix path cannot gain multi-node graphs" — is not even approached. |
| **`def_hash`** | The **flattened** graph is what's hashed and pinned — a child edit changes every parent's effective hash, which is exactly the provenance truth. |

D-a is therefore *authoring reuse* — a macro over declared graphs. Nothing about execution becomes dynamic.

### 2.2 D-b — Static peer delegation as a node target: **cheap, the port already exists**

`DelegationPort` (`ports.rs:119`) and `PeerDelegation` (`bridge-a2a-outbound/src/lib.rs:23-60`) already do this for `RouteTarget::Delegate`: `delegate(auth, local_task, parts) -> Delegation { events, peer_task }` over SSE, with `cancel(peer_task)`. `NodeTarget::Peer` wires that into the executor's node runner: render the prompt → `delegate` → drain the event stream into node output text → checkpoint. Cancel maps the node's token to `DelegationPort::cancel`. Config: v1 restricts `Peer` to the single configured `[delegation]` peer (multi-peer `[[peers]]` is additive later).

Honest caveats, stated in the ADR: (1) **resume re-delegates** — a resumed peer node re-submits to the remote; this mirrors how any resumed node re-prompts, but the side effect is remote, so note the idempotency expectation on the peer; (2) **identity forwarding is still deferred** — the code says so itself: *"v1 caller-identity forwarding is deferred to a later increment"* (`lib.rs:21-22`); a peer node runs under the bridge's configured bearer, not the original caller's identity. Neither blocks a single-operator deployment; both matter before any multi-tenant story.

### 2.3 D-c — Runtime, agent-initiated, LLM-chosen delegation: **the planner line**

The variant where an agent *decides mid-turn* to invoke another agent or workflow — the graph stops being the execution plan; topology becomes emergent. What it actually requires: a delegation tool surface offered into agent context; runtime cycle/depth/budget guards (static acyclicity can't help — the "graph" no longer exists before execution); an authz matrix (which agent may invoke what — `bridge-policy` bloat, verbatim re-trigger #4); target enumeration (discovery, re-trigger #2); durable semantics for a half-completed *emergent* tree that pin-by-value cannot express (there is no definition to pin); and, combined with memory, re-trigger #3. This is the point where the bridge stops executing declared graphs and starts hosting a planner.

### 2.4 The adversarial finding: the loopback door is already open

The bridge's MCP adapter exposes `run`, `continue`, `inject`, `permit`, `run_workflow`, `status`, `clear`, `cancel_task` as tools (`crates/bridge-mcp/src/transport.rs:71-180`), backed by the same `Coordinator`. And ADR-0028 lets any `[[agents]]` entry carry arbitrary stdio MCP servers. **Composing the two — an `[[agents.mcp]]` entry pointing at `a2a-bridge mcp` — hands a bridge-managed agent the ability to invoke bridge workflows mid-turn. D-c exists today, with zero new code, as an unguarded configuration.** Nothing validates against it; nothing caps recursion (workflow → agent → `run_workflow` → same workflow…), and a self-call chain also interacts with the batch concurrency cap (a parent turn blocking on a child admission it is itself occupying a slot for).

Two honest readings. First, the *sanctioned* version of this composition is fine and already load-bearing: an **external** agent (Claude Code as the caller) driving `a2a-bridge mcp` is precisely ADR-0008's model — *the orchestrator is the caller*; the planner sits outside the bridge, which stays a deterministic executor. Second, the *loopback* version (bridge-managed agent → bridge's own MCP) silently moves the planner inside with none of the guards D-c would demand. Recommendation: an explicit stance ADR (§4) that (a) documents external-caller MCP as the supported "agents calling agents" path, (b) declares loopback **unsupported**, and (c) adds the cheap guard: stamp a call-depth marker (env var or MCP client-info) through `a2a-bridge mcp` spawns and have the Coordinator reject `run_workflow` above depth 1 unless explicitly configured. Fail loud, not silent.

---

## 3. ADR-0008 verdict, per variant

ADR-0008's re-triggers: **#1** multi-hop agent graphs · **#2** dynamic discovery · **#3** shared cross-agent session/context · **#4** routing-policy complexity · **#5** self-hosting the dev workflow.

| Variant | Verdict | Re-triggers hit | Evidence present today? |
|---|---|---|---|
| **M1** read-only memory-as-context | Inside the line — pinned content, declarative, epoch-stamped | none (it's `role_prompt`++ with a store) | Yes — role/context curation already happens manually in prompt files (#5 pressure) |
| **M2** declared distillation writes | Inside the line — the *writer* is a pinned workflow; append-only, provenance-stamped | brushes #3 only if a distilled memory is shared across agents — keep scope per-agent | Partial — plausible next step of #5; build the seam when a concrete distill workflow exists |
| **M3** in-turn read-write learning | **Over the line** | #3 (and forces #4 for write-policy) | **No** — and disqualified independently by cross-tier laundering (§1.2) until a taint model exists |
| **D-a** flattened sub-workflows | Inside the line — still one declared, pinned, acyclic DAG. ADR-0008 #1 itself classifies chain-of-brains DAGs as *"BUILD-greenfield"*, and the conductor review's "hierarchical nesting → IGNORE" targets proxy-chain trees, not composed declared DAGs | none | Yes — #5 is live: review DAGs already share synth/reviewer sub-structures that beg for reuse |
| **D-b** peer node target | Inside the line — static, declared, single-hop per node, existing port | brushes #1 (remote brains in a DAG) — the greenfield-build branch of it, per the review's *"middleware-chain ≠ brain-pipeline"* | Yes, weakly — `[delegation]` exists and is tested; the node form is a small generalization |
| **D-c** runtime agent-chosen delegation | **Over the line — this is the fork** | #1 + #2 + #4 (and #3 when paired with memory) | **No.** Every operator use case (reviews, implement, panels) is declared-topology. And the sanctioned form already exists *outside* the bridge (external caller via MCP) |
| **Loopback** (§2.4) | Over the line *by accident* — needs a stance, not a feature | #1/#4 latent | n/a — close the door loudly |

**Naming the fork.** The bridge stops being a bridge at the moment either of two things is decided by an agent at runtime instead of by a pinned definition: **execution topology** (D-c) or **prompt content lineage** (M3). Cross either and the same properties fall together: pin-by-value resume (nothing to pin), eval reproducibility (nothing to replay), the tier model's containment (memory/topology as cross-tier channels), and the "one honest engine" story ADR-0024 already strains under. What the operator would buy: adaptive, self-extending behavior — an agent framework. That is a legitimate thing to want; it is a *different project*, and per ADR-0008's escalation rule the move would be partial-adopt of specific patterns under evidence, never a wholesale pivot. The evidence table today reads "No" on every runtime row.

---

## 4. How this extends the last RFC — deltas, ADRs, migration, phases

### 4.1 Type/port deltas (cumulative over ADR-0033/0034)

- `AgentDef` += `memory: Option<MemoryRef>` (§1.2). `ProviderEntry` unchanged — memory is an *agent* concern, which the split makes expressible at all (fused entries would smear memory onto the substrate).
- `WorkflowNode.agent: AgentId` → `target: NodeTarget { Agent | Workflow | Peer }` with back-compat deserialization (`agent` field keeps parsing; exactly-one-of, loud otherwise).
- `WorkflowCatalog.apply` += reference-graph acyclicity (Kahn over `Workflow` targets) + transitive **flatten** capability used at submit; `def_hash` computed over the flattened form.
- New port `MemoryStore` (`bridge-core/src/ports.rs`), SQLite impl in `bridge-store` (`agent_memory` table, §1.2); `TurnContext` += `memory_epoch/memory_sha`; `turn_log` += same (additive PRAGMA migration).
- Run snapshot envelope += `"memory"` map (serde-default; version stays 1; old snapshots resume).
- Executor: `NodeTarget::Peer` arm in the node runner over the existing `DelegationPort`; no signature change — still takes `Arc<WorkflowGraph>` (flat).

### 4.2 New ADRs

- **ADR-0036 — Sub-workflow composition by flattening.** NodeTarget, submit-time expansion, ref-graph acyclicity, terminal-renaming rule, retry/overrides-on-composite = error, flattened `def_hash`. Amends ADR-0009's node model; preserves ADR-0015/0024 untouched by construction.
- **ADR-0037 — Peer node targets.** `NodeTarget::Peer` over `DelegationPort`; single-peer v1; resume-re-delegates and identity-deferral caveats recorded.
- **ADR-0038 — Agent memory as pinned context.** `MemoryStore`, scopes, run-start pinning, epoch/sha stamping in `TurnContext`/`turn_log`, size caps, the M2 distillation seam (defined now, built on demand), and the retention-class change (first long-lived content-bearing table — with the explicit note that `turn_log` stays content-free).
- **ADR-0039 — The planner line (decision-not-to-build, ADR-0012 style).** D-c and M3 as documented non-goals with their re-trigger mapping; external-caller-via-MCP canonized as the supported "agents calling agents" path; the **loopback guard** (depth stamp + reject-above-depth-1) as the enforcement. This ADR is the cheap insurance that "should the bridge plan?" is not re-litigated ad hoc.

### 4.3 Migration & back-compat

Every existing TOML and every persisted snapshot keeps working: `agent =` on nodes parses as `NodeTarget::Agent` (golden parity across `examples/*.toml`, same discipline as the split's `compose_agent` parity gate); envelope fields are serde-default additive; `turn_log`/`tasks` columns use the existing `migrate_tasks_columns` idempotent-ALTER pattern; `agent_memory` is a new table, absent = M0 behavior. No `SUPPORTED_SNAPSHOT_VERSION` bump.

### 4.4 Phases (spec → dual-adversarial-review → live-gate sized)

- **P-G (first, tiny): the loopback guard + ADR-0039.** Docs + depth stamp + Coordinator rejection. *Live gate:* wire an agent with the bridge's own MCP, observe the loud rejection.
- **P-1: `NodeTarget` + flatten + ref-cycle validation** (rides the prior RFC's W1/W2 catalog work). *Live gate:* a `code-review` variant embedding a shared `synth-pair` sub-workflow; kill `serve` mid-child-node; resume completes inside the child.
- **P-2: Peer node target.** *Live gate:* a DAG with one remote node against a second local `serve` as peer; cancel propagates.
- **P-3: `MemoryStore` + M1** (File + Store refs, run-start pinning, stamps, `memory` CLI verb for curated writes). *Live gate:* two identical runs, memory epoch bumped between them; `turn_log` rows differ only in `memory_sha`; resume of run 1 provably uses pinned epoch 1.
- **P-4 (evidence-gated): M2 distillation** — only when a concrete distill workflow is actually wanted; it's config + one write call on an existing port by then.

**Do NOT build:** M3 in-turn writes; D-c internal runtime delegation; multi-peer discovery/enumeration; shared cross-agent memory scopes; child-task recursion for sub-workflows; any memory auto-summarization inside the bridge.

---

## 5. Recommendation

**Build:** D-a (flattened sub-workflows) and M1 (read-only pinned memory-as-context), plus the cheap P-G guard first and D-b (peer nodes) opportunistically — it's mostly wiring an existing tested port into the node runner. This subset delivers the real operator value on the table — reusable DAG components and per-agent curated knowledge — while every core invariant (pin-by-value resume, content-free `turn_log`, eval comparability via stamping, static acyclicity, the two-engines constraint, tier containment) survives *by construction*, because both capabilities resolve to pinned data before the executor ever runs. M2's seam is defined in ADR-0038 but built only against a concrete distillation use case.

**Documented non-goals pending real evidence (ADR-0039):** runtime agent-initiated delegation and read-write learning memory. The first is already served in its sanctioned form — the planner as *caller* over `a2a-bridge mcp` — and its internal form re-triggers #1/#2/#4 with no present evidence; the second is disqualified today by cross-tier memory laundering independent of any architecture-taste argument. If the operator later genuinely wants the agent-framework fork, that is a new project decision made against ADR-0008's escalation rule with eyes open — not an increment, and the trade is named: adaptivity purchased with reproducibility, resumability, and the containment model.
