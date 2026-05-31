# A2A Bridge Increment 3b — Agent Registry (greenfield) Design

**Goal:** Replace the single hardcoded local backend with a **runtime-mutable registry of named "agent entries"**, selected per request by a dedicated `agent` axis (with a configured default and raw per-request overrides), dispatched by a now-meaningful `AgentId`. Make the canonical config a swappable **`ConfigSource`** (File adapter + hot-reload in 3b) reconciled into the live registry, so agents can be added/edited/removed **without restarting the bridge**.

**Architecture:** A `ConfigSource` port (File adapter) yields a declarative `RegistrySnapshot`; a **reconciler** diffs it against the live `AgentRegistry` and applies `upsert`/`remove`. The registry maps `AgentId → (entry config, lazily-spawned Arc<dyn AgentBackend>)`. `RouteDecision` resolves a request to `Local(agent_id)` (from `agent` metadata or the default); the inbound server resolves the id to a backend and applies per-session overrides via an additive `AgentBackend::configure_session`. Agent entries are a **typed core** (adapter, model_provider, model, effort, mode) plus an **open extension map** for everything agent-/provider-divergent. The seams are deliberately conductor-compatible; the fork/continue-greenfield decision is **deferred to post-3c** (see §2).

**Tech stack:** Rust 2021 (1.94), `tokio`, `agent-client-protocol` =0.12.1 (already adopted in 3a), `serde`/`toml`, a filesystem-notify crate (e.g. `notify`), `a2a-lf` =0.3.0.

**Spec status:** decisions locked in brainstorming. Tags `[probe]` = grounded in the live codex-acp 0.15.0 / kiro-cli 2.5.0 capability probes (Appendix A).

---

## 1. Scope & boundary

**3b BUILDS:**
- The **runtime-mutable `AgentRegistry`**: `resolve`/`upsert`/`remove`/`list`, lazy exactly-once spawn per agent, Arc-lifetime concurrency, dynamic edit/remove lifecycle (§5).
- The **`ConfigSource` port + File adapter + reconciler** (§6), with **file-watch hot-reload** (file = canonical source of truth).
- `[[agents]]` config with the **typed core** entry schema + the **open extension map** stored (§3).
- **Selection**: a dedicated `agent` axis (request metadata) + configured `default`, replacing the hardcoded `AgentId::parse("kiro")` in `RouteDecision` (§4).
- **Raw per-request override** of the typed core (model/effort/mode) via the additive `AgentBackend::configure_session` (§4.3, §7).
- **AgentId-aware dispatch** in the inbound server (the `RouteTarget::Local(_)` wildcard becomes a real registry lookup).
- `effort` wired **best-effort per adapter** (codex via its model/effort surface; kiro skip) (§3.3).

**3b DOCUMENTS (designed/seamed, not built — see §9 Future evolutions):**
- Option 3: **per-entry A2A AgentCards** (Appendix C).
- The **admin HTTP API + `ConfigStore` write path + persistence write-back** → **Increment 3b.2**.
- **DB/remote `ConfigSource` adapters** (the port makes them drop-in).
- **Saveable/loadable config bundles** (promote an ad-hoc override into a reusable bundle; ties into ACP `loadSession`).
- General `config_options` passthrough beyond effort; **`tools`/MCP** wiring.
- Per-provider effort-mapping tables for non-codex providers (Appendix B) — bite at **3c (Gemini)**.
- **Fan-out across the registry** → **Increment 3d** (3b leaves fan-out/delegation unchanged).

**Non-goals:** no conductor fork (§2); no change to the `Fanout`/`Delegate` routing paths; no change to the ACP client internals from 3a beyond the additive `configure_session`.

---

## 2. The conductor decision — deferred to post-3c (with criteria)

ADR-0002 parked the "fork/adopt the conductor vs. continue greenfield" decision for "when the 2nd/3rd CLI agent arrives and per-agent composition becomes concrete." That moment is partially here (kiro + codex both work as `AcpBackend` instances), but a **strong** decision is not yet possible, because:
- We have **not yet composed multiple *local* agents** (this increment is what makes that concrete).
- Both agents are the **same protocol family (ACP)**; the adapter seam has not faced a structurally different protocol (Gemini/3c is the real test).
- **Proxy-chain / multi-hop / dynamic-discovery composition** — the conductor's home turf — has not been built, so we have no felt evidence we need it.

