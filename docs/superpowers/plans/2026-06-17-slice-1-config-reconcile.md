# Slice 1 — Config reconcile + capabilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Upgrade warm continuation from Slice 0's "reject on any config mismatch" to "reconcile model/effort
on the live session when the agent advertises it; cwd→reject; mode→reseed-required" + record agent lifecycle
capabilities.

**Architecture:** A new callable-on-warm `AgentBackend::reconcile_config` (ACP lifts the mint-time
model/effort application into a shared `apply_model_effort` helper + caches the `session/new` config surface
on `AgentSession`); `SessionManager::checkout_turn` routes a full mismatch SET (not first-field) — frozen
fields reject, mode → reseed-required, `{model,effort}` → reconcile (claim handle → drop lock → reconcile →
re-acquire → advance fingerprint). Bridge-owned `AgentSessionCaps` recorded on the handle + surfaced in
`session/status`. New typed `BridgeError::ConfigReseedRequired`.

**Tech Stack:** Rust workspace; bridge-core, bridge-acp, bridge-a2a-inbound; the `agent-client-protocol`
0.12.1 SDK (features `unstable_session_usage`+`unstable_session_model` only — `delete` cap is NOT compiled).

**Spec:** `docs/superpowers/specs/2026-06-17-slice-1-config-reconcile.md` (v2, dual-reviewed). **Slicing
authority:** `2026-06-17-orchestration-slicing.md` (Slice 1). Built on Slice 0 (shipped, main).

**Implementor:** codex gpt-5.5/high host (`run-workflow slice0-impl --session-cwd <repo>`), test+impl together,
controller verifies+commits (the `_dyld_start` flake). **Gate enum additions with `cargo test --workspace
--no-run` (`--all-targets`)** — `cargo build` misses test-target match exhaustiveness (Slice-0 lesson).

**Grounded seams (verbatim-verified):**
- `AgentSession` struct `acp_backend.rs:266-310` (no config-surface cache today).
- mint closure `acp_backend.rs:1184-1290`: `session/new`→`opts0`(`resp.config_options`)/`models0`(`resp.models`)
  → `set_mode` → `configure_model_option`→`(refreshed_opts,current_model)` → effort walk-down (`effort_opt` +
  `apply_effort_walkdown`) — surface discarded at closure end.
- `configure_model_option` `:524-584` `(opts0,models0,model)->Result<(Vec<SessionConfigOption>,String)>`;
  `apply_effort_walkdown` `:622-710` (async, infallible, `->EffortDecision`); `set_config_option` `:480-495`
  (returns refreshed `Vec<SessionConfigOption>`); `set_model` `:605-620` (kiro).
- helpers in `crates/bridge-acp/src/model_effort.rs`: `EffortDecision`, `ModelDecision`, `AdvertisedEffort`,
  `effort_opt`, `resolve_effort`, `model_values`, `model_state_values`, `EFFORT_ORDER`.
- `agent_capabilities()->Option<&AgentCapabilities>` `:1058-1068`. ACP `AgentCapabilities{load_session: bool,
  session_capabilities: SessionCapabilities{list: Option, resume: Option, close: Option}}` (delete cfg-gated,
  NOT compiled).
- `checkout_turn` resume branch `session_manager.rs:90-115` (first_mismatch→ConfigMismatch);
  `SessionStatusInfo` `:162-171`; `WarmHandle` `:23-36`.
- `SessionSpecFingerprint::first_mismatch` `session_fingerprint.rs:17-34`.
- `session_status` JSON `server.rs:~2842`.

---

## v2 — dual plan-review fixes folded (codex-xhigh + Opus, both `fix-then-execute`)

All findings investigated against the code and confirmed accurate. These resolutions amend the tasks below;
where they conflict, THESE win. codex (correctness lead) found the deeper concurrency/semantics issues.

