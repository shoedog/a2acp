# Capability-driven Model & Effort Pinning — Implementation Plan (v2, for spec v2.1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`).
> **Supersedes** the v1 plan (which targeted the removed `session/set_model` + `models` surface). Implements `docs/superpowers/specs/2026-06-09-model-effort-pinning-design.md` (v2.1).

**Goal:** Pin & validate model + effort against each agent's advertised `config_options` at mint, via `set_config_option` (the unified 0.44.0/codex surface); fix the silent claude-effort no-op; hard-error on a bad model with the valid list; log the resolved model/effort.

**Architecture:** Pure `crates/bridge-acp/src/model_effort.rs` (alias map, discovery, resolvers, effort-error predicate — all total, unit-tested) + the `AcpBackend` mint closure doing only synchronous request/response I/O (`session/new` response, `set_config_option` response). No `session/set_model`, no notification capture, no `fallback_model` (descoped).

**Surfaces (probed live 2026-06-09):** claude-agent-acp 0.44.0 & codex-acp both advertise `config_options` with `category ∈ {mode, model, thought_level}`, `kind=Select{currentValue, options}`. Model option values: claude `default|claude-fable-5[1m]|sonnet|sonnet[1m]|haiku`; codex `gpt-5.5|gpt-5.4|gpt-5.4-mini|gpt-5.3-codex-spark`. Effort id `effort` (claude) / `reasoning_effort` (codex); levels model-dependent; `set_config_option` **errors `-32603`** on an unsupported level and its **response carries refreshed `config_options`**. Rust `agent-client-protocol 0.12.1` (schema 0.13.2) already has `SessionConfigOptionCategory::{Mode,Model,ThoughtLevel,Other}`, `Select` (`options: Ungrouped|Grouped`), `set_config_option` + `SetSessionConfigOptionResponse.config_options`. No Rust SDK bump.

---

## Task 1: `Effort::Xhigh` (no `Ord`)

**Files:** Modify `crates/bridge-core/src/domain.rs` (+ test mod).

- [ ] **Step 1: Failing test** — `parse`/round-trip of `Xhigh`. (Find the enum: `grep -n 'enum Effort' crates/bridge-core/src/domain.rs`; today `Minimal,Low,Medium,High,Max`.)
```rust
#[test] fn effort_has_xhigh() { assert_eq!(Effort::Xhigh.as_str(), "xhigh"); }
```
- [ ] **Step 2:** Run → FAIL. `cargo test -p bridge-core effort_has_xhigh`
- [ ] **Step 3:** Add `Xhigh` between `High` and `Max`; add the `"xhigh"` arm to any `as_str`/`Display`/serde map. **Do NOT add `Ord`** (ordering lives in `model_effort`, per spec §5.3 — avoids `Minimal→low` rank shadowing). Fix exhaustive matches the compiler flags.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** `git commit -m "feat(core): add Effort::Xhigh tier"`

## Task 2: One `FromStr<Effort>` shared by both parsers (M7)

**Files:** `crates/bridge-core/src/domain.rs` (impl `FromStr`), `bin/a2a-bridge/src/config.rs` (`parse_effort`), `crates/bridge-a2a-inbound/src/server.rs` (`parse_effort_meta`).