**Decision:** continue greenfield for 3b. **Re-evaluate fork/continue/partial-adopt after 3c**, when we have (a) felt N-agent composition, (b) a second protocol family, and (c) a fresh read of the conductor codebase itself. **Criteria that would favor the conductor (or partial adoption):** we find ourselves needing proxy-chaining / multi-hop agent graphs, dynamic agent discovery, shared cross-agent session/context, or routing-policy complexity that bloats `bridge-policy`. **Criteria that confirm greenfield:** composition stays "select an agent by id; optionally fan-out/delegate," and the ports absorb each new adapter without domain changes. This will be recorded as an ADR at the post-3c decision point.

---

## 3. The agent-entry schema (typed core + open extension map)

An **agent entry** is a named bundle. The registry indexes entries by `id`; the caller selects an entry by id (or the default). The four user-facing dimensions (**model provider · model · effort · mode**) are *fields of an entry*, not separate per-request knobs; a "custom agent" is a saved bundle, a raw override is an unsaved one (§4.3).

### 3.1 Config shape

```toml
default = "codex-fast"                # registry default; required to resolve to a real entry

[[agents]]
  id             = "codex-fast"       # caller-facing id; also AgentSkill.id for the Option-3 card
  adapter        = "codex-acp"        # which ACP CLI process (the AgentBackend); see §3.2
  args           = []                 # optional adapter args (e.g. kiro: ["acp"]) — see §3.2
  # ── typed core (all optional except adapter; best-effort per adapter) ──
  model_provider = "openai"           # LLM vendor enum: openai|anthropic|google|other  (NOT the A2A provider — §8)
  model          = "gpt-5.5"          # opaque model id; adapter maps to session/set_model
  effort         = "high"             # normalized enum: minimal|low|medium|high|max (best-effort; §3.3)
  mode           = "read-only"        # opaque per-agent SessionModeId (codex preset | kiro persona)
  cwd            = "/abs/path"        # optional; default current_dir (absolute)
  auth_method    = "chatgpt"          # optional ACP auth method id
  # ── card forward-compat (optional) ──
  name           = "Codex (fast)"
  description     = "Codex on GPT-5.5, high reasoning"
  tags           = ["model:gpt-5.5", "effort:high"]
  version        = "1"                # entry config version (distinct from A2A protocol version — §8)
  # ── open extension map (escape hatch — no schema churn) ──
  [agents.extensions]
    config_options = { reasoning_effort = "xhigh" }  # raw ACP set_config_option overrides by id
    tools          = { web_search = "live" }         # passthrough; wiring deferred (§9)
    raw            = { }                             # codex -c, kiro agent-json, raw effort budget, etc.
```

The TOML `[[agents]]`/`extensions` map 1:1 to a Rust `AgentEntry { id, adapter, args, model_provider, model, effort, mode, cwd, auth_method, name, description, tags, version, extensions }`, where `extensions: BTreeMap<String, toml::Value>` is the open seam. **The adapter owns all messy per-agent mapping** (so entries stay clean): `(model_provider, model, effort, mode)` → the agent's actual ACP/CLI surface.

### 3.2 `adapter`

In 3b every adapter is an `AcpBackend` parameterized by `cmd`/`args` (kiro-cli + codex-acp are both ACP). `adapter` names the command (e.g. `"codex-acp"`, `"kiro-cli"` with `args=["acp"]`). The field is a string, validated at boot against a known-adapter set so a typo fails loudly; the set is open for future non-ACP adapter kinds (the `AgentBackend` trait already abstracts this). Resolution of `adapter` → an `AcpBackend::spawn(cmd, args, AcpConfig{...})` factory is the registry's spawn closure.

### 3.3 `effort` — normalized, best-effort