- **PF-1 (BLOCKER, codex B1 + Opus M1) — helper contract preserves mint, serves warm.** `apply_model_effort`
  must NOT collapse mint's detailed errors into a fieldless outcome. Signature:
  `apply_model_effort(cx, agent_session_id, agent_id, surface, model, effort, purpose: ApplyPurpose)
  -> Result<(ConfigSurface, String /*current_model*/), ApplyConfigError>` where
  `enum ApplyPurpose { Mint, Warm }` and `enum ApplyConfigError { NotAdvertised(BridgeError),
  Rejected(BridgeError) }` (carries the NATIVE error so the message is preserved). **Mint caller:**
  `.map_err(|e| match e { NotAdvertised(b) | Rejected(b) => b })?` → re-raises the exact `config_invalid`/
  `agent_crashed` (mint byte-identical; the "valid models: …" detail intact). **Warm caller
  (`reconcile_config`):** maps `NotAdvertised(_) → ReconcileOutcome::NotAdvertised`, `Rejected(b) →
  {log b; ReconcileOutcome::Rejected}`. **Effort nuance:** at `Mint`, effort-with-no-surface stays Skip
  (non-fatal, today's behavior); at `Warm`, a *requested* effort with **no effort surface** → `Err(
  NotAdvertised)` (NOT Applied). The `purpose` arg drives this one divergence; everything else is identical.
- **PF-2 (BLOCKER, codex B2) — stale-handle race across the dropped lock.** `release()` does NOT guard on
  `Running`, so during the reconcile await it can remove the claimed handle, and a fresh checkout can re-mint
  a new handle under the same `contextId`; the old reconcile must not mutate/return the new one. **Fix:**
  before `drop(tab)` capture `let claimed_id = h.id.clone();` (and `backend_session`, `generation`). After
  re-acquire: `match tab.get_mut(ctx) { Some(h) if h.id == claimed_id && h.state == SessionState::Running =>
  { ...advance/return... } _ => return Err(BridgeError::SessionExpired) }` — never mutate a non-matching or
  released handle. **Requires `WarmHandle.id` be comparable** (it's a `SessionHandleId`, derives `PartialEq`
  via the macro). Add a unit test: release + fresh checkout while a fake `reconcile_config` is blocked → the
  in-flight reconcile returns `SessionExpired`, does not corrupt the new handle.
- **PF-3 (MAJOR, codex M1) — unminted reconcile.** `reconcile_config`: if `entry.agent_id.get().is_none()`
  (not yet minted) → `self.configure_session(session, spec).await?` THEN `ensure_session` (mints with the new
  spec) → return `Applied`. If already minted → do NOT `configure_session` (avoid the `minted_cwd` guard);
  `ensure_session` returns the existing id, then `apply_model_effort`. (In the warm-continue flow it's always
  already-minted, but handle both.)
- **PF-4 (MAJOR, codex M2) — serialize reconcile under `turn_lock`.** `reconcile_config` must hold the
  per-session ACP turn boundary: `let _g = Arc::clone(&entry.turn_lock).lock_owned().await;` around
  `apply_model_effort` (after any PF-3 pre-mint stash). Matches how `prompt` guards the live session
  (`acp_backend.rs:1577`). No deadlock (released before return; the subsequent prompt re-acquires).
- **PF-5 (MAJOR, codex M3) — keep the cache fresh.** `set_config_option` returns refreshed opts but
  `apply_effort_walkdown` discards them. Change the effort helper to return the LAST successful refreshed
  `Vec<SessionConfigOption>` (e.g. `-> (EffortDecision, Option<Vec<SessionConfigOption>>)`); `apply_model_effort`
  folds it into the returned `ConfigSurface.opts` (so a 2nd warm effort reconcile reads fresh values). For the
  kiro `set_model` path, update the cached `models.current_model_id` (or store only available model values if
  current is intentionally unused — document which). Add a "high → low" warm-effort test proving TWO
  `set_config_option` RPCs fire AND the cache updates between them.
- **PF-6 (MAJOR, codex M4) — clearing an override must not silently lie.** `model:None`/`effort:None` mean
  "leave as-is" in the existing helpers, so a `Some(x) → None` warm delta would return `Applied` while the
  live session keeps the old value. **Fix (in the SessionManager routing, Task 6):** for a `{model,effort}`
  delta where the NEW effective value is `None` (override cleared, no entry default), return
  `ConfigReseedRequired{field}` — do NOT reconcile a clear. (Only `Some→Some'` changes reconcile.) Add
  SessionManager tests for clearing model + clearing effort.
