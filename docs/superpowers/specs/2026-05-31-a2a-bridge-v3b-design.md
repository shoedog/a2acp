# A2A Bridge Increment 3b — Agent Registry (greenfield) Design

> **Revision 2** — folds the dual spec review (Codex gpt-5.5 + Claude opus-4.8, 2026-05-31). The central change: **effective config is applied per-session at mint, not baked at spawn** (resolves the sync/async, config-only-edit, override-isolation, and lifecycle findings together). Plus a persisted **task→agent binding**, an explicit **async retirement** path, **atomic reconciliation**, the **opaque (non-`provider@`) model id**, and **parent-dir file-watch**.

**Goal:** Replace the single hardcoded local backend with a **runtime-mutable registry of named "agent entries"**, selected per request by a dedicated `agent` axis (with a configured default and raw per-request overrides), dispatched by a now-meaningful `AgentId`. Make the canonical config a swappable **`ConfigSource`** (File adapter + hot-reload in 3b) reconciled atomically into the live registry, so agents can be added/edited/removed **without restarting the bridge** — including upgrading an entry's model live (no respawn).

**Architecture:** A `ConfigSource` port (File adapter) yields a declarative `RegistrySnapshot`; a **reconciler** atomically diffs it against the live `AgentRegistry` and applies `upsert`/`remove`. The registry maps `AgentId → Arc<RegistrySlot>` where a slot holds an `ArcSwap<AgentEntry>` (config, hot-swappable) and a lazily-spawned `OnceCell<Arc<dyn AgentBackend>>` (warm process). `RouteDecision` resolves a request to `Local(agent_id)`; the inbound server resolves it to `(entry, backend)` and **always** applies the entry's current **effective config** (base ⊕ override) for that session via the additive `AgentBackend::configure_session`, which stashes per-`SessionId` config that the backend applies at lazy ACP mint. A bridge task is **bound** to its `(agent_id, effective_config)` at creation so multi-message tasks (follow-ups, cancel, get) always reach the same backend. Agent entries are a **typed core** (cmd/args, model_provider, model, effort, mode) plus an **open extension map**. Seams are conductor-compatible; the fork/continue decision is **deferred to post-3c** (§2).

**Tech stack:** Rust 2021 (1.94), `tokio`, `agent-client-protocol` =0.12.1, `arc-swap`, `serde`/`toml`, a filesystem-notify crate (`notify`), `a2a-lf` =0.3.0.

**Spec status:** decisions locked in brainstorming + the dual review folded. Tags `[probe]` = grounded in the live codex-acp 0.15.0 / kiro-cli 2.5.0 probes (Appendix A); `[Cx]`/`[Cl]` trace to accepted Codex/Claude review findings.

---

## 1. Scope & boundary

**3b BUILDS:**
- The **runtime-mutable `AgentRegistry`**: `async resolve`/`upsert`/`remove`/`set_default`/`list`, lazy exactly-once spawn per agent, Arc-lifetime concurrency, explicit async **retirement** on edit/remove (§5).
- **Per-session effective config** via `AgentBackend::configure_session` (additive, default no-op) — base config **always** applied, overrides layered, stashed per `SessionId`, applied at lazy ACP mint (§4.4).
- **Persisted task→agent binding** so multi-message A2A tasks reach the same backend+config even after registry edits (§4.5) `[Cx]`.
- The **`ConfigSource` port + File adapter + atomic reconciler** (§6), with **parent-directory file-watch hot-reload** (file = canonical source of truth).
- `[[agents]]` config with the **typed core** entry schema + the **open extension map** stored (§3).
- **Selection**: a dedicated `agent` axis (request metadata) + configured `default`, replacing the hardcoded `AgentId::parse("kiro")` (§4.1–4.2).
- **AgentId-aware dispatch** (the `RouteTarget::Local(_)` wildcard becomes a real registry resolve).
- `effort` wired **best-effort per adapter** (codex via its config-option/model surface; kiro skip) (§3.3).

**3b DOCUMENTS (designed/seamed, not built — §9):** per-entry A2A AgentCards (Option 3, Appendix C); the admin HTTP API + `ConfigStore` write path + persistence write-back (**3b.2**); DB/remote `ConfigSource` adapters; saveable/loadable config bundles; general `config_options` passthrough beyond effort; `tools`/MCP wiring; per-provider effort tables for non-codex adapters (3c); **fan-out across the registry (3d)**.

**Non-goals:** no conductor fork (§2); no change to the `Fanout`/`Delegate` routing paths (they continue to use the default/selected agent); no admin API.