`effort` is a **normalized enum `minimal | low | medium | high | max`** (default `medium`). Providers converge on a `low|medium|high` core with ragged edges, so the field is **best-effort and adapter-mapped**, with a **raw passthrough** in `extensions.raw`/`extensions.config_options` for exact values:
- **codex-acp** `[probe]`: exposes `reasoning_effort = low|medium|high|xhigh` as a first-class ACP config-option (category `thought_level`) and also folds effort into model ids (`gpt-5.5/high`). The adapter maps `(model, effort)` to codex's surface (folded `set_model` id and/or `set_config_option`); `max→xhigh`, `minimal→` the lowest available. The **folded-id footgun** (the `models` catalog lists `base/effort` pairs while the `model` config-option is base-only) is handled in the codex adapter mapping, not the entry.
- **kiro-cli acp** `[probe]`: no structured effort (interactive `/effort` only) → effort is a **no-op (logged "unsupported")**, same best-effort contract as 3a's `set_model`.
- Other providers (Appendix B): mapping tables documented; wired when 3c lands.

`model` and `mode` are opaque per-agent strings (ACP `ModelId`/`SessionModeId` are transparent newtypes — no structure to enforce), wired via 3a's existing `session/set_model` (best-effort) and `session/set_mode` (hard error on a rejected mode id — inherited 3a contract).

---

## 4. Selection & routing

### 4.1 Selection axis (not `skill`)

`skill` stays what it is today (the v1 routing-mode overload: `delegate`/`fan-out`/local). 3b does **not** add to that overload. Agent selection is a **separate metadata axis**:
- `a2a-bridge.agent` → the agent entry id.
- `a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode` → raw per-request overrides (§4.3).

`TaskMeta` (currently `{ skill }`) gains `agent: Option<AgentId>` and `overrides: Option<AgentOverride>`. `AgentOverride { model: Option<String>, effort: Option<Effort>, mode: Option<String> }`.

### 4.2 `RouteDecision`

```
route(meta):
  skill = "delegate" → Delegate
  skill = "fan-out"  → Fanout
  else               → Local( meta.agent.unwrap_or(registry.default_id()) )
```
This replaces the hardcoded `AgentId::parse("kiro")` in `bin/a2a-bridge/src/route.rs`. The decision stays synchronous and does not touch a backend. (The router needs read access to `registry.default_id()`; the registry default is config-validated at boot.)

### 4.3 Dispatch + overrides

The inbound server's `RouteTarget::Local(agent_id)` arm (today `RouteTarget::Local(_) =>`) becomes:
```
let backend = registry.resolve(&agent_id)?;        // lazy-spawns; unknown id → clear error (§7)
if let Some(ov) = overrides { backend.configure_session(&session, effective_config(entry, ov)).await?; }
backend.prompt(&session, parts) ...                // as today
```
`effective_config(entry, override)` layers the override's `model`/`effort`/`mode` over the entry's base. Because these are **per-session** ACP calls, an override is *session-scoped config*, not a new agent. The same A2A task/session is bound to one agent + one effective config for its lifetime.

---

## 5. The runtime-mutable registry

### 5.1 Interface

```rust
trait AgentRegistry: Send + Sync {
    fn resolve(&self, id: &AgentId) -> Result<Arc<dyn AgentBackend>, BridgeError>; // request path; lazy-spawn
    fn default_id(&self) -> AgentId;
    fn set_default(&self, id: AgentId) -> Result<(), BridgeError>;                 // reconciler sets default (id must be present)
    async fn upsert(&self, entry: AgentEntry) -> Result<(), BridgeError>;          // add OR edit
    async fn remove(&self, id: &AgentId) -> Result<(), BridgeError>;
    fn list(&self) -> Vec<AgentEntrySummary>;
}
```
Concrete impl: an internal `RwLock<HashMap<AgentId, RegistrySlot>>`, where
`RegistrySlot { entry: AgentEntry, backend: OnceCell<Arc<dyn AgentBackend>> }`.

### 5.2 Lazy exactly-once spawn

`resolve` takes a read lock, finds the slot, and `backend.get_or_try_init(|| spawn(entry))`-style mints the `Arc<dyn AgentBackend>` exactly once (the same `OnceCell` discipline proven for session mint in 3a, including: a spawn failure leaves the cell uninitialized so a retry re-attempts, and other agents are unaffected). The `Arc` is cloned out before the lock is released.