- **PF-7 (MINOR, both) — import path.** `bridge-core` does not re-export `orch` types at the crate root. Use
  `use bridge_core::orch::{ReconcileOutcome, AgentSessionCaps};` in `session_manager.rs` (Task 6) and
  `acp_backend.rs` (Task 5) — NOT `crate::ReconcileOutcome`.
- **PF-8 (process) — gate enum additions with `--all-targets`.** Every task that adds a variant/type uses
  `cargo test --workspace --no-run` (not `cargo build`) before commit. The SessionManager caps unit test
  needs a fake-backend `capabilities()` OVERRIDE to prove non-default recording (default-all-false only
  covers JSON shape). The ACP `capabilities()` mapping test uses the `spawn_fake_agent` seam
  (`acp_backend.rs:~1880`) with a non-default `AgentCapabilities` (the `Recorder` advertises defaults) —
  mirror `connect_runs_initialize_and_captures_agent_capabilities` (`:2018`).

### v3 — targeted re-review fix (codex-xhigh): reconcile is "apply-or-expire" (transactional by discard)

The re-review found PF-1/2/5 insufficient on ATOMICITY: `apply_model_effort` applies model then effort
(`acp_backend.rs:1225`), so a partial apply (model ok, effort fails/falls-back) leaves the live session in a
state the bridge's fingerprint doesn't reflect — and the infallible `apply_effort_walkdown` hides effort
failures. Because the live ACP session can't be cheaply rolled back, the fix is **discard-on-doubt**:

