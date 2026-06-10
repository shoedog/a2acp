# Capability-driven Model & Effort Pinning — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the bridge validate model & effort against what each ACP agent *advertises* at mint, fix the silent claude-effort no-op, hard-error on a bad model pin, and log the resolved model/effort — plus an optional `fallback_model`.

**Architecture:** A new pure module `crates/bridge-acp/src/model_effort.rs` holds total, unit-tested resolvers over the agent's advertised `SessionModelState` + effort `SessionConfigOption`. `AcpBackend`'s mint closure reads the advertised state from the `session/new` response (and the `ConfigOptionUpdate` notification the agent pushes after `set_model`), calls the resolvers, applies via the **discovered** config-id, and logs. `fallback_model` is a spawn-time arg (claude `--fallback-model`) wired in the `SpawnFn`/`acp_program_argv` path.

**Tech Stack:** Rust, `agent-client-protocol =0.12.1` (schema 0.13.2, `unstable_session_model` already enabled), tokio, tracing.

**Key SDK types (all present in 0.12.1, verified):**
- `NewSessionResponse { session_id, modes, models: Option<SessionModelState>, config_options: Option<Vec<SessionConfigOption>> }`
- `SessionModelState { current_model_id: ModelId, available_models: Vec<ModelInfo{model_id, name}> }`
- `SessionConfigOption { id: SessionConfigId, category: Option<SessionConfigOptionCategory{Mode|Model|ThoughtLevel|Other}>, kind: SessionConfigKind::Select(SessionConfigSelect{ current_value: SessionConfigValueId, options: SessionConfigSelectOptions(Vec<SessionConfigSelectOption{value: SessionConfigValueId, name}>) }) }`
- `SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate{ config_options: Vec<SessionConfigOption> })`
- Newtypes wrap `Arc<str>`: `ModelId(pub Arc<str>)`, `SessionConfigId(pub Arc<str>)`, `SessionConfigValueId(pub Arc<str>)` — read via `&*x.0`.
- Existing request builders in `acp_backend.rs`: `set_model_request`, `set_mode_request`, `set_effort_request` (to be replaced by a capability-driven config-option set), `EFFORT_CONFIG_ID`/`effort_value` (to be removed/relocated).

**Probed ground truth (from the spec §3):** claude advertises `default/sonnet/sonnet[1m]/haiku`, effort id `effort`, levels model-dependent (Opus 4.8: low/medium/high/xhigh/max; Sonnet 4.6: low/medium/high/max). codex advertises `gpt-5.5/5.4/5.4-mini/5.3-codex-spark`, effort id `reasoning_effort`, levels low/medium/high/xhigh. Both accept `set_model` for ANY string (no validation) → the bridge must validate. Level order: `low < medium < high < xhigh < max`.

---

## File Structure

- **Create** `crates/bridge-acp/src/model_effort.rs` — pure resolvers + types + unit tests. One responsibility: decide, given advertised capability + desired config, what to apply (or fail).
- **Modify** `crates/bridge-core/src/domain.rs` (or wherever `Effort` lives) — add `Effort::Xhigh`.
- **Modify** `crates/bridge-acp/src/lib.rs` — `mod model_effort;`.
- **Modify** `crates/bridge-acp/src/acp_backend.rs` — mint closure integration; `AcpConfig.fallback_model`; capture `ConfigOptionUpdate`.
- **Modify** `bin/a2a-bridge/src/config.rs` — `parse_effort` accepts `xhigh`; `AgentToml.fallback_model`; thread to `AcpConfig`.
- **Modify** `bin/a2a-bridge/src/main.rs` (+ `crates/bridge-container/src/lib.rs`) — append claude `--fallback-model` in the spawn-arg path.
- **Modify** `examples/a2a-bridge.slicing-*.toml` + `docs/` — effort guidance + fallback_model example.
- **Create** `docs/adr/0029-model-effort-pinning.md`.

---

## Task 1: Add `Effort::Xhigh` variant