### 5.3 Concurrency (Arc-lifetime discipline)

An in-flight request holds its `Arc<dyn AgentBackend>`. A concurrent `remove`/edit removes the slot (or swaps its config) but the **old backend lives until its last `Arc` drops**, then `Supervised::terminate` (3a) reaps the subprocess. No torn state; the read lock is held only for the map lookup + `Arc` clone, never across `.await`.

### 5.4 CRUD semantics

- **`upsert` new id** → insert slot, unspawned (lazy).
- **`upsert` existing, config-only change** (model/mode/effort/extensions; **same adapter+cmd+args+cwd**) → swap the slot's `entry`; **new sessions** pick up the new config (model/mode/effort are per-session ACP calls) — **no respawn**. This is the "upgrade the model when a new one launches" path. In-flight sessions keep their already-applied config.
- **`upsert` existing, adapter/cmd/args/cwd change** → replace the slot (fresh `OnceCell`); the old backend drops out and reaps when its in-flight sessions finish; the new backend spawns lazily on next `resolve`.
- **`remove`** → delete the slot (routing stops immediately); backend reaps on Arc-drop; `resolve(removed)` → unknown-agent error.

All operations are idempotent against a desired snapshot (§6), so applying the same snapshot twice is a no-op.

---

## 6. `ConfigSource` port + File adapter + reconciler

### 6.1 Ports (interface segregation)

```rust
trait ConfigSource: Send + Sync {                     // 3b: File adapter
    async fn load(&self) -> Result<RegistrySnapshot, BridgeError>;  // declarative desired state
    fn watch(&self) -> BoxStream<'static, RegistrySnapshot>;        // change events (re-emit on change)
}
trait ConfigStore: ConfigSource {                     // 3b.2+: admin API / write-back
    async fn upsert(&self, entry: AgentEntry) -> Result<(), BridgeError>;
    async fn remove(&self, id: &AgentId) -> Result<(), BridgeError>;
}
```
`RegistrySnapshot { default: AgentId, entries: Vec<AgentEntry> }` is the full desired set.

### 6.2 File adapter (3b)

- `load`: parse `[[agents]]` + `default` from the bridge TOML into a `RegistrySnapshot`.
- `watch`: a filesystem-notify watcher on the config file; on change, re-parse and emit a fresh snapshot (debounced). Parse/validation errors on reload are **logged and the previous good snapshot is kept** (a bad edit never takes the registry down); the error surfaces in logs (and, later, the admin API).

### 6.3 Reconciler (declarative → diff → CRUD)

A reconcile loop consumes `load()` once at boot, then each `watch()` snapshot:
```
reconcile(desired: RegistrySnapshot, registry):
  validate(desired)                       # unique ids; known adapters; default resolves (§7)
  for entry in desired.entries: registry.upsert(entry)        # add or edit (idempotent)
  for id in registry.ids() - desired.ids(): registry.remove(id)
  registry.set_default(desired.default)
```
This is K8s-style desired-state reconciliation: idempotent, source-agnostic. A future DB/remote `ConfigSource` is a drop-in — it just yields snapshots; the reconciler + registry are unchanged.

---

## 7. Error handling

- **Boot config validation** (even though spawn is lazy): the initial `load()` + reconcile validates **unique ids**, **known adapters**, and **`default` resolves to a present entry**. A malformed initial config **fails boot loudly**. (Subsequent hot-reload validation failures keep the last-good snapshot — §6.2.)
- **Unknown agent id at request time** → a clear client-facing error (JSON-RPC error for unary; a `Failed` terminal with `unknown agent "x"` for streaming) — distinct from an agent crash; never a panic.
- **Lazy spawn failure** → that agent's first request fails with a clear error; the `OnceCell` stays uninitialized so a retry re-attempts (3a semantics); other agents unaffected.
- **Override validation** (inherits 3a): override `mode` rejected by the agent → **hard error**, fails the request clearly; override `model`/`effort` → **best-effort** (logged, non-fatal).
- **Edit/remove vs in-flight** → Arc-lifetime discipline (§5.3): in-flight sessions complete on their already-applied config; teardown is deferred to Arc-drop.