- **PF-9 (BLOCKER) — `reconcile_config`/`apply_model_effort` returns `Applied` ONLY on an EXACT full apply.**
  Warm semantics: every requested-AND-changed field must apply to EXACTLY the requested value. Make the
  effort helper report exact-apply (it's infallible today): change `apply_effort_walkdown` to return
  `(EffortDecision, Option<Vec<SessionConfigOption>> /*refreshed*/)` and have `apply_model_effort` (purpose
  `Warm`) treat **no-effort-surface OR Unsupported OR FellBack OR Skip-of-a-requested-change** as
  `Err(ApplyConfigError::NotAdvertised)`, and an effort RPC **rejection** as `Err(Rejected)`. At purpose
  `Mint`, keep today's NON-fatal fallback (FellBack/Skip are fine — mint behavior byte-identical). Preserve
  the `resolved_log_line` logging (log inside the helper, or return the `EffortDecision` for the caller).
- **PF-10 (BLOCKER) — SessionManager EXPIRES the handle on any non-clean reconcile.** In the Task-6 reconcile
  branch, after re-acquiring the lock: on `Ok(Applied)` AND the PF-2 identity revalidation passing (same
  `claimed_id`, still `Running`) → advance fingerprint + proceed. On **ANYTHING else** — `NotAdvertised` /
  `Rejected` / transport `Err`, OR revalidation failing (handle released / id-mismatch / no longer `Running`
  because `cancel()` flipped it) — **EXPIRE the handle**: remove it from `by_context`, `backend.release_session
  (&backend_session).await`, drop the lease; return `ConfigReseedRequired{field}` (config cases) or
  `SessionExpired` (concurrent-change/identity cases). **Never return a potentially-dirty handle to `Idle`
  with a stale fingerprint.** This makes reconcile transactional-by-discard: the warm session is either
  EXACTLY the requested config, or gone (the next continue cold-remints correctly). No rollback needed.
- **Tests (add):** (a) model applies then effort fails → handle expired, next `checkout_turn` → fresh mint
  (cold); (b) `cancel()` during a blocked fake `reconcile_config` → handle expired, no dirty reuse; (c) a
  warm effort that can only FellBack → `ConfigReseedRequired` + handle expired (not a lying `Applied`).

> **Net effect on tasks:** T1 unchanged. T2 unchanged. T3 unchanged (defaults fine). **T4** grows: the helper
> returns `Result<(ConfigSurface,String), ApplyConfigError>` + `ApplyPurpose`, the effort helper returns
> refreshed opts (PF-1/PF-5). **T5** grows: PF-3 (mint-if-absent) + PF-4 (turn_lock) + map `ApplyConfigError`
> → `ReconcileOutcome` + cache-fresh on Applied (PF-5) + import (PF-7) + caps test via `spawn_fake_agent`
> (PF-8). **T6** grows: PF-2 (identity revalidation) + PF-6 (clearing→reseed) + import (PF-7). T7 unchanged.
> T8 uses `--all-targets` gate (PF-8).

---

## Task 1: `ReconcileOutcome` + `AgentSessionCaps` + `BridgeError::ConfigReseedRequired`

**Files:** `crates/bridge-core/src/orch.rs` (add the two types); `crates/bridge-core/src/error.rs` (variant +
disposition + test).

- [ ] **Step 1: Write failing tests.** In `orch.rs` tests add ser/de + default checks for `AgentSessionCaps`;
  in `error.rs` add to a test module:

```rust
// error.rs test
#[test]
fn config_reseed_required_rejects_request() {
    assert_eq!(BridgeError::ConfigReseedRequired { field: "model" }.disposition(), A2aDisposition::RejectRequest);
    assert!(BridgeError::ConfigReseedRequired { field: "mode" }.client_message().contains("mode"));
}
```
```rust
// orch.rs test
#[test]
fn agent_session_caps_default_is_all_false() {
    let c = AgentSessionCaps::default();
    assert!(!c.load_session && !c.resume && !c.close && !c.list && !c.delete);
}
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-core --lib config_reseed_required_rejects && cargo test -p bridge-core --lib agent_session_caps_default` (types/variant undefined).

- [ ] **Step 3: Add the types** to `orch.rs`:

```rust
/// Outcome of reconciling model/effort on a LIVE warm session (Slice 1). Fieldless —
/// the backend LOGS any rejection reason internally (no wire leak; reason not surfaced).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileOutcome { Applied, NotAdvertised, Rejected }

/// Bridge-owned agent SESSION-LIFECYCLE capabilities (distinct from `catalog::AgentCaps`,
/// which is model-catalog data). Sourced from initialize-time ACP `AgentCapabilities`.
/// `delete` is behind the SDK `unstable_session_delete` feature (NOT enabled) → always false in Slice 1.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentSessionCaps {
    pub load_session: bool,
    pub resume: bool,
    pub close: bool,
    pub list: bool,
    pub delete: bool,
}
```

- [ ] **Step 4: Add the error variant** in `error.rs` after `ConfigMismatch`:

```rust
    #[error("config reseed required: {field}")]
    ConfigReseedRequired { field: &'static str },
```
and add it to the `RejectRequest` arm of `disposition()` (alongside `ConfigMismatch`).

- [ ] **Step 5: Run + build** — `cargo test -p bridge-core --lib config_reseed agent_session_caps && cargo build --workspace`.
- [ ] **Step 6: Commit** — `feat(core): ReconcileOutcome + AgentSessionCaps + BridgeError::ConfigReseedRequired`

---

## Task 2: `SessionSpecFingerprint::diff()` (full mismatch set)

**Files:** `crates/bridge-core/src/session_fingerprint.rs`.

- [ ] **Step 1: Write failing test:**

```rust
#[test]
fn diff_returns_all_mismatched_fields() {
    let a = fp_full("codex", "m1", Effort::Low, "auto", Some("/w"));
    let b = fp_full("codex", "m2", Effort::High, "auto", Some("/x"));  // model+effort+cwd differ
    let d = a.diff(&b);
    assert!(d.contains(&"model") && d.contains(&"effort") && d.contains(&"cwd"));
    assert!(!d.contains(&"mode") && !d.contains(&"agent"));
    assert!(a.diff(&a).is_empty());
}
```
(Add an `fp_full(agent, model, effort, mode, cwd)` test helper.)

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-core --lib diff_returns_all_mismatched`.

- [ ] **Step 3: Implement `diff`** (keep `first_mismatch` for existing callers):

```rust
    /// ALL differing fields (order-independent). Slice 1 routes on the full set so a
    /// multi-field delta (e.g. model+cwd) is never partially reconciled.
    pub fn diff(&self, other: &SessionSpecFingerprint) -> Vec<&'static str> {
        let mut d = Vec::new();
        if self.agent != other.agent { d.push("agent"); }
        if self.config.model != other.config.model { d.push("model"); }
        if self.config.effort != other.config.effort { d.push("effort"); }
        if self.config.mode != other.config.mode { d.push("mode"); }
        if self.cwd != other.cwd { d.push("cwd"); }
        d
    }
