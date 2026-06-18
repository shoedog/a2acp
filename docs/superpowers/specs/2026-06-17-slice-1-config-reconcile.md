# Slice 1 — Config reconcile + capabilities — design spec

**Status:** design (2026-06-17). Second orchestration slice. Governed by
`2026-06-17-orchestration-slicing.md` (Slice 1) over the converged architecture (P-1, CORRECTION-3).
Builds on Slice 0 (`2026-06-17-slice-0-live-session-core.md`, shipped): the `SessionManager` + warm Local
dispatch + `SessionSpecFingerprint`. ACP grounding: `docs/references/acp-protocol-v1.md`.

## Goal

Upgrade warm continuation from Slice 0's **"reject on any config mismatch"** to **"reconcile when possible."**
When a `continue` on a warm `contextId` carries a different **model/effort**, attempt to apply it on the live
session via the ACP config-option surface (`set_config_option` / `session/set_model`); only if the agent
doesn't advertise it (or rejects it) return a typed error. **cwd** stays frozen (reject). **mode** can't be
safely re-applied mid-session on all agents → a typed "reseed required" (deferred to clear/compact, S3/S4).
Also **record the agent's session capabilities** (loadSession / resume / close / delete / list /
config-options) into the warm handle — raw metadata now; the *actions* (S2-P2) are deferred slices.

This slice is purely an upgrade of the Slice-0 `checkout_turn` mismatch path + a lifted backend method + a
capabilities accessor. No new surfaces, no clear/compact, no journal.

## v2 — dual spec-review fixes folded (codex-xhigh + Opus, both `fix-then-ship`)

Both lenses converged; these resolutions SUPERSEDE the conflicting detail in the sections below. No redesign —
the reconcile-on-continue + bridge-owned caps + typed-error shape is sound; the gaps were lift-mechanics.

- **FIX-1 (BLOCKER, both) — multi-field mismatch routing.** `first_mismatch` (returns only the FIRST field)
  is unsafe: a `model+cwd` or `effort+mode` continue would route to the reconcilable branch, reconcile, then
  advance the fingerprint to include the *unreconciled* frozen field. **Compute the FULL mismatch set**
  (`SessionSpecFingerprint::diff(&other) -> SmallSet<&'static str>`): if it contains `agent` or `cwd` →
  `ConfigMismatch` (reject); else if it contains `mode` → `ConfigReseedRequired{mode}`; else (set ⊆
  `{model, effort}`, non-empty) → reconcile. Add `diff()`; keep `first_mismatch` for the Slice-0 callers
  unchanged (or migrate them).
- **FIX-2 (BLOCKER, both) — cache the config surface for warm reconcile.** `configure_model_option`
  (`acp_backend.rs:524`) + the effort walk-down need `opts0: &[SessionConfigOption]` + `models0:
  Option<&SessionModelState>` from the `session/new` response — `AgentSession` (`acp_backend.rs:266`) caches
  NEITHER (consumed inline at mint `:1197/1201`, discarded). **Add config-surface state to `AgentSession`**
  (e.g. `config_surface: OnceCell<(Vec<SessionConfigOption>, Option<SessionModelState>)>` or a small struct),
  populated in the mint closure from `NewSessionResponse`, refreshed from `set_config_option` responses
  (which return updated `config_options`); `reconcile_config` reads it. The shared helper becomes
  `apply_model_effort(cx, agent_session_id, surface, spec) -> (ReconcileOutcome, refreshed_surface)` and the
  mint closure refactors to call it with the freshly-minted surface (model→effort threads via `refreshed`).
- **FIX-3 (MAJOR, both) — `AgentCaps` name collides + fields unsourceable.** `bridge_core::catalog::AgentCaps`
  ALREADY EXISTS (model-catalog data, live consumers `card.rs`/`acp_backend.rs:1398`). **Rename to
  `AgentSessionCaps`** (lifecycle caps only). **Source ONLY from `agent_capabilities()`** (initialize-time):
  `{ load_session, resume, close, list }` (all on ACP `AgentCapabilities`/`SessionCapabilities`). **`delete`
  is behind `unstable_session_delete` (NOT enabled, slicing-row confirms) → record `delete: false`
  explicitly** (do not read the cfg-gated field). **Drop `config_options`** from the caps struct entirely —
  it is per-session (`session/new`), NOT an initialize-time capability; it does not belong in lifecycle caps.
