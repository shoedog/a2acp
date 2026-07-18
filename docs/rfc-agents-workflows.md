# RFC — Runtime-Mutable Workflows & the Agent/Provider Split

**Repo:** `a2a-bridge` · **Status:** proposal, no code changes · **Revisits:** ADR-0005; amends ADR-0009's boot-load clause; preserves ADR-0008, ADR-0012, ADR-0024.
**Author:** Fable-5 architecture pass, 2026-07-17.
**Companion:** [`rfc-agents-workflows-diagram.html`](rfc-agents-workflows-diagram.html) (diagrams) · [`rfc-agents-workflows-part2-memory-delegation.md`](rfc-agents-workflows-part2-memory-delegation.md) (Part II — memory & delegation). The Phase-0 defect is filed as [issue #35](https://github.com/shoedog/a2acp/issues/35).

---

## 1. Current model, precisely

### 1.1 What an "agent" is today: a provider binding with agent-ish barnacles

`AgentEntry` (`crates/bridge-core/src/domain.rs:121`) is one flat struct fusing three concerns:

```rust
pub struct AgentEntry {
    pub id: AgentId,
    // — execution substrate (provider) —
    pub cmd: Option<String>,           // acp: process to spawn
    pub base_url: Option<String>,      // api: OpenAI-compatible endpoint
    pub api_key_env: Option<String>,
    pub args: Vec<String>,
    pub kind: AgentKind,               // Acp | Api | ContainerRw
    pub auth_method: Option<String>,
    pub sandbox: Option<SandboxConfig>,     // tier 2/3 containment
    pub watchdog: Option<WatchdogConfig>,
    // — model tuning (substrate defaults) —
    pub model: Option<String>, pub effort: Option<Effort>, pub mode: Option<String>,
    pub model_provider: Option<String>,     // metadata only, never on the wire
    // — agent-ish things already living here —
    pub mcp: Vec<McpServerSpec>,            // per-agent tools (ADR-0028)
    pub mcp_delivery: McpDelivery,
    pub cwd: Option<String>, pub session_cwd: Option<String>,
    pub name/description/tags/version, pub extensions: BTreeMap<String, toml::Value>,
}
```

The operator's read is **correct and validated by the code**, with one sharpening: an entry is *not purely* a provider binding — it already carries per-agent tools (`mcp`, ADR-0028) and identity metadata. What it has **none** of is role: no system/role prompt, no persona. Role lives entirely in the workflow node's `prompt_template`. The concrete evidence that the fusion hurts is in the shipped configs: `examples/a2a-bridge.tiers.toml` and `a2a-bridge.containerized.toml` define **four+ entries over two binaries** (`tier0-review`, `tier1-codex-ro`, `tier2-reader`, `tier3-impl`) — the role/tier/tool axis is already forcing duplication of the substrate axis.

Execution: `AgentBackend` (`crates/bridge-core/src/ports.rs:46`) — `prompt`/`prompt_observed`/`cancel`/`configure_turn`/`configure_session`/`forget_session`/`release_session`/`reconcile_config`/`capabilities`/`retire`. Per-session tuning is layered via `effective_config(entry, override?)` (`domain.rs:195`) and applied at lazy ACP mint (`configure_session` → `session/set_config_option`). Routing is `RouteTarget { Local(AgentId), Delegate, Fanout, Workflow(WorkflowId) }` (`domain.rs:234`).

### 1.2 The registry: the live-mutable machinery ADR-0005 built

`Registry` (`crates/bridge-registry/src/registry.rs:96`): `ArcSwap<State{ slots: HashMap<AgentId, Arc<Slot>>, default }>`; `Slot { entry: ArcSwap<AgentEntry>, backend: OnceCell<Arc<dyn AgentBackend>>, retired, leases, lease_notify }`; lazy exactly-once spawn via injected `SpawnFn` (`registry.rs:19`); RAII `LeaseGuard`; `apply()` (`registry.rs:372`) validates first, reuses the live slot for a "config-only" edit, creates cold slots otherwise, retires by `Arc::ptr_eq` identity with lease-draining + 30 s grace. The reconcile loop (`bin/a2a-bridge/src/main.rs:6004-6027`) consumes `ConfigSource::watch()` and calls `reg.apply(snap)` per file change (200 ms debounce, atomic-rename-safe, last-good-on-error).

**Found defect (WRONG, pre-existing, Shift-2-relevant).** The warm-reuse criterion (`registry.rs:388-402`) compares `cmd, base_url, args, cwd, auth_method, kind, sandbox, session_cwd, api_key_env` — its own comment says spawn-frozen fields "MUST force a fresh slot" — but **omits `mcp`/`mcp_delivery`/`watchdog`, which are also frozen at spawn** (codex MCP renders into argv via `render_codex_mcp_args` in `acp_spawn_inputs`; claude's MCP set rides `AcpConfig.mcp` read at `new_session_request`; watchdog forwards into `AcpConfig` — test `acp_spawn_inputs_forwards_watchdog`, `main.rs:6720`). Concrete failure: edit `[[agents.mcp]]` on an agent with a warm backend → hot-reload takes the config-only path → new sessions still get the **old** MCP set until an unrelated field forces a respawn. Today it's a quiet foot-gun; under Shift 2 (agents differentiated chiefly *by* toolset) it becomes a first-order correctness bug. Fix belongs in Phase 0 regardless of either shift.

The port seams ADR-0005 deferred **already exist as code**: `ConfigStore: ConfigSource { upsert(AgentEntry); remove(id) }` (`ports.rs:386` — "3b.2+ admin API write-back — defined now, impl'd later"); deferred list also names DB/remote `ConfigSource` adapters and per-entry AgentCards.

### 1.3 What a workflow is, and exactly why it's boot-fixed

`WorkflowNode { id: NodeId, agent: AgentId, prompt_template: String, inputs: Vec<NodeId>, retry: Option<RetryPolicy> }`; `WorkflowGraph { id, nodes, panel }` with `validate()` = non-empty, unique ids, known inputs, acyclic (Kahn), exactly one terminal (`crates/bridge-workflow/src/graph.rs:50-141`). Note: the node's prompt is **fully resolved text** — `Config::load_workflows` (`bin/a2a-bridge/src/config.rs:1220`) inlines `prompt`(registry id)/`prompt_file`/`prompt_text` at load and cross-checks `node.agent` against the declared `[[agents]]` ids. The graph that reaches the engine carries no file references.

The map is then frozen: `Coordinator { workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>, … }` (`crates/bridge-coordinator/src/coordinator.rs:168`), constructed once by `build_coordinator` (`main.rs:701`); the A2A inbound server gets the same map via `.with_workflows` at boot. ADR-0009 states the reason: *"Load-once at boot (not hot-reload) — removes a `RegistrySnapshot`-vs-workflows TOCTOU class; agents stay hot-reloadable, workflows do not."* The protected invariant is **validation-time referential integrity** (a workflow validated against agent ids can't race a registry edit) — not run-time integrity, because:

### 1.4 The decisive existing fact: runs are already def-immutable

`Coordinator::run_workflow` (`coordinator.rs:651`) looks up the graph, then **persists the entire graph into the durable task record**: `workflow_spec_json = Some(encode_workflow_spec(&graph))` — envelope `{"v":1,"graph":…}` (`detached.rs:1420`, `SUPPORTED_SNAPSHOT_VERSION=1`, single construction site shared with the A2A unary path). The executor takes `graph: Arc<WorkflowGraph>` per run (`executor.rs:853-951`) and never re-reads the map. Crash-resume (`resume_one_working_task`, `detached.rs:1463`) reconstructs the graph **from the stored snapshot**, version-checked, `Interrupted` if absent/unreadable — it never consults config. And at dispatch the executor resolves `node.agent` against the *live* registry anyway, so an agent removed mid-run already surfaces as a node error under ADR-0009's graceful-degradation rule.

**Conclusion for Shift 1:** the hard problem ("what happens to in-flight/durable runs when a def mutates?") is *already solved by the run-snapshot architecture*. Boot-load protects only cross-validation atomicity — a much smaller thing, closable by construction.

### 1.5 The other fixed points

`[server]`, `[store]` read once at boot (unchanged by this RFC). The `implement` loop runs on **two engines** (ADR-0024 HARD CONSTRAINT): edit/fix via `warm.prompt` + `drain_turn`; review via the executor; `resolve_impl_identity` (`main.rs:1243`) rejects anything but a single-node graph on the warm path. ADR-0008 confirmed greenfield, rejected the conductor, and fixed the escalation as partial-adopt; ADR-0012 keeps output markdown-at-the-boundary.

---

## 2. Shift 1 — runtime-mutable workflow definitions

### 2.1 Shape: a `WorkflowCatalog`, deliberately *not* a second Registry

A workflow definition, unlike an agent, owns **no runtime resources** — no process, no leases, no retirement. It is pure data. So the catalog is an atomic map swap, not a slot machine:

```rust
// crates/bridge-workflow/src/catalog.rs (new; trait could live in bridge-core if the
// coordinator should not depend on a concrete type — but Coordinator already depends
// on bridge-workflow for WorkflowExecutor, so a concrete struct is fine and simpler).
pub struct WorkflowCatalog {
    defs: arc_swap::ArcSwap<HashMap<WorkflowId, Arc<VersionedGraph>>>,
}
pub struct VersionedGraph {
    pub graph: WorkflowGraph,          // prompt_templates fully resolved, as today
    pub def_hash: DefHash,             // sha256 over canonical serde_json of `graph`
}
impl WorkflowCatalog {
    pub fn get(&self, id: &WorkflowId) -> Option<Arc<VersionedGraph>>;
    pub fn list(&self) -> Vec<(WorkflowId, DefHash)>;
    /// Atomic replace-all. Every graph is ALREADY validated by the caller
    /// (parse layer), but validate() is re-run here defensively — a catalog
    /// can never hold an invalid graph.
    pub fn apply(&self, defs: Vec<VersionedGraph>) -> Result<(), BridgeError>;
}
```

`Coordinator.workflows` becomes `Arc<WorkflowCatalog>`; the lookup at `coordinator.rs:663` and the inbound server's `RouteTarget::Workflow` arm change from map-get to catalog-get. The executor's signature **does not change** — it keeps taking `Arc<WorkflowGraph>` per run. That signature is now a load-bearing invariant: *the engine can never observe a def mutation mid-run.* Write that into the ADR.

### 2.2 The apply path: one file, one snapshot, ordered apply — TOCTOU closed by construction

Do **not** add a second watcher. Widen the existing reconcile so registry and workflows derive from the *same parse* of the same file:

```rust
// bridge-core/src/ports.rs — widen the source's item (back-compat via a default impl
// or a new trait; see §5):
pub struct ConfigSnapshot {
    pub registry: RegistrySnapshot,
    pub workflows: Vec<VersionedGraph>,   // resolved + validated against `registry`
}
pub trait UnifiedConfigSource: Send + Sync {
    async fn load(&self) -> Result<ConfigSnapshot, BridgeError>;
    fn watch(&self) -> BoxStream<'static, ConfigSnapshot>;
}
```

Reconcile loop (`main.rs:6004`) becomes: parse file → build `RegistrySnapshot` → run `load_workflows`-equivalent **against that same in-memory snapshot's agent ids** → if either fails, keep last-good for *both* and log (mirroring today's parse-error behavior) → `registry.apply(snap.registry)` **then** `catalog.apply(snap.workflows)`. Ordering means a workflow can never be published referencing an agent the *published* registry lacks. This is strictly stronger than today's boot check, because it re-establishes the invariant on every edit instead of once.

Prompt indirection: `prompt_file`/`prompt` ids are re-read/re-resolved at each reload, exactly as at boot — a def's identity is its **resolved** form, which is what `def_hash` hashes. An editor-driven upsert therefore produces a deterministic hash regardless of whether it wrote `prompt_text` or a registry id.

### 2.3 Versioning, durability, resume

- **Pin-by-value stays the mechanism** (it already is): the run's `workflow_spec_json` is the authoritative def for that run and for resume. No def-version indirection table — a run must remain resumable after its def is edited *or deleted*.
- **Additive provenance:** stamp `def_hash` into the envelope — `{"v":1,"graph":…,"def_hash":"…"}`. `WorkflowSpecEnvelope` gains an `Option<String>` field (serde-default), so old snapshots resume unchanged and `SUPPORTED_SNAPSHOT_VERSION` stays 1. Surfaces "which version produced this run" in `task get`/traces; the operator UI's workflow screens want exactly this.
- **Resume-attempt semantics unchanged** (`resume_attempt_cap`, poison-pill guard) — resume never touches the catalog.

### 2.4 Where definitions live; the editor's write path

**File remains the single source of truth.** The DAG editor writes through a validated write-back path, not a parallel store:

```rust
// Extends the existing (already-declared, never-implemented) write seam, ports.rs:386:
#[async_trait]
pub trait WorkflowConfigStore: Send + Sync {
    /// Validate against the CURRENT registry snapshot + DAG rules, then persist
    /// (toml_edit surgical edit + atomic rename). The file watcher is the apply path.
    async fn upsert_workflow(&self, def: WorkflowGraphSpec) -> Result<DefHash, BridgeError>;
    async fn remove_workflow(&self, id: &WorkflowId) -> Result<(), BridgeError>;
}
```

Exposed as `Coordinator` methods → the MCP adapter (`a2a-bridge mcp`) gets `workflow upsert/remove/list` for free, and a CLI verb follows. **No admin HTTP routes in `serve`** — consistent with the operator-UI RFC's sidecar stance and ADR-0005's still-deferred 3b.2. A DB-backed def store is explicitly **deferred** (ADR-0012-style decision record): two SSOTs is the real risk here; revisit only if concurrent multi-writer editing materializes.

### 2.5 Small consequential surfaces

- **Agent Card skills:** ADR-0009 advertises one A2A skill per workflow; the card is built at serve start. Rebuild the skill list on `catalog.apply` (the `agent-models` extension already refreshes on SIGHUP — reuse that refresh path).
- **`validate` CLI** already runs the full parse+cross-check; it becomes the dry-run for any editor write, unchanged.
- **`run-workflow` (offline CLI)** builds its own registry+graphs per invocation — untouched.

---

## 3. Shift 2 — the Agent/Provider split

### 3.1 The concept split, sharpened against the code

| Concern | Today | Target |
|---|---|---|
| Execution substrate (adapter, process/http, auth, sandbox tier, watchdog) | `AgentEntry` core | **`ProviderEntry`** |
| Model/effort/mode defaults | `AgentEntry` | Provider defaults, overridable per Agent |
| Tools (MCP set) | `AgentEntry.mcp` (ADR-0028) | **`AgentDef.mcp`** |
| Role / system prompt | workflow node `prompt_template` only | **`AgentDef.role_prompt`** ⊕ node template |
| Identity/metadata | `AgentEntry.name/description/tags` | `AgentDef` |
| Composition target of a workflow node | `node.agent → AgentEntry` | `node.agent → AgentDef` (id unchanged) |

```rust
// bridge-core/src/domain.rs (new types; AgentEntry survives as the COMPOSED form)
pub struct ProviderEntry {
    pub id: ProviderId,
    pub kind: AgentKind,                    // Acp | Api | ContainerRw — unchanged
    pub cmd: Option<String>, pub args: Vec<String>,
    pub base_url: Option<String>, pub api_key_env: Option<String>,
    pub auth_method: Option<String>,
    pub sandbox: Option<SandboxConfig>,     // the tier is a property of the substrate
    pub watchdog: Option<WatchdogConfig>,
    pub model: Option<String>, pub effort: Option<Effort>, pub mode: Option<String>,
    pub model_provider: Option<String>,
}
pub struct AgentDef {
    pub id: AgentId,
    pub provider: ProviderId,
    pub role_prompt: Option<PromptRef>,     // Registry(PromptId) | Inline(String)
    pub mcp: Vec<McpServerSpec>,
    pub model: Option<String>, pub effort: Option<Effort>, pub mode: Option<String>, // override provider
    pub cwd: Option<String>, pub session_cwd: Option<String>,
    pub name: Option<String>, pub description: Option<String>, pub tags: Vec<String>,
}
/// THE keystone: composition happens at snapshot build, producing exactly today's type.
pub fn compose_agent(def: &AgentDef, provider: &ProviderEntry) -> AgentEntry { … }
```

### 3.2 The load-bearing engineering decision: compose *above* the registry, don't split the runtime

The tempting design — a `ProviderRegistry` holding processes and a thin `AgentCatalog` on top, N agents sharing one warm provider slot — is **wrong for this codebase today**, for a reason ADR-0028 makes concrete: **the tool set is frozen into the spawned process for two of three agent families** (codex: MCP → `-c` argv at spawn; kiro: rendered agent-config file + `--agent` at spawn; only claude delivers MCP per-session — and even there it rides the `AcpConfig` frozen into the backend). `session_cwd` is likewise spawn-frozen (canonicalized `{cwd}` → prism `--repo`). So the true spawn unit is *(provider × tools × cwd)* — i.e., approximately today's `AgentEntry`. Slot-sharing across agents on one provider is an optimization that is only *sometimes* legal, per delivery channel.

Therefore: **keep `Registry`, `SpawnFn`, `AgentBackend`, leases, and retirement exactly as they are, keyed by `AgentId`, consuming the composed `AgentEntry`.** The split lives in the config/domain layer: `config.rs` parses `[[providers]]` + `[[agents]]`, composes, and emits today's `RegistrySnapshot`. Blast radius: config parsing + domain types + docs. Zero changes to `bridge-registry`, `bridge-acp`, `bridge-container`, the executor, or the inbound server. Slot-sharing by an `InstanceKey = hash(provider, spawn-frozen fields)` is a **later, optional** optimization (§6 Phase S2c) — and it is only safe after the §1.2 reuse-criterion defect is fixed, since that comparison is precisely the "what is spawn-frozen" ledger.

### 3.3 Where the role prompt applies

There is no separate system-prompt channel in the ACP path the bridge drives; role composition is prompt composition, and the seams already exist:

- **Executor path (workflows):** effective node prompt = `role_prompt ⊕ rendered(node.prompt_template)`. Implement in the executor's render site (`run_node`), sourced from the composed `AgentEntry` (add `role_prompt: Option<String>` to the composed form). Template authors may also reference `{{agent.role}}` explicitly; bare prepend is the default.
- **Warm path:** `NodeTurn.seed` (`executor.rs:63` — "warm-only; prepended to the node prompt parts") is exactly where a role lands for warm turns. No new mechanism.
- **Unary/`submit` path:** prepend at the same place `configure_session` metadata is assembled in the producer.

### 3.4 Per-node overrides

Today `WorkflowNode` has none, and `run_workflow` *rejects* request-level overrides (`coordinator.rs:652`). Add optional node-level knobs, resolved by extending the existing layering fn:

```rust
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,
    pub prompt_template: String,
    pub inputs: Vec<NodeId>,
    pub retry: Option<RetryPolicy>,
    #[serde(default)] pub overrides: Option<AgentOverride>,   // model/effort/mode — additive
}
// domain.rs: effective_config(entry, node_override) — same precedence machinery,
// now three layers by construction: node ⊕ agent-def ⊕ provider (the last two are
// already flattened by compose_agent).
```

`AgentOverride` (`domain.rs:160`) is reused verbatim. Serde-default keeps every persisted `workflow_spec_json` resumable (mirrors the `panel`/`retry` additive precedents in `graph.rs` tests).

### 3.5 Interaction with the hard constraints

- **ADR-0024 two-engines:** untouched, and explicitly **not subsumed**. `implement`'s warm edit/fix path consumes the composed `AgentEntry` exactly as today; `resolve_impl_identity` still enforces single-node. An agent's `role_prompt` is *ignored* by the implement loop in v1 (it builds its own prompts); wiring it through the seed seam is a separate, later decision — do not couple Shift 2 to roadmap L5 (engine unification). L5 remains its own item.
- **ADR-0028 per-agent MCP:** strengthened — MCP moves to where it conceptually belonged (`AgentDef`), and the delivery machinery is untouched because composition flattens before spawn. The §1.2 fix (add `mcp`/`mcp_delivery`/`watchdog` to the reuse comparison) is a **prerequisite**, otherwise editing an agent's tools hot-reloads into a stale warm process.
- **Warm-session invalidation on agent edit:** already correct once §1.2 is fixed — a tool/provider change falls out of config-only reuse and takes the new-slot + lease-drain + retire path; a pure role_prompt or model-default edit is config-only (role is applied per-turn/per-session, not spawn-frozen) and keeps the warm backend. State this classification in the ADR so it's a tested contract, not folklore.

### 3.6 Is this becoming an agent framework? Where the line is

Honestly: Shift 2 moves the bridge one deliberate step from "model-provider mux" toward "agent runtime" — that step is *role + tools as data*. The line to hold, in ADR language: **agents are configuration, workflows are data, execution is ports; the bridge never plans.** Concretely out of scope, permanently unless re-triggered: agent memory/state across tasks; dynamic tool acquisition; agent-initiated delegation ("agents calling agents" outside a declared DAG); conditional/dynamic edges chosen by an LLM at runtime; any planner node. These are exactly the conductor-shaped pressures ADR-0008 requires evidence for, and the evidence table there still reads "No" on every row. The orchestrator remains the caller; the bridge executes declared graphs.

---

## 4. ADR actions

1. **ADR-0033 — Runtime workflow definitions.** Decides: `WorkflowCatalog` (ArcSwap map), unified `ConfigSnapshot` + ordered apply, run-pinning-by-value as the immutability mechanism (canonizing what `encode_workflow_spec` already does), additive `def_hash`, agent-card skill refresh, defs-live-in-the-file. **Amends ADR-0009's "load-once at boot" clause only**; every other ADR-0009 decision (single terminal, degradation, streaming-only triggers) stands.
2. **ADR-0034 — Agent/Provider split.** Decides: `ProviderEntry`/`AgentDef`/`compose_agent`, composition-above-the-registry (explicitly rejecting the ProviderRegistry runtime split, with the ADR-0028 spawn-freezing evidence), role-as-prompt-composition, node-level `AgentOverride`, the warm-invalidation classification table. **Amends ADR-0005 §8's config schema** (additively — §5). Records the §1.2 reuse-criterion fix as a prerequisite.
3. **ADR-0035 — Definition store: deferred** (ADR-0012-style "decision not to build"): file is SSOT; editor writes go through `WorkflowConfigStore` (realizing ADR-0005's deferred 3b.2 write-back for workflows first); DB/remote store re-triggered only by concurrent-editing or multi-host pressure.
4. **Explicitly preserved non-goals:** ADR-0008 (no conductor, no orchestrator-in-the-bridge; partial-adopt only on evidenced pressure — restate the re-trigger table); ADR-0012 (markdown at the boundary — a DAG editor does not change who consumes output); ADR-0024's two-engines constraint (neither shift touches the warm path); no admin HTTP in `serve`.

## 5. Migration & back-compat

**Rule: every existing TOML parses byte-for-byte into the same `RegistrySnapshot` and the same graphs.**

1. **Legacy `[[agents]]` synthesis (Shift 2).** An entry with `cmd`/`base_url` and no `provider =` key synthesizes an implicit `ProviderEntry` with `id == agent id` plus an `AgentDef` referencing it; `compose_agent(synth_def, synth_provider)` must equal today's parse output. Gate with a golden parity test over every `examples/*.toml` (the repo already has the parity-test pattern — `config.rs` tests against `examples/a2a-bridge.workflows.toml`).
2. **New syntax is additive:** `[[providers]]` tables + `[[agents]] provider = "codex-bin"`. Setting both `provider` and any substrate field (`cmd`/`base_url`/`sandbox`/…) on one agent is a **load error** (loud, per house style). `[[agents.mcp]]`, `role_prompt = "<prompt-id>"`, and node `overrides` are all optional additions.
3. **Workflows:** node `agent = "<id>"` continues to name the (now-composed) agent id — zero workflow TOML changes. Node `overrides` and envelope `def_hash` are serde-default additive; `SUPPORTED_SNAPSHOT_VERSION` stays 1; every persisted `workflow_spec_json` remains resumable.
4. **Hot-reload behavior change (Shift 1):** `[[workflows]]`/`[[prompts]]` edits start applying live. This is the intended feature but is a behavior change; ship behind `[workflows] hot_reload = true` for one release with a boot log line, default-on the next. `[server]`/`[store]` stay boot-read.
5. **`ConfigSource` widening:** keep the existing trait untouched for external impls; introduce `UnifiedConfigSource` and have `FileConfigSource` implement both (registry-only consumers keep compiling).

## 6. Phased plan, risks, hard problems

Sized for the repo's spec → dual-adversarial-review → live-gate discipline; each phase is independently landable and gate-able.

- **Phase 0 — the reuse-criterion fix** (`registry.rs:388`): add `mcp`, `mcp_delivery`, `watchdog` to the spawn-frozen comparison; regression test: MCP edit on a warm slot ⇒ new slot + retirement. Small, ships alone, prerequisite for both shifts. *Live gate:* edit `[[agents.mcp]]` under a running serve; confirm respawn + new tool visible.
- **Phase W1 — `WorkflowCatalog` + unified snapshot + ordered reconcile + `def_hash`.** *The cheapest first increment* — coordinator/inbound lookups change, executor untouched. *Live gate:* edit a `[[workflows]]` node prompt mid-`serve` with a run in flight; the in-flight run finishes on the old graph (verify via `def_hash` in traces), the next submit uses the new one.
- **Phase W2 — `WorkflowConfigStore` write-back + Coordinator/MCP `workflow upsert/remove/list`** (realizes deferred 3b.2 for workflows; the DAG editor's server half). *Live gate:* mobile/TUI builder → upsert → hot apply → run.
- **Phase S2a — domain split, pure refactor:** `ProviderEntry`/`AgentDef`/`compose_agent` + legacy synthesis; golden parity across `examples/`. No new syntax yet.
- **Phase S2b — new capability:** `[[providers]]` syntax, `role_prompt` composition in the executor render + unary producer, node `overrides`, three-layer `effective_config`. *Live gate:* two agents on one provider binary with different roles/toolsets in one `code-review` DAG.
- **Phase S2c (optional, evidence-gated):** slot-sharing by spawn-frozen `InstanceKey`; per-agent AgentCard skills (ADR-0005 deferred "Option-3").

**Genuinely hard parts / risks:**
- **Mutating defs with in-flight durable runs** — *already solved* by pin-by-value; the risk is regression, not design: any future "optimization" that makes the executor or resume path read the catalog instead of the run snapshot re-opens it. Guard with a test that deletes the def mid-run and asserts both completion and resume.
- **Does Shift 1 subsume roadmap L5 (unify the two engines)?** No — L5 is about `warm.prompt`+`drain_turn` vs the executor, orthogonal to *where defs live*. Neither shift moves it; say so in ADR-0033 to prevent scope creep.
- **Two SSOT drift** (editor vs file): closed only if *every* write goes through `WorkflowConfigStore` → file → watcher. Resist a "fast path" that applies to the catalog directly and writes the file second — that's the split-brain generator.
- **Registry↔workflow apply ordering under rapid edits:** the debounced single-file parse makes each snapshot internally consistent, but a *failed* registry apply with a *valid* workflow set must keep last-good for both (one outcome per snapshot, never mixed).
- **Prompt-registry coupling:** a `[[prompts]]` edit now silently changes every workflow referencing it on next reload — correct, but `def_hash` churn is the operator's tell; surface it in the UI.
- **What NOT to build:** DB def store; conditional/LLM-chosen edges; agent memory; runtime provider mux ("try codex, fall back to claude" belongs to the caller per ADR-0008); admin HTTP in `serve`; any executor knowledge of warm sessions as part of these shifts.

---

**One-paragraph verdict.** Both shifts are cheaper than they look because ADR-0005's machinery and the W3 durability work already carry the hard invariants: runs pin their graph by value (`encode_workflow_spec` → resume-from-snapshot), and the registry already knows how to atomically swap, lease-drain, and retire. Shift 1 is a ~small refactor (ArcSwap catalog + unified snapshot) whose ADR mostly *canonizes existing behavior*; Shift 2 is a config/domain-layer split that deliberately leaves the runtime untouched, with one real prerequisite bug fix (`registry.rs:388` reuse criterion omits spawn-frozen `mcp`/`mcp_delivery`/`watchdog`) and one bright line to defend: the bridge composes declared agents into declared graphs — it never plans, and the orchestrator remains the caller.