**Files:**
- Modify: `crates/bridge-core/src/domain.rs` (the `Effort` enum + any exhaustive `match`)
- Test: same file's `#[cfg(test)]` mod

- [ ] **Step 1: Locate the enum.** Run `grep -rn 'enum Effort' crates/bridge-core/src` to find the definition (variants today: `Minimal, Low, Medium, High, Max`).

- [ ] **Step 2: Write the failing test** (in the domain test mod):

```rust
#[test]
fn effort_xhigh_orders_between_high_and_max() {
    assert!(Effort::High < Effort::Xhigh);
    assert!(Effort::Xhigh < Effort::Max);
}
```

- [ ] **Step 3: Run it, expect FAIL** (`Xhigh` undefined): `cargo test -p bridge-core effort_xhigh`

- [ ] **Step 4: Add the variant** between `High` and `Max` (ordering matters — derive `PartialOrd`/`Ord` already present or add it):

```rust
pub enum Effort { Minimal, Low, Medium, High, Xhigh, Max }
```
Fix any non-exhaustive `match` the compiler flags (e.g. a `Display`/serde mapping) to handle `Xhigh => "xhigh"`.

- [ ] **Step 5: Run, expect PASS.** `cargo test -p bridge-core effort_xhigh`

- [ ] **Step 6: Commit.** `git add -A && git commit -m "feat(core): add Effort::Xhigh tier"`

---

## Task 2: Pure model resolver

**Files:**
- Create: `crates/bridge-acp/src/model_effort.rs`
- Modify: `crates/bridge-acp/src/lib.rs` (add `mod model_effort;`)

- [ ] **Step 1: Create the module skeleton + failing test.** Write `model_effort.rs`:

```rust
//! Pure, capability-driven resolution of model & effort against what an ACP agent
//! advertises at mint. No I/O — the backend does the wire calls; this decides.

/// A model id the agent advertises as selectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedModels {
    pub current: String,
    pub available: Vec<String>, // model_id values
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelDecision {
    /// Leave the agent's default (no `model=` configured).
    Default,
    /// Apply this advertised model id.
    Apply(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ModelNotAdvertised {
    pub want: String,
    pub valid: Vec<String>,
}

/// Resolve a configured `model=` against the advertised list.
/// `None` => Default. Present => Apply. Absent => Err (caller fails the mint).
pub fn resolve_model(
    want: Option<&str>,
    adv: &AdvertisedModels,
) -> Result<ModelDecision, ModelNotAdvertised> {
    match want {
        None => Ok(ModelDecision::Default),
        Some(w) if adv.available.iter().any(|m| m == w) => Ok(ModelDecision::Apply(w.to_string())),
        Some(w) => Err(ModelNotAdvertised { want: w.to_string(), valid: adv.available.clone() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn claude() -> AdvertisedModels {
        AdvertisedModels { current: "default".into(),
            available: ["default","sonnet","sonnet[1m]","haiku"].iter().map(|s| s.to_string()).collect() }
    }
    #[test] fn none_is_default() { assert_eq!(resolve_model(None, &claude()).unwrap(), ModelDecision::Default); }
    #[test] fn advertised_applies() { assert_eq!(resolve_model(Some("haiku"), &claude()).unwrap(), ModelDecision::Apply("haiku".into())); }
    #[test] fn typo_errors_with_valid_list() {
        let e = resolve_model(Some("bogus-zzz"), &claude()).unwrap_err();
        assert_eq!(e.want, "bogus-zzz");
        assert!(e.valid.contains(&"haiku".to_string()));
    }
    #[test] fn cli_alias_not_in_list_errors() {
        // "opus" is not advertised (it's "default"); strict by design.
        assert!(resolve_model(Some("opus"), &claude()).is_err());
    }
}
```
Add `mod model_effort;` to `crates/bridge-acp/src/lib.rs`.

- [ ] **Step 2: Run, expect FAIL→PASS as you fill in.** `cargo test -p bridge-acp model_effort::tests::`