- **FIX-4 (MAJOR, both) — the helper must return reconcile outcomes.** The mint path maps a missing model
  surface → `ConfigInvalid`, an RPC rejection → `AgentCrashed`, a missing effort → silent Skip
  (`acp_backend.rs:577/660/1265`). `apply_model_effort` must return `ReconcileOutcome`
  (`Applied`/`NotAdvertised`/`Rejected`) for the warm path WHILE preserving mint's existing error/skip
  behavior (mint maps the outcome back to its current semantics; reconcile surfaces it directly).
- **FIX-5 (MAJOR, Opus) — concurrency + live `agent_session_id`.** `checkout_turn` holds the **table-wide**
  `by_context` `tokio::Mutex` across the resume branch; calling the async `reconcile_config` under it blocks
  ALL warm sessions. **Discipline:** on a reconcilable mismatch, set `state = Running` (claim the handle),
  **drop the lock**, call `reconcile_config`, **re-acquire**, advance `fingerprint` on `Applied` (the handle
  is claimed → no concurrent mutation), reset to `Idle` + return the typed error otherwise. `reconcile_config`
  must reach the EXISTING minted `AgentSessionId` (add an internal accessor by bridge `SessionId`); it does
  **NOT** re-mint and re-stashes ONLY model/effort (never cwd → never arms the `minted_cwd` immutability
  guard `acp_backend.rs:1298`).
- **FIX-6 (MINOR→required) — wire `ConfigReseedRequired` into `disposition()`** (`error.rs` RejectRequest arm,
  alongside `ConfigMismatch`) + extend the slice0 error test; else the catch-all makes it `Failed`. **Drop
  `ReconcileOutcome::Rejected{reason}`'s field** (fieldless 3-variant enum; the backend logs the reason
  internally — avoids the dead-data + wire-leak trap). **Define the `session/status` `capabilities` JSON
  shape** explicitly (manual assembly at `server.rs:2842`): `"capabilities": {"loadSession":bool,
  "resume":bool,"close":bool,"list":bool,"delete":false}`.
- **FIX-7 (DoD) — strengthen the live proof.** "normal reply proves effort applied" is weak (codex). DoD-1
  must assert the `set_config_option`/`set_session_model` request ACTUALLY FIRED (recording-transport unit
  assertion + a serve-log line on the live gate). DoD-4 (`NotAdvertised`) is **unit-test-gated** (codex
  advertises everything → unreachable live; kiro is NOT in the live loop).

## Findings (grounded in the code)

- **Config is applied ONLY inside the `session/new` init closure** (P-1, confirmed): `ensure_session`
  applies mode (`set_mode`), model + effort (`configure_model_option` `acp_backend.rs:524`, the effort
  walk-down `:649`) inside the `get_or_try_init` mint (`acp_backend.rs:~1197-1411`). There is **no
  mid-session reconcile path** — so `reconcile_config` is a genuinely NEW callable-on-warm method that
  **lifts** that logic out of the closure to run against an already-minted `agent_session_id`.
- **The 3-surface model reality already exists** (`configure_model_option` `:524`): (1) `config_options`
  category=model (codex/claude), (2) the unstable `models` + `session/set_model` (kiro,
  `set_session_model` `:602`), (3) neither → `config_invalid`. The effort walk-down (`:649-691`) applies
  `reasoning_effort` via `set_config_option` with fallback to lower advertised levels. `reconcile_config`
  reuses these helpers — it does NOT reinvent the per-backend matrix.
- **Request builders exist:** `set_mode_request` (`:459`), `set_config_option_request` (`:468`),
  `set_config_option` (`:480`). All golden-frame-tested.
- **Capabilities are already captured:** `agent_capabilities() -> Option<&AgentCapabilities>`
  (`acp_backend.rs:1060`), populated at connect (`:970`). Slice 1 exposes them via a trait accessor so the
  `SessionManager` records them on the handle. (ACP `AgentCapabilities` carries `loadSession` +
  `sessionCapabilities.{resume,close,list,delete}` + `mcpCapabilities` etc.)
- **Slice 0 today:** `SessionManager::checkout_turn` on a known contextId computes the fingerprint and, on
  any `first_mismatch`, returns `BridgeError::ConfigMismatch{field}` (no reconcile). Slice 1 changes this
  one branch.