```

- [ ] **Step 4: Run** — `cargo test -p bridge-core --lib diff_returns_all_mismatched` → PASS.
- [ ] **Step 5: Commit** — `feat(core): SessionSpecFingerprint::diff (full mismatch set for Slice-1 routing)`

---

## Task 3: `AgentBackend::reconcile_config` + `capabilities()` trait methods

**Files:** `crates/bridge-core/src/ports.rs` (trait + object-safety test).

- [ ] **Step 1: Extend the object-safety test** (`agentbackend_defaults_are_noops_and_object_safe`): after the
  `release_session` call add:

```rust
        let _ = f.reconcile_config(&crate::ids::SessionId::parse("s").unwrap(),
            &crate::domain::SessionSpec::from_config(Default::default())).await;
        let _ = f.capabilities();
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-core --lib agentbackend_defaults_are_noops`.

- [ ] **Step 3: Add the trait methods** (after `release_session`):

```rust
    /// Reconcile model/effort on a LIVE warm session (Slice 1). Default: NotAdvertised
    /// (non-ACP/non-process backends can't reconcile a live session). cwd/mode are NOT
    /// reconciled here (the caller routes those). [Slice 1]
    async fn reconcile_config(
        &self, _session: &SessionId, _spec: &crate::domain::SessionSpec,
    ) -> Result<crate::orch::ReconcileOutcome, BridgeError> {
        Ok(crate::orch::ReconcileOutcome::NotAdvertised)
    }
    /// Agent session-lifecycle capabilities (initialize-time). Default: empty. [Slice 1]
    fn capabilities(&self) -> crate::orch::AgentSessionCaps { crate::orch::AgentSessionCaps::default() }
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-core --lib agentbackend_defaults && cargo build --workspace`.
- [ ] **Step 5: Commit** — `feat(core): AgentBackend::{reconcile_config,capabilities} trait methods (defaults)`

---

## Task 4: ACP — cache the config surface + extract `apply_model_effort` (the lift)

**Files:** `crates/bridge-acp/src/acp_backend.rs`. This refactors the mint closure WITHOUT changing mint
behavior. The shared helper is the keystone for Task 5.

- [ ] **Step 1: Add a config-surface cache to `AgentSession`** (`acp_backend.rs:266`):

```rust
    /// The advertised config surface from `session/new` (+ refreshed by set_config_option),
    /// cached so a warm `reconcile_config` can re-apply model/effort without re-minting.
    /// Set once at mint; updated under the turn_lock on a warm re-apply. [Slice 1]
    config_surface: StdMutex<Option<ConfigSurface>>,
```
with a small struct near `AgentSession`:
```rust
#[derive(Clone, Default)]
struct ConfigSurface {
    opts: Vec<SessionConfigOption>,
    models: Option<SessionModelState>,
}
```
and init `config_surface: StdMutex::new(None)` in `AgentSession::new()`.

- [ ] **Step 2: Extract `apply_model_effort`** — a method that takes the surface + spec and applies model
  then effort, returning the outcome + refreshed surface. Move the closure's model+effort block (acp_backend
  `:1225-1288`, the `configure_model_option` call + the effort `match`) into:

```rust
    /// Apply model + effort against an advertised surface on a live agent session.
    /// Returns the reconcile outcome + the refreshed surface. Pure of session/new —
    /// callable at mint (with the freshly-minted surface) AND warm (with the cached surface).
    async fn apply_model_effort(
        cx: &ConnectionTo<Agent>,
        agent_session_id: &AgentSessionId,
        agent_id: &str,
        surface: &ConfigSurface,
        model: Option<&str>,
        effort: Option<Effort>,
    ) -> Result<(ReconcileOutcome, ConfigSurface, String), BridgeError> {
        // model: reuse configure_model_option (returns (refreshed_opts, current_model)).
        // Map: config_invalid (unadvertised) -> NotAdvertised; rejected -> Rejected (log reason);
        //      applied/default -> Applied. (At MINT the caller maps NotAdvertised->ConfigInvalid to
        //      preserve today's hard-fail; at WARM the caller surfaces NotAdvertised directly.)
        // effort: reuse effort_opt + resolve_effort + apply_effort_walkdown over refreshed opts.
        // Return (outcome, ConfigSurface{opts: refreshed, models: surface.models.clone()}, current_model).
    }