- [ ] **Step 3: Commit.** `git add -A && git commit -m "feat(acp): pure resolve_model (validate against advertised models)"`

---

## Task 3: Pure effort resolver (with highest-≤-requested fallback)

**Files:**
- Modify: `crates/bridge-acp/src/model_effort.rs`

- [ ] **Step 1: Write the failing tests** (append to the module):

```rust
/// The advertised effort option for the ACTIVE model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedEffort {
    pub config_id: String,        // e.g. "effort" (claude) / "reasoning_effort" (codex)
    pub levels: Vec<String>,      // advertised values, e.g. ["low","medium","high","max"]
}

/// Canonical ordering low<medium<high<xhigh<max. Levels outside this list sort last
/// (defensive: unknown vendor level never wins a `<=` comparison).
pub const EFFORT_ORDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];
fn rank(level: &str) -> usize { EFFORT_ORDER.iter().position(|l| *l == level).unwrap_or(usize::MAX) }

#[derive(Debug, PartialEq, Eq)]
pub enum EffortDecision {
    /// No effort configured — leave the model default.
    Skip,
    /// Apply `level` via `config_id`.
    Apply { config_id: String, level: String },
    /// Requested unsupported; applying the highest advertised level <= requested.
    FellBack { config_id: String, from: String, to: String },
    /// Requested unsupported and nothing advertised <= requested; leave default.
    Unsupported { from: String, levels: Vec<String> },
}

/// `want` is the canonical level string the bridge maps an `Effort` tier to
/// (Minimal/Low->"low", Medium->"medium", High->"high", Xhigh->"xhigh", Max->"max").
pub fn resolve_effort(want: Option<&str>, adv: &AdvertisedEffort) -> EffortDecision {
    let want = match want { None => return EffortDecision::Skip, Some(w) => w };
    if adv.levels.iter().any(|l| l == want) {
        return EffortDecision::Apply { config_id: adv.config_id.clone(), level: want.to_string() };
    }
    // highest advertised level whose rank <= want's rank
    let want_rank = rank(want);
    let best = adv.levels.iter()
        .filter(|l| rank(l) <= want_rank)
        .max_by_key(|l| rank(l));
    match best {
        Some(to) => EffortDecision::FellBack { config_id: adv.config_id.clone(), from: want.to_string(), to: to.clone() },
        None => EffortDecision::Unsupported { from: want.to_string(), levels: adv.levels.clone() },
    }
}

#[cfg(test)]
mod effort_tests {
    use super::*;
    fn sonnet() -> AdvertisedEffort { AdvertisedEffort { config_id: "effort".into(), levels: ["low","medium","high","max"].iter().map(|s| s.to_string()).collect() } }
    fn codex() -> AdvertisedEffort { AdvertisedEffort { config_id: "reasoning_effort".into(), levels: ["low","medium","high","xhigh"].iter().map(|s| s.to_string()).collect() } }
    #[test] fn none_skips() { assert_eq!(resolve_effort(None, &sonnet()), EffortDecision::Skip); }
    #[test] fn exact_applies_with_discovered_id() {
        assert_eq!(resolve_effort(Some("high"), &sonnet()), EffortDecision::Apply { config_id: "effort".into(), level: "high".into() });
    }
    #[test] fn sonnet_xhigh_falls_back_to_high() {
        assert_eq!(resolve_effort(Some("xhigh"), &sonnet()), EffortDecision::FellBack { config_id: "effort".into(), from: "xhigh".into(), to: "high".into() });
    }
    #[test] fn sonnet_max_applies() { // sonnet DOES advertise max
        assert_eq!(resolve_effort(Some("max"), &sonnet()), EffortDecision::Apply { config_id: "effort".into(), level: "max".into() });
    }
    #[test] fn codex_max_falls_back_to_xhigh() { // codex has no "max"
        assert_eq!(resolve_effort(Some("max"), &codex()), EffortDecision::FellBack { config_id: "reasoning_effort".into(), from: "max".into(), to: "xhigh".into() });
    }
}
```