## Architecture

### 1. `ReconcileOutcome` + the backend method (bridge-core + AcpBackend)

```rust
// bridge-core/src/ports.rs (or orch.rs)
#[derive(Clone, Debug, PartialEq)]
pub enum ReconcileOutcome {
    Applied,                         // model/effort applied on the live session
    NotAdvertised,                   // the agent doesn't expose this config surface
    Rejected { reason: String },     // the agent rejected the change
}

// AgentBackend trait — additive, default = NotAdvertised (non-ACP backends can't reconcile a live session)
async fn reconcile_config(
    &self,
    _session: &SessionId,
    _spec: &SessionSpec,
) -> Result<ReconcileOutcome, BridgeError> {
    Ok(ReconcileOutcome::NotAdvertised)
}
```

**`AcpBackend::reconcile_config`** (the lift): resolve the live `agent_session_id` for the bridge
`SessionId` (must already be minted — `ensure_session` first if absent, then apply); re-run
`configure_model_option` (model) + the effort walk-down (effort) against the live session for the fields that
DIFFER from the current spec; map results → `Applied` / `NotAdvertised` (no `config_options`/`models`
surface) / `Rejected{reason}`. **Does NOT touch mode** (mode is the caller's reseed concern). To avoid
duplicating the init-closure body, **extract a shared `apply_model_effort(cx, agent_session_id, spec)` helper**
that both `ensure_session` (mint) and `reconcile_config` (warm) call (DRY — one place owns the 3-surface
matrix). Re-stash the new `SessionSpec` via `configure_session` so a later mint/repeat is consistent.

### 2. Capability recording (bridge-core + AcpBackend + SessionManager)

```rust
// bridge-core — a bridge-owned, backend-neutral snapshot (NOT the raw SDK type — feature-flag-shift safe).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentCaps {
    pub load_session: bool,
    pub resume: bool,
    pub close: bool,
    pub delete: bool,
    pub list: bool,
    pub config_options: bool,   // advertises a model/effort/mode config surface
}

// AgentBackend trait — additive, default empty (test/non-ACP backends advertise nothing)
fn capabilities(&self) -> AgentCaps { AgentCaps::default() }
```

`AcpBackend::capabilities()` maps from `agent_capabilities()` (`:1060`). The `SessionManager` records
`AgentCaps` on the `WarmHandle` at mint (a new field) and exposes it via `session/status` (additive field).
**Slice 1 only RECORDS + surfaces** the caps; load/resume/close/delete/list ACTIONS are deferred (S2-P2/later).

### 3. `SessionManager::checkout_turn` reconcile path (the behavior change)

On a **known** contextId whose recomputed `SessionSpecFingerprint` differs from the handle's:
```text
match first_mismatch(field):
  "agent" | "cwd"   -> Err(ConfigMismatch{field})          // frozen — reject (cwd immutable post session/new)
  "mode"            -> Err(ConfigReseedRequired{field:"mode"})   // can't reconcile mid-session → clear/compact (S3/S4)
  "model" | "effort" -> match backend.reconcile_config(backend_session, &new_spec):
        Applied              -> update handle.fingerprint = new_fp; configure_session(new_spec); proceed (Running)
        NotAdvertised        -> Err(ConfigReseedRequired{field})
        Rejected{reason}     -> Err(ConfigReseedRequired{field})   // (reason logged, not leaked)
```
On `Applied`, the handle's frozen fingerprint advances to the new effective config (so the NEXT continue with
the same config matches). The mint path (fresh contextId) is unchanged (Slice 0).

### 4. New error variant

`BridgeError::ConfigReseedRequired { field: &'static str }` (mirrors `ConfigMismatch`): "this field can't be
reconciled on the warm session; clear/compact to change it." Disposition = `RejectRequest`; `client_message`
safe (field name only). (`ConfigMismatch` is retained for agent/cwd — truly frozen — vs `ConfigReseedRequired`
for fields that a future clear/compact CAN change.)

## Scope

**IN:** `ReconcileOutcome` + `AgentBackend::reconcile_config` (default NotAdvertised) + `AcpBackend` impl
(lift via a shared `apply_model_effort` helper; model+effort only); `AgentCaps` + `AgentBackend::capabilities`
(default empty) + `AcpBackend` mapping; `SessionManager` records `AgentCaps` on the handle + surfaces it in
`session/status` + the reconcile-on-continue path (model/effort→reconcile, mode→reseed-required, cwd/agent→
reject); `BridgeError::ConfigReseedRequired`.