```
**Preserve mint behavior:** the mint closure (Step 3) keeps its current HARD-fail semantics by treating a
`NotAdvertised`/`Rejected` from the model step as the existing `config_invalid`/`agent_crashed` error (mint
must not silently proceed). Keep `set_mode` in the closure (NOT in `apply_model_effort` — mode is mint-only +
the caller's reseed concern). The effort walk-down's fallback semantics are unchanged (it's the same
`apply_effort_walkdown`). Implement `apply_model_effort` to return `NotAdvertised` only when NEITHER model
surface is advertised (the `config_invalid` branch of `configure_model_option`); a pinned-but-unadvertised
model at mint stays a hard error via the caller's mapping.

- [ ] **Step 3: Rewire the mint closure** to (a) call `apply_model_effort` with the freshly-read `opts0`/
  `models0`, mapping its outcome to the existing mint errors (NotAdvertised+model-pinned → `config_invalid`;
  Rejected → `agent_crashed`), and (b) **cache the refreshed surface**: `*entry.config_surface.lock() =
  Some(refreshed_surface)`. Keep `set_mode` + `minted_cwd.set` exactly as today. The mint's observable
  behavior (golden frames + ensure_session tests) MUST stay green.

- [ ] **Step 4: Test** — add an acp_backend test that mint still applies model/effort (regression) AND that
  `config_surface` is populated after mint. Run `cargo test -p bridge-acp --lib` + the golden frames
  (`cargo test -p bridge-acp --test golden_frames`). (If `_dyld_start` hangs, build-only + leave for the
  controller.)

- [ ] **Step 5: Commit** — `refactor(acp): cache session/new config surface + extract apply_model_effort (mint-parity)`

---

## Task 5: `AcpBackend::reconcile_config` + `capabilities()`

**Files:** `crates/bridge-acp/src/acp_backend.rs` (the `impl AgentBackend`).

- [ ] **Step 1: Write failing tests** (recording transport): `reconcile_config` on a minted session with a
  changed model fires `session/set_config_option` (codex surface) and returns `Applied`; with a model the
  agent doesn't advertise returns `NotAdvertised`; `capabilities()` maps `agent_capabilities()`.

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Implement `reconcile_config`:**

```rust
    async fn reconcile_config(&self, session: &SessionId, spec: &SessionSpec)
        -> Result<ReconcileOutcome, BridgeError> {
        let entry = self.session_entry(session).await;          // existing accessor
        // Must already be minted (warm). If not minted, ensure_session mints with the FULL spec
        // (so reconcile is a no-op Applied right after). Reach the live agent_session_id:
        let agent_session_id = self.ensure_session(session).await?;  // returns AgentSessionId; mints if absent
        let cx = self.cx()?;
        let surface = entry.config_surface.lock().ok().and_then(|g| g.clone()).unwrap_or_default();
        let agent_id = /* self.config agent id, as the mint closure derives it */;
        let (outcome, refreshed, _current) = Self::apply_model_effort(
            &cx, &agent_session_id, &agent_id, &surface, spec.config.model.as_deref(), spec.config.effort,
        ).await?;
        if outcome == ReconcileOutcome::Applied {
            if let Ok(mut g) = entry.config_surface.lock() { *g = Some(refreshed); }
        }
        Ok(outcome)
    }
```
NOTE: does NOT re-stash cwd, does NOT call `configure_session` (avoids the `minted_cwd` immutability guard);
only re-applies model/effort on the live session. (If `apply_model_effort`'s model step needs to distinguish
a Rejected RPC from NotAdvertised, map the `configure_model_option` `Err(config_invalid)` → NotAdvertised and
`Err(agent_crashed)` → Rejected, logging the reason — do NOT surface it.)

- [ ] **Step 4: Implement `capabilities()`:**

```rust
    fn capabilities(&self) -> AgentSessionCaps {
        match self.agent_capabilities() {
            Some(c) => AgentSessionCaps {
                load_session: c.load_session,
                resume: c.session_capabilities.resume.is_some(),
                close: c.session_capabilities.close.is_some(),
                list: c.session_capabilities.list.is_some(),
                delete: false, // unstable_session_delete not enabled
            },
            None => AgentSessionCaps::default(),
        }
    }