- [ ] **Step 2: Run, expect PASS.** `cargo test -p bridge-acp model_effort`

- [ ] **Step 3: Add the `Effort`→canonical-level mapping helper** (used by the backend; keep it in this module for testability):

```rust
use bridge_core::domain::Effort;
pub fn effort_level_name(e: Effort) -> &'static str {
    match e {
        Effort::Minimal | Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}
#[test] fn max_maps_to_max_not_xhigh() { assert_eq!(super::effort_level_name(Effort::Max), "max"); }
```
(Put the test inside an existing `#[cfg(test)] mod`.)

- [ ] **Step 4: Commit.** `git add -A && git commit -m "feat(acp): pure resolve_effort with highest-<=-requested fallback"`

---

## Task 4: Capability extraction from advertised config options

**Files:**
- Modify: `crates/bridge-acp/src/model_effort.rs`

Extract `AdvertisedModels`/`AdvertisedEffort` from the SDK types so the backend stays thin. These take borrowed SDK structs; test with hand-built SDK values.

- [ ] **Step 1: Write the failing test + functions:**

```rust
use agent_client_protocol::{SessionModelState, SessionConfigOption, SessionConfigOptionCategory, SessionConfigKind};

pub fn models_from(state: &SessionModelState) -> AdvertisedModels {
    AdvertisedModels {
        current: (*state.current_model_id.0).to_string(),
        available: state.available_models.iter().map(|m| (*m.model_id.0).to_string()).collect(),
    }
}

/// Find the effort option (category == ThoughtLevel) among advertised config options.
pub fn effort_from(opts: &[SessionConfigOption]) -> Option<AdvertisedEffort> {
    opts.iter().find(|o| matches!(o.category, Some(SessionConfigOptionCategory::ThoughtLevel)))
        .and_then(|o| match &o.kind {
            SessionConfigKind::Select(sel) => Some(AdvertisedEffort {
                config_id: (*o.id.0).to_string(),
                // exclude the synthetic "default" entry; keep concrete levels
                levels: sel.options.iter().map(|v| (*v.value.0).to_string()).filter(|v| v != "default").collect(),
            }),
            _ => None,
        })
}
```
(Confirm `SessionConfigSelectOptions` iterates to `&SessionConfigSelectOption`; if it is a newtype wrapper, use `sel.options.0.iter()` — check the type at impl time.)

```rust
#[test]
fn effort_from_picks_thoughtlevel_and_strips_default() {
    // build a SessionConfigOption with category ThoughtLevel, options [default, low, medium, high, max]
    // assert config_id + levels == ["low","medium","high","max"]
}
```

- [ ] **Step 2: Run, expect PASS.** `cargo test -p bridge-acp model_effort`

- [ ] **Step 3: Commit.** `git add -A && git commit -m "feat(acp): extract advertised models/effort from SDK config options"`

---

## Task 5: Capture `ConfigOptionUpdate` notifications per session

**Files:**
- Modify: `crates/bridge-acp/src/acp_backend.rs`

The agent pushes a `SessionUpdate::ConfigOptionUpdate` after `set_model` (re-scoping the effort levels to the new model). The mint must read the **post-set_model** options. Store the latest per session in shared state the `Client` sessionUpdate handler writes.

- [ ] **Step 1: Locate the session-update handler.** `grep -n 'SessionUpdate' crates/bridge-acp/src/acp_backend.rs` — find where the `Client` impl matches update variants.

- [ ] **Step 2: Add shared latest-config-options state.** On the struct that handles updates (the `Client` impl / connection state), add:

```rust
latest_config_options: Arc<Mutex<HashMap<SessionId, Vec<agent_client_protocol::SessionConfigOption>>>>,
```
Initialize it in the constructor.

- [ ] **Step 3: Record on `ConfigOptionUpdate`.** In the update match, add:

```rust
SessionUpdate::ConfigOptionUpdate(u) => {
    self.latest_config_options.lock().await.insert(session_id.clone(), u.config_options.clone());
}
```
(Use the existing locking idiom — match sync/async Mutex already in the file.)