---

## 2. The conductor decision — deferred to post-3c (with criteria)

ADR-0002 parked "fork/adopt the conductor vs. continue greenfield" for "when the 2nd/3rd CLI agent arrives and per-agent composition becomes concrete." A **strong** decision is not yet possible: we have not composed multiple *local* agents (this increment does), both agents are the same protocol family (ACP; Gemini/3c is the cross-family test), and proxy-chain/multi-hop/dynamic-discovery composition — the conductor's home turf — has not been built. **Decision:** continue greenfield for 3b; **re-evaluate after 3c** with a fresh read of the conductor codebase. **Favors conductor/partial-adopt:** needing proxy-chaining, multi-hop agent graphs, dynamic discovery, shared cross-agent session/context, or routing-policy complexity that bloats `bridge-policy`. **Confirms greenfield:** composition stays "select an agent by id; optionally fan-out/delegate," and the ports absorb each adapter without domain change. Recorded as an ADR at that point.

---

## 3. The agent-entry schema (typed core + open extension map)

An **agent entry** is a named bundle, indexed by `id`. The four user-facing dimensions (**model provider · model · effort · mode**) are *fields of an entry*, not separate per-request knobs; a "custom agent" is a saved bundle, a raw override is an unsaved one (§4.3).

### 3.1 Config shape

```toml
default = "codex-fast"                # registry default; must resolve to a present entry (validated)

[[agents]]
  id             = "codex-fast"       # caller-facing id; also AgentSkill.id for the Option-3 card
  cmd            = "codex-acp"        # the ACP CLI executable (the adapter is one ACP adapter, parameterized) — §3.2
  args           = []                 # adapter args (e.g. kiro: ["acp"])
  # ── typed core (all optional except cmd; best-effort per adapter) ──
  model_provider = "openai"           # DESCRIPTIVE metadata only (LLM vendor); NOT folded into the ACP ModelId (§3.3, §8)
  model          = "gpt-5.5"          # the agent-native model id, passed to session/set_model AS-IS (§3.3)
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

Maps 1:1 to `AgentEntry { id, cmd, args, model_provider, model, effort, mode, cwd, auth_method, name, description, tags, version, extensions: BTreeMap<String, toml::Value> }`. The adapter owns all messy per-agent mapping (entries stay clean).

### 3.2 The adapter / `cmd` `[Cx][Cl]`

In 3b there is **one ACP adapter** (`AcpBackend` from 3a) parameterized by **`cmd` + `args`** — both explicit entry fields. There is no separate `adapter`-kind enum; the entry's `cmd`/`args` *are* the adapter parameterization, and the `AgentBackend` trait already abstracts non-ACP kinds for the future. At boot/reconcile, `cmd` is validated against a **config-driven allow-list** (`[registry] allowed_cmds = ["codex-acp","kiro-cli",…]`, default = the cmds named by the configured entries) — so a genuinely new ACP CLI is **runtime-addable** by adding it to config, with no recompile. (A compiled-in allow-list would have contradicted "add agents without restart" — `[Cl]`.) A spawn closure resolves `(cmd, args, AcpConfig{cwd, auth_method, timeouts})` → `AcpBackend::spawn(...)`.

### 3.3 `model`, `effort`, `mode` — opaque, agent-native, best-effort

- **`model`** is the **agent-native model id passed to `session/set_model` verbatim** — `gpt-5.5`, `claude-sonnet-4.5`, `auto` (Appendix A). The bridge does **NOT** synthesize a `{provider}@{model}` id; the probes show no agent uses that form, and a synthesized `openai@gpt-5.5` would be rejected (best-effort → silent no-op), defeating selection `[Cl]`. `model_provider` is **descriptive metadata / routing/card label only** (§8) and is never folded into the `ModelId`.
- **`mode`** is an opaque per-agent `SessionModeId` (codex approval preset `read-only|auto|full-access`; kiro persona `kiro_default|kiro_planner|kiro_guide`), applied via `session/set_mode` (a rejected mode id is a **hard error**, inherited 3a contract).
- **`effort`** is a normalized enum `minimal|low|medium|high|max` (default `medium`), **best-effort + adapter-mapped**, with a raw passthrough in `extensions`:
  - **codex-acp** `[probe]`: maps to its `reasoning_effort` config-option (`low|medium|high|xhigh`, category `thought_level`) and/or the folded model id; the **folded-id footgun** (catalog lists `base/effort`, config-option `model` is base-only) is handled in the codex adapter mapping.
  - **kiro-cli** `[probe]`: no structured effort → **no-op, logged "unsupported"** (same best-effort contract as `set_model`).
  - Other providers (Appendix B): tables documented; wired at 3c.

---

## 4. Selection, dispatch, per-session config, task binding

### 4.1 Selection axis (not `skill`)

`skill` keeps its current routing-mode role (`delegate`/`fan-out`/local); 3b does **not** add to that overload. Agent selection is a **separate metadata axis**:
- `a2a-bridge.agent` → entry id.
- `a2a-bridge.model` / `.effort` / `.mode` → raw per-request overrides.

`TaskMeta` (today `{ skill }`) gains `agent: Option<AgentId>` and `overrides: Option<AgentOverride { model: Option<String>, effort: Option<Effort>, mode: Option<String> }>`. Invalid `effort` metadata → a clear parse error (tested).

### 4.2 `RouteDecision`

```
route(meta):
  skill = "delegate" → Delegate
  skill = "fan-out"  → Fanout
  else               → Local( meta.agent.unwrap_or(registry.default_id()) )
