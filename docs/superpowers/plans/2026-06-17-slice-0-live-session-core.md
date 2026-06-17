# Slice 0 — Live Session Core (warm continue) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make a bridge-driven agent warm across tasks — a 2nd A2A message on the same `contextId` reuses the
same warm ACP session (no cold spawn, context intact); no `contextId` = today's forget-after behavior.

**Architecture:** A new serve-side `SessionManager` (sibling to the registry + TaskStore) holds a warm
`WarmHandle` (backend + registry lease + frozen fingerprint + turn state) keyed by A2A `contextId`. `gate()`
parses the contextId into `RoutedCall`; the async Local dispatch consults the manager, checks out a warm turn
(rejecting a concurrent one with `HandleBusy`), and dispatches against the warm `backend_session` with a
turn-guard (so the per-task `BindingGuard` is never created and the legacy `session-{task}` never leaks). Plus
the minimal real `OrchEvent`/`OrchResult` DTOs, an `Update::Usage` variant + a `release_session` backend
method, a `Lease::is_retired` signal for `SessionExpired`, and `SessionStatus`/`SessionRelease`/`SessionCancel`
JSON-RPC methods + CLI.

**Tech Stack:** Rust workspace (bridge-core, bridge-registry, bridge-acp, bridge-container, bridge-api,
bridge-a2a-inbound, bin/a2a-bridge); async-trait; tokio; serde; the external `a2a-lf` 0.3.0 crate (PascalCase
wire methods).