- [ ] **Step 4: Build, expect PASS.** `cargo test -p bridge-acp` (no behavior change yet; just storage).

- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat(acp): record latest config_options per session from ConfigOptionUpdate"`

---

## Task 6: Wire resolvers into the mint closure (model fail-loud + effort via discovered id + log)

**Files:**
- Modify: `crates/bridge-acp/src/acp_backend.rs` (the `configure_session` mint closure, ~lines 1000-1075)

- [ ] **Step 1: Read advertised models + initial config options from the `session/new` response.** After `let resp = cx.send_request(req)...?;` and `let id = resp.session_id;`, capture:

```rust
let adv_models = resp.models.as_ref().map(model_effort::models_from);
let initial_opts = resp.config_options.clone().unwrap_or_default();
```

- [ ] **Step 2: Replace the model block with validation.** Where `set_model` is sent today (the `if let Some(model) = model.as_deref()` block), change to:

```rust
if let Some(adv) = &adv_models {
    match model_effort::resolve_model(model.as_deref(), adv) {
        Err(e) => {
            tracing::error!(want = %e.want, valid = ?e.valid, "model not advertised by agent; failing session mint");
            return Err(BridgeError::agent_crashed(format!(
                "configured model '{}' is not advertised by this agent (valid: {})", e.want, e.valid.join(", "))));
        }
        Ok(model_effort::ModelDecision::Apply(m)) => {
            // best-effort send is fine now that we've validated; on transport error, fail mint
            cx.send_request(Self::set_model_request(id.clone(), m)).block_task().await
                .map_err(|e| BridgeError::agent_crashed(format!("session/set_model failed: {e}")))?;
        }
        Ok(model_effort::ModelDecision::Default) => {}
    }
} else if model.is_some() {
    tracing::warn!("model configured but agent advertises no models; leaving default");
}
```
(Note: validation is the loud guard; we still `?` on transport failure. If `adv_models` is `None` but a model was requested, warn — can't validate.)

- [ ] **Step 3: Resolve effort against the POST-set_model options.** Replace the old `apply_effort`/`reasoning_effort` block with:

```rust
// Prefer the latest pushed options (post set_model); fall back to the initial ones.
let opts = {
    let map = self.latest_config_options.lock().await;
    map.get(&id).cloned().unwrap_or(initial_opts)
};
if let Some(adv_eff) = model_effort::effort_from(&opts) {
    let want = effort.map(model_effort::effort_level_name);
    match model_effort::resolve_effort(want, &adv_eff) {
        model_effort::EffortDecision::Apply { config_id, level } => {
            Self::set_config_option(cx, &id, &config_id, &level).await;
        }
        model_effort::EffortDecision::FellBack { config_id, from, to } => {
            tracing::warn!(%from, %to, "requested effort unsupported by active model; using highest supported <= requested");
            Self::set_config_option(cx, &id, &config_id, &to).await;
        }
        model_effort::EffortDecision::Unsupported { from, levels } => {
            tracing::warn!(%from, ?levels, "requested effort has no supported level <= it; leaving model default");
        }
        model_effort::EffortDecision::Skip => {}
    }
} else if effort.is_some() {
    tracing::warn!("effort configured but agent advertises no thought-level option; skipping");
}
```
Add a small private async helper `set_config_option(cx, id, config_id, value)` that builds a `SetSessionConfigOptionRequest` from the **discovered** `config_id` (NOT the old constant) and sends it best-effort (log on error). Remove `EFFORT_CONFIG_ID`/`effort_value`/`set_effort_request`/`apply_effort` (now subsumed) — or keep `effort_value` only if a test references it; otherwise delete to avoid dead code.

- [ ] **Step 4: Emit the resolved log line** after both applied:

```rust
tracing::info!(
    agent = %self.agent_id_for_log(),
    model = %adv_models.as_ref().map(|m| m.current.as_str()).unwrap_or("<default>"),
    requested_model = ?model, requested_effort = ?effort,
    "model_effort_resolved");
```
(The post-apply `current` from `adv_models` reflects the default; for the actually-set model, log `model.as_deref()`-or-current. Choose the most informative available — the requested + the advertised-current.)

- [ ] **Step 5: Build + run.** `cargo test -p bridge-acp` — fix the unit/golden tests that referenced the removed `EFFORT_CONFIG_ID`/`set_effort_request` (update them to the new `set_config_option` path or move those assertions into `model_effort` tests).

- [ ] **Step 6: Commit.** `git add -A && git commit -m "feat(acp): capability-driven model validation + effort at mint (fixes claude effort no-op)"`

---

## Task 7: `fallback_model` config surface

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (`AgentToml`, `AcpConfig` build), `crates/bridge-acp/src/acp_backend.rs` (`AcpConfig.fallback_model`)

- [ ] **Step 1: Add the field.** `AgentToml.fallback_model: Option<String>` and `AcpConfig.fallback_model: Option<String>`; thread it through where `AcpConfig` is built from `AgentToml`.

- [ ] **Step 2: Failing test** (config.rs test mod): a `[[agents]]` with `cmd="claude-agent-acp"` + `fallback_model="sonnet"` parses with `fallback_model == Some("sonnet")`; an agent without it parses `None`.

- [ ] **Step 3: Implement to pass; run** `cargo test -p a2a-bridge config`.

- [ ] **Step 4: `parse_effort` accepts `xhigh`.** Add the `"xhigh" => Effort::Xhigh` arm + a test `parse_effort("xhigh") == Effort::Xhigh`.

- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat(config): fallback_model field + parse_effort xhigh"`

---

## Task 8: Wire claude `--fallback-model` into the spawn args

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the `acp_program_argv`/`SpawnFn` site), `crates/bridge-container/src/lib.rs` (the container spawn arg path)