```

- [ ] **Step 5: Run + build** — `cargo test -p bridge-acp --lib reconcile capabilities && cargo build -p bridge-acp`.
- [ ] **Step 6: Commit** — `feat(acp): reconcile_config (warm model/effort re-apply) + capabilities mapping`

---

## Task 6: SessionManager — diff-based reconcile routing + capability recording

**Files:** `crates/bridge-a2a-inbound/src/session_manager.rs`.

- [ ] **Step 1: Write failing tests** (fake backend returns a configurable `ReconcileOutcome` + `AgentSessionCaps`):
  - model/effort-only mismatch + fake `Applied` → `checkout_turn` succeeds + handle fingerprint advances (a
    follow-up at the new config matches).
  - model mismatch + fake `NotAdvertised`/`Rejected` → `Err(ConfigReseedRequired{field:"model"})`.
  - mode mismatch → `Err(ConfigReseedRequired{field:"mode"})`.
  - cwd (or agent) mismatch → `Err(ConfigMismatch{field})`.
  - model+cwd mismatch → `Err(ConfigMismatch{field:"cwd"})` (frozen wins; NOT reconciled).
  - caps recorded on the handle at mint + surfaced in `status`.

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Rewrite the resume branch** (`session_manager.rs:90-115`) to diff-route with the concurrency
  discipline:

```rust
        if let Some(h) = tab.get_mut(ctx) {
            if h.lease.is_retired() { return Err(BridgeError::SessionExpired); }
            if h.state == SessionState::Running { return Err(BridgeError::HandleBusy); }
            let resolved = self.registry.resolve(&agent).await?;
            let eff = effective_config(&resolved.entry, overrides.as_ref());
            let new_fp = SessionSpecFingerprint { agent: agent.clone(), config: eff,
                cwd: cwd.as_ref().map(|c| c.as_str().to_string()) };
            let d = h.fingerprint.diff(&new_fp);
            if !d.is_empty() {
                // frozen fields reject; mode reseed; else must be a subset of {model,effort} -> reconcile.
                if d.iter().any(|f| *f == "agent" || *f == "cwd") {
                    let field = if d.contains(&"agent") { "agent" } else { "cwd" };
                    return Err(BridgeError::ConfigMismatch { field });
                }
                if d.contains(&"mode") { return Err(BridgeError::ConfigReseedRequired { field: "mode" }); }
                // d ⊆ {model, effort}: reconcile. Claim the handle, DROP the lock for the async call.
                let backend = h.backend.clone();
                let backend_session = h.backend_session.clone();
                h.state = SessionState::Running;            // claim (blocks concurrent checkout via HandleBusy)
                let spec = SessionSpec { config: new_fp.config.clone(), cwd: cwd.clone() };
                drop(tab);
                let outcome = backend.reconcile_config(&backend_session, &spec).await;
                let mut tab = self.by_context.lock().await;
                let Some(h) = tab.get_mut(ctx) else { return Err(BridgeError::SessionNotFound) };
                match outcome {
                    Ok(crate::ReconcileOutcome::Applied) => {
                        h.fingerprint = new_fp;             // advance only on Applied, under the lock, handle still claimed
                        h.op = Some(op); h.last_used = (self.now)();
                        return Ok(WarmTurn { backend: h.backend.clone(), session: h.backend_session.clone() });
                    }
                    Ok(_) | Err(_) => {                     // NotAdvertised/Rejected/transport err
                        h.state = SessionState::Idle;
                        let field = if d.contains(&"model") { "model" } else { "effort" };
                        return Err(BridgeError::ConfigReseedRequired { field });
                    }
                }
            }
            // no diff: unchanged Slice-0 path
            h.state = SessionState::Running; h.op = Some(op); h.last_used = (self.now)();
            return Ok(WarmTurn { backend: h.backend.clone(), session: h.backend_session.clone() });
        }