```
Replaces the hardcoded `AgentId::parse("kiro")` in `bin/a2a-bridge/src/route.rs`. **TOCTOU note** `[Cl]`: `route` reads `default_id()` and dispatch `resolve`s separately; a concurrent reconcile that drops the just-read default yields a clean unknown-agent error for that request (rare, acceptable, documented).

### 4.3 Dispatch

The inbound server's `RouteTarget::Local(agent_id)` arm (today `Local(_) =>`) becomes:
```rust
let (entry, backend) = registry.resolve(&agent_id).await?;     // async; lazy-spawns; unknown id → clear error (§7)
let eff = effective_config(&entry, meta.overrides.as_ref());    // base, with override layered when present
backend.configure_session(&session, &eff).await?;              // ALWAYS — base config flows even with no override [Cx][Cl]
// bind the task (§4.5), then:
backend.prompt(&session, parts) ...                            // as today
```
`effective_config` layers the override's `model`/`effort`/`mode` over the entry base. Overrides are **best-effort, not cross-validated** against the adapter/provider (a mismatched model → best-effort no-op; a mismatched mode → hard error) — documented, not silently "fixed" `[Cl]`.

### 4.4 Per-session effective config (`configure_session`) — the central mechanism

`AgentBackend` gains one **additive** method with a **default no-op** (so non-ACP/test backends are unaffected; the blast radius — every implementor + test fakes — is covered by the default impl) `[Cl]`:
```rust
async fn configure_session(&self, session: &SessionId, cfg: &EffectiveConfig) -> Result<(), BridgeError> { Ok(()) }
```
`AcpBackend` implementation: **stash `cfg` keyed by `SessionId`** (a per-session config map). The ACP session is minted lazily inside `prompt`/`ensure_session`; at mint, `ensure_session` reads the stashed config and applies `set_mode` (hard) then `set_model`/effort (best-effort) **once** — base unless overridden. Semantics `[Cx][Cl]`:
- **Spawn-time `AcpConfig` shrinks to `{cwd, auth_method, timeouts}`** — model/mode/effort are no longer baked at spawn; they come per session. This is what makes a **config-only edit take effect with no respawn**: the next session reads the slot's now-swapped entry (§5) and applies the new model/mode.
- **Idempotency/ordering:** `configure_session` is called once per session before the first `prompt`; it only stashes (no ACP round-trip), so it cannot race the lazy mint. The mint applies config exactly once (the 3a `OnceCell`), so re-prompts on the same session reuse it.
- **Override isolation** `[Cl]`: because config is stashed **per `SessionId`**, an override on task A's session never bleeds into task B's session on the **same multiplexed backend** (one process serves many sessions). Tested (§10).
- **Rejected mode** still hard-errors at mint (3a), failing the request clearly.

### 4.5 Task→agent binding `[Cx]`

A2A tasks are multi-message: follow-up `message/send`, `tasks/cancel`, `tasks/get`. Each must reach the **same** backend + effective config the task started on — *not* re-route by current metadata/default (which may have changed). At task creation the bridge **persists the binding `task_id → (agent_id, effective_config)`** in the store (`bridge-store`); follow-ups/cancel/get resolve the backend via the stored binding. A `remove`/edit of a bound agent does not strand in-flight tasks: the **retirement path (§5.4)** keeps the old backend alive until its bound tasks finish. A follow-up to a task whose agent was removed resolves via the retained handle (or fails with a clear "agent retired" terminal if already reaped).

---

## 5. The runtime-mutable registry

### 5.1 Interface

```rust
trait AgentRegistry: Send + Sync {
    async fn resolve(&self, id: &AgentId) -> Result<(Arc<AgentEntry>, Arc<dyn AgentBackend>), BridgeError>; // lazy-spawn
    fn default_id(&self) -> AgentId;
    async fn apply(&self, snapshot: RegistrySnapshot) -> Result<(), BridgeError>; // atomic reconcile entrypoint (§6.3)
    fn list(&self) -> Vec<AgentEntrySummary>;
}
```
`resolve` is **async** (spawn = ACP `initialize`/auth) `[Cx][Cl]`. Single concrete impl over `ArcSwap<HashMap<AgentId, Arc<RegistrySlot>>>`:
```rust
struct RegistrySlot {
    entry:   ArcSwap<AgentEntry>,                  // hot-swappable config (config-only edit = store(new))
    backend: OnceCell<Arc<dyn AgentBackend>>,      // lazily spawned warm process
}
```
The map itself is held in an `ArcSwap` so a reconcile swaps the whole map atomically (§6.3); the per-slot `ArcSwap<AgentEntry>` lets a config-only edit mutate config in place without disturbing the live `OnceCell` backend.

### 5.2 Lazy exactly-once spawn

`resolve`: load the map (`ArcSwap::load`), get `Arc<RegistrySlot>`, **clone the Arc out** (no lock held), then `slot.backend.get_or_try_init(|| spawn(slot.entry.load()))` mints the backend exactly once (3a `OnceCell` discipline: a spawn failure leaves the cell uninitialized → retry re-attempts; other agents unaffected). Returns `(slot.entry.load_full(), backend.clone())`. No lock is held across the spawn `await` `[Cx][Cl]`.

### 5.3 Concurrency (Arc-lifetime)

An in-flight request holds its `Arc<dyn AgentBackend>` (and the bound task holds its binding, §4.5). A concurrent reconcile swaps the map / a slot's entry, but the **old backend lives until its last `Arc` drops** — never torn mid-use.

### 5.4 Edit/remove + async retirement `[Cx]`

- **Config-only change** (model/mode/effort/extensions; **same `cmd`+`args`+`cwd`+`auth_method`**) → `slot.entry.store(new)` (the same slot/backend). New sessions apply the new config at mint (§4.4); in-flight sessions keep their already-applied config. **No respawn.**
- **`cmd`/`args`/`cwd` change** → replace the slot (fresh `OnceCell`); the old slot is **retired** (below); the new backend spawns lazily.
- **Remove** → drop the slot from the map; **retire** the old backend.
- **Retirement** (because `Supervised::terminate` is **async** and `Drop` is sync — Arc-drop cannot reap gracefully `[Cx]`): on edit/remove, the displaced `Arc<dyn AgentBackend>` is handed to a spawned **retirement task** that waits until its **bound in-flight tasks** complete (or a grace deadline), then calls `Supervised::terminate` (SIGTERM→SIGKILL, 3a) and drops. `kill_on_drop` remains a backstop, not the primary path.

---

## 6. `ConfigSource` port + File adapter + atomic reconciler

### 6.1 Ports (interface segregation)

```rust
trait ConfigSource: Send + Sync {                     // 3b: File adapter
    async fn load(&self) -> Result<RegistrySnapshot, BridgeError>;
    fn watch(&self) -> BoxStream<'static, RegistrySnapshot>;
}
trait ConfigStore: ConfigSource {                     // 3b.2+: admin API / write-back
    async fn upsert(&self, entry: AgentEntry) -> Result<(), BridgeError>;
    async fn remove(&self, id: &AgentId) -> Result<(), BridgeError>;
}
```
`RegistrySnapshot { default: AgentId, entries: Vec<AgentEntry>, allowed_cmds: Vec<String> }` is the full desired state.

### 6.2 File adapter (3b) — parent-directory watch `[Cl]`

- `load`: parse `[[agents]]` + `default` + `[registry] allowed_cmds` into a `RegistrySnapshot`.
- `watch`: watch the **parent directory** and match the config filename (NOT an inode-level watch on the file — editors save via temp-write + atomic rename, replacing the inode and silently breaking a file watch). **Debounce** (coalesce rapid events; ignore transient mid-write parse errors). On a settled change, re-parse and emit a fresh snapshot.
- A reload **validation failure keeps the last-good snapshot** (a bad edit never takes the registry down); the error is logged (and surfaced via the admin API later).

### 6.3 Atomic reconciler `[Cx]`

The reconcile loop consumes `load()` once at boot, then each `watch()` snapshot, and calls `registry.apply(snapshot)`, which builds the **next** map and swaps it in **atomically** (one `ArcSwap::store`):
```
apply(desired):
  validate(desired)                 # unique ids; cmds ∈ allowed_cmds; default ∈ entries  (§7)
  next = {}
  for entry in desired.entries:
    if cur slot exists with same cmd/args/cwd/auth: next[id] = cur slot; cur.entry.store(entry)   # config-only edit, keep warm backend
    else: next[id] = new slot (fresh OnceCell)                                                     # add or adapter-change
  set self.default = desired.default                # default updated as part of the same swap
  map.store(next)                                   # atomic — no partial-snapshot window
  for id in (old.ids - next.ids): retire(old[id])   # retire dropped/replaced AFTER the swap (their Arcs/bindings keep them alive)
