# Capability-driven model & effort pinning — design (v1)

**Date:** 2026-06-09
**Status:** Draft (brainstorming output; pending user spec-review)
**Builds on:** ADR-0024 (effective_config fold for warm sessions), the existing `AcpBackend`
mint-time `set_mode`/`set_model`/`set_effort` path (`crates/bridge-acp/src/acp_backend.rs`),
ADR-0006 (claude via `claude-agent-acp`).

---

## 1. Context

The bridge already plumbs `model`, `effort`, and `mode` per `[[agents]]` (TOML → `AgentToml` →
`AcpConfig` → applied at session mint via `session/set_model`, `session/set_config_option`,
`session/set_mode`). That plumbing was built and validated against **codex-acp** and is
**codex-shaped**: it hardcodes the effort config-id (`reasoning_effort`) and a codex value
vocabulary (`Max → "xhigh"`). Live protocol probes (2026-06-09, see §3) showed this is **partly
broken and entirely unvalidated** for the other agents — most importantly **claude effort is a
silent no-op** and **model typos are silently accepted by every agent**.

The root cause is uniform: the bridge **hardcodes** config-ids and value vocabularies instead of
using what each agent **advertises at mint**. Every ACP adapter reports, in its `session/new`
response and follow-up `config_option_update` notifications, the real config-option `id`, its
valid `options[]`, the `currentValue`, and (for the model option) the `availableModels` list with
the resolved `currentModelId`. The fix is to make the bridge **capability-driven**: discover,
validate against, and report the advertised state.

## 2. Goals / Non-goals

**Goals**

- **Validate** the configured `model=` against the agent's advertised `availableModels`; a typo or
  non-advertised id **fails the session mint loudly** (no silent fallback to a default model).
- **Fix claude effort** (today a silent no-op): apply effort via the **agent's advertised effort
  config-id** (`effort` for claude, `reasoning_effort` for codex), discovered at mint.
- **Validate/adapt effort** against the **active model's** advertised levels: when the requested
  level is unsupported (e.g. `xhigh` on Sonnet 4.6), **fall back to the highest advertised level
  ≤ requested** and **warn** (mirrors the Claude CLI; the ACP path errors rather than falling back,
  so the bridge must do the mapping itself).
- **Observability:** one mint-time log line per agent stating the **resolved** model and the
  **applied** effort (and any fallback), so a pin is honest and auditable.