```
(Use the real import path for `ReconcileOutcome`. Note `tab` is re-bound after the await — the original guard
was dropped; re-check the handle still exists.)

- [ ] **Step 4: Record caps at mint + surface in status.** Add `caps: AgentSessionCaps` to `WarmHandle`; at
  the mint branch set `caps: resolved.backend.capabilities()`; add `capabilities: AgentSessionCaps` to
  `SessionStatusInfo` and populate it in `status()`.

- [ ] **Step 5: Run + build** — `cargo test -p bridge-a2a-inbound --lib session_manager && cargo build -p bridge-a2a-inbound`.
- [ ] **Step 6: Commit** — `feat(inbound): SessionManager reconcile-on-continue (diff routing + concurrency) + caps recording`

---

## Task 7: `session/status` capabilities JSON field

**Files:** `crates/bridge-a2a-inbound/src/server.rs` (`session_status` handler ~2842).

- [ ] **Step 1: Write failing test** — extend the Slice-0 `session_status_release_cancel_dispatch` (or add
  one) asserting `result.capabilities` is present with the expected bool keys.

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Add the field** to the `session_status` `json!`:

```rust
        "capabilities": {
            "loadSession": s.capabilities.load_session,
            "resume": s.capabilities.resume,
            "close": s.capabilities.close,
            "list": s.capabilities.list,
            "delete": s.capabilities.delete,
        },
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-a2a-inbound --lib session_status && cargo build -p bridge-a2a-inbound`.
- [ ] **Step 5: Commit** — `feat(inbound): surface agent capabilities in session/status`

---

## Task 8: Workspace gate + live-gate

- [ ] **Step 1: Exhaustiveness + gate** — `cargo test --workspace --no-run` (catch any new match break from
  the variant additions); then `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
  && cargo test --workspace`. Fix any test-target match arms for `ReconcileOutcome`/`ConfigReseedRequired`.

- [ ] **Step 2: Build release** — `cargo build --release -p a2a-bridge`.

- [ ] **Step 3: Live-gate (real codex, serve `examples/a2a-bridge.slice0-livegate.toml`):**
  - **DoD-1 reconcile applies + fires the RPC:** `submit --context C --effort low` then `submit --context C
    --effort high` → 2nd **succeeds** (vs Slice-0 error). Confirm the serve log shows a
    `session/set_config_option` (effort) request fired. A 3rd call at `--effort high` also succeeds (fingerprint
    advanced); back to `--effort low` reconciles again.
  - **DoD-2 cwd reject:** `submit --context C --cwd <other>` → `ConfigMismatch{cwd}`.
  - **DoD-3 mode reseed:** `submit --context C --mode <other>` → `ConfigReseedRequired{mode}`.
  - **DoD-5 caps:** `session status C` includes `capabilities` (codex's advertised set).
  - **DoD-6 no regression:** Slice-0 warm continue/isolation/release/idle-reap still green (re-run the Slice-0
    live-gate scenarios).
  - DoD-4 (NotAdvertised → reseed) is **unit-test-gated** (codex advertises model+effort → unreachable live).

- [ ] **Step 4: Record results** in the PR/notes; then `superpowers:finishing-a-development-branch` (merge to main).

---

## Self-review notes

- **Spec coverage:** reconcile (T4/T5/T6), capability recording (T5/T6/T7), diff routing (T2/T6), typed
  errors (T1), no clear/compact/actions (deferred). All v2 fixes folded: full-mismatch-set (T2/T6), surface
  cache (T4), AgentSessionCaps rename+trim+delete=false (T1/T5), helper returns ReconcileOutcome (T4),
  concurrency claim→drop→reconcile→reacquire (T6), live agent_session_id via ensure_session (T5), fieldless
  Rejected (T1), disposition wiring (T1), status JSON shape (T7), RPC-fired proof (T8).
- **Type consistency:** `ReconcileOutcome`/`AgentSessionCaps`/`ConfigSurface`/`apply_model_effort` used
  consistently T1→T4→T5→T6.
- **Risk hotspots for review:** T4 (mint-parity refactor — gate on golden frames + ensure_session tests) and
  T6 (the drop-lock-across-await concurrency — re-check handle exists after re-acquire; advance fingerprint
  only on Applied). These two warrant a code-quality review pass.
