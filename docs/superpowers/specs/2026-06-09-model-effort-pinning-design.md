# Capability-driven model & effort pinning — design (v2)

**Date:** 2026-06-09
**Status:** Draft v2 (revised after the bridge's own spec-review found 3 blockers, and after bumping
`claude-agent-acp` → 0.44.0 / `claude-agent-sdk` 0.3.170, which restabilized the model API).
**Builds on:** the existing `AcpBackend` mint-time config path (`crates/bridge-acp/src/acp_backend.rs`).
**Supersedes:** v1 (which targeted the now-removed `session/set_model` + `models` surface).

---

## 1. Context

The bridge plumbs `model`/`effort`/`mode` per `[[agents]]`, applied at session mint. That path was
**codex-shaped and partly broken**: it hardcoded the effort config-id (`reasoning_effort`) and a codex
value vocabulary (`Max→xhigh`), so **claude effort was a silent no-op** (claude's id is `effort`) and
**model pins were unvalidated** (typos silently accepted, then served by a fallback or died mid-turn).

Two events reshaped the fix:
1. The bridge's own **spec-review** (codex rigor + claude soundness) found 3 blockers in v1 — chiefly
   that the v1 effort-capture mechanism was unbuildable and `fallback_model`'s argv seam was dead.
2. We bumped **`claude-agent-acp` 0.39.0 → 0.44.0** (to get `claude-agent-sdk 0.3.170` / Fable 5),
   which **restabilized model selection into the config-option framework**: the dedicated
   `session/set_model` method and the `models`/`availableModels` field are **gone**; model is now a
   `SessionConfigOption` with `category == "model"`, set via `session/set_config_option` exactly like
   effort and mode.

The new surface makes the right design *simpler and uniform*: **discover model + effort from the
agent's advertised `config_options`, validate against the advertised values, apply via
`set_config_option`, and read the refreshed options back from the synchronous response.** No
hardcoded ids/vocabularies; no event-loop changes; no separate model method.

## 2. Goals / Non-goals

**Goals**
- **Validate** `model=` against the advertised **model config-option values**; a non-advertised id
  **fails the session mint loudly** with the valid list (no silent default, no mid-turn death).
- **Fix claude effort** (silent no-op today): apply via the agent's **advertised effort config-id**
  (`effort` for claude; `reasoning_effort` for codex), discovered at mint.
- **Adapt effort** to the active model's supported levels: on an unsupported level the ACP path
  errors (`-32603`), so the bridge **walks down** the canonical order to the highest supported
  ≤ requested and **warns** (mirrors the Claude CLI, which the ACP path does *not* do itself).
- **Observability:** one mint-time log line per agent with the **resolved** model + **applied**
  effort (and any fallback).
- **Loud, not silent, on missing capability:** a *configured* pin whose advertised option is absent
  is a loud warning/error — never the quiet "agent has no such knob" skip.

**Non-goals**
- `mode` handling (already a hard error on reject — correct).
- `fallback_model` — **descoped to a follow-up** (spec-review B3): the claude adapter ignores argv for
  model options in ACP mode, so the argv seam is dead; the real channel is `_meta.claudeCode.options`
  on `session/new`, and the SDK shape (single `string` vs the CLI's comma chain) needs its own design.
  Removing it keeps this increment to the core fix-and-validate goal.
- A cross-vendor abstract effort tier (vocabularies differ: claude `max` vs codex `xhigh`).
- `api`/ollama model (mandatory in the request body already) and kiro (advertises no model/effort knob).
- Adding Fable serving — already delivered by the 0.44.0 bump; this increment makes it *pinnable+validated*.

## 3. Probed ground truth (live, 2026-06-09 — post-bump)

Installed: **claude-agent-acp 0.44.0** (bundled `@anthropic-ai/claude-agent-sdk 0.3.170`, node ACP SDK
0.25.0); **codex-cli 0.135.0**; Rust `agent-client-protocol =0.12.1` (schema 0.13.2,
`unstable_session_model` enabled — still fine; see §10).

**Model is now a config option** (`category == "model"`, id `"model"`). Claude advertises:
`default` (= Opus 4.8), `claude-fable-5[1m]`, `sonnet`, `sonnet[1m]`, `haiku`; `currentValue` = `default`.
- `set_config_option(configId="model", value="fable")` resolves the **alias** to **`claude-fable-5`** and
  serves it end-to-end (transcript-confirmed). So aliases resolve even when not in the advertised list;
  the advertised list is the *picker* set, not the only acceptable input.
- The dedicated `session/set_model` is **not routed** by 0.44.0 (the bridge's current call is a no-op).
- **Fable now serves** (was `model_not_found` on 0.39.0's older bundled SDK).

**Effort** (`category == "thought_level"`, id `"effort"` for claude / `"reasoning_effort"` for codex):
claude levels are model-dependent — Opus 4.8/4.7 = low/medium/high/xhigh/max; Sonnet 4.6/Opus 4.6 =
low/medium/high/max (no `xhigh`). Order: **low < medium < high < xhigh < max**. The ACP
`set_config_option` **errors `-32603`** on an unsupported level (no graceful fallback). codex levels:
low/medium/high/xhigh.

**`set_config_option` response carries the refreshed options** — `SetSessionConfigOptionResponse
.config_options: Vec<SessionConfigOption>` is populated (live-confirmed: a `set_config_option(effort,
"high")` returned the 3 refreshed options). This is the in-band read-back the design relies on.

## 4. Architecture

All resolution happens **at session mint**, inside the `AcpBackend` `configure_session` closure, using
only the **synchronous request/response path** the closure already speaks (`session/new` response +
`set_config_option` response) — **no event-loop / notification handler changes** (spec-review B2).

A new pure module `crates/bridge-acp/src/model_effort.rs` holds the decision logic as total functions
over the advertised `config_options`; the closure does the I/O. Order: `set_mode` (unchanged) →
**model** (validate → `set_config_option(model)`) → **effort** (resolve against the refreshed options
from the model response, apply via `set_config_option`, walk-down on `-32603`) → **log**.

```
session/new ─► config_options[ {id, category: Mode|Model|ThoughtLevel|Other, kind: Select{currentValue, options[]}} ]
   │
   ├─ find Model option ─► resolve_model(cfg.model, model_opt.values) ─► Apply(id) | Default | Err(NotAdvertised)  (Err ⇒ FAIL mint)
   ├─ set_config_option(model_opt.id, id) ─► response.config_options  (refreshed, scoped to the new model)
   ├─ find ThoughtLevel option in the REFRESHED options ─► resolve_effort(cfg.effort, effort_opt) ─► Apply|FellBack|Skip
   │     └─ set_config_option(effort_opt.id, level); on -32603 walk DOWN order, retry; warn on fallback
   └─ log: agent=… model=<resolved currentValue> effort=<applied> [fellback …]
```

## 5. Components

### 5.1 Capability discovery (`model_effort.rs`, called from the mint closure)

From `NewSessionResponse.config_options` (and later from `SetSessionConfigOptionResponse.config_options`),
locate options by **`category`** (`SessionConfigOptionCategory::{Model, ThoughtLevel}`), falling back to
**`id`** (`"model"`; `"effort"`/`"reasoning_effort"`) only if `category` is absent. Each option's
`kind` must be `Select`; extract `current_value` + the `options[].value` list. Determinism rules
(spec-review M6): first option matching the category wins; a non-`Select` kind or missing option ⇒ treat
as "not advertised" for that dimension; never panic.

### 5.2 Model resolution (`resolve_model`)

```
resolve_model(want: Option<&str>, values: &[String]) -> Result<ModelDecision, ModelNotAdvertised>
// None => Default ; Some in values => Apply(value) ; Some absent => Err{want, valid: values}
```
- **Decision (hard-error, spec-review-aligned):** a configured `model=` not in the advertised values
  **fails the mint** with the valid list. Reproducible; catches typos; converts the cryptic mid-turn
  `model_not_found` (e.g. an un-serveable model on an old adapter) into a clear mint error.
- **Alias note:** aliases (`fable`, `opus`) resolve at the adapter but are *not* in the advertised
  values, so they are **rejected** — config must use advertised ids (`claude-fable-5[1m]`, `default`).
  The error message lists them. (Documented; a future alias-map is a YAGNI follow-up.)
- **Apply via `set_config_option(model_opt.id, value)`**, not `session/set_model` (gone). Capture the
  response's `config_options` as the **refreshed** set for effort resolution.
- **M2/M3 (loud-on-missing):** if `cfg.model.is_some()` but the agent advertised **no** Model option
  (or `config_options`/`models` deserialized to `None` via the SDK's `DefaultOnError`), that is a
  **distinct loud error** ("model pinned but agent advertised no model option — possible adapter/schema
  skew"), never the silent skip. Only an *unconfigured* model dimension skips quietly.

### 5.3 Effort resolution (`resolve_effort`) + Effort::Xhigh

Keep the typed `Effort` enum, **add `Xhigh`** → `Minimal/Low/Medium/High/Xhigh/Max`. Map a tier to a
canonical level name (`Minimal/Low→"low"`, `Medium→"medium"`, `High→"high"`, `Xhigh→"xhigh"`,
`Max→"max"`).

```
resolve_effort(want: Option<&str>, opt: &AdvertisedEffort{config_id, levels}) -> EffortDecision
// None => Skip ; want in levels => Apply{config_id, level} ; else highest level <= want by order => FellBack ; none => Unsupported
```
Resolve against the **refreshed** effort option (post-model `set_config_option` response). Because the
adapter *errors* on an unsupported level rather than clamping, the closure also **walks down on
`-32603`** as a belt-and-suspenders: try the resolved level; on `-32603`, drop to the next-lower
advertised level and retry, until one applies or none remain (warn). Both `Effort` parsers must learn
`xhigh` (see M7).

### 5.4 Observability (`§5.6` of v1, kept)

One `tracing::info` per agent at mint: `model_effort_resolved agent=<id> model=<resolved currentValue>
requested_model=<cfg> effort=<applied|fellback from→to|default>`. `tracing::error` + mint-fail on a bad
model; `tracing::warn` on effort fallback or a loud-missing-capability condition. **Agent-id threading
(m1):** `AcpConfig` carries no id today — add an `agent_id` field to `AcpConfig` (or emit the line from
the registry layer that holds `AgentEntry.id`).

## 6. Data flow (mint, ordered)

1. `session/new` → `config_options`.
2. `set_mode` (unchanged; hard error on reject).
3. **Model:** `resolve_model` → on `Err` **fail mint**; else `set_config_option(model_opt.id, value)`;
   keep the response's `config_options` as `refreshed`.
4. **Effort:** locate the ThoughtLevel option in `refreshed` (fallback to the initial set if the
   response omitted it); `resolve_effort`; `set_config_option(effort_opt.id, level)` with walk-down on
   `-32603`.
5. Emit the resolved log line.

## 7. Files touched

- **Create** `crates/bridge-acp/src/model_effort.rs` — pure `resolve_model`/`resolve_effort`,
  `AdvertisedEffort`, decisions, level order, option-discovery helpers, `effort_level_name`, log-line
  builder + unit tests. **Modify** `crates/bridge-acp/src/lib.rs` (`mod model_effort;`).
- **Modify** `crates/bridge-core/src/domain.rs` — add `Effort::Xhigh` (ordered High<Xhigh<Max); fix
  exhaustive matches.
- **Modify** `crates/bridge-acp/src/acp_backend.rs` — mint closure: discover from `config_options`, set
  model+effort via `set_config_option` reading the response, walk-down, log; add `AcpConfig.agent_id`;
  **remove** the `session/set_model`/`set_effort_request`/`EFFORT_CONFIG_ID`/`effort_value` path.
- **Modify** `bin/a2a-bridge/src/config.rs` — `parse_effort` accepts `xhigh`; thread `agent_id`.
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — **`parse_effort_meta` (M7)**: the second,
  per-request A2A-metadata effort parser must also accept `xhigh`. Factor a single `FromStr for Effort`
  so the two parsers can't drift.
- **Docs:** example configs (pin advertised model ids; effort guidance §11); ADR-0029.
- **NOT touched (descoped):** `fallback_model` config/spawn paths — follow-up.

## 8. Error handling

| Case | Behavior |
|---|---|
| `model=` not in advertised values | **Hard error**, fail mint, list valid ids |
| `model=` set but agent advertised no Model option / `config_options` None | **Hard error** (loud skew), not silent skip (M2/M3) |
| `effort=` unsupported by active model | **Walk down** to highest advertised ≤ requested, **warn** |
| `effort=` set but no ThoughtLevel option | **Warn** (configured-but-unavailable), skip |
| unconfigured model/effort dimension | quiet `info` skip (only when *not* configured) |
| `set_config_option` transport/other error | model: fail mint; effort: best-effort warn |
| `mode` rejected | unchanged (hard error) |

## 9. Testing

- **Unit (`model_effort.rs`):** `resolve_model` present/absent/none + NotAdvertised list; option
  discovery by category then id, non-Select/missing ⇒ not-advertised; `resolve_effort` exact / fallback
  (`xhigh`→`high` on a sonnet-like set) / `max` present / codex `max`→`xhigh` / none-≤; `effort_level_name`
  (`Max`→`max`); log-line text. Table-driven from §3 vocabularies.
- **Config:** `parse_effort("xhigh")`==`Xhigh`; both parsers (`config.rs` + `server.rs parse_effort_meta`)
  accept `xhigh` (one `FromStr`).
- **Live gate** (claude 0.44.0 + codex): (a) `model="haiku"` → transcript `claude-haiku-4-5-*`; (b)
  `model="claude-fable-5[1m]"` → transcript `claude-fable-5`; (c) `model="bogus"` → mint fails loudly;
  (d) `effort="high"` on a sonnet pin applies with no `Unknown config option`; (e) `effort="xhigh"` on
  sonnet → warns + applies `high`; (f) codex `model="gpt-5.5"` + `effort="high"` unchanged. Confirm via
  transcripts/rollouts.

## 10. Rust client SDK — still fine on 0.44.0

The pinned `agent-client-protocol 0.12.1` (schema 0.13.2) already has everything the new surface needs:
`SessionConfigOptionCategory::{Mode,Model,ThoughtLevel,Other}`, `SessionConfigOption`/`Select`,
`set_config_option` request + `SetSessionConfigOptionResponse.config_options`, and `config_options` on
`NewSessionResponse`. The removed `models`/`session/set_model` are simply unused (the `models` field is
`Optional`/`DefaultOnError` → `None`, harmless). **No Rust SDK bump.** (Tolerant `DefaultOnError`
deserialization is why M3's loud-on-`None`-when-pinned guard matters.)

## 11. Reference: effort level guidance (for config authors)

| Level | When to use |
| --- | --- |
| `low` | Short, scoped, latency-sensitive, not intelligence-sensitive |
| `medium` | Cost-sensitive work that can trade off some intelligence |
| `high` | Balances tokens & intelligence. Default on Fable 5, Opus 4.8, Opus 4.6, Sonnet 4.6 |
| `xhigh` | Deeper reasoning, higher spend. Default on Opus 4.7 |
| `max` | Deepest reasoning, no token constraint; diminishing returns — test first. Session-only (except via `CLAUDE_CODE_EFFORT_LEVEL`) |

## 12. Open items / follow-ups

- **`fallback_model` (descoped):** revisit via `_meta.claudeCode.options` on `session/new`; design the
  chain shape (CLI uses a comma list; SDK field is a single string).
- **Model aliases:** advertised ids only for now; an optional alias-map (`fable`→`claude-fable-5[1m]`) is
  a YAGNI follow-up if the ergonomics bite.
- **Fable:** delivered by the 0.44.0 bump; this increment makes it pinnable+validated. (The global
  `claude-agent-acp` is now 0.44.0 — other bridge sessions spawn the new surface.)