---

## 8. Naming: model-provider vs A2A-provider

A2A's `AgentProvider { organization, url }` is the **serving organization**, NOT the LLM vendor. To avoid a hard collision once entries publish cards (§9 / Appendix C):
- LLM vendor → **`model_provider`** (entry field; not published directly to a card — surfaced via skill tags / extension params).
- A2A serving org → **`serving_org` / `serving_org_url`** (reserved entry fields, mapped to `AgentProvider` only when publishing a per-entry card later).
- Entry **`version`** (the agent config version) is kept distinct from `AgentInterface.protocol_version` (the A2A wire version, "1.0").

---

## 9. Future evolutions (designed/seamed, not built in 3b)

- **Option 3 — per-entry A2A AgentCards** (Appendix C): publish each entry as its own card/skill so A2A clients discover/address agents natively. Seam: the registry can generate `Vec<AgentSkill>` (one per entry) for the existing single card, or mount per-entry `.well-known` paths.
- **Increment 3b.2 — admin HTTP API + `ConfigStore` write path + persistence** (write-back through the canonical source) + **promote-bundle-to-card**.
- **DB/remote `ConfigSource`/`ConfigStore` adapters** (the §6 ports make them drop-in).
- **Saveable/loadable config bundles**: promote an ad-hoc override into a reusable named bundle without a full card; ties into ACP `loadSession: true` (both agents advertise it).
- **`tools`/MCP wiring** + general `config_options` passthrough (beyond effort) via `session/set_config_option`.
- **Increment 3d — fan-out across the registry** (N-way fan-out over registered agents).
- **Per-provider effort tables** (Appendix B) wired as **3c (Gemini)** and other non-codex adapters land.

---

## 10. Testing

- **Unit:** registry `resolve` (id→backend, unknown→error, default fallback); lazy exactly-once spawn + spawn-failure-retry (mirror 3a session-mint tests); CRUD semantics (config-only edit = no respawn; adapter-change = respawn; remove = teardown on Arc-drop, with an in-flight Arc surviving a concurrent remove); the **reconciler diff** (add/edit/remove from successive snapshots; idempotent re-apply); config validation (dup ids, unknown adapter, dangling default); File adapter load/parse; `configure_session` applies the right per-session effective config; `RouteDecision` with `agent` metadata + default + override.
- **File-watch:** a temp config file edited at runtime → the registry reflects add/edit/remove without restart (a bad edit keeps the last-good set).
- **Gated e2e — the real multi-agent proof:** register kiro + codex as two entries; route to each **by id** and confirm the right agent answers; apply a model/mode override and confirm it takes effect. (Both agents are installed + authenticated; this closes the "never composed multiple local agents" gap that feeds the §2 post-3c decision.)
- Existing fan-out / delegation / 3a tests stay green (unchanged paths).
- **Coverage gates (unchanged):** workspace ≥85%; `bridge-core` ≥90%; new registry crate/module ≥90% — measured after `cargo llvm-cov clean --workspace` (stale-cache bug).

---

## 11. Review