```
No request can observe a state missing the new default or a half-applied set. Idempotent: re-applying the same snapshot is a no-op (same cmds → slots reused, `entry.store` of an equal value is harmless). A future DB/remote `ConfigSource` is a drop-in (it just yields snapshots).

---

## 7. Error handling

- **Boot validation** (spawn is lazy, but config is checked at first `load`/`apply`): unique ids; every `cmd ∈ allowed_cmds`; `default` resolves. Malformed initial config **fails boot loudly**. Hot-reload failures keep the last-good snapshot (§6.2).
- **Unknown agent id** at request time → clear client-facing error (JSON-RPC error / `Failed` terminal `unknown agent "x"`), never a panic.
- **Lazy spawn failure** → that agent's first request fails clearly; `OnceCell` stays uninitialized so a retry re-attempts; other agents unaffected.
- **Override** `mode` rejected → hard error; `model`/`effort` → best-effort (logged). Overrides are not cross-validated (§4.3).
- **`configure_session`** stashes only; the hard/best-effort split happens at mint (§4.4). Rejected base/override mode → clear request failure.
- **Edit/remove vs in-flight** → Arc-lifetime + retirement (§5.3–5.4); bound tasks (§4.5) reach their original backend until completion.
- **Follow-up to a retired agent** → resolves via the retained handle, or a clear "agent retired" terminal if already reaped.

---

## 8. Naming: model-provider vs A2A-provider

A2A's `AgentProvider { organization, url }` is the **serving organization**, NOT the LLM vendor. Entry fields: **`model_provider`** (LLM vendor — descriptive/routing/card-tag only, never in the `ModelId`); **`serving_org`/`serving_org_url`** reserved for the Option-3 card's `AgentProvider`. Entry **`version`** (config version) is distinct from `AgentInterface.protocol_version` (A2A wire version "1.0").

---

## 9. Future evolutions (designed/seamed, not built in 3b)

Per-entry A2A AgentCards (Option 3, Appendix C); **3b.2** admin HTTP API + `ConfigStore` write-back + promote-bundle-to-card; DB/remote `ConfigSource` adapters; saveable/loadable config bundles (ties to ACP `loadSession`); `tools`/MCP + general `config_options` passthrough; **3d** fan-out across the registry; per-provider effort tables (Appendix B) at 3c.

---

## 10. Testing

- **Unit:** `resolve` (id→(entry,backend); unknown→error; default fallback; **no lock across the spawn await**); lazy exactly-once spawn + spawn-failure-retry; `effective_config` layering; `configure_session` stash + apply-at-mint; `RouteDecision` agent/default/override; invalid `effort` metadata → parse error.
- **The headline path** `[Cl]`: a **new session AFTER a config-only edit uses the NEW model/mode** (config-only edit = warm backend reused, next session re-configured) — and the edit does **not** disturb an in-flight session.
- **Override isolation** `[Cl]`: an override on task A's session does **not** affect a concurrent task B on the **same** backend/process (per-`SessionId` stash).
- **Registry lifecycle:** config-only edit = no respawn; cmd-change = retire+respawn; remove = retire (in-flight Arc + bound task survive concurrent remove; retirement awaits then terminates).
- **Atomic reconcile** `[Cx]`: successive snapshots → add/edit/remove; no partial-snapshot/default-gap window observable; idempotent re-apply.
- **Task binding** `[Cx]`: follow-up send / cancel / get after a metadata change or a registry edit resolve to the **original** backend + config.
- **File-watch** `[Cl]`: edit via in-place write AND via atomic-rename (temp+rename) both trigger reconcile (parent-dir watch + debounce); a bad edit keeps the last-good set.
- **Gated e2e — the real multi-agent proof:** kiro + codex registered as two entries; route to each **by id**; apply a model/mode override and confirm it takes effect; live-edit an entry's model and confirm a fresh task uses it without restart. (Both agents installed + authenticated.)
- Existing fan-out / delegation / 3a tests stay green.
- **Coverage (unchanged):** workspace ≥85%; `bridge-core` ≥90%; the new registry crate/module ≥90% — after `cargo llvm-cov clean --workspace`.

---

## 11. Review

Spec **Revision 2** has folded the dual Codex (gpt-5.5) + Claude (opus-4.8) review (8 distinct accepted findings + 2 adopted design decisions). If this revision passes user review, the implementation **plan** gets its own Codex+Claude review pass (via the a2a-local-bridge tooling, firewalled) before build.

---

## Appendix A — per-adapter option sets (live probes, 2026-05-31)

**codex-acp 0.15.0** (ACP `protocolVersion: 1`): modes `read-only`(default)/`auto`/`full-access`; models `gpt-5.5/5.4/5.4-mini/5.3-codex/5.3-codex-spark/5.2` × `/low /medium /high /xhigh` (folded; default `gpt-5.5/xhigh`); configOptions `mode`, `model`(6 bases), `reasoning_effort`(`low|medium|high|xhigh`, cat `thought_level`); CLI `-c model_reasoning_effort=minimal|low|medium|high|xhigh`, `sandbox_mode`, `web_search=disabled|cached|live`, `[mcp_servers.*]`; auth chatgpt/codex-api-key/openai-api-key. **model ids used verbatim** — e.g. `gpt-5.5` (no `provider@`).

**kiro-cli acp 2.5.0** (ACP `protocolVersion: 1`): modes (personas) `kiro_default`(default)/`kiro_planner`/`kiro_guide`; models `auto`(default)/`claude-sonnet-4.5`/`claude-sonnet-4`/`claude-haiku-4.5`/`deepseek-3.2`/`minimax-m2.5`/`minimax-m2.1`/`glm-5`/`qwen3-coder-next`; **no configOptions**; effort = interactive `/effort` only (→ no-op); 13 built-in tools; per-tool trust (`--trust-all-tools`), not a mode; auth `[]`. **model ids used verbatim** — e.g. `auto`, `claude-sonnet-4.5`.

**Cross-agent:** model + mode ACP-native for both; effort ACP-native for codex, absent for kiro; tools/web_search/MCP are CLI-only/agent-specific → the open extension map. No agent uses a `{provider}@{model}` id form.

## Appendix B — `effort` normalization (cross-provider)

Normalized `minimal|low|medium|high|max` (default `medium`), best-effort + raw passthrough. **OpenAI** (codex): `reasoning_effort` `minimal|low|medium|high|xhigh` (map `max→xhigh`). **Anthropic:** effort enum `low|medium|high|xhigh|max` (newer); legacy `budget_tokens` deprecated. **Gemini:** 3.x `thinking_level` `low|medium|high`; 2.5 numeric `thinkingBudget` → synthesize from bucket (lossy; use `extensions.raw` for an exact budget). All are per-request params applied at session/mint. `model` and `model_provider` stay **separate** fields; **the ACP `ModelId` is the agent-native `model` string verbatim — no `{provider}@{model}` synthesis** (corrected per review `[Cl]`).

## Appendix C — A2A AgentCard mapping (Option-3 future)

`a2a` (a2a-lf 0.3.0) `AgentCard` required: `name, description, version, supported_interfaces, capabilities, default_input_modes, default_output_modes`; optional `skills (AgentSkill{id,name,description,tags,examples,…})`, `provider (AgentProvider{organization,url})`, `documentation_url`, `icon_url`, security. Extension seam: `capabilities.extensions: Vec<AgentExtension{uri, params}>`. Today the bridge serves one card at `/.well-known/agent-card.json` (`crates/bridge-a2a-inbound/src/card.rs`) describing the bridge + 3 routing skills. Entry→card: `id→AgentSkill.id`; `name/description→skill`; `model/model_provider/effort/mode→AgentSkill.tags` or `AgentExtension.params` (URI `<bridge-ns>/agent-entry` — no native A2A field); `serving_org→AgentProvider`. Option-3 publishes one skill per entry (single card) or per-entry well-known paths.
