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
2. **Model — validated, fatal.** Resolve the requested model against the advertised values; a pin
   **not** in the set **hard-fails the session** (`BridgeError::config_invalid`) *before any prompt
   is sent*. Aliases resolve **before** validation via a small static map (`fable → claude-fable-5[1m]`,
   `opus → default`). Agent advertises a model option + valid pin → `set_config_option(model)`;
   advertises none + a pin → `config_invalid`; advertises none + no pin → skip.
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
  actually applies.** Model typos **fail fast at mint**, not mid-turn. **Fable serves end-to-end**
  (`model="fable"` → `claude-fable-5`).
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
- **No Rust SDK bump** (types present in 0.12.1). Dropped the `unstable_session_model` cargo feature;
  kept `unstable_session_usage` (the usage_update hang fix, ADR/`cfc1ce3`).
- **Live gate (Task 10).** Pending — to be recorded here once run against real claude 0.44.0 + codex:
  `model=haiku`→haiku, `model=fable`→`claude-fable-5`, `model=bogus`→mint fails (valid list on
  stderr/log, wire stays the static category), `model=sonnet effort=high`→applies (no `Unknown config
  option`), `effort=xhigh` on sonnet→falls back to `high`, codex `gpt-5.5 effort=high`→unchanged. The
  live-only `live_edit_changes_new_session_model` e2e test (kiro model-flip) needs re-pointing to an
  agent that advertises a model option, since kiro no longer does.