- [ ] **Step 1: Failing test** in domain: `"xhigh".parse::<Effort>() == Ok(Effort::Xhigh)`, plus `minimal/low/medium/high/max`.
- [ ] **Step 2:** Implement `impl FromStr for Effort` (accepts `minimal,low,medium,high,xhigh,max`, case-insensitive; `Err(String)` otherwise).
- [ ] **Step 3:** Repoint `config.rs::parse_effort` and `server.rs::parse_effort_meta` to delegate to `Effort::from_str` (so the two can't drift). Add a test in `server.rs` that `parse_effort_meta("xhigh")` ⇒ `Xhigh` (previously rejected).
- [ ] **Step 4:** Run `cargo test -p bridge-core -p a2a-bridge -p bridge-a2a-inbound effort`.
- [ ] **Step 5:** `git commit -m "refactor(core): single FromStr<Effort>; both parsers accept xhigh"`

## Task 3: Pure `model_effort.rs` — alias map + `resolve_model`

**Files:** Create `crates/bridge-acp/src/model_effort.rs`; modify `crates/bridge-acp/src/lib.rs` (`mod model_effort;`).

- [ ] **Step 1: Write the module + failing tests:**
```rust
//! Pure capability-driven resolution of model & effort against advertised config options.

/// Static shorthand → advertised-id map, applied BEFORE validation (spec §5.2).
const MODEL_ALIASES: &[(&str, &str)] = &[("fable", "claude-fable-5[1m]"), ("opus", "default")];
pub fn apply_alias(want: &str) -> &str {
    MODEL_ALIASES.iter().find(|(a, _)| *a == want).map(|(_, v)| *v).unwrap_or(want)
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelDecision { Default, Apply(String) }
#[derive(Debug, PartialEq, Eq)]
pub struct ModelNotAdvertised { pub want: String, pub valid: Vec<String> }

/// `want` None => Default. Else alias-map then exact-match against advertised values.
pub fn resolve_model(want: Option<&str>, values: &[String]) -> Result<ModelDecision, ModelNotAdvertised> {
    let raw = match want { None => return Ok(ModelDecision::Default), Some(w) => w };
    let mapped = apply_alias(raw);
    if values.iter().any(|v| v == mapped) { Ok(ModelDecision::Apply(mapped.to_string())) }
    else { Err(ModelNotAdvertised { want: raw.to_string(), valid: values.to_vec() }) }
}

#[cfg(test)]
mod model_tests {
    use super::*;
    fn claude() -> Vec<String> { ["default","claude-fable-5[1m]","sonnet","sonnet[1m]","haiku"].iter().map(|s| s.to_string()).collect() }
    #[test] fn none_default() { assert_eq!(resolve_model(None,&claude()).unwrap(), ModelDecision::Default); }
    #[test] fn advertised() { assert_eq!(resolve_model(Some("haiku"),&claude()).unwrap(), ModelDecision::Apply("haiku".into())); }
    #[test] fn fable_alias_maps_to_1m() { assert_eq!(resolve_model(Some("fable"),&claude()).unwrap(), ModelDecision::Apply("claude-fable-5[1m]".into())); }
    #[test] fn opus_alias_maps_to_default() { assert_eq!(resolve_model(Some("opus"),&claude()).unwrap(), ModelDecision::Apply("default".into())); }
    #[test] fn typo_errs_with_valid_list() { let e=resolve_model(Some("bogus"),&claude()).unwrap_err(); assert_eq!(e.want,"bogus"); assert!(e.valid.contains(&"haiku".into())); }
    #[test] fn alias_target_not_advertised_errs() { assert!(resolve_model(Some("fable"), &["sonnet".to_string()]).is_err()); } // vendor-rename safety
    #[test] fn codex_base_id() { assert_eq!(resolve_model(Some("gpt-5.5"), &["gpt-5.5".to_string()]).unwrap(), ModelDecision::Apply("gpt-5.5".into())); }
}
```
- [ ] **Step 2:** Run → PASS. `cargo test -p bridge-acp model_effort::model_tests`
- [ ] **Step 3:** `git commit -m "feat(acp): model_effort alias map + resolve_model"`

## Task 4: Option discovery from advertised config options (category/id, Grouped, Other)

**Files:** Modify `crates/bridge-acp/src/model_effort.rs`.

- [ ] **Step 1: Failing tests + functions** (uses SDK types):
```rust
use agent_client_protocol::{SessionConfigOption, SessionConfigOptionCategory as Cat, SessionConfigKind, SessionConfigSelectOptions};

fn select_values(sel_options: &SessionConfigSelectOptions) -> Vec<String> {
    // Select.options is untagged Ungrouped | Grouped — flatten groups (spec M2).
    match sel_options {
        SessionConfigSelectOptions::Ungrouped(v) => v.iter().map(|o| (*o.value.0).to_string()).collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups.iter().flat_map(|g| g.options.iter().map(|o| (*o.value.0).to_string())).collect(),
    }
}
fn matches(opt: &SessionConfigOption, cat: Cat, ids: &[&str]) -> bool {
    match &opt.category {
        Some(c) if *c == cat => true,
        None | Some(Cat::Other(_)) => ids.contains(&&*opt.id.0), // m2: id fallback when absent OR Other
        _ => false,
    }
}
/// Returns (current_value, values) for the first matching Select option, flattening groups.
fn find_select(opts: &[SessionConfigOption], cat: Cat, ids: &[&str]) -> Option<(String, String, Vec<String>)> {
    opts.iter().find(|o| matches(o, cat.clone(), ids)).and_then(|o| match &o.kind {
        SessionConfigKind::Select(sel) => Some(((*o.id.0).to_string(), (*sel.current_value.0).to_string(), select_values(&sel.options))),
        _ => None,
    })
}
pub fn model_values(opts: &[SessionConfigOption]) -> Option<(String, Vec<String>)> {
    find_select(opts, Cat::Model, &["model"]).map(|(id,_cur,vals)| (id, vals))
}
pub struct AdvertisedEffort { pub config_id: String, pub levels: Vec<String> }
pub fn effort_opt(opts: &[SessionConfigOption]) -> Option<AdvertisedEffort> {
    find_select(opts, Cat::ThoughtLevel, &["effort","reasoning_effort"])
        .map(|(id,_cur,vals)| AdvertisedEffort { config_id: id, levels: vals.into_iter().filter(|v| v != "default").collect() })
}
```
Tests: build SDK `SessionConfigOption`s — an Ungrouped model option (values match), a **Grouped** model option (assert flattened), an **`Other`-category** option with id `"model"` (assert found via id fallback), a non-`Select` kind (assert `None`).
(Confirm the exact `SessionConfigSelectOptions` variant names + `group.options` field at impl time: `grep -n 'enum SessionConfigSelectOptions\|struct.*Group' ~/.cargo/registry/src/*/agent-client-protocol-schema-0.13.2/src/v1/agent.rs`.)
- [ ] **Step 2:** Run → PASS.
- [ ] **Step 3:** `git commit -m "feat(acp): discover model/effort options (category+id, grouped, Other)"`

## Task 5: Pure `resolve_effort` + `is_unsupported_effort_error`

**Files:** Modify `crates/bridge-acp/src/model_effort.rs`.

- [ ] **Step 1: Failing tests + code:**
```rust
use bridge_core::domain::Effort;
pub const EFFORT_ORDER: &[&str] = &["low","medium","high","xhigh","max"];
fn rank(l: &str) -> Option<usize> { EFFORT_ORDER.iter().position(|x| *x == l) }
pub fn effort_level_name(e: Effort) -> &'static str {
    match e { Effort::Minimal | Effort::Low => "low", Effort::Medium => "medium", Effort::High => "high", Effort::Xhigh => "xhigh", Effort::Max => "max" }
}
#[derive(Debug, PartialEq, Eq)]
pub enum EffortDecision { Skip, Apply{config_id:String, level:String}, FellBack{config_id:String, from:String, to:String}, Unsupported{from:String} }
pub fn resolve_effort(want: Option<Effort>, adv: &AdvertisedEffort) -> EffortDecision {
    let want = match want { None => return EffortDecision::Skip, Some(e) => effort_level_name(e) };
    if adv.levels.iter().any(|l| l == want) { return EffortDecision::Apply{config_id: adv.config_id.clone(), level: want.into()}; }
    let wr = rank(want);
    let best = adv.levels.iter().filter(|l| rank(l).is_some() && rank(l) <= wr).max_by_key(|l| rank(l));
    match best { Some(to) => EffortDecision::FellBack{config_id: adv.config_id.clone(), from: want.into(), to: to.clone()}, None => EffortDecision::Unsupported{from: want.into()} }
}
/// Walk-down predicate (spec MAJOR 4): -32603 AND an invalid/unsupported-value message.
pub fn is_unsupported_effort_error(code: i64, message: &str) -> bool {
    code == -32603 && (message.contains("Invalid value") || message.contains("not support") || message.contains("model_not_found"))
}
```
Tests: `none→Skip`; sonnet `high→Apply{effort,high}`; sonnet `xhigh→FellBack to high`; sonnet `max→Apply` (sonnet has max); codex `max→FellBack to xhigh`; `effort_level_name(Max)=="max"`; `is_unsupported_effort_error(-32603,"Invalid value …")` true, `(-32603,"usage_update")` false, `(-32000,"Invalid value")` false.
- [ ] **Step 2:** Run → PASS. **Step 3:** `git commit -m "feat(acp): resolve_effort + precise unsupported-effort predicate"`

## Task 6: `BridgeError::config_invalid` (operator-facing list, redacted on wire)

**Files:** Modify `crates/bridge-core/src/error.rs`.

- [ ] **Step 1: Failing test** — a `config_invalid("model 'x' not advertised (valid: a, b)")`: `Display`/internal form contains the list; `client_message()` returns the **static** category (no list), matching the wire-leak split (`error.rs:74-86`).
- [ ] **Step 2:** Add the variant + `client_message()` arm (static, e.g. `"agent configuration rejected"`). **Step 3:** Run → PASS. **Step 4:** `git commit -m "feat(core): BridgeError::config_invalid (list in logs, static on wire)"`

## Task 7: Wire into the mint closure (validate model, apply via set_config_option, effort, log)

**Files:** Modify `crates/bridge-acp/src/acp_backend.rs` (`configure_session` mint closure); add `AcpConfig.agent_id`.

- [ ] **Step 1:** Add `pub agent_id: String` to `AcpConfig`; thread it at construction (`main.rs:247-261`, `crates/bridge-container/src/lib.rs:249-261`) — pass the registry `AgentEntry.id`. Build (fix call sites).
- [ ] **Step 2:** In the closure, after `session/new`:
```rust
let opts0 = resp.config_options.clone().unwrap_or_default();
// MODEL — validate then apply via set_config_option; M2/M3 loud-on-missing.
let refreshed = if let Some((model_id_cfg, values)) = model_effort::model_values(&opts0) {
    match model_effort::resolve_model(model.as_deref(), &values) {
        Err(e) => return Err(BridgeError::config_invalid(format!("model '{}' is not advertised by agent '{}' (valid: {})", e.want, self.agent_id, e.valid.join(", ")))),
        Ok(model_effort::ModelDecision::Apply(v)) => {
            let r = Self::set_config_option(cx, &id, &model_id_cfg, &v).await
                .map_err(|e| BridgeError::config_invalid(format!("set model '{v}' failed: {e}")))?;
            r.config_options.unwrap_or_else(|| opts0.clone())
        }
        Ok(model_effort::ModelDecision::Default) => opts0.clone(),
    }
} else if model.is_some() {
    return Err(BridgeError::config_invalid(format!("agent '{}' advertised no model option but model='{}' was configured (possible adapter/schema skew)", self.agent_id, model.as_deref().unwrap())));
} else { opts0.clone() };
```
- [ ] **Step 3:** EFFORT — resolve against `refreshed` (warn if a model was applied but `refreshed` lacks the effort option), apply with precise walk-down:
```rust
if let Some(adv) = model_effort::effort_opt(&refreshed) {
    match model_effort::resolve_effort(effort, &adv) {
        model_effort::EffortDecision::Apply{config_id, level} | model_effort::EffortDecision::FellBack{config_id, to: level, ..} => {
            // (log FellBack with from→to before applying)
            Self::apply_effort_walkdown(cx, &id, &config_id, &level, &adv.levels).await; // tries level, then walks down on is_unsupported_effort_error
        }
        model_effort::EffortDecision::Unsupported{from} => tracing::warn!(%from, levels=?adv.levels, "no supported effort level <= requested; leaving default"),
        model_effort::EffortDecision::Skip => {}
    }
} else if effort.is_some() { tracing::warn!("effort configured but agent advertised no thought-level option; skipping"); }
tracing::info!(agent=%self.agent_id, requested_model=?model, requested_effort=?effort, "model_effort_resolved");
```
- [ ] **Step 4:** Add private helpers `set_config_option(cx,id,config_id,value) -> Result<SetSessionConfigOptionResponse,_>` (builds `SetSessionConfigOptionRequest` from the **discovered** id) and `apply_effort_walkdown(...)` (loop: try level; on err, if `is_unsupported_effort_error(code,msg)` drop to next-lower advertised level by `EFFORT_ORDER` and retry; else `warn` raw error and stop). **Remove** `EFFORT_CONFIG_ID`/`effort_value`/`set_effort_request`/`set_model_request` usage; delete now-dead items + their tests (or move assertions into `model_effort`).
- [ ] **Step 5:** `cargo test -p bridge-acp` — fix golden/wire tests referencing the removed builders.
- [ ] **Step 6:** `git commit -m "feat(acp): capability-driven model+effort at mint via set_config_option"`

## Task 8: Config + migration + docs

**Files:** `bin/a2a-bridge/src/config.rs` (agent_id thread-through if not done), `examples/*.toml`, `examples/a2a-bridge.multi-agent.toml`, `README.md`, `docs/onboarding.md`, `bin/a2a-bridge/src/init-readme-template.md`, `main.rs:2106-2118`.

- [ ] **Step 1:** **Remove the kiro `model="auto"` pin** in `multi-agent.toml` (kiro advertises no model option → M2/M3 would hard-fail). Add a migration comment.
- [ ] **Step 2:** Pin advertised model ids in the example review/implement configs; add one alias example (`model="fable"` with a comment "→ claude-fable-5[1m]").
- [ ] **Step 3:** Fix stale docs: README (codex effort values; remove "claude gets no bridge effort"), onboarding, init template + `main.rs` init fragment. Add the effort-level guidance table (spec §11).
- [ ] **Step 4:** `cargo test -p a2a-bridge` (config parse tests). **Step 5:** `git commit -m "docs+config: migrate kiro model pin, fix stale model/effort docs"`

## Task 9: ADR-0029

- [ ] **Step 1:** Write `docs/adr/0029-model-effort-pinning.md` — context (codex-shaped plumbing; claude effort no-op; 0.44.0 restabilized model into config options; two dogfooded spec-reviews incl. Fable-as-reviewer), decision (capability-driven config-option resolution; alias map; precise effort walk-down; hard-error+valid-list on logs/CLI; fallback_model descoped), consequences (migration: kiro pin removed, claude effort now applies, model pins validated). Co-Authored-By trailer.
- [ ] **Step 2:** `git commit -m "docs: ADR-0029 capability-driven model & effort pinning"`

## Task 10: Live gate (claude 0.44.0 + codex)

- [ ] **a** `model="haiku"` → transcript `claude-haiku-4-5-*`.
- [ ] **b** `model="fable"` → alias→`claude-fable-5[1m]` → transcript `claude-fable-5`.
- [ ] **c** `model="bogus"` → mint fails; CLI stderr + log show "not advertised (valid: …)"; A2A wire stays the static category.
- [ ] **d** `model="sonnet" effort="high"` → applies; no `Unknown config option`; `model_effort_resolved` logged.
- [ ] **e** `model="sonnet" effort="xhigh"` → warn "fell back high"; mint ok.
- [ ] **f** codex `model="gpt-5.5" effort="high"` → rollout `gpt-5.5`; effort via discovered `reasoning_effort`.
- [ ] **Step:** record results in the ADR consequences + a memory update.

## Self-review

- **Spec v2.1 coverage:** §5.1 discovery → T4 (grouped/Other); §5.2 model+alias → T3; §5.3 effort+predicate+order → T5; §6 mint order → T7; §8 error surface → T6+T7; §7 files (agent_id, both parsers, migration) → T1/T2/T7/T8; §11 guidance → T8. fallback_model: absent (descoped) ✓.
- **Type consistency:** `ModelDecision`, `ModelNotAdvertised`, `AdvertisedEffort`, `EffortDecision`, `resolve_model`, `resolve_effort`, `model_values`, `effort_opt`, `effort_level_name`, `is_unsupported_effort_error`, `EFFORT_ORDER`, `apply_alias`, `BridgeError::config_invalid`, `AcpConfig.agent_id` — used consistently T3-T8.
- **No `Ord` on `Effort`** (ordering in `EFFORT_ORDER`); both effort parsers via one `FromStr`.