**OUT (later slices):** usage telemetry (S2); clear/reset (S3) + compact (S4) — so `ConfigReseedRequired` is a
typed dead-end in Slice 1 (the caller must start a fresh contextId until S3 lands); load/resume/close/delete/
list ACTIONS (capabilities are recorded, not acted on); the journal/MCP/Turn-Channel.

## Definition of Done + LIVE-GATE (real serve + codex)

1. **model/effort reconcile applies:** warm `continue` on a contextId with a changed `--effort` (advertised
   by codex) **succeeds** (no error) and the turn runs at the new effort — proven via a 2nd `submit --context
   C --effort <different>` returning a normal reply (vs Slice 0 which errored). The handle's fingerprint
   advances (a 3rd call at the new effort also succeeds; a call back at the old effort reconciles again).
2. **cwd delta rejects:** `continue` with a different `--cwd` → typed `ConfigMismatch{cwd}` (unchanged).
3. **mode delta → reseed-required:** `continue` with a different `--mode` → typed `ConfigReseedRequired{mode}`.
4. **NotAdvertised path:** an agent without a config surface (or a field it doesn't advertise) → typed
   `ConfigReseedRequired` (not a silent apply, not a crash). (Gate with kiro or a fixture if codex advertises
   everything; else assert via unit test.)
5. **Capabilities recorded:** `session/status C` includes the agent's `capabilities` (e.g. codex's advertised
   set) — surfaced, accurate.
6. **No regression:** Slice 0 DoD (warm continue, isolation, release, idle reap) still green.

## Risks

- **Lifting the init-closure config logic** must not change mint behavior — extract `apply_model_effort` and
  have BOTH mint and reconcile call it; keep a mint test green (the existing set_mode/set_config_option golden
  tests + any ensure_session test). The effort walk-down's fallback semantics must be preserved.
- **reconcile on a not-yet-minted session:** `reconcile_config` must `ensure_session` first (mint applies the
  full spec at session/new — so a reconcile right after mint is a no-op Applied). Guard the ordering.
- **Fingerprint advance race:** update `handle.fingerprint` only AFTER `reconcile_config` returns `Applied`,
  under the `by_context` lock, so a concurrent `checkout` sees a consistent fingerprint (the Slice-0
  `HandleBusy` guard already serializes turns per handle).
- **`AgentCaps` mapping** must be bridge-owned (not the raw SDK `AgentCapabilities`) — SDK shape shifts under
  feature flags (the Slice-0 `cost` lesson).
- **kiro/claude variance:** model via `session/set_model` (kiro) vs `config_options` (codex/claude) — reuse
  `configure_model_option`'s existing 3-surface match; don't fork it.

## Testing approach

- **Unit (bridge-core):** `ReconcileOutcome`/`AgentCaps` shape; `AgentBackend::{reconcile_config,capabilities}`
  defaults (object-safety test extended). **SessionManager:** model/effort mismatch → calls
  `reconcile_config` (fake backend returns Applied → proceeds + fingerprint advances; NotAdvertised/Rejected →
  `ConfigReseedRequired`); mode mismatch → `ConfigReseedRequired`; cwd/agent mismatch → `ConfigMismatch`; caps
  recorded on the handle + in status.
- **Unit (bridge-acp):** `reconcile_config` against the recording transport — applies model via
  `set_config_option` (codex surface) / `session/set_model` (kiro surface) / NotAdvertised when neither;
  `capabilities()` maps `agent_capabilities()`; mint still applies config (regression). `apply_model_effort`
  shared-helper extraction keeps the golden frames passing.
- **Live-gate (real codex):** DoD 1-3, 5, 6 via `submit --context --effort` + `session status`.

## Constraints (carried)

codex gpt-5.5/high implementor (host, via `run-workflow slice0-impl`-style; controller verifies + commits —
the `_dyld_start` flake); codex high-risk/final + Opus arch review; `max_attempts=3`; each slice
**dual spec-review (codex xhigh + Opus) before planning** + **dual plan-review** + **LIVE-GATED** before merge.
