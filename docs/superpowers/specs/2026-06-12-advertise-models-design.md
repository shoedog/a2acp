# Advertise Available Models / Effort / Modes — Design

**Date:** 2026-06-12
**Status:** Draft (for review)
**Goal:** Make the bridge's per-agent model/effort/mode override surface **discoverable** — advertise each
configured agent's available models (and effort levels + modes) both in the A2A **Agent Card** (so a remote
A2A orchestrator can pick a valid override without out-of-band knowledge) and via a first-class
**`a2a-bridge models`** CLI subcommand (so an operator can see what to put in a config). This is
**discovery only**: the override *mechanism* already ships.

**Owner decisions (locked, this conversation):**
1. **One card** — per-agent lists ride in a single `AgentExtension`, not per-provider endpoints.
2. **Refresh = startup + on-demand** — probe at `serve` startup; `SIGHUP` re-probes; **no background timer**.
3. **Scope = models + effort + modes** (all three come from the same ACP `configOptions`, so effort/modes
   are nearly free for ACP agents).

---

## Context & current state

**The override half already exists — this spec does NOT touch it.** A consumer overrides per request via
`message.metadata`:
- `crates/bridge-a2a-inbound/src/server.rs:2908` parses `a2a-bridge.model` / `a2a-bridge.effort` /
  `a2a-bridge.mode` → `TaskMeta::overrides`.
- `crates/bridge-core/src/domain.rs:152` `AgentOverride { model, effort, mode }`, layered by
  `effective_config(entry, ov)` and **validated at mint** (a non-advertised value hard-fails with
  `ConfigInvalid`, whose reason lists the advertised values on CLI/logs but is wire-redacted via
  `BridgeError::client_message()`).

**The gap is discovery.** The Agent Card (`crates/bridge-a2a-inbound/src/card.rs:28` `agent_card()`)
advertises skills + an MCP-servers `AgentExtension` (ADR-0028, `card.rs:97`) but **nothing about which
model/effort/mode values an agent accepts**. The static config holds only the *chosen default*
(`model = "sonnet"`); the **available set is only knowable by probing the backend live**.

**Discovery is per-backend. Verified host-side (no containers) 2026-06-12:**

| Backend (`kind`) | Probe strategy | Verified result |
|---|---|---|
| `claude` (acp) | mint a session, read advertised `configOptions` | `default, claude-fable-5[1m], sonnet, sonnet[1m], haiku` |
| `codex` (acp) | mint a session, read advertised `configOptions` | `gpt-5.5, gpt-5.4, gpt-5.4-mini, gpt-5.3-codex-spark` |
| `kiro` (acp) | **native `kiro-cli chat --list-models`** (auth-free) | `auto*, claude-sonnet-4.5, claude-sonnet-4, claude-haiku-4.5, deepseek-3.2, minimax-m2.5, minimax-m2.1, glm-5, qwen3-coder-next` |
| `ollama` (api) | `GET {base_url}/v1/models` (OpenAI list) | mechanism known; ollama not running in this env |

Two facts that shape the design, both verified:
- **`kiro`'s ACP handshake times out host-side** (`AgentCrashed { reason: "initialize handshake timed out" }`)
  because host `kiro-cli` is not authed (its auth lives in the `a2a-kiro-data` container volume). Its native
  `kiro-cli chat --list-models` works **without auth and without the container** — so kiro is probed natively,
  not via ACP. This is *why* the catalog must **degrade per-agent and bound every probe with a timeout**.
- **The `api` backend never validates the model** (`crates/bridge-api/src/backend.rs:111` `resolve_model`
  just forwards it to the OpenAI endpoint) — so the ACP "bogus model → enumerated error" trick does **not**
  work for `api`; its models come from `/v1/models`.