- [ ] **Step 1: Verify the adapter forwards `--fallback-model`.** Run, from a scratch dir:
```bash
claude-agent-acp --fallback-model sonnet </dev/null   # should not error on the flag (adapter forwards argv to claude)
```
Confirm via the Claude CLI reference that `--fallback-model <model>` is the right flag. If `claude-agent-acp` does NOT forward it, fall back to setting it via the documented env (`ANTHROPIC_*`)/spawn and adjust this task; record the finding in the ADR.

- [ ] **Step 2: Append the arg when set.** In `acp_program_argv` (the function that assembles the ACP program argv per agent), when the agent `cmd` basename is `claude-agent-acp` and `fallback_model` is `Some(m)`, append `["--fallback-model", m]`. For a codex agent with `fallback_model` set, return a config error at load ("fallback_model is claude-only; codex uses …") unless Step-1 finds a codex equivalent.

- [ ] **Step 3: Validate the `fallback_model` chain against advertised models at mint** (reuse `resolve_model`): `--fallback-model` accepts a **comma-separated chain** (verified: claude docs; up to 3, `"default"` expands). In the mint closure, if `fallback_model` is set and `adv_models` is present, split on `,`, and for each element (skip the literal `"default"`, which always resolves) run `resolve_model(Some(elem), adv)` and **fail the mint** on `Err`. This catches a typo'd fallback element. (codex: `fallback_model` is rejected at config load per Step 2 — no codex equivalent exists.)

- [ ] **Step 4: Test.** Unit-test `acp_program_argv` includes `--fallback-model sonnet` for a claude agent and omits it otherwise. Run `cargo test -p a2a-bridge`.

- [ ] **Step 5: Commit.** `git add -A && git commit -m "feat: wire claude --fallback-model spawn arg + validate against advertised models"`

---

## Task 9: Examples + docs

**Files:**
- Modify: `examples/a2a-bridge.slicing-review.toml`, `examples/a2a-bridge.slicing-implement.toml`
- Modify/Create: `docs/onboarding.md` (or the config-reference doc) — model/effort/fallback_model section + the effort-level guidance table

