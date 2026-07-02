# ADR-0029 — Capability-driven model & effort pinning (via `session/set_config_option`)

**Date:** 2026-06-09
**Status:** Accepted

**Builds on:** the `AcpBackend` mint seam (`configure_session`), ADR-0024 (warm loop
session — the `effective_config` fold that effort/model resolution must read so warm
sessions don't drop the pin). Rust client `agent-client-protocol =0.12.1`.

**Spec:** `docs/superpowers/specs/2026-06-09-model-effort-pinning-design.md` (v2.1).
**Plan:** `docs/superpowers/plans/2026-06-09-model-effort-pinning.md` (v2).

---

## Context

The bridge's model/effort plumbing was codex-shaped and partly broken:

- **claude effort was a silent no-op.** The bridge sent the config id `reasoning_effort`;
  claude advertises the option as `effort`, so set returned `-32602/-32603 Unknown config
  option` — swallowed. Effort never reached claude.
- **model pins were unvalidated.** A typo (`sonnnet`) was silently accepted, then served by
  the agent's default/fallback or died mid-turn — no fail-fast.
- **the `Max → "xhigh"` value mapping was wrong** for models without `xhigh` (Sonnet 4.6,
  Opus 4.6), and the bridge had no walk-down for an unsupported effort level.

**Keystone surface finding (probed live 2026-06-09).** Bumping `claude-agent-acp`
0.39.0 → **0.44.0** (done to get **Fable 5**; brings `@anthropic-ai/claude-agent-sdk`
0.3.170, node ACP SDK 0.25.0) **removed `session/set_model` and the `models`/`availableModels`
field**. Model is now a unified `SessionConfigOption` with `category=="model"` (id `"model"`),
set via `session/set_config_option` — exactly like effort (`category=="thought_level"`, id
`effort` claude / `reasoning_effort` codex) and mode. The `set_config_option` **response carries
the refreshed `config_options`** (an in-band read-back; no notification capture needed).
**codex-acp (unbumped) ALSO advertises `category=model`** (base ids gpt-5.5/5.4/5.4-mini/
5.3-codex-spark), so the unified config-option design holds for **both** agents. The Rust client
`=0.12.1` already carries `SessionConfigOptionCategory::{Mode,Model,ThoughtLevel,Other}` +
`set_config_option` + the response `config_options` → **no Rust SDK bump**.

The spec and plan were dual-reviewed by the bridge's **own** `spec-review`/`plan-review`
workflows (two dogfooded spec passes + one plan pass), the last two with **Fable as the claude
reviewer** (served `claude-fable-5`) — they caught the codex id-space blocker, the grouped-options
flatten, and six compile-level plan errors (import path, two `#[non_exhaustive]` wildcards,
`config_options` is a `Vec` not `Option`, a missing `agent_id` field, and a pre-existing
`BridgeError::ConfigInvalid` that did not need re-adding).

## Decision

Resolve model and effort **capability-driven at mint**, in a pure `model_effort` module fed by
the advertised options, with the SDK calls confined to `AcpBackend`:

1. **Discover.** Read the options the agent advertises at `session/new` (and the refreshed
   options from each `set_config_option` response). `model_values` finds the `category=="model"`
   option, flattening `Grouped`/`Ungrouped` and falling back to an `Other`-category id; `effort_opt`
   finds the thought-level option and its `config_id` (`effort` vs `reasoning_effort`) + levels.
2. **Model — validated, fatal, config-option surface.** Resolve the requested model against the advertised
   values; a pin **not** in the set **hard-fails the session** (`BridgeError::config_invalid`)
   *before any prompt is sent*. Raw advertised ids win; fallback aliases then resolve via a small
   static map (`opus → default` when `opus` is not advertised). As of the 2026-07-02 SDK 1.x amendment,
   the bridge applies model pins only through **`config_options` (category=model)** via
   `session/set_config_option`; the former unstable typed `models` state + `session/set_model`
   Kiro fallback is no longer exposed by the Rust client schema. Advertises a config option +
   valid pin → apply; advertises no config option + a pin → `config_invalid`; advertises no
   config option + no pin → skip.
3. **Effort — walked-down, non-fatal.** Resolve against the **refreshed** post-model options. Fall
   back to the highest supported level **≤** requested. The ACP path **errors** (`-32603`) on an
   unsupported level rather than clamping, so the bridge **walks down itself** by `EFFORT_ORDER`,
   using a precise `is_unsupported_effort_error` predicate to separate a walk-down-able error from
   an unrelated one (on which it stops and warns). A level **below** the lowest advertised level is
   skipped with a warn (`Unsupported`). `Effort::Xhigh` was added (between `High` and `Max`; no `Ord`
   — ordering lives in `EFFORT_ORDER`); both effort parsers route through one `FromStr`.
4. **Error surface.** The advertised valid list appears on **logs + CLI stderr**, but is redacted to
   the static `BridgeError` category on the **A2A wire** (the values could leak agent internals).
5. **Warm sessions.** Resolution reads `effective_config` (the ADR-0024 fold), so a warm `:rw`
   implement session does not silently drop the model/effort pin.

**Descoped:** `fallback_model`. claude's `--fallback-model` has no ACP session-config equivalent
(it is a CLI/turn concept), the value would be unvalidated, and the design's argv seam was dead
(spec-review B3). Mode is unchanged (still `session/set_mode`, hard-fail on an unknown id).

## Consequences

- **Migration (ADR-relevant breakage).** The kiro `model="auto"` pin is removed from
  `examples/a2a-bridge.multi-agent.toml`, the `init` scaffold fragment (`main.rs`), and the config
  parse-test fixture — kiro advertises **no** model option, so the pin would now hard-fail at mint.
  Any external config pinning a non-advertised model (notably kiro) must drop it. **claude effort now
  actually applies.** Model typos **fail fast at mint**, not mid-turn.
- **Docs.** README, onboarding, the init template, and the config headers were rewritten off the old
  `set_model`/"codex-only effort"/"claude model not observable" claims; an effort-level guidance table
  (model-dependent levels) was added.
- **Built by the bridge dogfooding itself.** Both code chunks were implemented via `a2a-bridge
  implement` (containerized **codex** implementor + the bridge's own verify→review→tweak loop). Chunk 1
  (pure `model_effort.rs`) converged in 2 attempts; chunk 2 (the mint rewire + ~10 test rewrites)
  converged in 3, the `implement-review` catching an **inverted `Unsupported` warn string** ("higher
  than all advertised" → corrected to "below the lowest advertised"; fixed in `a5fa607`). The
  warm-SessionSpec regression net (`per_session_config_is_isolated`, the stash/forget tests) survived
  the rewrite, as the plan-review demanded.
- **No Rust SDK bump** (types present in 0.12.1). Kept `unstable_session_usage` (the usage_update
  hang fix, ADR/`cfc1ce3`). *(Originally dropped `unstable_session_model` — RESTORED 2026-06-10; see
  the amendment below.)*
- **Live gate (Task 10) — PASS** (2026-06-09, real claude-agent-acp 0.44.0 + codex-cli 0.135.0 on the
  host, via `run-workflow` single-node probes):
  - **(a)** `model=haiku` → bridge `model_effort_resolved … model=haiku`; transcript served
    `claude-haiku-4-5-20251001`.
  - **(b)** `model=fable` → alias resolved to `claude-fable-5[1m]` (the advertised id); served
    `claude-fable-5`.
  - **(c)** `model=bogus-not-a-model` → mint **hard-failed**: `ConfigInvalid { reason: "… is not
    advertised; valid models: default, claude-fable-5[1m], sonnet, sonnet[1m], haiku" }` surfaced on
    the CLI/logs (the A2A-wire redaction to the static category is the separate, unit-tested
    `client_message()` path).
  - **(d)** `model=sonnet effort=high` → applied cleanly, **no** `Unknown config option`; served
    `claude-sonnet-4-6`.
  - **(e)** `model=sonnet effort=xhigh` → `effort=high (fell back from xhigh)` (Sonnet 4.6 advertises no
    `xhigh`); served `claude-sonnet-4-6`.
  - **(f)** codex `model=gpt-5.5 effort=high` → unchanged: rollout records `"model":"gpt-5.5"` +
    `"reasoning_effort":"high"`, no `Unknown config option`.

  Pre-gate also green: `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, full
  `--workspace` test (via the coverage run), and the ci.yml coverage floors (workspace 87.4%≥85;
  bridge-core 94.8%, bridge-acp 94.4% [`model_effort.rs` 96.6% line], bridge-api 95.8%,
  bridge-workflow 92.9%, all ≥90).
## Amendment (2026-06-10) — kiro's model surface (`unstable_session_model` restored)

The initial cut **dropped `unstable_session_model`** on the assumption that all agents had moved model
selection into `config_options`. A live probe (prompted by the owner) disproved that for **kiro**:
kiro returns `config_options: None` at `session/new` but **does** advertise its model via the unstable
`models` field — `SessionModelState { current_model_id: "auto", available_models: [auto,
claude-sonnet-4.5, claude-sonnet-4, claude-haiku-4.5, …] }` — and accepts `session/set_model`. Dropping
the feature made `resp.models` undeserializable (the field is `#[cfg]`-gated out), so the bridge could
not see kiro's models and `model=…` on kiro hard-failed at mint — a **regression** vs the pre-increment
best-effort `set_model` path.

**Fix:** re-enable `unstable_session_model` and make `configure_model_option` try **both** surfaces —
`config_options` first (claude/codex), then the `models` state + `session/set_model` (kiro) — reusing
the same `resolve_model` validation + alias map; a pin on neither surface is still `config_invalid`.
`model_effort::model_state_values` extracts the advertised ids; `AcpBackend::set_model` applies them.
The live-only `live_edit_changes_new_session_model` e2e (kiro `auto` → `claude-sonnet-4.5` on a new
session, same warm backend) now **PASSES** (both turns PONG/`end_turn`). 24 `model_effort` unit tests +
99 `bridge-acp` tests green; effort is unaffected (kiro advertises no thought-level option → skipped).

## Amendment (2026-07-02) — ACP SDK 1.x removes the typed models surface

The Rust SDK was upgraded to `agent-client-protocol =1.0.1`. In that SDK line,
the typed `models` state and `session/set_model` request used by the 2026-06-10
Kiro fallback are no longer exposed on the v1 client schema. The current bridge
therefore applies model pins only through `session/set_config_option` on an
advertised `category="model"` config option.

Operationally, this preserves the validated/fail-fast contract: a pinned model
that is not in the advertised config-option values hard-fails mint, and a pinned
model on an agent with no model config option hard-fails with "advertised no
model option". The live catalog now separates discovery from configurability:
Kiro's native `kiro-cli chat --list-models` values remain visible for operator
awareness, but the Agent Card / `models --json` entry marks them with
`model_configurable: false`, and the table output labels the model override as
unavailable. Current `kiro-cli acp` configs should leave `model` unset unless
the catalog marks that agent `model_configurable: true`.

## Amendment (2026-07-02) — Fable-family model ids are blocked

The bridge now blocks model ids containing `fable` even when an ACP agent
advertises them. A config pin or per-request `a2a-bridge.model` override using a
Fable-family value fails at mint with `BridgeError::config_invalid` before any
prompt is sent. The same ids are filtered out of the usable model catalog so
Agent Card and `models` output do not present them as valid override choices.
The remaining Claude fallback shorthand is `opus -> default` when `opus` is not
advertised.