- **`fallback_model`:** an optional, advertised-validated secondary model the agent uses when the
  primary is unavailable/overloaded at runtime (the resilience lever — claude's `--fallback-model`).

**Non-goals**

- Changing `mode` handling (already a hard error on reject — correct).
- A cross-vendor abstract effort tier. Vocabularies genuinely differ (claude `max` vs codex
  `xhigh`); effort is treated as a **validated raw string**, not an abstract enum (§5.3).
- Pinning models for the `api`/ollama backend (already mandatory in the request body) or kiro
  (no model/effort knobs advertised).
- Auto-retry/resume orchestration beyond what `fallback_model` provides natively.

## 3. Probed ground truth (reference — captured live 2026-06-09)

Installed: **claude-agent-acp 0.39.0**, **codex-cli 0.135.0** (codex-acp), ACP SDK (node) 0.22.1.
(The repo's `=0.12.1` pin is the **Rust** `agent-client-protocol` *client* SDK — unrelated to the
node adapters. See §10 risk.)

**Model** — both adapters behave identically:

| | claude-agent-acp | codex-acp |
|---|---|---|
| Reports `availableModels` at `session/new` | ✅ `default`, `sonnet`, `sonnet[1m]`, `haiku` | ✅ `gpt-5.5`, `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex-spark` (×`low/medium/high/xhigh`) |
| Un-pinned **default float** | `default` = **Opus 4.8 · 1M** | `gpt-5.5/xhigh` |
| `set_model` honored **end-to-end** | ✅ `haiku` → served `claude-haiku-4-5-20251001` (transcript) | ✅ `gpt-5.4-mini` → served `gpt-5.4-mini` (rollout) |
| Validates the id? | ❌ `bogus-zzz` accepted | ❌ `o3`/`gpt-5.1`/`bogus-zzz` accepted |

**Effort** — claude broken through the bridge:

- claude effort config-id = **`effort`**; the bridge sends **`reasoning_effort`** → live:
  `-32603 Unknown config option: reasoning_effort` → swallowed (best-effort) → **never applied.**
- claude effort levels are **model-dependent**: Opus 4.8/4.7 = `low/medium/high/xhigh/max`;
  **Sonnet 4.6 / Opus 4.6 = `low/medium/high/max` (no `xhigh`)**.
- The bridge maps `Max → "xhigh"`; on Sonnet that is live `-32603 Internal error` (no `xhigh`).
- codex effort config-id = `reasoning_effort`, levels `low/medium/high/xhigh` → bridge works.
- Level ordering (for fallback): **`low < medium < high < xhigh < max`**.

## 4. Architecture

All resolution happens **at session mint**, inside `AcpBackend`'s `configure_session` closure,
immediately after `session/new` returns — the only place that has the agent's advertised state.
A new pure module `crates/bridge-acp/src/model_effort.rs` holds the **decision logic** (validation,
level fallback, log-line construction) as total pure functions over the advertised data; the mint
closure does the I/O (read response, send requests, log). Pure core ⇒ unit-testable without a live
agent; the live gate covers the wiring.

```
session/new ──► NewSessionResponse { models{availableModels,currentModelId},
                                     configOptions[ {id, options[], currentValue, category} ] }
        │
        ├─ resolve_model(cfg.model, availableModels)         ─► Ok(modelId) | Err(NotAdvertised{valid})
        │     └─ on Err: FAIL mint (loud)                     (§8)
        ├─ set_model(modelId)                                 (existing request builder)
        │     └─ capture config_option_update ► currentModelId, refreshed effort option
        ├─ resolve_effort(cfg.effort, activeModelEffortOption) ─► Applied{level} | FellBack{from,to} | Unsupported
        │     └─ apply via advertised effort id; warn on FellBack
        ├─ (fallback_model) wire agent-native fallback         (§5.4)
        └─ log: agent=… model=<currentModelId> effort=<applied> [fallback=…]
```

## 5. Components

### 5.1 Capability discovery (`AcpBackend`, mint closure)

After `session/new`, read from `NewSessionResponse`:
- `models.availableModels` (`Vec<ModelInfo{modelId,name}>`) and `models.currentModelId`.
- `configOptions`: locate the **model** option (`category == "model"` / `id == "model"`) and the
  **effort** option (`category == "thought_level"`, or `id ∈ {"effort","reasoning_effort"}`) — by
  advertised metadata, **not** a hardcoded id. Each option carries `id`, `currentValue`,
  `options[]{value}`.
- Effort options change with the active model and the adapter **pushes a `config_option_update`**
  after `set_model`. The mint closure must **capture that notification** (it already receives
  session updates) to read the **post-`set_model`** effort option before resolving effort.

### 5.2 Model resolution (`resolve_model`)

```
resolve_model(want: Option<&str>, available: &[ModelInfo]) -> Result<Option<ModelId>, ModelError>
```
- `None` ⇒ `Ok(None)` (leave the agent's default — documented as the float, e.g. Opus 4.8 1M).
- `Some(id)` present in `available` (exact match on `modelId`) ⇒ `Ok(Some(id))`.
- `Some(id)` **absent** ⇒ `Err(NotAdvertised { want, valid: available ids })` ⇒ **fail the mint**
  with a message listing valid ids. **Decision:** hard-error, no silent default (catches typos;
  forces reproducible config-driven pins to use advertised canonical ids — `default`/`sonnet`/
  `haiku`, not CLI shorthand like `opus`).

### 5.3 Effort resolution (`resolve_effort`)

Effort stays a **typed `Effort` enum**, extended with **`Xhigh`** (so it expresses the full
vocabulary `Minimal/Low/Medium/High/Xhigh/Max`). The bridge maps a tier to a canonical level name
(`Minimal/Low→"low"`, `Medium→"medium"`, `High→"high"`, `Xhigh→"xhigh"`, `Max→"max"`), then resolves
that name against the active model's advertised `options[]` with the highest-≤-requested fallback.
This keeps `effort=` type-safe (no `SessionSpec`/`effective_config` ripple) **and** subsumes both
vendor vocabularies with one mapping: codex `Max`→(no `max` advertised)→falls back to `xhigh`; claude
`Max`→`max`. (Refinement over an earlier "raw validated string" idea — less ripple, same coverage.)

```
resolve_effort(want: &str, opt: &EffortOption /* {id, levels: Vec<String>} */, order: &[&str])
    -> EffortPlan
// EffortPlan = Apply{id, level} | FellBack{id, from, to} | Unsupported{from, levels}
```
- `want` in `opt.levels` ⇒ `Apply{ id: opt.id, level: want }`.
- `want` absent ⇒ pick the **highest advertised level ≤ `want`** by `order`
  (`low<medium<high<xhigh<max`) ⇒ `FellBack{...}` + **warn**. **Decision:** fallback + warn.
- No level ≤ `want` advertised (shouldn't happen for real inputs) ⇒ `Unsupported` ⇒ warn, skip.
- Applied via `session/set_config_option` using **`opt.id`** (the advertised id) — this is the
  claude-effort fix.

### 5.4 `fallback_model` (resilience)

Optional `fallback_model =` on `[[agents]]`, **validated against `availableModels` exactly like the
primary** (hard-error if not advertised). Wired to the agent's native runtime fallback:
- **claude:** `--fallback-model <id>` (per the Claude CLI reference). Verify at implementation how
  `claude-agent-acp` forwards argv to the underlying `claude` (the adapter reads `process.argv`);
  pass it at spawn alongside the existing args.
- **codex:** verify the codex-cli equivalent (user can provide codex CLI docs); if none, document
  `fallback_model` as **claude-only** and reject it on a codex agent at config load.

This is distinct from the typo case: a typo `model=` still hard-errors; `fallback_model` only
engages when the (valid) primary is unavailable/overloaded at runtime.

### 5.5 Config surface (`bin/a2a-bridge/src/config.rs`)

`AgentToml` already has `model`, `effort`, `mode`. Add `fallback_model: Option<String>`. `effort`
parsing widens to accept any string (validated later at mint against advertised levels) while still
accepting the enum words. `AcpConfig` carries `model`, `effort` (string), `fallback_model`.

### 5.6 Observability

One `tracing::info` per agent at mint:
`model_effort_resolved agent=<id> model=<currentModelId> effort=<applied|fellback:from→to|default> [fallback_model=<id>]`.
On `NotAdvertised` model ⇒ `tracing::error` + mint failure. On effort fallback ⇒ `tracing::warn`.

## 6. Data flow / order of operations (mint)

1. `session/new` → capture `models` + `configOptions`.
2. `set_mode` (unchanged, hard error on reject).
3. `resolve_model` → on `Err` **fail mint**; else `set_model(id)`; capture the `config_option_update`
   (new `currentModelId` + refreshed effort option).
4. `resolve_effort` against the **post-`set_model`** effort option → `set_config_option(opt.id, level)`.
5. Apply `fallback_model` at spawn (claude `--fallback-model`) — note this is a **spawn-arg**, so it
   is wired in the `SpawnFn`/`acp_program_argv` path, not the session closure (§9).
6. Emit the resolved log line.

Order rationale: model **before** effort (switching model resets effort to the model default and
re-scopes valid levels); effort resolved against the **active** model's advertised levels.

## 7. Files touched

- **Create** `crates/bridge-acp/src/model_effort.rs` — pure `resolve_model`, `resolve_effort`,
  `EffortPlan`, `ModelError`, level-order constant, log-line builder + unit tests.
- **Modify** `crates/bridge-acp/src/acp_backend.rs` — mint closure: capture advertised state, call
  the pure resolvers, apply via advertised ids, capture `config_option_update`, log. Remove the
  hardcoded `EFFORT_CONFIG_ID`/`effort_value` codex assumptions (move into capability-driven path;
  keep codex behavior via discovery).
- **Modify** `bin/a2a-bridge/src/config.rs` — `fallback_model` field; effort string passthrough;
  `AcpConfig` wiring; both-or-neither/validation as needed.
- **Modify** the `SpawnFn`/`acp_program_argv` site(s) (`bin/a2a-bridge/src/main.rs`,
  `crates/bridge-container`) — append claude `--fallback-model` when set.
- **Docs:** example config comments with the effort-level guidance (§11); ADR-00xx.

## 8. Error handling summary

| Case | Behavior |
|---|---|
| `model=` not advertised (typo / non-advertised alias) | **Hard error**, fail mint, list valid ids |
| `fallback_model=` not advertised | **Hard error** at validation, list valid ids |
| `fallback_model=` on an agent with no native fallback | Hard error at config load (documented per-agent) |
| `effort=` unsupported by active model | **Fall back** to highest advertised ≤ requested, **warn** |
| `effort=` with no level ≤ requested advertised | Warn, leave model default (skip) |
| advertised state missing (agent reports no `models`/effort option) | Skip that dimension, `info`-log "not advertised"; never fatal |
| `mode=` rejected | Unchanged (hard error) |

## 9. Testing

- **Unit (pure `model_effort.rs`):** `resolve_model` (present/absent/none), `resolve_effort`
  (exact / fallback `xhigh→high` on a sonnet-like level set / `max` present / no-≤ level), level
  ordering, log-line text. Table-driven from the §3 probed vocabularies.
- **Config:** `fallback_model` parse + both-agents validation; effort string passthrough.
- **Live gate** (real claude + codex): (a) `model="haiku"` → transcript serves
  `claude-haiku-4-5-*`; (b) `model="bogus"` → mint fails loudly; (c) `effort="high"` on a sonnet
  pin actually applies (no `Unknown config option`); (d) `effort="xhigh"` on sonnet → warns +
  applies `high`; (e) codex unchanged (`reasoning_effort` still works via discovery); (f)
  `fallback_model` accepted + spawn-arg present. Confirm via adapter transcripts/rollouts, not
  self-report.

## 10. Rust client SDK skew — VERIFIED RETIRED

The capability-driven design requires the bridge's Rust `agent-client-protocol` 0.12.1 client to
parse `NewSessionResponse.models`, `config_options`, and the `config_option_update` notification.
**Verified present** in the pinned 0.12.1 (schema crate 0.13.2) with the **already-enabled**
`unstable_session_model` feature (`bridge-acp/Cargo.toml`): `NewSessionResponse.models:
Option<SessionModelState>` + `config_options: Option<Vec<SessionConfigOption>>`; `SessionModelState
{ current_model_id, available_models: Vec<ModelInfo{model_id,name}> }`; `SessionConfigOption { id,
category: Mode|Model|ThoughtLevel|Other, kind: Select{current_value, options} }`;
`SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate{config_options})`. **No SDK bump needed** —
`unstable_session_usage` (the usage-hang fix `cfc1ce3`) is untouched. The effort option is found by
`category == ThoughtLevel`, the model option by `category == Model` — no hardcoded ids.

## 11. Reference: effort level guidance (for config authors)

| Level | When to use |
|---|---|
| `low` | Short, scoped, latency-sensitive tasks that are not intelligence-sensitive |
| `medium` | Cost-sensitive work that can trade off some intelligence |
| `high` | Balances tokens and intelligence. **Default** on Fable 5, Opus 4.8, Opus 4.6, Sonnet 4.6 |
| `xhigh` | Deeper reasoning at higher token spend. **Default** on Opus 4.7 |
| `max` | Deepest reasoning, no token constraint; diminishing returns / overthinking risk — test first. **Session-only** (except via `CLAUDE_CODE_EFFORT_LEVEL`) |

Defaults applied on first run of Fable 5 / Opus 4.8 / Opus 4.7 override a previously-set level —
which is why the bridge **always sets effort explicitly per session** and logs what it applied.

## 12. Open items (resolved during implementation)

- Exact `claude-agent-acp` argv forwarding for `--fallback-model` (§5.4).
- codex `fallback_model` equivalent, or document claude-only (§5.4).
- Rust SDK 0.12.1 field support / bump scope (§10).