- [ ] **Step 1: Pin models explicitly in the examples** with comments, e.g. review reviewers `model="sonnet"`, implementor `model="gpt-5.5" effort="high"` (use advertised ids), and one `fallback_model="sonnet"` example on a claude agent.

- [ ] **Step 2: Add the effort-level guidance table** (spec §11, verbatim) to the docs so config authors know low/medium/high/xhigh/max semantics + the model-dependent levels.

- [ ] **Step 3: Commit.** `git add -A && git commit -m "docs: model/effort/fallback_model config guidance + pinned example models"`

---

## Task 10: ADR-0029

**Files:**
- Create: `docs/adr/0029-model-effort-pinning.md`

- [ ] **Step 1: Write the ADR** — Context (codex-shaped plumbing; claude effort silent no-op; unvalidated model pins; the live probes), Decision (capability-driven mint-time resolution; discover by `category` Model/ThoughtLevel; hard-error bad model; highest-≤-requested effort fallback; `fallback_model` spawn arg), Alternatives (hardcode per-agent table — rejected, brittle; stringly-typed effort — rejected, kept typed `Effort`+`Xhigh`), Consequences (the SDK 0.12.1 already exposes the types; CLI shorthand like `opus` no longer accepted — use advertised ids). End with the `Co-Authored-By` trailer.

- [ ] **Step 2: Commit.** `git add -A && git commit -m "docs: ADR-0029 capability-driven model & effort pinning"`

---

## Task 11: Live gate (real claude + codex)

**Files:** none (validation)

- [ ] **Step 1: claude model pin serves the pinned model.** A minimal config with a claude agent `model="haiku"`; drive one prompt through the bridge; confirm via the transcript `~/.claude/projects/<encoded-cwd>/*.jsonl` that `"model":"claude-haiku-4-5-*"` served. Expected PASS.

- [ ] **Step 2: bad model fails loudly.** Set `model="bogus-zzz"` → the run fails the mint with the "not advertised (valid: …)" error. Expected non-zero exit + clear message.

- [ ] **Step 3: claude effort now applies.** Set `model="sonnet" effort="high"` → no `Unknown config option` warning; the mint log shows `model_effort_resolved … effort` applied; (optional) confirm via transcript/usage the effort took.

- [ ] **Step 4: effort fallback warns.** `model="sonnet" effort="xhigh"` → warn "using highest supported <= requested" and applies `high`; mint succeeds.

- [ ] **Step 5: codex unchanged.** A codex agent `model="gpt-5.4-mini" effort="high"` → rollout serves `gpt-5.4-mini`; effort applied via discovered `reasoning_effort`. Expected PASS.

- [ ] **Step 6: fallback_model accepted + present.** claude agent `model="sonnet" fallback_model="haiku"` mints; the spawned argv includes `--fallback-model haiku` (check process args / adapter behavior).

- [ ] **Step 7: Record results** in the ADR's consequences / a memory update.

---

## Self-Review notes (filled by author)

- **Spec coverage:** §2 goals → model validation (T2,T6), claude effort fix (T3-T6), effort fallback+warn (T3,T6), observability log (T6), fallback_model (T7,T8). §5 surfaces → config (T7), spawn arg (T8). §10 SDK risk → retired (types confirmed present; no Task-0 bump). §11 guidance → T9.
- **Refinement vs spec §5.3:** kept the typed `Effort` enum + added `Xhigh` (instead of a raw string) — less ripple through `SessionSpec`/`effective_config`, and the highest-≤-requested fallback subsumes both vendor vocabularies (codex `Max`→`xhigh`, claude `Max`→`max`). Spec §5.3 updated to match.
- **Type consistency:** `AdvertisedModels`, `AdvertisedEffort`, `ModelDecision`, `EffortDecision`, `resolve_model`, `resolve_effort`, `effort_from`, `models_from`, `effort_level_name`, `set_config_option` used consistently T2-T8.
- **Order:** mint applies mode → model (validate, fail-loud) → re-read options → effort (discovered id, fallback) → log.
```