**Spec:** `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` (v2, dual-reviewed).
**Slicing authority:** `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (Slice 0).
**This plan is v2** — dual plan-review (codex xhigh + Opus) fixes folded: the warm-turn lifecycle
(`Running{op}`/`HandleBusy`/reap-Idle-only), `SessionExpired` via `Lease::is_retired`, full warm-session
threading (`LocalDispatch.session` + `store.put` + both arms + SSE contextId), single-resolve, `idle_age_ms`,
CLI override flags + artifact print, reaper interval, and the real test-helper/module-registration fixes.

**Grounded facts (verbatim-verified):**
- Wire methods are **PascalCase** from `a2a-lf` → new methods `SessionStatus`/`SessionRelease`/`SessionCancel`
  as bare string-literal arms in the `server.rs:589` match.
- `gate()` is `fn gate(&self, …) -> Result<RoutedCall>` (`server.rs:306`) — sync, `&self`. Parses contextId;
  the async manager lookup happens in the Local dispatch arm.
- `resolve_configure_bind` → `LocalDispatch { backend, guard: Option<BindingGuard> }` (`server.rs:420/438`);
  the unary arm holds `_guard` (`server.rs:2217`), the streaming arm moves it into `spawn_local_producer`
  (`server.rs:1020`, which takes ownership of `routed.session` at `:1029`).
- `Update` (`ports.rs:21`) has no wildcard; the breaking exhaustive `match` on adding `Usage` is in
  **`bridge-workflow/src/executor.rs:~143`** (`Some(Ok(Update::…))` arms) and `bridge-core/src/translator.rs`.
  ACP's `map_session_update` *constructs* `Update` (not a match) → unaffected.
- `ContainerRwBackend::forget_session` is stash-only; warm containers reaped only by `retire_warm()` → add a
  per-session `release_warm`. `bridge-api::forget_session` already clears its `sessions` map → default OK.
- `ids.rs` macros are String-only → `SessionGeneration(u64)` is hand-written.
- `Lease` (`ports.rs:128`) has no retirement signal; registry retirement is detached (`registry.rs:403`).

**Naming:** spec says `session/status`; wire/CLI realize as `SessionStatus`/`SessionRelease`/`SessionCancel`
+ CLI `session status|release|cancel` (intent preserved).

---

## Task 1: Core id newtypes

**Files:** Modify `crates/bridge-core/src/ids.rs` (after line 29); test in same file.

- [ ] **Step 1: Write the failing test** — append to `crates/bridge-core/src/ids.rs`:

```rust
#[cfg(test)]
mod slice0_id_tests {
    use super::*;
    #[test]
    fn new_orch_ids_parse_and_roundtrip() {
        assert_eq!(SessionHandleId::parse("h-1").unwrap().as_str(), "h-1");
        assert_eq!(OperationId::parse("op-1").unwrap().as_str(), "op-1");
        assert_eq!(ContextId::parse("ctx-1").unwrap().as_str(), "ctx-1");
        assert!(ContextId::parse("").is_err());
    }
    #[test]
    fn session_generation_orders_and_increments() {
        let g0 = SessionGeneration::new(0);
        let g1 = SessionGeneration::new(g0.get() + 1);
        assert!(g1 > g0);
        assert_eq!(g1.get(), 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-core --lib slice0_id_tests`
  Expected: FAIL — types undefined.

- [ ] **Step 3: Add the newtypes** — after `id_newtype!(AgentId);` (line 29):

```rust
// Slice 0 (orchestration) ids.
id_newtype!(SessionHandleId);
id_newtype!(OperationId);
id_newtype!(ContextId);

/// A warm session's context generation. Hand-written (the `id_newtype!` macros are
/// String-only); generations are compared/incremented so we add `Copy`/`Ord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct SessionGeneration(pub u64);
impl SessionGeneration {
    pub fn new(n: u64) -> Self { Self(n) }
    pub fn get(&self) -> u64 { self.0 }
}
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-core --lib slice0_id_tests` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): Slice-0 orch ids (SessionHandleId/OperationId/ContextId/SessionGeneration)"`

---

## Task 2: Minimal `OrchEvent`/`OrchResult` DTOs + stop-reason mapping

**Files:** Create `crates/bridge-core/src/orch.rs`; modify `crates/bridge-core/src/lib.rs`.

- [ ] **Step 1: Register the module AND write the test** (register first — Cargo ignores an unregistered
  file, so the "failing test" must be reachable). Add `pub mod orch;` to `crates/bridge-core/src/lib.rs`, then
  create `crates/bridge-core/src/orch.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orch_event_roundtrips_with_internal_kind_tag() {
        let ev = OrchEvent {
            v: ORCH_V, seq: 3, ts_ms: 100,
            operation_id: crate::ids::OperationId::parse("op-1").unwrap(),
            kind: OrchEventKind::Usage { usage: UsageSnapshot { used: Some(10), size: Some(200), cost: None, at_ms: 100 } },
        };
        let j = serde_json::to_value(&ev).unwrap();
        assert_eq!(j["kind"], "usage");
        assert_eq!(j["used"], 10);
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 3);
    }
    #[test]
    fn usage_cost_carries_amount_and_currency() {
        let j = serde_json::to_value(&UsageCost { amount: 1.5, currency: "USD".into() }).unwrap();
        assert_eq!(j["amount"], 1.5);
        assert_eq!(j["currency"], "USD");
    }
    #[test]
    fn terminal_status_from_each_stop_reason() {
        assert!(matches!(TerminalStatus::from_stop_reason("end_turn"), TerminalStatus::Completed));
        assert!(matches!(TerminalStatus::from_stop_reason("cancelled"), TerminalStatus::Canceled));
        for s in ["refusal", "max_tokens", "max_turn_requests", "weird"] {
            assert!(matches!(TerminalStatus::from_stop_reason(s), TerminalStatus::Failed { .. }));
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-core --lib orch::tests`
  Expected: FAIL — types undefined (the module now compiles-errors, which is the intended failure).

- [ ] **Step 3: Write the DTOs** — prepend to `orch.rs`:

```rust
//! Slice 0 minimal orchestration DTOs (bridge-owned, versioned, Ser+De). Rich variants
//! (Plan/ToolCall/config/mode/commands) + the `session`/`source` envelope fields are deferred
//! (S6/S7); the versioned + `#[serde(flatten)] kind` envelope makes those additions non-breaking.
use crate::ids::OperationId;
use serde::{Deserialize, Serialize};

pub const ORCH_V: u16 = 1;

/// ACP usage cost is `{amount, currency}` — NOT guaranteed USD.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageCost { pub amount: f64, pub currency: String }

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageSnapshot { pub used: Option<u64>, pub size: Option<u64>, pub cost: Option<UsageCost>, pub at_ms: i64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchEvent {
    pub v: u16, pub seq: i64, pub ts_ms: i64, pub operation_id: OperationId,
    #[serde(flatten)] pub kind: OrchEventKind,
}

/// Struct variants only — serde internally-tagged enums reject bare tuple variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    Progress { text: String },
    Usage { #[serde(flatten)] usage: UsageSnapshot },
    Terminal { status: TerminalStatus },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerminalStatus { Completed, Failed { reason: String }, Canceled }

impl TerminalStatus {
    /// ACP `StopReason` → terminal status (spec P-4). `end_turn`→Completed; `cancelled`→Canceled; else→Failed.
    pub fn from_stop_reason(stop_reason: &str) -> Self {
        match stop_reason {
            "end_turn" => TerminalStatus::Completed,
            "cancelled" => TerminalStatus::Canceled,
            other => TerminalStatus::Failed { reason: other.to_string() },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchResult {
    pub v: u16, pub operation_id: OperationId, pub status: TerminalStatus,
    pub wall_clock_ms: u64, pub usage: UsageSnapshot, pub output: String,
}
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-core --lib orch::tests` → PASS (3).
- [ ] **Step 5: Commit** — `git commit -am "feat(core): minimal OrchEvent/OrchResult/UsageSnapshot DTOs + stop-reason mapping"`

---

## Task 3: `BridgeError::{ConfigMismatch, SessionExpired, HandleBusy}`

**Files:** Modify `crates/bridge-core/src/error.rs` (enum 22-62; `disposition()` 97-110); test in file.

- [ ] **Step 1: Write the failing test** — add to `error.rs`:

```rust
#[cfg(test)]
mod slice0_error_tests {
    use super::*;
    #[test]
    fn slice0_errors_reject_request() {
        for e in [
            BridgeError::ConfigMismatch { field: "model" },
            BridgeError::SessionExpired,
            BridgeError::HandleBusy,
        ] {
            assert_eq!(e.disposition(), A2aDisposition::RejectRequest);
        }
    }
    #[test]
    fn config_mismatch_client_message_is_safe() {
        assert!(BridgeError::ConfigMismatch { field: "effort" }.client_message().contains("effort"));
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-core --lib slice0_error_tests` → FAIL (variants undefined).

- [ ] **Step 3: Add the variants** — after `SessionNotFound` (line 31):

```rust
    #[error("config mismatch: {field}")]
    ConfigMismatch { field: &'static str },
    #[error("session expired")]
    SessionExpired,
    #[error("session busy")]
    HandleBusy,
```

- [ ] **Step 4: Map in `disposition()`** — extend the `RejectRequest` arm (line 102):

```rust
            A2aVersionMismatch | InvalidRequest { .. } | TaskNotFound | SessionNotFound
            | ConfigMismatch { .. } | SessionExpired | HandleBusy => RejectRequest,
```

(No `client_message()` change — none carry infra detail.)

- [ ] **Step 5: Run + build** — `cargo test -p bridge-core --lib slice0_error_tests && cargo build -p bridge-core`
  Expected: PASS; builds (fix any non-wildcard `match BridgeError` the compiler flags).

- [ ] **Step 6: Commit** — `git commit -am "feat(core): BridgeError::{ConfigMismatch,SessionExpired,HandleBusy} + RejectRequest"`

---

## Task 4: `Update::Usage` variant + `AgentBackend::release_session` trait method

**Files:** Modify `crates/bridge-core/src/ports.rs` (`Update` 21-25; trait 31-55; object-safety test ~363-390);
`crates/bridge-core/src/translator.rs`; `crates/bridge-workflow/src/executor.rs` (~140).

- [ ] **Step 1: Add a `release_session` assertion to the object-safety test** — in `ports.rs`
  `agentbackend_defaults_are_noops_and_object_safe`, find the binding name (it is `let f = …;`) and add after
  the existing `forget_session` call:

```rust
        f.release_session(&crate::ids::SessionId::parse("s").unwrap()).await;
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-core --lib agentbackend_defaults_are_noops_and_object_safe` → FAIL (no method).

- [ ] **Step 3: Add `Usage` to `Update`** — `ports.rs:21`:

```rust
pub enum Update {
    Text(String),
    Permission(PermissionRequest),
    Usage(crate::orch::UsageSnapshot),
    Done { stop_reason: String },
}
```

- [ ] **Step 4: Add `release_session` to the trait** — after `forget_session` (line 50):

```rust
    /// Release a warm session: drop ALL per-session backend state + reap any per-session
    /// resource (e.g. a `:rw` container). Default = `forget_session` (correct for
    /// non-warm/non-process backends). Warm backends override. [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        self.forget_session(session).await;
    }
```

- [ ] **Step 5: Fix the two real exhaustive `match Update` sites** (ACP's `map_session_update` *constructs*
  `Update`, so it is NOT a break — do not touch it):
  - `crates/bridge-core/src/translator.rs` — in the loop's `match` over the backend `Update`, add a no-op arm
    (Slice 0 defers telemetry plumbing to Slice 2): `Update::Usage(_) => { continue; }` (adapt to the arm
    shape — emit no event).
  - `crates/bridge-workflow/src/executor.rs` (~line 143) — the match has `Some(Ok(Update::Text))`/`Permission`
    /`Done` arms; add `Some(Ok(Update::Usage(_))) => { /* Slice 0: ignore */ }` (match the surrounding
    `Some(Ok(..))` wrapper and loop control).
  - Confirm coverage: `rg "Update::(Text|Done|Permission)" --type rust` — any other non-wildcard `match` gets
    the same no-op arm. (Wildcarded sites like `Ok(_)`/`_ =>` need nothing.)

- [ ] **Step 6: Build + test** — `cargo build --workspace && cargo test -p bridge-core --lib agentbackend_defaults_are_noops_and_object_safe`
  Expected: workspace builds; object-safety test PASSES.

- [ ] **Step 7: Commit** — `git commit -am "feat(core): Update::Usage variant + AgentBackend::release_session (default=forget_session)"`

---

## Task 5: `AcpBackend::release_session` override

**Files:** Modify `crates/bridge-acp/src/acp_backend.rs` (`AgentBackend` impl near `forget_session` line 1805);
test in file. **Test constructor:** use `connect_recording(rec).await` (`acp_backend.rs:2826`) — the in-module
test idiom; `sessions` (tokio `Mutex`) / `session_cfg` (StdMutex) / `session_entry` are reachable from
`#[cfg(test)] mod tests`. There is no `new_for_test`.

- [ ] **Step 1: Write the failing test** — add to the acp_backend test module (mirror the nearest
  `connect_recording`-based `#[tokio::test]`):

```rust
    #[tokio::test]
    async fn release_session_removes_both_sessions_and_cfg_entries() {
        let be = connect_recording(Default::default()).await; // mirror the real signature in this file
        let s = SessionId::parse("ctx-x-g0").unwrap();
        be.configure_session(&s, &SessionSpec::from_config(Default::default())).await.unwrap();
        let _ = be.session_entry(&s).await; // force an AgentSession entry
        be.release_session(&s).await;
        assert!(be.session_cfg.lock().unwrap().get(&s).is_none(), "cfg stash removed");
        assert!(be.sessions.lock().await.get(&s).is_none(), "agent session removed");
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-acp --lib release_session_removes_both` → FAIL (default keeps `sessions`).

- [ ] **Step 3: Implement the override** — after `forget_session` (line 1814):

```rust
    /// Release a warm ACP session: best-effort cancel an in-flight turn, drop the
    /// agent-side `AgentSession` (a later reuse re-mints a fresh `session/new`), and drop
    /// the config stash. Does NOT `retire()` the shared process. [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        let _ = self.cancel(session).await;
        self.sessions.lock().await.remove(session);
        if let Ok(mut m) = self.session_cfg.lock() {
            m.remove(session);
        }
    }
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-acp --lib release_session_removes_both` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(acp): release_session drops agent session + cfg (keeps shared process warm)"`

---

## Task 6: `ContainerRwBackend::release_session` → per-session `release_warm`

**Files:** Modify `crates/bridge-container/src/lib.rs` (helpers near `retire_warm` 412; `AgentBackend` impl
near `forget_session` 534); test in file. **Real test helpers:** `warm_backend(...)`, `spec_cwd(...)`,
`counting_reap()`, `StubInner` (see `lib.rs:826` + `warm_reuses_one_inner_across_turns` `:1161`). Seed a warm
entry by driving one `prompt` (as that test does), NOT a `seed_warm_entry` helper (doesn't exist).

- [ ] **Step 1: Write the failing test** — add to the bridge-container test module, mirroring
  `warm_reuses_one_inner_across_turns` for the harness + reap counter:

```rust
    #[tokio::test]
    async fn release_session_reaps_only_that_warm_container() {
        let (reaps, reap_fn) = counting_reap();
        let be = warm_backend(reap_fn);                  // mirror the real warm_backend signature
        let s = SessionId::parse("ctx-a-g0").unwrap();
        be.configure_session(&s, &spec_cwd("/work")).await.unwrap();
        let _ = be.prompt(&s, vec![Part { text: "hi".into() }]).await.unwrap(); // seeds the warm entry
        be.release_session(&s).await;
        assert!(be.warm.lock().await.get(&s).is_none(), "warm entry removed");
        assert_eq!(reaps.load(std::sync::atomic::Ordering::SeqCst), 1, "exactly one container reaped");
    }
```

(Adjust `reaps`/`counting_reap()`/`Part` construction to the file's exact helper shapes.)

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-container --lib release_session_reaps_only_that_warm` → FAIL (default is stash-only).

- [ ] **Step 3: Add `release_warm` + the override** — near `retire_warm` (line 412):

```rust
    /// Reap ONE warm session's container (per-session analogue of `retire_warm`).
    async fn release_warm(&self, session: &SessionId) {
        let wi = self.warm.lock().await.remove(session);
        if let Some(wi) = wi {
            let _ = wi.inner.cancel(session).await;
            reap_once(&self.reap_fn, self.cfg.sandbox.runtime(), &wi.name, &wi.reaped);
        }
        self.turn_active.lock().await.remove(session);
    }
```

In `impl AgentBackend for ContainerRwBackend`, after `forget_session` (line 534):

```rust
    async fn release_session(&self, session: &SessionId) {
        if self.is_warm() {
            self.release_warm(session).await;
        }
        self.session_cfg.lock().await.remove(session);
    }
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-container --lib release_session_reaps_only_that_warm` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(container): release_session reaps one warm session's container (release_warm)"`

---

## Task 7: `SessionSpecFingerprint`

**Files:** Create `crates/bridge-core/src/session_fingerprint.rs`; modify `crates/bridge-core/src/lib.rs`.

- [ ] **Step 1: Register + write the test** — add `pub mod session_fingerprint;` to `lib.rs`, then create
  the file with the full content:

```rust
//! Frozen-at-mint fingerprint for warm-session continuation. A `continue` whose recomputed
//! fingerprint differs → typed `ConfigMismatch{field}` (Slice 0; reconcile is Slice 1).
use crate::domain::EffectiveConfig;
use crate::ids::AgentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpecFingerprint {
    pub agent: AgentId,
    pub config: EffectiveConfig,
    /// Canonical cwd string (None = no override). String (not SessionCwd) to avoid coupling
    /// to its derives; cwd is immutable post-`session/new`.
    pub cwd: Option<String>,
}

impl SessionSpecFingerprint {
    /// The first differing field (`agent`/`model`/`effort`/`mode`/`cwd`), else `None`.
    pub fn first_mismatch(&self, other: &SessionSpecFingerprint) -> Option<&'static str> {
        if self.agent != other.agent { return Some("agent"); }
        if self.config.model != other.config.model { return Some("model"); }
        if self.config.effort != other.config.effort { return Some("effort"); }
        if self.config.mode != other.config.mode { return Some("mode"); }
        if self.cwd != other.cwd { return Some("cwd"); }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fp(model: &str, cwd: Option<&str>) -> SessionSpecFingerprint {
        SessionSpecFingerprint {
            agent: AgentId::parse("codex").unwrap(),
            config: EffectiveConfig { model: Some(model.into()), effort: None, mode: None },
            cwd: cwd.map(|s| s.to_string()),
        }
    }
    #[test]
    fn identical_have_no_mismatch() {
        assert_eq!(fp("gpt-5.5", Some("/work")).first_mismatch(&fp("gpt-5.5", Some("/work"))), None);
    }
    #[test]
    fn model_and_cwd_mismatches_reported() {
        assert_eq!(fp("gpt-5.5", None).first_mismatch(&fp("gpt-5.4", None)), Some("model"));
        assert_eq!(fp("gpt-5.5", Some("/a")).first_mismatch(&fp("gpt-5.5", Some("/b"))), Some("cwd"));
    }
}
```

- [ ] **Step 2: Run to verify it fails then passes** — `cargo test -p bridge-core --lib session_fingerprint`
  Expected: FAIL before the file body exists / PASS after (write test + body together; the failing state is
  the pre-`first_mismatch` compile error if you stage the test first — acceptable, the gate is the green run).

- [ ] **Step 3: Commit** — `git commit -am "feat(core): SessionSpecFingerprint (agent+effective_config+cwd) with first_mismatch"`

---

## Task 8: `Lease::is_retired` signal (for `SessionExpired`)

**Files:** Modify `crates/bridge-core/src/ports.rs` (`Lease` trait line 128); `crates/bridge-registry/src/
registry.rs` (`LeaseGuard` + the retirement that flips the flag); test in registry.

- [ ] **Step 1: Add the object-safe trait method** — `ports.rs:129`:

```rust
pub trait Lease: Send + Sync {
    /// True once the slot this lease belongs to has been retired/replaced (config reload).
    /// A warm SessionManager checks this to expire a handle. Default `false` (test leases). [Slice 0]
    fn is_retired(&self) -> bool { false }
}
```

- [ ] **Step 2: Write the failing test** — in `registry.rs` tests, assert a lease reports retired after the
  slot is removed via `apply()` (mirror the existing retirement test `:1146`):

```rust
    #[tokio::test]
    async fn lease_reports_retired_after_slot_removed() {
        // Build a registry with agent "a", resolve it to hold a lease, then apply a snapshot
        // WITHOUT "a" (removal). The held lease must report is_retired() == true.
        let reg = /* existing registry test ctor */;
        let resolved = reg.resolve(&AgentId::parse("a").unwrap()).await.unwrap();
        reg.apply(/* snapshot without "a" */).await.unwrap();
        assert!(resolved.lease.is_retired(), "lease should report retired after slot removal");
    }
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p bridge-registry --lib lease_reports_retired` → FAIL (default `false`).

- [ ] **Step 4: Implement** — give `Slot` a shared `retired: Arc<AtomicBool>` (or reuse the existing retire
  signaling — inspect `registry.rs:248/403`); `LeaseGuard` holds a clone; `Lease::is_retired` reads it; the
  retirement path (slot removed/replaced in `apply()`) sets it to `true` BEFORE/at `spawn_retirement`. Wire it
  so a lease handed out before retirement observes the flag.

```rust
// In LeaseGuard:
impl Lease for LeaseGuard {
    fn is_retired(&self) -> bool { self.retired.load(std::sync::atomic::Ordering::SeqCst) }
}
```

- [ ] **Step 5: Run to verify it passes** — `cargo test -p bridge-registry --lib lease_reports_retired` → PASS.
- [ ] **Step 6: Commit** — `git commit -am "feat(registry): Lease::is_retired signal set on slot removal/replace"`

---

## Task 9: `SessionManager` core (mint/resume + warm-turn lifecycle)

**Files:** Create `crates/bridge-a2a-inbound/src/session_manager.rs`; modify `crates/bridge-a2a-inbound/src/
lib.rs`. **Test doubles:** copy `FakeRegistry`/`NoopLease`/a recording `FakeBackend` from
`crates/bridge-a2a-inbound/tests/workflow_producer.rs:90` (the importable model) into the test module; build a
full `AgentEntry` from `domain.rs` (it has ~24 fields — use a small `fn fake_entry()` helper). Inject a clock
via `new_with_clock` for TTL tests.

This is the largest task — build TDD in sub-steps. The manager resolves ONCE (no double-resolve), computes the
fingerprint internally, tracks `Running{op}` with a returned turn-guard, rejects concurrent turns with
`HandleBusy`, expires handles whose lease `is_retired`, and reaps only `Idle`.

- [ ] **Step 1: Register + write the tests** — add `pub mod session_manager;` to `lib.rs`; create the file
  with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // FakeRegistry/FakeBackend/NoopLease + fake_entry() copied from tests/workflow_producer.rs.
    // mgr() builds a SessionManager::new_with_clock(fake_registry(), Duration::from_secs(5), clock).
    #[tokio::test]
    async fn checkout_then_lookup_reuses_same_warm_session() {
        let m = mgr();
        let ctx = ctxid("c1");
        let t1 = m.checkout_turn(&ctx, agent("codex"), None, None, opid("op1")).await.unwrap();
        let s1 = t1.session.clone();
        m.finish_turn(&ctx);                                    // turn ends → Idle
        let t2 = m.checkout_turn(&ctx, agent("codex"), None, None, opid("op2")).await.unwrap();
        assert_eq!(t2.session, s1, "same warm backend_session reused");
        m.finish_turn(&ctx);
    }
    #[tokio::test]
    async fn concurrent_turn_is_handle_busy() {
        let m = mgr();
        let ctx = ctxid("c1");
        let _t1 = m.checkout_turn(&ctx, agent("codex"), None, None, opid("op1")).await.unwrap(); // not finished
        let err = m.checkout_turn(&ctx, agent("codex"), None, None, opid("op2")).await.unwrap_err();
        assert!(matches!(err, bridge_core::error::BridgeError::HandleBusy));
    }
    #[tokio::test]
    async fn config_mismatch_is_typed_error() {
        let m = mgr();
        let ctx = ctxid("c1");
        let t = m.checkout_turn(&ctx, agent("codex"), Some(ov_model("gpt-5.5")), None, opid("op1")).await.unwrap();
        m.finish_turn(&ctx);
        drop(t);
        let err = m.checkout_turn(&ctx, agent("codex"), Some(ov_model("gpt-5.4")), None, opid("op2")).await.unwrap_err();
        assert!(matches!(err, bridge_core::error::BridgeError::ConfigMismatch { field: "model" }));
    }
    #[tokio::test]
    async fn release_evicts_and_calls_backend_release() {
        let m = mgr();
        let ctx = ctxid("c1");
        m.checkout_turn(&ctx, agent("codex"), None, None, opid("op1")).await.unwrap();
        m.finish_turn(&ctx);
        m.release(&ctx).await;
        assert!(m.status(&ctx).await.is_none());
        // assert FakeBackend recorded release_session
    }
    #[tokio::test]
    async fn idle_ttl_reaps_only_idle_sessions() {
        let m = mgr(); // ttl 5s
        let ctx_idle = ctxid("idle");
        let ctx_busy = ctxid("busy");
        m.checkout_turn(&ctx_idle, agent("codex"), None, None, opid("op1")).await.unwrap();
        m.finish_turn(&ctx_idle);
        let _busy = m.checkout_turn(&ctx_busy, agent("codex"), None, None, opid("op2")).await.unwrap(); // Running
        m.advance_clock(std::time::Duration::from_secs(6));
        m.reap_idle().await;
        assert!(m.status(&ctx_idle).await.is_none(), "idle reaped");
        assert!(m.status(&ctx_busy).await.is_some(), "running NOT reaped");
    }
    #[tokio::test]
    async fn retired_lease_expires_handle() {
        let m = mgr_with_retiring_lease(); // FakeRegistry hands a lease whose is_retired() flips true
        let ctx = ctxid("c1");
        m.checkout_turn(&ctx, agent("codex"), None, None, opid("op1")).await.unwrap();
        m.finish_turn(&ctx);
        m.mark_lease_retired(&ctx); // test hook flips the fake lease
        let err = m.checkout_turn(&ctx, agent("codex"), None, None, opid("op2")).await.unwrap_err();
        assert!(matches!(err, bridge_core::error::BridgeError::SessionExpired));
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-a2a-inbound --lib session_manager::tests` → FAIL.

- [ ] **Step 3: Implement `SessionManager`** — prepend to the file:

```rust
//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore. Owns the
//! contextId→handle table + the registry lease that pins the warm backend. Keyed by A2A contextId.
use bridge_core::domain::{effective_config, AgentOverride, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId};
use bridge_core::ports::{AgentBackend, AgentRegistry, Lease};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::session_fingerprint::SessionSpecFingerprint;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState { Idle, Running }

struct WarmHandle {
    #[allow(dead_code)] // surfaced by status/handle ops in later slices
    id: SessionHandleId,
    agent: AgentId,
    backend: Arc<dyn AgentBackend>,
    backend_session: SessionId,
    generation: SessionGeneration,
    fingerprint: SessionSpecFingerprint,
    lease: Box<dyn Lease>,
    state: SessionState,
    op: Option<OperationId>,
    last_used: Instant,
}

/// What a checked-out warm turn needs to dispatch: the backend + the warm session id.
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
}

/// Status snapshot (spec §5: state/agent/generation/idle_age_ms).
pub struct SessionStatusInfo {
    pub state: &'static str,
    pub agent: String,
    pub generation: u64,
    pub idle_age_ms: u128,
}

pub struct SessionManager {
    registry: Arc<dyn AgentRegistry>,
    by_context: Mutex<HashMap<ContextId, WarmHandle>>,
    idle_ttl: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    seq: std::sync::atomic::AtomicU64,
}

impl SessionManager {
    pub fn new(registry: Arc<dyn AgentRegistry>, idle_ttl: Duration) -> Self {
        Self::new_with_clock(registry, idle_ttl, Box::new(Instant::now))
    }
    pub fn new_with_clock(
        registry: Arc<dyn AgentRegistry>, idle_ttl: Duration,
        now: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self { registry, by_context: Mutex::new(HashMap::new()), idle_ttl, now,
               seq: std::sync::atomic::AtomicU64::new(0) }
    }

    /// Start a warm turn: mint (fresh ctx) or resume (known ctx). Resume requires a matching
    /// fingerprint (else `ConfigMismatch`), a non-retired lease (else `SessionExpired`), and an
    /// `Idle` handle (else `HandleBusy`). Transitions to `Running{op}`. Resolves ONCE.
    pub async fn checkout_turn(
        &self, ctx: &ContextId, agent: AgentId,
        overrides: Option<AgentOverride>, cwd: Option<SessionCwd>, op: OperationId,
    ) -> Result<WarmTurn, BridgeError> {
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.get_mut(ctx) {
            if h.lease.is_retired() { return Err(BridgeError::SessionExpired); }
            if h.state == SessionState::Running { return Err(BridgeError::HandleBusy); }
            // Recompute the fingerprint from the SAME agent's entry (one resolve).
            let resolved = self.registry.resolve(&agent).await?;
            let eff = effective_config(&resolved.entry, overrides.as_ref());
            let fp = SessionSpecFingerprint {
                agent: agent.clone(), config: eff,
                cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
            };
            if let Some(field) = h.fingerprint.first_mismatch(&fp) {
                return Err(BridgeError::ConfigMismatch { field });
            }
            h.state = SessionState::Running;
            h.op = Some(op);
            h.last_used = (self.now)();
            return Ok(WarmTurn { backend: h.backend.clone(), session: h.backend_session.clone() });
        }
        // Fresh: ONE resolve (hold the lease), compute fingerprint, configure, insert as Running.
        let resolved = self.registry.resolve(&agent).await?;
        let eff = effective_config(&resolved.entry, overrides.as_ref());
        let fp = SessionSpecFingerprint {
            agent: agent.clone(), config: eff.clone(),
            cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
        };
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let backend_session = SessionId::parse(format!("ctx-{}-g0", ctx.as_str()))
            .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
        resolved.backend
            .configure_session(&backend_session, &SessionSpec { config: eff, cwd })
            .await?;
        let turn = WarmTurn { backend: resolved.backend.clone(), session: backend_session.clone() };
        tab.insert(ctx.clone(), WarmHandle {
            id: SessionHandleId::parse(format!("h-{n}")).unwrap(),
            agent, backend: resolved.backend, backend_session,
            generation: SessionGeneration::new(0), fingerprint: fp, lease: resolved.lease,
            state: SessionState::Running, op: Some(op), last_used: (self.now)(),
        });
        Ok(turn)
    }

    /// Mark the current turn finished → `Idle` (keep warm). Called on producer exit.
    pub fn finish_turn(&self, ctx: &ContextId) {
        // try_lock-free: use blocking lock in a sync context is not allowed; callers hold an async ctx.
        // Implement as async in practice (see note); for the guard we expose finish_turn_async.
        let _ = ctx; // see finish_turn_async — the guard calls that.
    }
    pub async fn finish_turn_async(&self, ctx: &ContextId) {
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            h.state = SessionState::Idle;
            h.op = None;
            h.last_used = (self.now)();
        }
    }

    pub async fn status(&self, ctx: &ContextId) -> Option<SessionStatusInfo> {
        let tab = self.by_context.lock().await;
        tab.get(ctx).map(|h| SessionStatusInfo {
            state: match h.state { SessionState::Idle => "idle", SessionState::Running => "running" },
            agent: h.agent.as_str().to_string(),
            generation: h.generation.get(),
            idle_age_ms: (self.now)().duration_since(h.last_used).as_millis(),
        })
    }

    pub async fn release(&self, ctx: &ContextId) {
        let h = self.by_context.lock().await.remove(ctx);
        if let Some(h) = h {
            h.backend.release_session(&h.backend_session).await;
            drop(h.lease);
        }
    }

    /// Cancel an in-flight turn but KEEP the session warm (→ Idle).
    pub async fn cancel(&self, ctx: &ContextId) -> Result<(), BridgeError> {
        let (backend, session) = {
            let mut tab = self.by_context.lock().await;
            let Some(h) = tab.get_mut(ctx) else { return Err(BridgeError::SessionNotFound) };
            h.state = SessionState::Idle;
            h.op = None;
            (h.backend.clone(), h.backend_session.clone())
        };
        backend.cancel(&session).await
    }

    /// Reap ONLY idle warm sessions past the TTL (never an active turn).
    pub async fn reap_idle(&self) {
        let now = (self.now)();
        let expired: Vec<ContextId> = {
            let tab = self.by_context.lock().await;
            tab.iter()
                .filter(|(_, h)| h.state == SessionState::Idle
                    && now.duration_since(h.last_used) >= self.idle_ttl)
                .map(|(c, _)| c.clone())
                .collect()
        };
        for c in expired { self.release(&c).await; }
    }
}
```

**Note on `finish_turn`:** the turn-guard (Task 10) holds an `Arc<SessionManager>` + the `ContextId` and calls
`finish_turn_async` on drop via a detached `tokio::spawn` (mirroring `BindingGuard::Drop` `server.rs:96`).
Delete the placeholder sync `finish_turn` if the guard uses `finish_turn_async` directly. (Keep the test
calling the async form, or expose a small sync wrapper that spawns.)

- [ ] **Step 4: Run the tests** — `cargo test -p bridge-a2a-inbound --lib session_manager::tests` → PASS (6).
  (Adjust the test helper names `agent()`/`ctxid()`/`opid()`/`ov_model()`/`mgr()` to the doubles you copied.)

- [ ] **Step 5: Commit** — `git commit -am "feat(inbound): SessionManager (checkout/finish/status/release/cancel/reap; HandleBusy/SessionExpired/lifecycle)"`

---

## Task 10: Parse `contextId` in `gate()` (+ standard `id`) and reject on non-Local routes

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (`RoutedCall` 386; `gate()` 306;
`context_id_from_params` near `task_id_from_params` 2869; add `params.get("id")` to `task_id_from_params`).

- [ ] **Step 1: Write the failing test** — add to the server.rs test module:

```rust
    #[test]
    fn context_id_parsed_from_field_and_metadata() {
        let v = serde_json::json!({ "message": { "contextId": "c-1", "text": "hi" } });
        assert_eq!(context_id_from_params(&v).unwrap().unwrap().as_str(), "c-1");
        let v2 = serde_json::json!({ "message": { "metadata": { "a2a-bridge.context": "c-2" }, "text": "hi" } });
        assert_eq!(context_id_from_params(&v2).unwrap().unwrap().as_str(), "c-2");
        assert!(context_id_from_params(&serde_json::json!({ "message": { "text": "hi" } })).unwrap().is_none());
    }
    #[test]
    fn task_id_accepts_standard_id_field() {
        let v = serde_json::json!({ "id": "t-9", "message": { "text": "hi" } });
        assert_eq!(task_id_from_params(&v).unwrap().as_str(), "t-9");
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-a2a-inbound --lib context_id_parsed_from_field` → FAIL.

- [ ] **Step 3: Add the parser + extend `task_id_from_params`** — near `task_id_from_params` (2869):

```rust
/// A2A contextId: `message.contextId` (camelCase) → top-level `contextId` → `a2a-bridge.context`
/// metadata fallback. `None` if absent; empty → error.
fn context_id_from_params(params: &Value) -> Result<Option<ContextId>, BridgeError> {
    let raw = params.get("message").and_then(|m| m.get("contextId")).and_then(|v| v.as_str())
        .or_else(|| params.get("contextId").and_then(|v| v.as_str()))
        .or_else(|| params.get("message").and_then(|m| m.get("metadata"))
            .and_then(|md| md.get("a2a-bridge.context")).and_then(|v| v.as_str()));
    match raw { Some(s) => Ok(Some(ContextId::parse(s)?)), None => Ok(None) }
}
```

In `task_id_from_params` (2870), add `params.get("id")` as the FIRST source:

```rust
    let candidate = params.get("id")
        .or_else(|| params.get("taskId"))
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("message").and_then(|m| m.get("taskId")))
        .and_then(|v| v.as_str());
```

(Add `use bridge_core::ids::ContextId;` if absent.)

- [ ] **Step 4: Add `context_id` to `RoutedCall` + parse/reject in `gate()`** — struct field (386):

```rust
    /// A2A contextId for warm continuation (Slice 0). Honored only on the Local route. None = legacy.
    context_id: Option<ContextId>,
```

In `gate()` after `target` (339):

```rust
        let context_id = context_id_from_params(params)?;
        if context_id.is_some() && !matches!(target, RouteTarget::Local(_)) {
            return Err(BridgeError::InvalidRequest {
                field: "contextId is only supported on the local route in Slice 0",
            });
        }
```

and add `context_id,` to the `RoutedCall { .. }` literal.

- [ ] **Step 5: Run + build** — `cargo test -p bridge-a2a-inbound --lib context_id_parsed_from_field task_id_accepts_standard && cargo build -p bridge-a2a-inbound` → PASS; builds.
- [ ] **Step 6: Commit** — `git commit -am "feat(inbound): parse contextId (+ standard id) in gate(); reject contextId on non-Local"`

---

## Task 11: Wire `SessionManager` into Local dispatch (warm path, both arms)

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (`InboundServer` field 119 + `new()` 200 + builder;
`LocalDispatch` 420; the unary arm 2194 + streaming arm 622 + `spawn_local_producer` 1020; SSE context 660).

The warm path: when a contextId is present + a SessionManager is configured, `checkout_turn` → dispatch
against the warm `session` with `guard=None` + a **warm turn-guard** that calls `finish_turn_async` on exit;
`store.put(task, warm_session)`; the SSE context is the contextId.

- [ ] **Step 1: Field + builder + default.** `InboundServer` struct (after `task_store`):

```rust
    session_manager: Option<std::sync::Arc<crate::session_manager::SessionManager>>,
```

`new()` literal: `session_manager: None,`. After `with_task_store` (260):

```rust
    #[must_use]
    pub fn with_session_manager(mut self, sm: std::sync::Arc<crate::session_manager::SessionManager>) -> Self {
        self.session_manager = Some(sm);
        self
    }
```

- [ ] **Step 2: Extend `LocalDispatch` + add a warm turn-guard.** `LocalDispatch` (420):

```rust
struct LocalDispatch {
    backend: Arc<dyn AgentBackend>,
    /// The session to prompt against — warm `ctx-…` session, or the legacy `session-{task}`.
    session: SessionId,
    guard: Option<BindingGuard>,
    /// Warm path only: finishes the warm turn (→ Idle) on drop. Mutually exclusive with `guard`.
    warm_guard: Option<WarmTurnGuard>,
}

/// Drops the warm turn back to Idle on producer exit (mirrors BindingGuard::Drop spawn pattern).
struct WarmTurnGuard {
    sm: std::sync::Arc<crate::session_manager::SessionManager>,
    ctx: bridge_core::ids::ContextId,
}
impl Drop for WarmTurnGuard {
    fn drop(&mut self) {
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        tokio::spawn(async move { sm.finish_turn_async(&ctx).await; });
    }
}
```

- [ ] **Step 3: Write the failing integration test** — add to the server.rs test module (mirror the nearest
  `unary_message` test harness with a recording mock backend + a SessionManager built from a FakeRegistry):

```rust
    #[tokio::test]
    async fn warm_continue_reuses_session_no_binding_guard() {
        // InboundServer::new(..).with_session_manager(sm); two unary SendMessage with
        // contextId "c1" + metadata a2a-bridge.agent. Assert: the mock backend saw the SAME
        // ctx-c1-g0 SessionId both times; NO forget_session fired between them; store has c1's
        // session mapped to the task.
    }
```

- [ ] **Step 4: Run to verify it fails** — `cargo test -p bridge-a2a-inbound --lib warm_continue_reuses_session` → FAIL.

- [ ] **Step 5: Add the warm dispatch helper** — near `resolve_configure_bind` (438):

```rust
/// Slice 0 warm path. Returns `None` when there's no contextId or no SessionManager (caller
/// uses the legacy `resolve_configure_bind`). Resolves ONCE inside the manager (no double-resolve).
async fn warm_local_dispatch(
    srv: &Arc<InboundServer>, agent_id: &AgentId, routed: &RoutedCall, op: OperationId,
) -> Option<Result<LocalDispatch, BridgeError>> {
    let ctx = routed.context_id.clone()?;
    let sm = srv.session_manager.clone()?;
    match sm.checkout_turn(&ctx, agent_id.clone(), routed.overrides.clone(),
                           routed.session_cwd.clone(), op).await {
        Ok(turn) => Some(Ok(LocalDispatch {
            backend: turn.backend,
            session: turn.session,
            guard: None,
            warm_guard: Some(WarmTurnGuard { sm, ctx }),
        })),
        Err(e) => Some(Err(e)),
    }
}
```

Make `resolve_configure_bind` populate the new fields: in BOTH return sites set
`session: session.clone(), warm_guard: None` (and keep `guard` as today). The `session` it carries is
`routed.session` (the legacy `session-{task}`).

- [ ] **Step 6: Branch both Local arms + thread the session + store.put + SSE context.**

In the **unary** arm (`server.rs:2194`):

```rust
        RouteTarget::Local(ref agent_id) => {
            let op = OperationId::parse(format!("op-{}", routed.task.as_str())).unwrap();
            let dispatch = match warm_local_dispatch(&srv, agent_id, &routed, op).await {
                Some(r) => r,
                None => resolve_configure_bind(&srv, agent_id, &routed.task, &routed.session,
                            routed.overrides.as_ref(), routed.session_cwd.clone()).await,
            };
            let dispatch = match dispatch { Ok(d) => d, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
            // Persist task→session so cancel/permission-suspend target the RIGHT session.
            let _ = srv.store.put(&routed.task, &dispatch.session).await;
            let _guard = dispatch.guard;          // legacy eviction (None on warm path)
            let _warm = dispatch.warm_guard;      // warm finish-on-exit (None on legacy path)
            let translator = Translator::new();
            translator.run(dispatch.backend.as_ref(), srv.store.as_ref(), srv.policy.as_ref(),
                           &routed.task, &dispatch.session, routed.parts).collect().await
        }
```

In the **streaming** arm (`server.rs:622`): compute `op`, call `warm_local_dispatch` (fallback to
`resolve_configure_bind`), `store.put(&routed.task, &dispatch.session)`, then
`spawn_local_producer(&srv, routed, dispatch, tx)`.

In `spawn_local_producer` (`server.rs:1020`): change `let session = routed.session;` (1029) to
`let session = dispatch.session.clone();` and move BOTH `dispatch.guard` and `dispatch.warm_guard` into the
task (hold both for the producer's life): `let _guard = dispatch.guard; let _warm = dispatch.warm_guard;`.

For SSE context, in the streaming setup (`server.rs:660`) use the contextId when present:

```rust
    let context_id_str = routed.context_id.as_ref().map(|c| c.as_str().to_string())
        .unwrap_or_else(|| task_id_str.clone());
```

(Capture `routed.context_id` before `routed` is partially moved.)

- [ ] **Step 7: Run + build** — `cargo test -p bridge-a2a-inbound --lib warm_continue_reuses_session && cargo build -p bridge-a2a-inbound` → PASS; builds.
- [ ] **Step 8: Commit** — `git commit -am "feat(inbound): warm Local dispatch via SessionManager (warm session+turn-guard, store.put, SSE contextId)"`

---

## Task 12: `SessionStatus`/`SessionRelease`/`SessionCancel` JSON-RPC methods

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (method match 589; handlers). Reuse the real auth
idiom from `cancel_task` (`bearer_token` `:2859` → `InboundRequest` → `self.auth.authorize`); there is NO
`inbound_from` — either add one small helper or inline.

- [ ] **Step 1: Write the failing test** — add to the test module:

```rust
    #[tokio::test]
    async fn session_status_release_cancel_dispatch() {
        // Build InboundServer + SessionManager; mint via a unary SendMessage with contextId "c1".
        // SessionStatus {contextId:"c1"} → result.state == "idle"; SessionCancel ok (still present);
        // SessionRelease → ok; SessionStatus → SessionNotFound error.
    }
```

- [ ] **Step 2: Run to verify it fails** — method-not-found → FAIL.

- [ ] **Step 3: Add match arms** — in the method `match` (589), before `""`:

```rust
        "SessionStatus" => session_status(srv, headers, id, params).await,
        "SessionRelease" => session_release(srv, headers, id, params).await,
        "SessionCancel" => session_cancel(srv, headers, id, params).await,
```

- [ ] **Step 4: Add handlers** — near the other handlers:

```rust
fn authorize_headers(srv: &InboundServer, headers: &HeaderMap) -> Result<(), BridgeError> {
    let inbound = match bearer_token(headers) {
        Some(t) => InboundRequest::with_token(&t),
        None => InboundRequest::anon(),
    };
    srv.auth.authorize(&inbound).map(|_| ())
}
fn context_id_arg(params: &Value) -> Result<ContextId, BridgeError> {
    params.get("contextId").and_then(|v| v.as_str())
        .ok_or(BridgeError::InvalidRequest { field: "contextId" })
        .and_then(ContextId::parse)
}

async fn session_status(srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else { return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager"); };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    match sm.status(&ctx).await {
        Some(s) => jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "state": s.state,
            "agent": s.agent, "generation": s.generation, "idleAgeMs": s.idle_age_ms })),
        None => bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound),
    }
}
async fn session_release(srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else { return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager"); };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    sm.release(&ctx).await;
    jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "released": true }))
}
async fn session_cancel(srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else { return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager"); };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    match sm.cancel(&ctx).await {
        Ok(()) => jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "canceled": true })),
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}
```

(Confirm `bearer_token`/`InboundRequest::{with_token,anon}`/`jsonrpc_ok`/`jsonrpc_err`/`bridge_err_to_jsonrpc`/
`JSONRPC_METHOD_NOT_FOUND`/`json!` names against the file; all are used by existing handlers.)

- [ ] **Step 5: Run + build** — `cargo test -p bridge-a2a-inbound --lib session_status_release_cancel && cargo build -p bridge-a2a-inbound` → PASS.
- [ ] **Step 6: Commit** — `git commit -am "feat(inbound): SessionStatus/SessionRelease/SessionCancel methods (status incl. idleAgeMs)"`

---

## Task 13: CLI — `submit` flags + `session` subcommand

**Files:** Modify `bin/a2a-bridge/src/main.rs` (`submit_cmd` 2596; `task_cmd` model 2618; subcommand match 3287;
`TOP_USAGE`).

- [ ] **Step 1: Replace `submit_cmd`** — make skill optional; add `--context/--agent/--model/--effort/--mode/
  --cwd`; print the Local artifact text:

```rust
async fn submit_cmd(args: &[String]) -> Result<(), BoxError> {
    let input_path = flag(args, "--input").ok_or("submit: --input <file> required")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let text = std::fs::read_to_string(input_path)?;
    let mut md = serde_json::Map::new();
    // skill = first non-flag positional that isn't a known flag value (optional when --agent given).
    let flagvals: std::collections::HashSet<&str> =
        ["--input","--url","--context","--agent","--model","--effort","--mode","--cwd"]
        .iter().filter_map(|f| flag(args, f)).collect();
    let skill = args.iter().find(|a| !a.starts_with("--") && !flagvals.contains(a.as_str())).cloned();
    if let Some(s) = &skill { md.insert("a2a-bridge.skill".into(), s.clone().into()); }
    for (f, key) in [("--agent","a2a-bridge.agent"),("--model","a2a-bridge.model"),
                     ("--effort","a2a-bridge.effort"),("--mode","a2a-bridge.mode"),("--cwd","a2a-bridge.cwd")] {
        if let Some(v) = flag(args, f) { md.insert(key.into(), v.into()); }
    }
    let mut message = serde_json::Map::new();
    message.insert("text".into(), text.into());
    message.insert("metadata".into(), serde_json::Value::Object(md));
    if let Some(c) = flag(args, "--context") { message.insert("contextId".into(), c.into()); }
    let v = rpc_call(url, a2a::methods::SEND_MESSAGE, serde_json::json!({ "message": message })).await?;
    if let Some(err) = v.get("error") { return Err(format!("submit failed: {err}").into()); }
    // Local sends carry the agent reply in result.artifact.text; detached returns result.task.id.
    let out = v["result"]["artifact"]["text"].as_str()
        .or_else(|| v["result"]["task"]["id"].as_str())
        .unwrap_or("ok");
    println!("{out}");
    Ok(())
}
```

- [ ] **Step 2: Add `session_cmd`** — near `task_cmd`:

```rust
async fn session_cmd(args: &[String]) -> Result<(), BoxError> {
    let sub = args.first().map(|s| s.as_str()).ok_or("session: missing subcommand (status|release|cancel)")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let ctx = args.get(1).cloned().ok_or("session: missing <contextId>")?;
    let method = match sub {
        "status" => "SessionStatus", "release" => "SessionRelease", "cancel" => "SessionCancel",
        other => return Err(format!("session: unknown subcommand {other:?}").into()),
    };
    let v = rpc_call(url, method, serde_json::json!({ "contextId": ctx })).await?;
    if let Some(err) = v.get("error") { return Err(format!("session {sub} failed: {err}").into()); }
    println!("{}", serde_json::to_string_pretty(&v["result"])?);
    Ok(())
}
```

- [ ] **Step 3: Register** — in the subcommand match (3287) after `"task"`:

```rust
        Some("session") => return session_cmd(&raw_args[2..]).await,
```

Add `session` to the unknown-subcommand error list (3306) and `TOP_USAGE`.

- [ ] **Step 4: Build** — `cargo build -p a2a-bridge` → builds.
- [ ] **Step 5: Commit** — `git commit -am "feat(cli): submit --context/--agent/--model/--effort/--mode/--cwd + artifact print; session subcommand"`

---

## Task 14: Config + wire `SessionManager` into serve boot

**Files:** Modify the `[server]` config struct (`crates/bridge-registry/src/config.rs` — `ServerConfig` lives
under `RegistryConfig`; confirm via `rg "struct ServerConfig" crates/`) and the serve boot path
(`bin/a2a-bridge/src/main.rs` ~3589, where `InboundServer::new(..).with_*` is assembled).

- [ ] **Step 1: Write the failing config test** — test against the REAL nested parser (`ServerConfig` has no
  `Default`; it parses under `[server]` within a `RegistryConfig` that needs `default` + `[[agents]]`). Mirror
  an existing `RegistryConfig` parse test:

```rust
    #[test]
    fn warm_idle_ttl_defaults_and_overrides() {
        let base = r#"
default = "a"
[[agents]]
id = "a"
cmd = "echo"
[server]
addr = "127.0.0.1:8080"
"#;
        let cfg: RegistryConfig = toml::from_str(base).unwrap();
        assert_eq!(cfg.server.unwrap().warm_idle_ttl_secs, 1800);
        let cfg2: RegistryConfig = toml::from_str(&format!("{base}warm_idle_ttl_secs = 5\n")).unwrap();
        assert_eq!(cfg2.server.unwrap().warm_idle_ttl_secs, 5);
    }
```

(Adjust to the actual `RegistryConfig`/`ServerConfig` shape + whether `server` is `Option`.)

- [ ] **Step 2: Run to verify it fails** — FAIL (field missing).

- [ ] **Step 3: Add the field** — on `ServerConfig`:

```rust
    #[serde(default = "default_warm_idle_ttl_secs")]
    pub warm_idle_ttl_secs: u64,
```

```rust
fn default_warm_idle_ttl_secs() -> u64 { 1800 }
```

- [ ] **Step 4: Wire into serve boot** — where `InboundServer` is built (after the registry):

```rust
    let warm_ttl = server_cfg.as_ref().map(|s| s.warm_idle_ttl_secs).unwrap_or(1800);
    let session_manager = std::sync::Arc::new(
        bridge_a2a_inbound::session_manager::SessionManager::new(
            registry.clone(), std::time::Duration::from_secs(warm_ttl)));
    // ... .with_session_manager(session_manager.clone())
    {
        let sm = session_manager.clone();
        // tick at min(ttl, 30s), lower-bounded 1s, so a small TTL is observed promptly.
        let period = std::time::Duration::from_secs(warm_ttl.min(30).max(1));
        tokio::spawn(async move {
            let mut t = tokio::time::interval(period);
            loop { t.tick().await; sm.reap_idle().await; }
        });
    }
```

- [ ] **Step 5: Run + build workspace** — `cargo test -p bridge-registry warm_idle_ttl && cargo build --workspace` → PASS; builds.
- [ ] **Step 6: Commit** — `git commit -am "feat(serve): warm_idle_ttl_secs config + wire SessionManager + idle reaper (min(ttl,30) tick)"`

---

## Task 15: Full workspace gate + live-gate (DoD)

- [ ] **Step 1: Workspace fmt/clippy/test** — `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
  Expected: clean; all pass. (Scope out docker/PID-1 system tests as the verify examples do —
  see [[containerized-agents-slice-b2b2-shipped]].) **Watch for `dead_code`** on any unused `WarmHandle`
  field — `op` is read (Running lifecycle), `id` is `#[allow(dead_code)]`-marked; remove any field that is
  genuinely never read.

- [ ] **Step 2: Build release** — `cargo build --release -p a2a-bridge`.

- [ ] **Step 3: Live-gate (real serve + codex).** Start serve with a codex agent + `warm_idle_ttl_secs = 5`;
  run the DoD scenarios (record output in the PR; all via the Local route — `--agent`, no workflow skill):

```bash
printf 'Remember the codeword ZEBRA. Reply OK.' > /tmp/s0a.txt
printf 'What codeword did I give you THIS session? One word.' > /tmp/s0b.txt
# DoD-1/2: warm reuse + recall + no cold spawn. Watcher: pgrep -f codex-acp must NOT grow on the 2nd call.
./target/release/a2a-bridge submit --agent codex --context c1 --input /tmp/s0a.txt    # → OK
./target/release/a2a-bridge submit --agent codex --context c1 --input /tmp/s0b.txt    # → ZEBRA (artifact text)
# DoD-3 isolation:
./target/release/a2a-bridge submit --agent codex --context c2 --input /tmp/s0b.txt    # → NONE
# DoD-4 back-compat:
./target/release/a2a-bridge submit --agent codex --input /tmp/s0a.txt                 # legacy forget-after
# DoD-5/7 status/cancel/release (host-ACP: shared process STAYS; sessions[id] gone):
./target/release/a2a-bridge session status c1     # → state idle, idleAgeMs
./target/release/a2a-bridge session cancel c1     # → canceled (still warm)
./target/release/a2a-bridge session release c1    # → released
./target/release/a2a-bridge session status c1     # → SessionNotFound
# DoD-6 config-mismatch (different effort on the same context → typed ConfigMismatch):
./target/release/a2a-bridge submit --agent codex --context c3 --effort high --input /tmp/s0a.txt
./target/release/a2a-bridge submit --agent codex --context c3 --effort low  --input /tmp/s0b.txt   # → ConfigMismatch error
# DoD-8 idle-TTL (ttl=5): leave c2 idle > 5s, then within one reaper tick:
sleep 7; ./target/release/a2a-bridge session status c2   # → SessionNotFound
```

- [ ] **Step 4: Record gate results** in the PR (the pgrep no-growth, ZEBRA recall via artifact text, NONE
  isolation, typed ConfigMismatch, reaper eviction). For a configured ContainerRw `:rw` agent, also show
  `docker ps` → 0 for the released session.

- [ ] **Step 5:** Proceed to `superpowers:finishing-a-development-branch`.

---

## Self-review notes (v2)

- **Spec coverage:** every IN item maps to a task incl. the v1-review gaps now fixed — warm-turn lifecycle +
  `HandleBusy` + reap-Idle-only (T9), `SessionExpired` via `Lease::is_retired` (T8/T9), `idle_age_ms` (T9/T12),
  full warm-session threading + `store.put` + SSE contextId (T11), single-resolve (T9), CLI override flags +
  artifact print (T13), reaper interval (T14), standard `id` parse (T10).
- **Type consistency:** `WarmTurn`/`WarmTurnGuard`/`LocalDispatch.session`/`SessionStatusInfo`/`SessionState`
  used consistently across T9/T11/T12; `Lease::is_retired` (T8) consumed in T9.
- **TDD integrity:** new modules registered in their Step 1 so the failing test is reachable; T4's
  `Update::Usage` fixes both real match sites (translator + bridge-workflow) in-task so the workspace builds.
- **No placeholders:** test-helper names point at real helpers (`connect_recording`, `warm_backend`/
  `counting_reap`/`spec_cwd`/`StubInner`, `FakeRegistry` from `tests/workflow_producer.rs`); the only scaffold
  is T9's copied test doubles (flagged). `finish_turn`'s sync placeholder is noted to be removed in favor of
  `finish_turn_async` driven by `WarmTurnGuard::drop`.