After this spec is written and self-reviewed, run the established **dual review pass via the a2a-local-bridge review tooling** before the implementation plan: **Codex (gpt-5.5)** and **Claude (opus-4.8)**, firewalled (black-box review of *this* spec; the PoC's schema must not influence the design). Fold findings; re-review if substantive. The plan then gets its own dual review.

---

## Appendix A — per-adapter option sets (from live probes, 2026-05-31)

**codex-acp 0.15.0** (`agentInfo.version` 0.15.0; ACP `protocolVersion: 1`):
- **modes** (`set_mode`): `read-only` (default), `auto`, `full-access` — approval presets.
- **models** (`set_model`): base × effort. Bases: `gpt-5.5`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.2`; effort suffixes `/low /medium /high /xhigh` (24 folded ids; default `gpt-5.5/xhigh`).
- **configOptions**: `mode` (read-only|auto|full-access), `model` (the 6 bases, no suffix), **`reasoning_effort`** (category `thought_level`): `low|medium|high|xhigh`.
- **CLI config** (`-c key=value`, `~/.codex/config.toml`): `model`, **`model_reasoning_effort`** = `minimal|low|medium|high|xhigh`, `sandbox_mode` = `read-only|workspace-write|danger-full-access`, `web_search` = `disabled|cached|live`, `[mcp_servers.*]`.
- **prompt caps:** image ✓, audio ✗, embeddedContext ✓. **authMethods:** chatgpt, codex-api-key, openai-api-key.

**kiro-cli acp 2.5.0** (`agentInfo` "Kiro CLI Agent" 2.5.0; ACP `protocolVersion: 1`):
- **modes** (`set_mode`, = personas, NOT approval): `kiro_default` (default), `kiro_planner`, `kiro_guide`.
- **models** (`set_model`): `auto` (default), `claude-sonnet-4.5`, `claude-sonnet-4`, `claude-haiku-4.5`, `deepseek-3.2`, `minimax-m2.5`, `minimax-m2.1`, `glm-5`, `qwen3-coder-next`.
- **configOptions:** none. **effort:** interactive `/effort` only (no structured values → no-op for the entry).
- **tools:** 13 built-in (`code glob grep introspect knowledge read shell subagent todo_list use_aws web_fetch web_search write`); permission = per-tool trust (`--trust-all-tools` / `/tools`), not a mode.
- **CLI:** `--agent <id>` (mode), `--model`, `--trust-all-tools`, `--trust-tools`, agent JSON in `~/.kiro/agents/*.json` (`tools, allowedTools, mcpServers, hooks, …`). **prompt caps:** image ✓, embeddedContext ✗. **authMethods:** [].

**Cross-agent:** model + mode are ACP-native for both; **effort** is ACP-native for codex (config-option) and **absent** for kiro; **tools/web_search/MCP** are CLI-config-only and agent-specific → the **open extension map**. Vendor frames (`_kiro.dev/*`, codex `available_commands_update`, `usage_update`) are tolerantly dropped (3a) and never typed.

## Appendix B — `effort` normalization (cross-provider)

Normalized `minimal|low|medium|high|max` (default `medium`), best-effort + raw passthrough:
- **OpenAI** (codex): `reasoning_effort` enum `minimal|low|medium|high|xhigh` (per-request). Map `max→xhigh`.
- **Anthropic:** effort enum `low|medium|high|xhigh|max` (newer; `output_config.effort`); legacy `thinking.budget_tokens` deprecated/rejected on 4.7/4.8. Drop the field when the agent doesn't think.
- **Google Gemini:** 3.x `thinking_level` enum `low|medium|high`; 2.5 numeric `thinkingBudget` (0–24576) → synthesize from the bucket (lossy; use `extensions.raw` for an exact budget).
All three are **per-request** params (set at session/prompt time), so a normalized enum is a reasonable portable primary, paired with a raw passthrough for the enum-vs-budget edges. `model_provider` + `model` stay separate typed strings; the `{provider}@{model}` ACP `ModelId` is derived at the bridge↔ACP boundary.

## Appendix C — A2A AgentCard mapping (for the Option-3 per-entry-card future)

`a2a` (a2a-lf 0.3.0) `AgentCard` required fields: `name, description, version, supported_interfaces (Vec<AgentInterface{url, protocol_binding, protocol_version}>), capabilities, default_input_modes, default_output_modes`; optional: `skills (Vec<AgentSkill{id,name,description,tags,examples,input_modes,output_modes}>), provider (AgentProvider{organization,url}), documentation_url, icon_url, security_*`. Extension seam: `capabilities.extensions: Vec<AgentExtension{uri, params: HashMap<String,Value>}>`.

Today the bridge serves **one** card at `/.well-known/agent-card.json` (`crates/bridge-a2a-inbound/src/card.rs`) describing the bridge + 3 routing skills. Entry→card mapping: `id→AgentSkill.id`, `name/description→skill`, `model/model_provider/effort/mode→AgentSkill.tags` or `AgentExtension.params` (no native A2A field — use the `<bridge-ns>/agent-entry` extension URI), `serving_org→AgentProvider`. Option-3 publishes one skill per entry (single card) or a per-entry well-known path; the crate imposes no limit.