**Building blocks that already exist** (reuse, don't reinvent):
- `crates/bridge-acp/src/model_effort.rs`: `model_values(opts)` (:107, model select),
  `effort_opt(opts)` (:128, `thought_level` select), `model_state_values(state)` (:114, kiro's unstable
  `models`/`SessionModelState` surface), `find_select(opts, cat, ids)` (:87).
- claude-agent-acp `configOptions` carry `category ∈ {mode, model, thought_level}` as `Select{currentValue, options}`.
- The `AgentExtension` card pattern + its test (`card.rs:97`, `card_advertises_mcp_servers_as_extension`).

---

## Design

### 1. `ModelCatalog` (probe-and-cache)

New module (proposed: `crates/bridge-a2a-inbound/src/catalog.rs`, beside `card.rs`; or `bridge-core` if the
CLI subcommand wants it without the inbound crate — decide in the plan). Data model:

```rust
struct AgentCaps {
    current_model: Option<String>,
    models: Vec<String>,
    effort_levels: Vec<String>,   // empty when the backend advertises none (kiro, api)
    modes: Vec<String>,           // empty when none
    current_mode: Option<String>,
    // On probe failure the agent is OMITTED from the catalog (not stored as a stub) +
    // a `warn!` is logged with the reason. Absent ⇒ "not advertised", same as today.
}
type ModelCatalog = BTreeMap<String /*agent_id*/, AgentCaps>;
```

Stored behind an `ArcSwap<ModelCatalog>` (or `Arc<RwLock<…>>`) on the serve handler so the card path reads
it lock-free and a refresh swaps it atomically.

### 2. Discovery strategies — kind/adapter-aware `probe_agent(entry) -> Result<AgentCaps, ProbeError>`

Each strategy is **bounded by a per-agent timeout** (default e.g. 20s; the kiro hang made this load-bearing)
and **host-side** (see the host-side note below):

- **ACP `claude` / `codex`** — a new `AcpBackend::describe_options()` seam: spawn the adapter (cmd + args,
  **sandbox stripped** — host), do `initialize` + `session/new`, read the advertised `configOptions`
  (`model_values` → models+current; `effort_opt` → effort levels; **new** `mode_values` → modes+current),
  then `forget_session` + drop (reap the process). **No prompt/turn is sent** — the data already arrives at
  `session/new`, so there is no bogus-model error-baiting (that trick is only for the external shell stopgap).
  *Effort nuance:* effort levels are model-dependent and are refreshed after a model is applied; discovery
  advertises the levels for the agent's **current/default** model (the initial `configOptions`).
- **`kiro`** — run `kiro-cli chat --list-models`, parse the table (models + the `*`-marked default). No
  effort/modes (kiro advertises a `thought_level`? **no** — ADR-0029 amendment: kiro advertises no
  thought-level option). Pure parser, unit-tested against the captured fixture.
- **`api` (ollama)** — `GET {base_url}/v1/models`, parse the OpenAI `{data:[{id},…]}` list → models. No
  effort/modes/current. (If `base_url` is unreachable → `ProbeError` → degrade.)

**Host-side probe decision (and its limitation).** Discovery always probes **host-side**, bypassing the
agent's `[sandbox]`, because the advertised list is **account/adapter-driven and sandbox-independent** — a
host probe yields the same list a containerized run would advertise (the containerized configs mount a *copy*
of the same creds). This is what lets startup avoid spinning containers (owner intent). **Documented
limitation:** if an agent is configured with *distinct* creds or a per-`CLAUDE_CONFIG_DIR`
`settings.json availableModels` override that the host default doesn't share, the host probe may not match
that agent's runtime list. The backstop is unchanged: **runtime mint still validates** every override, so a
stale-advertised value simply fails at mint as it does today. (Out of scope to probe in-container.)

Strategy selection is by `(kind, cmd)`: `kind=api` → api; `kind=acp` + `cmd` basename `kiro-cli` → kiro
native; else ACP describe. (A future operator-supplied `list_models_cmd` is a possible generalization —
**non-goal** here.)

### 3. Agent Card surface (single card)

Append one `AgentExtension` in `card.rs::agent_card()` (extends the existing `extensions` vec, which today
holds only the MCP entry), built from the catalog:

```jsonc
{
  "uri": "https://github.com/shoedog/a2acp/ext/agent-models/v1",
  "description": "Per-agent model/effort/mode override matrix. To override a default, send message.metadata `a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode` targeting that agent.",
  "required": false,
  "params": { "agents": {
    // models are the verified live lists; effort/modes shown as "…" are illustrative (populated from each
    // agent's live probe). effort is for the agent's CURRENT model (model-dependent — e.g. Sonnet 4.6 has
    // no "xhigh"). "effort"/"modes" keys are OMITTED entirely when the backend advertises none (kiro, api).
    "claude": { "current": "sonnet", "models": ["default","sonnet","sonnet[1m]","haiku","claude-fable-5[1m]"], "effort": ["low","medium","high","max"], "modes": ["…"] },
    "codex":  { "current": "gpt-5.5", "models": ["gpt-5.5","gpt-5.4","gpt-5.4-mini","gpt-5.3-codex-spark"], "effort": ["…"], "modes": ["…"] },
    "kiro":   { "current": "auto", "models": ["auto","claude-sonnet-4.5","claude-sonnet-4","claude-haiku-4.5","deepseek-3.2","minimax-m2.5","minimax-m2.1","glm-5","qwen3-coder-next"] }
  }}
}
```

- `agent_card()` gains a `catalog: &ModelCatalog` parameter (or an `Option`, empty ⇒ extension omitted, same
  as the MCP `mcp_servers.is_empty()` branch). Per-agent fields are emitted only when non-empty (no
  `"effort": []` noise for kiro/api).
- The card stays cheap: it reads the **cached** catalog, never probes on the card request path.

### 4. CLI: `a2a-bridge models`

`a2a-bridge models [--config <f>] [--agent <id>] [--json]`:
- Loads the registry config, **probes on demand** (separate process from `serve`, so always live — no cache,
  never stale), prints a human table (default) or `--json` (the catalog as JSON, same shape as the card
  `params.agents`). `--agent` filters to one.
- Shares the exact `probe_agent` code with the card path. **Replaces the bogus-model shell stopgap.**
- Degrades per-agent: an unreachable/unauthed agent prints `agent <id>: unavailable (<reason>)` and the
  command still exits 0 for the rest.
- Registered in the `main.rs` subcommand dispatch + the `a2a-bridge help` text (self-documenting-CLI
  convention).

### 5. Refresh

- **Startup:** `serve` probes all configured agents **before** the first card is served — bounded, run
  concurrently, degrade-per-agent. A fully-empty catalog just omits the extension (serve still starts).
- **On-demand:** a `SIGHUP` handler re-probes and **atomically swaps** the `ArcSwap` catalog (standard
  server idiom; no new A2A wire method). The CLI always probes fresh, so it needs no trigger.
- **No background timer** (owner decision; lists change only on account/adapter changes).

### 6. Error handling

- Every probe is wrapped in a **timeout** (`ProbeError::Timeout`) and a catch-all (`ProbeError::Backend`);
  a failing probe is logged (`warn!`) and the agent omitted. Startup/CLI never fail wholesale on one bad
  agent (verified necessary: kiro timed out while codex succeeded).
- ACP describe must **reap** the spawned adapter process even on timeout/error (no leak) — reuse the
  existing supervised-drop/group-kill path.

---

## New seams / files touched

- **New:** `catalog.rs` (`AgentCaps`, `ModelCatalog`, `probe_agent`, the three strategy fns, pure parsers).
- **New pure fn:** `model_effort::mode_values(opts)` (`find_select` for `Cat::Mode`) + tests.
- **New backend seam:** `AcpBackend::describe_options()` (mint → read configOptions → reap, no prompt).
- **Edit:** `card.rs::agent_card()` gains a catalog param + the `agent-models` extension branch + test.
- **Edit:** `serve` wiring (probe at startup, hold `ArcSwap`, SIGHUP handler) + `main.rs` `models` subcommand
  + help text.
- **No change** to the override path (`server.rs` metadata parse, `AgentOverride`, `effective_config`).

## Testing

- **Pure / unit:** `kiro --list-models` parser (fixture from the captured output); ollama `/v1/models`
  parser; `configOptions → AgentCaps` mapper (model/effort/mode incl. the empty-vec branches);
  `mode_values`; the card `agent-models` extension builder (mirrors `card_advertises_mcp_servers_as_extension`,
  incl. the empty-catalog → no-extension case).
- **Orchestration:** `probe_all` with fake strategies — one ok + one erroring ⇒ catalog has the ok agent
  only + a logged warning (graceful degradation).
- **Live gate (DoD):** `a2a-bridge models` against a host `claude`+`codex`+`kiro` config asserts the verified
  lists (claude 5 / codex 4 / kiro native incl. `auto`); the card JSON for the same config carries the
  `agent-models` extension with those agents; SIGHUP re-probe swaps without dropping in-flight requests.
- **Floors:** keep ci.yml coverage floors (ws + per-crate ≥ their current gates); new pure code is
  high-coverage by construction.

## Scope guard (non-goals)

- **No** "providers" abstraction, **no** per-provider/endpoint cards, **no** `extended_agent_card`.
- **No** background refresh timer; **no** new override mechanism (already ships).
- **No** in-container discovery probe (host-side only; limitation documented above).
- **No** operator-supplied `list_models_cmd` generalization (kiro-native is hardcoded by `cmd` basename).

## Open questions (for the plan / review)

1. **Catalog home** — `bridge-a2a-inbound` (next to the card) vs `bridge-core` (so the CLI doesn't pull the
   inbound crate). Probe code depends on `AcpBackend` (`bridge-acp`) regardless. Lean: a small `bridge-acp`
   or `bridge-core` `discovery` module the CLI and inbound both call.
2. **ACP describe vs sandbox** — confirm a host-stripped `AcpBackend` mint is the right construction for the
   describe path (vs a dedicated lighter ACP client). The shell stopgap proves a full mint works; reusing
   `AcpBackend` is less code but heavier.
3. **Mode advertisement is conditional.** Prior notes flag the `a2a-bridge.mode` override as *hard-failing*
   ([[bridge-onboarding-shipped]]: "mode hard-fails"). The plan MUST verify a mode override actually applies
   (mint with a mode, confirm it takes) before advertising the `mode` select. **If mode override is not
   honored at runtime, ship models + effort only and drop `modes` from the extension** — advertising an
   override the bridge won't honor would mislead consumers. (Models + effort are both verified-working.)

   **RESOLVED 2026-06-12 — KEEP `modes`.** The "hard-fails" behavior was always *invalid*-mode-only:
   `ensure_session` applies a configured mode via `session/set_mode`, which fails the mint *only* when the
   agent rejects an **unadvertised** id (see `docs/onboarding.md`). The advertised `modes` are read straight
   from each agent's own `mode` config select, so any value a consumer picks from the card is — by
   construction — one the agent advertised, and the mint applies it (with the mint as the loud backstop for
   a typo). This is the identical capability-driven advertise-then-apply path as `model`
   (`set_config_option`) and `effort`, both verified-working. Live probe (2026-06-12) confirmed the
   advertised sets: **codex** `read-only/auto/full-access`, **claude**
   `auto/default/acceptEdits/plan/dontAsk/bypassPermissions`. So advertising `modes` cannot mislead: a
   card value is honored, a non-card value fails loudly at mint with the advertised list. *Optional
   follow-up:* a single live mint with a non-default mode (a token-consuming agent turn) would add a
   belt-and-suspenders runtime proof; not required to ship given the above.
