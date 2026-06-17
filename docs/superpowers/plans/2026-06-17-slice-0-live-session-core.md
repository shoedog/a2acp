# Slice 0 — Live Session Core (warm continue) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make a bridge-driven agent warm across tasks — a 2nd A2A message on the same `contextId` reuses the
same warm ACP session (no cold spawn, context intact); no `contextId` = today's forget-after behavior.

**Architecture:** A new serve-side `SessionManager` (sibling to the registry + TaskStore) holds a warm
`WarmHandle` (backend + registry lease + frozen config fingerprint) keyed by A2A `contextId`. `gate()` parses
the contextId into `RoutedCall`; the async dispatch layer consults the manager on the `RouteTarget::Local`
path (dispatching with `guard=None` so the existing per-task `BindingGuard` never forgets the warm session).
Plus the minimal real `OrchEvent`/`OrchResult` DTOs, an `Update::Usage` variant + a `release_session` backend
method, and `SessionStatus`/`SessionRelease`/`SessionCancel` JSON-RPC methods + CLI.

**Tech Stack:** Rust workspace (bridge-core, bridge-acp, bridge-container, bridge-api, bridge-a2a-inbound,
bin/a2a-bridge); async-trait; tokio; serde; the external `a2a-lf` 0.3.0 crate (PascalCase wire methods).

**Spec:** `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` (v2, dual-reviewed).
**Slicing authority:** `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` (Slice 0).

**Key grounded facts the plan relies on (verbatim-verified):**
- Wire methods are **PascalCase** from `a2a-lf` (`SendMessage`, `GetTask`, …) — new methods are
  `SessionStatus`/`SessionRelease`/`SessionCancel` as bare string-literal arms in the `server.rs` match.
- `gate()` is `fn gate(&self, headers, params) -> Result<RoutedCall>` (`server.rs:306`) — sync but has
  `&self`. It only *parses* contextId; the async SessionManager lookup happens in the Local dispatch arm.
- `resolve_configure_bind` returns `LocalDispatch { backend, guard: Option<BindingGuard> }`
  (`server.rs:420/438`); the binding-reuse branch already returns `guard: None`. The warm path mirrors that.
- `ContainerRwBackend::forget_session` is stash-only; warm containers are reaped only by `retire_warm()`
  (drain-all) — Slice 0 adds a per-session `release_warm`.
- `Update` (`ports.rs:21`) has no wildcard → adding `Usage` is a breaking exhaustiveness change; every
  `match` on `Update` must gain a `Usage` arm in the same task.
- `ids.rs` macros are String-only with a `parse` arm; `SessionGeneration(u64)` is hand-written.

**Naming note:** the spec calls these `session/status` etc.; the wire/CLI realize them as PascalCase methods
`SessionStatus`/`SessionRelease`/`SessionCancel` + CLI `session status|release|cancel` (intent preserved).

---

## Task 1: Core id newtypes

**Files:**
- Modify: `crates/bridge-core/src/ids.rs` (after line 29, the existing `id_newtype!` invocations)
- Test: same file (the macro file has inline `#[cfg(test)]` patterns; add a small test module if absent)

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

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core --lib slice0_id_tests`
Expected: FAIL — `cannot find type SessionHandleId` / `OperationId` / `ContextId` / `SessionGeneration`.

- [ ] **Step 3: Add the newtypes** — in `crates/bridge-core/src/ids.rs` after `id_newtype!(AgentId);` (line 29):

```rust
// Slice 0 (orchestration) ids.
id_newtype!(SessionHandleId);
id_newtype!(OperationId);
id_newtype!(ContextId);

/// A warm session's context generation. Bumped by reset (Slice 3); 0 in Slice 0.
/// Hand-written (the `id_newtype!` macros are String-only) — generations are
/// compared/incremented so we add `Copy`/`Ord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct SessionGeneration(pub u64);

impl SessionGeneration {
    pub fn new(n: u64) -> Self {
        Self(n)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bridge-core --lib slice0_id_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/ids.rs
git commit -m "feat(core): add Slice-0 orch ids (SessionHandleId/OperationId/ContextId/SessionGeneration)"
```

---

## Task 2: Minimal `OrchEvent`/`OrchResult` DTOs + stop-reason mapping

**Files:**
- Create: `crates/bridge-core/src/orch.rs`
- Modify: `crates/bridge-core/src/lib.rs` (add `pub mod orch;`)
- Test: in `orch.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** — create `crates/bridge-core/src/orch.rs` with ONLY the test first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orch_event_roundtrips_with_internal_kind_tag() {
        let ev = OrchEvent {
            v: ORCH_V,
            seq: 3,
            ts_ms: 100,
            operation_id: crate::ids::OperationId::parse("op-1").unwrap(),
            kind: OrchEventKind::Usage {
                usage: UsageSnapshot { used: Some(10), size: Some(200), cost: None, at_ms: 100 },
            },
        };
        let j = serde_json::to_value(&ev).unwrap();
        assert_eq!(j["kind"], "usage");
        assert_eq!(j["used"], 10);
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 3);
    }

    #[test]
    fn usage_cost_carries_amount_and_currency() {
        let c = UsageCost { amount: 1.5, currency: "USD".into() };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["amount"], 1.5);
        assert_eq!(j["currency"], "USD");
    }

    #[test]
    fn terminal_status_from_each_stop_reason() {
        assert!(matches!(TerminalStatus::from_stop_reason("end_turn"), TerminalStatus::Completed));
        assert!(matches!(TerminalStatus::from_stop_reason("cancelled"), TerminalStatus::Canceled));
        assert!(matches!(TerminalStatus::from_stop_reason("refusal"), TerminalStatus::Failed { .. }));
        assert!(matches!(TerminalStatus::from_stop_reason("max_tokens"), TerminalStatus::Failed { .. }));
        assert!(matches!(TerminalStatus::from_stop_reason("max_turn_requests"), TerminalStatus::Failed { .. }));
        // Unknown stop reason → Failed (future-proof, per spec P-4).
        assert!(matches!(TerminalStatus::from_stop_reason("weird"), TerminalStatus::Failed { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core --lib orch::tests`
Expected: FAIL — module/types don't exist (and `pub mod orch;` not yet in lib.rs → compile error).

- [ ] **Step 3: Write the DTOs** — prepend to `crates/bridge-core/src/orch.rs` (above the test module):

```rust
//! Slice 0 minimal orchestration DTOs (bridge-owned, versioned, Ser+De).
//! Rich variants (Plan/ToolCall/config/mode/commands) + the `session`/`source` envelope
//! fields are deferred (S6/S7); the versioned + `#[serde(flatten)] kind` envelope makes
//! those additions non-breaking.

use crate::ids::OperationId;
use serde::{Deserialize, Serialize};

pub const ORCH_V: u16 = 1;

/// ACP usage cost is `{amount, currency}` — NOT guaranteed USD.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageCost {
    pub amount: f64,
    pub currency: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub used: Option<u64>,
    pub size: Option<u64>,
    pub cost: Option<UsageCost>,
    pub at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchEvent {
    pub v: u16,
    pub seq: i64,
    pub ts_ms: i64,
    pub operation_id: OperationId,
    #[serde(flatten)]
    pub kind: OrchEventKind,
}

/// Struct variants only — serde's internally-tagged enums reject bare tuple variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    Progress { text: String },
    Usage { #[serde(flatten)] usage: UsageSnapshot },
    Terminal { status: TerminalStatus },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerminalStatus {
    Completed,
    Failed { reason: String },
    Canceled,
}

impl TerminalStatus {
    /// Map an ACP `StopReason` wire string → terminal status (spec P-4).
    /// `end_turn`→Completed; `cancelled`→Canceled; everything else (incl. unknown)→Failed.
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
    pub v: u16,
    pub operation_id: OperationId,
    pub status: TerminalStatus,
    pub wall_clock_ms: u64,
    pub usage: UsageSnapshot,
    pub output: String,
}
```

- [ ] **Step 4: Register the module** — in `crates/bridge-core/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod orch;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p bridge-core --lib orch::tests`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-core/src/orch.rs crates/bridge-core/src/lib.rs
git commit -m "feat(core): minimal OrchEvent/OrchResult/UsageSnapshot DTOs + stop-reason mapping"
```

---

## Task 3: `BridgeError::ConfigMismatch` + `SessionExpired`

**Files:**
- Modify: `crates/bridge-core/src/error.rs` (enum lines 22-62; `disposition()` lines 97-110)
- Test: in `error.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** — add to a test module in `crates/bridge-core/src/error.rs`:

```rust
#[cfg(test)]
mod slice0_error_tests {
    use super::*;

    #[test]
    fn config_mismatch_and_session_expired_reject_request() {
        assert_eq!(
            BridgeError::ConfigMismatch { field: "model" }.disposition(),
            A2aDisposition::RejectRequest
        );
        assert_eq!(BridgeError::SessionExpired.disposition(), A2aDisposition::RejectRequest);
    }

    #[test]
    fn config_mismatch_client_message_is_safe() {
        // No infra detail in the field name → Display is fine to surface.
        assert!(BridgeError::ConfigMismatch { field: "effort" }
            .client_message()
            .contains("effort"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core --lib slice0_error_tests`
Expected: FAIL — `no variant ConfigMismatch` / `SessionExpired`.

- [ ] **Step 3: Add the variants** — in the `BridgeError` enum (after `SessionNotFound`, line 31):

```rust
    #[error("config mismatch: {field}")]
    ConfigMismatch { field: &'static str },
    #[error("session expired")]
    SessionExpired,
```

- [ ] **Step 4: Map them in `disposition()`** — extend the `RejectRequest` arm (line 102):

```rust
            A2aVersionMismatch | InvalidRequest { .. } | TaskNotFound | SessionNotFound
            | ConfigMismatch { .. } | SessionExpired => RejectRequest,
```

(No `client_message()` change needed — neither carries infra detail; the `other => other.to_string()` arm
surfaces the safe `Display`.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p bridge-core --lib slice0_error_tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Verify the whole core crate still builds** (exhaustive `match BridgeError` sites)

Run: `cargo build -p bridge-core`
Expected: builds (if any non-wildcard `match` on `BridgeError` exists, add `ConfigMismatch`/`SessionExpired`
arms it flags — search with `rg "match .*BridgeError|BridgeError::" --type rust` and fix non-exhaustive ones).

- [ ] **Step 7: Commit**

```bash
git add crates/bridge-core/src/error.rs
git commit -m "feat(core): BridgeError::{ConfigMismatch,SessionExpired} + RejectRequest disposition"
```

---

## Task 4: `Update::Usage` variant + `AgentBackend::release_session` trait method

**Files:**
- Modify: `crates/bridge-core/src/ports.rs` (`Update` enum line 21-25; `AgentBackend` trait line 31-55;
  object-safety test ~line 363-390)
- Modify: every `match` on `Update` (find them; at minimum the ACP/translator consumers)
- Test: `ports.rs` object-safety test

- [ ] **Step 1: Add a `Usage` assertion to the object-safety test** — in `ports.rs` test
  `agentbackend_defaults_are_noops_and_object_safe` (~line 363), add after the existing `forget_session` call:

```rust
        // release_session default must be callable through the trait object (Slice 0).
        b.release_session(&crate::ids::SessionId::parse("s").unwrap()).await;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core --lib agentbackend_defaults_are_noops_and_object_safe`
Expected: FAIL — `no method release_session`.

- [ ] **Step 3: Add `Usage` to the `Update` enum** — `ports.rs:21`:

```rust
pub enum Update {
    Text(String),
    Permission(PermissionRequest),
    Usage(crate::orch::UsageSnapshot),
    Done { stop_reason: String },
}
```

- [ ] **Step 4: Add `release_session` to the `AgentBackend` trait** — after `forget_session` (line 50):

```rust
    /// Release a warm session: drop ALL per-session backend state and reap any
    /// per-session resource (e.g. a `:rw` container). Default = `forget_session`
    /// (correct for non-warm/non-process backends). Warm backends override. [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        self.forget_session(session).await;
    }
```

- [ ] **Step 5: Fix every non-wildcard `match Update`** to handle `Usage`.

Run: `cargo build --workspace 2>&1 | rg "non-exhaustive|Update::"` to find them. Expected sites: the ACP
backend's own stream mapping and `bridge-core::translator`. For Slice 0, **`Usage` is dropped/ignored at the
consumer** (plumbing is Slice 2). In `crates/bridge-core/src/translator.rs`, in the `match` over `Update`,
add:

```rust
            Update::Usage(_) => { /* Slice 0: telemetry plumbing deferred to Slice 2 — ignore. */ continue; }
```

(adapt `continue`/`{}` to the surrounding loop shape — the point is: produce no event). Apply the analogous
no-op arm at each other flagged site.

- [ ] **Step 6: Run the build + object-safety test**

Run: `cargo build --workspace && cargo test -p bridge-core --lib agentbackend_defaults_are_noops_and_object_safe`
Expected: workspace builds; the object-safety test PASSES.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(core): Update::Usage variant + AgentBackend::release_session (default=forget_session)"
```

---

## Task 5: `AcpBackend::release_session` override (drop the agent session)

**Files:**
- Modify: `crates/bridge-acp/src/acp_backend.rs` (the `AgentBackend` impl, near `forget_session` line 1805)
- Test: `acp_backend.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** — add to the acp_backend test module:

```rust
    #[tokio::test]
    async fn release_session_removes_both_sessions_and_cfg_entries() {
        let be = AcpBackend::new_for_test(); // use the existing test constructor pattern in this file
        let s = SessionId::parse("ctx-x-g0").unwrap();
        be.configure_session(&s, &SessionSpec::from_config(Default::default())).await.unwrap();
        // Force an AgentSession entry to exist (mirror how other tests seed `sessions`).
        let _ = be.session_entry(&s).await;
        be.release_session(&s).await;
        assert!(be.session_cfg.lock().unwrap().get(&s).is_none(), "cfg stash removed");
        assert!(be.sessions.lock().await.get(&s).is_none(), "agent session removed");
    }
```

(If `new_for_test`/`session_entry` visibility differs, mirror the construction used by the nearest existing
`#[tokio::test]` in this file — e.g. an in-process transport test backend. Keep the two assertions.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-acp --lib release_session_removes_both`
Expected: FAIL — `release_session` falls back to the default (only removes cfg) → the `sessions` assertion fails.

- [ ] **Step 3: Implement the override** — in the `impl AgentBackend for AcpBackend` block, after
  `forget_session` (line 1814):

```rust
    /// Release a warm ACP session: cancel any in-flight turn, drop the agent-side
    /// `AgentSession` (so a later reuse re-mints a fresh `session/new`), and drop the
    /// config stash. Does NOT `retire()` the shared process (warm for serve's lifetime,
    /// shared across all sessions). [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        // Best-effort cancel of an in-flight turn on this session.
        let _ = self.cancel(session).await;
        self.sessions.lock().await.remove(session);
        if let Ok(mut m) = self.session_cfg.lock() {
            m.remove(session);
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bridge-acp --lib release_session_removes_both`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-acp/src/acp_backend.rs
git commit -m "feat(acp): release_session removes the agent session + cfg (keeps the shared process warm)"
```

---

## Task 6: `ContainerRwBackend::release_session` → per-session `release_warm`

**Files:**
- Modify: `crates/bridge-container/src/lib.rs` (warm helpers near `retire_warm` line 412; `AgentBackend` impl
  near `forget_session` line 534)
- Test: `lib.rs` (`#[cfg(test)]` — mirror the existing warm tests using the injectable `ReapFn`)

- [ ] **Step 1: Write the failing test** — add to the bridge-container test module (mirror the existing
  warm-mode tests that count `reap_fn` invocations):

```rust
    #[tokio::test]
    async fn release_session_reaps_only_that_warm_container() {
        // Build a warm ContainerRwBackend with a counting fake ReapFn + fake spawn
        // (reuse the test harness the other warm tests use in this file).
        let (be, reaped_names) = warm_test_backend(); // existing helper pattern
        let s = SessionId::parse("ctx-a-g0").unwrap();
        be.configure_session(&s, &spec_with_cwd("/work")).await.unwrap();
        // Seed a warm entry (drive one prompt via the fake inner, or insert directly as the warm tests do).
        seed_warm_entry(&be, &s).await;
        be.release_session(&s).await;
        assert!(be.warm.lock().await.get(&s).is_none(), "warm entry removed");
        assert_eq!(reaped_names.lock().unwrap().len(), 1, "exactly one container reaped");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-container --lib release_session_reaps_only_that_warm`
Expected: FAIL — default `release_session`→`forget_session` is stash-only; nothing reaped, warm entry remains.

- [ ] **Step 3: Implement `release_warm` + the override** — add the per-session reap (lift the `retire_warm`
  loop body, scoped to one session) near `retire_warm` (line 412):

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

And the trait override in `impl AgentBackend for ContainerRwBackend`, after `forget_session` (line 534):

```rust
    async fn release_session(&self, session: &SessionId) {
        if self.is_warm() {
            self.release_warm(session).await;
        }
        // Always drop the cfg stash (per-turn mode has no warm container to reap).
        self.session_cfg.lock().await.remove(session);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bridge-container --lib release_session_reaps_only_that_warm`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-container/src/lib.rs
git commit -m "feat(container): release_session reaps one warm session's container (per-session release_warm)"
```

---

## Task 7: `SessionSpecFingerprint`

**Files:**
- Create: `crates/bridge-core/src/session_fingerprint.rs`
- Modify: `crates/bridge-core/src/lib.rs` (`pub mod session_fingerprint;`)
- Test: in the new file

- [ ] **Step 1: Write the failing test** — create `crates/bridge-core/src/session_fingerprint.rs`:

```rust
//! Frozen-at-mint fingerprint for warm-session continuation. A `continue` whose
//! recomputed fingerprint differs → typed `ConfigMismatch{field}` (Slice 0; reconcile is Slice 1).

use crate::domain::EffectiveConfig;
use crate::ids::AgentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpecFingerprint {
    pub agent: AgentId,
    pub config: EffectiveConfig,
    /// Canonical cwd string (None = no override). Stored as String to avoid coupling
    /// to SessionCwd's derives; cwd is immutable post-`session/new`.
    pub cwd: Option<String>,
}

impl SessionSpecFingerprint {
    /// The first field that differs (`agent`/`model`/`effort`/`mode`/`cwd`), or `None` if identical.
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
    fn identical_fingerprints_have_no_mismatch() {
        assert_eq!(fp("gpt-5.5", Some("/work")).first_mismatch(&fp("gpt-5.5", Some("/work"))), None);
    }

    #[test]
    fn model_and_cwd_mismatches_are_reported() {
        assert_eq!(fp("gpt-5.5", Some("/work")).first_mismatch(&fp("gpt-5.4", Some("/work"))), Some("model"));
        assert_eq!(fp("gpt-5.5", Some("/work")).first_mismatch(&fp("gpt-5.5", Some("/other"))), Some("cwd"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-core --lib session_fingerprint`
Expected: FAIL — module not registered.

- [ ] **Step 3: Register the module** — add to `crates/bridge-core/src/lib.rs`:

```rust
pub mod session_fingerprint;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bridge-core --lib session_fingerprint`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/session_fingerprint.rs crates/bridge-core/src/lib.rs
git commit -m "feat(core): SessionSpecFingerprint (agent+effective_config+cwd) with first_mismatch"
```

---

## Task 8: `SessionManager` core (the heart of the slice)

**Files:**
- Create: `crates/bridge-a2a-inbound/src/session_manager.rs`
- Modify: `crates/bridge-a2a-inbound/src/lib.rs` (`pub mod session_manager;`)
- Test: in the new file (fake backend + fake clock)

This task is large; build it TDD in sub-steps. The manager is generic over a clock (`fn() -> Instant`-ish)
via an injected `now` closure so TTL is unit-testable without sleeping.

- [ ] **Step 1: Write the failing test** — create `crates/bridge-a2a-inbound/src/session_manager.rs` with the
  test module first (and a `FakeBackend`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, SessionSpec};
    use bridge_core::ids::{AgentId, ContextId};
    use std::sync::Arc;

    // A minimal AgentBackend that records release calls.
    // (Reuse/define a FakeBackend mirroring bridge-core ports.rs test backends.)
    fn mgr() -> SessionManager { /* construct with a fake registry + 5s ttl + manual clock */ unimplemented!() }

    #[tokio::test]
    async fn mint_then_lookup_returns_same_handle() {
        let m = mgr();
        let ctx = ContextId::parse("c1").unwrap();
        let h1 = m.mint_or_resume(&ctx, fp_codex(), spec()).await.unwrap();
        let h2 = m.mint_or_resume(&ctx, fp_codex(), spec()).await.unwrap();
        assert_eq!(h1.backend_session, h2.backend_session, "same warm session reused");
    }

    #[tokio::test]
    async fn config_mismatch_is_typed_error_not_silent() {
        let m = mgr();
        let ctx = ContextId::parse("c1").unwrap();
        m.mint_or_resume(&ctx, fp_codex(), spec()).await.unwrap();
        let err = m.mint_or_resume(&ctx, fp_codex_model("gpt-5.4"), spec()).await.unwrap_err();
        assert!(matches!(err, bridge_core::error::BridgeError::ConfigMismatch { field: "model" }));
    }

    #[tokio::test]
    async fn release_evicts_and_calls_backend_release() {
        let m = mgr();
        let ctx = ContextId::parse("c1").unwrap();
        m.mint_or_resume(&ctx, fp_codex(), spec()).await.unwrap();
        m.release(&ctx).await;
        assert!(m.status(&ctx).await.is_none(), "evicted");
        // assert the FakeBackend recorded a release_session call
    }

    #[tokio::test]
    async fn idle_ttl_reaps_warm_session() {
        let m = mgr(); // ttl = 5s, manual clock
        let ctx = ContextId::parse("c1").unwrap();
        m.mint_or_resume(&ctx, fp_codex(), spec()).await.unwrap();
        m.advance_clock(std::time::Duration::from_secs(6));
        m.reap_idle().await;
        assert!(m.status(&ctx).await.is_none(), "idle session reaped");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-a2a-inbound --lib session_manager`
Expected: FAIL — module not present / types missing.

- [ ] **Step 3: Implement `SessionManager`** — prepend to `session_manager.rs`:

```rust
//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore.
//! Owns the contextId→handle table + the registry lease that pins the warm backend.
//! Keyed by A2A `contextId`. NOT in TaskStore, NOT keyed by task id.

use bridge_core::domain::SessionSpec;
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId};
use bridge_core::ports::{AgentBackend, AgentRegistry, Lease};
use bridge_core::session_fingerprint::SessionSpecFingerprint;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Running,
    Releasing,
    Expired,
}

pub struct WarmHandle {
    pub id: SessionHandleId,
    pub context_id: ContextId,
    pub agent: AgentId,
    pub backend: Arc<dyn AgentBackend>,
    pub backend_session: SessionId,
    pub generation: SessionGeneration,
    pub fingerprint: SessionSpecFingerprint,
    // lease pins the registry slot's backend warm; dropped on release/reap.
    lease: Box<dyn Lease>,
    state: SessionState,
    op: Option<OperationId>,
    last_used: Instant,
}

/// A cloneable, lease-free view returned to callers (the lease stays in the table).
#[derive(Clone)]
pub struct HandleRef {
    pub backend: Arc<dyn AgentBackend>,
    pub backend_session: SessionId,
    pub handle_id: SessionHandleId,
}

pub struct SessionManager {
    registry: Arc<dyn AgentRegistry>,
    by_context: Mutex<HashMap<ContextId, WarmHandle>>,
    idle_ttl: Duration,
    // Injected clock for testability; defaults to Instant::now in production.
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    seq: std::sync::atomic::AtomicU64, // per-manager handle-id counter (Slice 0 seq stamping)
}

impl SessionManager {
    pub fn new(registry: Arc<dyn AgentRegistry>, idle_ttl: Duration) -> Self {
        Self {
            registry,
            by_context: Mutex::new(HashMap::new()),
            idle_ttl,
            now: Box::new(Instant::now),
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Mint a new warm handle for a fresh contextId, or resume an existing one.
    /// On resume, the fingerprint MUST match (else typed `ConfigMismatch`); an
    /// `Expired` handle returns `SessionExpired`.
    pub async fn mint_or_resume(
        &self,
        ctx: &ContextId,
        fingerprint: SessionSpecFingerprint,
        spec: SessionSpec,
    ) -> Result<HandleRef, BridgeError> {
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.get_mut(ctx) {
            if h.state == SessionState::Expired {
                return Err(BridgeError::SessionExpired);
            }
            if let Some(field) = h.fingerprint.first_mismatch(&fingerprint) {
                return Err(BridgeError::ConfigMismatch { field });
            }
            h.last_used = (self.now)();
            return Ok(HandleRef {
                backend: h.backend.clone(),
                backend_session: h.backend_session.clone(),
                handle_id: h.id.clone(),
            });
        }
        // Fresh: resolve (hold the lease), allocate handle + backend_session, configure, insert.
        let resolved = self.registry.resolve(&fingerprint.agent).await?;
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let handle_id = SessionHandleId::parse(format!("h-{n}")).unwrap();
        let backend_session = SessionId::parse(format!("ctx-{}-g0", ctx.as_str()))
            .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
        resolved
            .backend
            .configure_session(&backend_session, &spec)
            .await?;
        let href = HandleRef {
            backend: resolved.backend.clone(),
            backend_session: backend_session.clone(),
            handle_id: handle_id.clone(),
        };
        tab.insert(
            ctx.clone(),
            WarmHandle {
                id: handle_id,
                context_id: ctx.clone(),
                agent: fingerprint.agent.clone(),
                backend: resolved.backend,
                backend_session,
                generation: SessionGeneration::new(0),
                fingerprint,
                lease: resolved.lease,
                state: SessionState::Idle,
                op: None,
                last_used: (self.now)(),
            },
        );
        Ok(href)
    }

    pub async fn status(&self, ctx: &ContextId) -> Option<(SessionState, AgentId, SessionGeneration)> {
        let tab = self.by_context.lock().await;
        tab.get(ctx).map(|h| (h.state, h.agent.clone(), h.generation))
    }

    /// Evict + release the backend session + drop the lease.
    pub async fn release(&self, ctx: &ContextId) {
        let h = self.by_context.lock().await.remove(ctx);
        if let Some(h) = h {
            h.backend.release_session(&h.backend_session).await;
            drop(h.lease);
        }
    }

    /// Cancel an in-flight turn but KEEP the session warm (state→Idle).
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

    /// Reap idle warm sessions whose last-use exceeds the TTL.
    pub async fn reap_idle(&self) {
        let now = (self.now)();
        let expired: Vec<ContextId> = {
            let tab = self.by_context.lock().await;
            tab.iter()
                .filter(|(_, h)| now.duration_since(h.last_used) >= self.idle_ttl)
                .map(|(c, _)| c.clone())
                .collect()
        };
        for c in expired {
            self.release(&c).await;
        }
    }
}
```

(For the test's manual clock + `advance_clock`, add a `#[cfg(test)]` constructor `new_with_clock(registry,
ttl, shared_instant)` that reads an `Arc<Mutex<Instant>>` and an `advance_clock` test helper. Define a
`FakeBackend`/`FakeRegistry`/fake `Lease` in the test module mirroring `bridge-core::ports` test doubles.)

- [ ] **Step 4: Register the module** — `crates/bridge-a2a-inbound/src/lib.rs`:

```rust
pub mod session_manager;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p bridge-a2a-inbound --lib session_manager`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/session_manager.rs crates/bridge-a2a-inbound/src/lib.rs
git commit -m "feat(inbound): SessionManager core (mint/resume/status/release/cancel/reap_idle, fingerprint-gated)"
```

---

## Task 9: Parse `contextId` in `gate()` + reject on non-Local routes

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`RoutedCall` struct line 386; `gate()` line 306;
  add `context_id_from_params` near `task_id_from_params` line 2869)
- Test: `server.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing test** — add to the server.rs test module:

```rust
    #[test]
    fn context_id_parsed_from_message_field_and_metadata_fallback() {
        let v = serde_json::json!({ "message": { "contextId": "c-1", "text": "hi" } });
        assert_eq!(context_id_from_params(&v).unwrap().unwrap().as_str(), "c-1");
        let v2 = serde_json::json!({ "message": { "metadata": { "a2a-bridge.context": "c-2" }, "text": "hi" } });
        assert_eq!(context_id_from_params(&v2).unwrap().unwrap().as_str(), "c-2");
        let v3 = serde_json::json!({ "message": { "text": "hi" } });
        assert!(context_id_from_params(&v3).unwrap().is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-a2a-inbound --lib context_id_parsed_from_message`
Expected: FAIL — `context_id_from_params` not defined.

- [ ] **Step 3: Add the parser** — near `task_id_from_params` (server.rs:2869):

```rust
/// Read the A2A contextId: `message.contextId` (camelCase wire), then the
/// `a2a-bridge.context` metadata fallback. `None` if absent. Empty string → error.
fn context_id_from_params(params: &Value) -> Result<Option<ContextId>, BridgeError> {
    let raw = params
        .get("message")
        .and_then(|m| m.get("contextId"))
        .and_then(|v| v.as_str())
        .or_else(|| params.get("contextId").and_then(|v| v.as_str()))
        .or_else(|| {
            params
                .get("message")
                .and_then(|m| m.get("metadata"))
                .and_then(|md| md.get("a2a-bridge.context"))
                .and_then(|v| v.as_str())
        });
    match raw {
        Some(s) => Ok(Some(ContextId::parse(s)?)),
        None => Ok(None),
    }
}
```

(Add `use bridge_core::ids::ContextId;` if not already imported.)

- [ ] **Step 4: Add `context_id` to `RoutedCall`** — struct at server.rs:386:

```rust
    /// A2A contextId for warm-session continuation (Slice 0). Only honored on the
    /// Local route; rejected on Workflow/Delegate/Fanout. `None` = legacy per-task path.
    context_id: Option<ContextId>,
```

- [ ] **Step 5: Parse + reject in `gate()`** — in `gate()` after computing `target` (server.rs:339), add:

```rust
        let context_id = context_id_from_params(params)?;
        if context_id.is_some() && !matches!(target, RouteTarget::Local(_)) {
            return Err(BridgeError::InvalidRequest {
                field: "contextId is only supported on the local (single-agent) route in Slice 0",
            });
        }
```

and add `context_id,` to the `RoutedCall { .. }` literal at the end of `gate()`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p bridge-a2a-inbound --lib context_id_parsed_from_message && cargo build -p bridge-a2a-inbound`
Expected: PASS; the crate builds (the new `RoutedCall` field is set in `gate()`'s single constructor).

- [ ] **Step 7: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(inbound): parse contextId in gate() into RoutedCall; reject on non-Local routes"
```

---

## Task 10: Wire `SessionManager` into the Local dispatch (the warm path)

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`InboundServer` field list line 119; `new()` line 200;
  add `with_session_manager` builder; the `RouteTarget::Local` arms in `unary_message` line 2194 +
  `stream_message` line 622, via `resolve_configure_bind` line 438)
- Test: an in-crate integration test (mocked backend) asserting warm-reuse + guard=None

- [ ] **Step 1: Add the field + builder + default.**

In the `InboundServer` struct (after `task_store`, line ~143):

```rust
    session_manager: Option<std::sync::Arc<crate::session_manager::SessionManager>>,
```

In `new()` (in the struct literal, line ~225):

```rust
            session_manager: None,
```

After `with_task_store` (line 260), add:

```rust
    #[must_use]
    pub fn with_session_manager(
        mut self,
        sm: std::sync::Arc<crate::session_manager::SessionManager>,
    ) -> Self {
        self.session_manager = Some(sm);
        self
    }
```

- [ ] **Step 2: Write the failing integration test** — add to the server.rs test module (mirror existing
  `unary_message` tests that build an `InboundServer` with a mock backend/registry):

```rust
    #[tokio::test]
    async fn warm_continue_reuses_session_and_sets_no_binding_guard() {
        // Build an InboundServer with a recording mock backend + a SessionManager.
        // Send two unary messages with the same contextId "c1" + a2a-bridge.agent.
        // Assert: the backend saw the SAME bridge SessionId both times (warm reuse),
        // and NO BindingGuard forget_session fired between them.
        // (Mirror the mock setup of the nearest existing unary_message test.)
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p bridge-a2a-inbound --lib warm_continue_reuses_session`
Expected: FAIL — dispatch still derives `session-{task}` and uses the per-task binding.

- [ ] **Step 4: Branch the Local dispatch on a present contextId.** In BOTH the unary arm (server.rs:2194)
  and the streaming arm (server.rs:622), before calling `resolve_configure_bind`, add a warm branch. Extract a
  helper to avoid duplication — add near `resolve_configure_bind` (server.rs:438):

```rust
/// Slice 0 warm path: when a contextId is present, mint/resume a warm handle and
/// dispatch against it with NO BindingGuard (the SessionManager owns the lease).
/// Returns `None` if there's no contextId or no SessionManager → caller uses the
/// legacy `resolve_configure_bind` path.
async fn warm_local_dispatch(
    srv: &InboundServer,
    agent_id: &AgentId,
    routed: &RoutedCall,
) -> Option<Result<LocalDispatch, BridgeError>> {
    let ctx = routed.context_id.clone()?;
    let sm = srv.session_manager.clone()?;
    // Resolve the entry to compute the effective fingerprint (agent + effective_config + cwd).
    let resolved = match srv.registry.resolve(agent_id).await {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };
    let eff = effective_config(&resolved.entry, routed.overrides.as_ref());
    drop(resolved); // release this probe lease; SessionManager holds its own.
    let fingerprint = bridge_core::session_fingerprint::SessionSpecFingerprint {
        agent: agent_id.clone(),
        config: eff.clone(),
        cwd: routed.session_cwd.as_ref().map(|c| c.as_str().to_string()),
    };
    let spec = SessionSpec { config: eff, cwd: routed.session_cwd.clone() };
    match sm.mint_or_resume(&ctx, fingerprint, spec).await {
        Ok(href) => Some(Ok(LocalDispatch { backend: href.backend, guard: None })),
        Err(e) => Some(Err(e)),
    }
}
```

Then in each Local arm, replace the direct `resolve_configure_bind(...)` call with:

```rust
            let dispatch = match warm_local_dispatch(&srv, agent_id, &routed).await {
                Some(r) => r,
                None => resolve_configure_bind(
                    &srv, agent_id, &routed.task, &routed.session,
                    routed.overrides.as_ref(), routed.session_cwd.clone(),
                ).await,
            };
            let dispatch = match dispatch { Ok(d) => d, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
```

**Important:** the warm path must dispatch the prompt against `href.backend_session`, NOT `routed.session`.
Thread the warm `backend_session` through: have `warm_local_dispatch` also return the session to use (extend
`LocalDispatch` with an `Option<SessionId> warm_session`, or carry the `HandleRef`), and in the
`Translator::run(..., &session, ...)` call use the warm session when present. The streaming arm
(`spawn_local_producer`) takes `routed.session` — pass the warm session there too. Keep the change minimal:
add `warm_session: Option<SessionId>` to `LocalDispatch`, set it in `warm_local_dispatch`, and at the
`Translator::run` sites use `dispatch.warm_session.as_ref().unwrap_or(&routed.session)`.

- [ ] **Step 5: Run the test + build**

Run: `cargo test -p bridge-a2a-inbound --lib warm_continue_reuses_session && cargo build -p bridge-a2a-inbound`
Expected: PASS; crate builds.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(inbound): warm Local dispatch via SessionManager (guard=None, warm backend_session)"
```

---

## Task 11: `SessionStatus` / `SessionRelease` / `SessionCancel` JSON-RPC methods

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (the method match line 589; add 3 handlers)
- Test: `server.rs`

- [ ] **Step 1: Write the failing test** — add to the server.rs test module:

```rust
    #[tokio::test]
    async fn session_status_release_cancel_methods_dispatch() {
        // Build an InboundServer with a SessionManager + mock backend; mint a warm
        // handle via a unary message with contextId "c1"; then:
        //  - SessionStatus {contextId:"c1"} → result.state == "idle"
        //  - SessionCancel {contextId:"c1"} → ok, handle still present
        //  - SessionRelease {contextId:"c1"} → ok, handle gone (SessionStatus → not found)
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bridge-a2a-inbound --lib session_status_release_cancel`
Expected: FAIL — methods not routed (method-not-found).

- [ ] **Step 3: Add the match arms** — in the method `match` (server.rs:589), before the `""` arm:

```rust
        "SessionStatus" => session_status(srv, headers, id, params).await,
        "SessionRelease" => session_release(srv, headers, id, params).await,
        "SessionCancel" => session_cancel(srv, headers, id, params).await,
```

- [ ] **Step 4: Add the handlers** — near the other method handlers in server.rs:

```rust
fn context_id_arg(params: &Value) -> Result<ContextId, BridgeError> {
    params
        .get("contextId")
        .and_then(|v| v.as_str())
        .ok_or(BridgeError::InvalidRequest { field: "contextId" })
        .and_then(ContextId::parse)
}

async fn session_status(
    srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value,
) -> Response {
    if let Err(e) = srv.auth.authorize(&inbound_from(&headers)) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "session manager not configured");
    };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    match sm.status(&ctx).await {
        Some((state, agent, gen)) => jsonrpc_ok(id, json!({
            "contextId": ctx.as_str(),
            "state": format!("{state:?}").to_lowercase(),
            "agent": agent.as_str(),
            "generation": gen.get(),
        })),
        None => bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound),
    }
}

async fn session_release(
    srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value,
) -> Response {
    if let Err(e) = srv.auth.authorize(&inbound_from(&headers)) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "session manager not configured");
    };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    sm.release(&ctx).await;
    jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "released": true }))
}

async fn session_cancel(
    srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value,
) -> Response {
    if let Err(e) = srv.auth.authorize(&inbound_from(&headers)) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "session manager not configured");
    };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    match sm.cancel(&ctx).await {
        Ok(()) => jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "canceled": true })),
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}
```

(Use the existing auth/inbound helper the other handlers use — mirror how `cancel_task`/`get_task` build the
`InboundRequest` from headers; replace `inbound_from` with the real helper name found in the file. Reuse the
existing `jsonrpc_ok`/`jsonrpc_err`/`bridge_err_to_jsonrpc`/`json!` helpers already used in server.rs.)

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p bridge-a2a-inbound --lib session_status_release_cancel && cargo build -p bridge-a2a-inbound`
Expected: PASS; builds.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(inbound): SessionStatus/SessionRelease/SessionCancel JSON-RPC methods"
```

---

## Task 12: CLI — `submit --context --agent` + `session` subcommand

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`submit_cmd` line 2596; subcommand match line 3287; add `session_cmd`)
- Test: a small unit test for arg parsing if feasible; otherwise covered by the live-gate (Task 14)

- [ ] **Step 1: Extend `submit_cmd`** — make the skill optional, add `--context`/`--agent`. Replace
  `submit_cmd` (main.rs:2596):

```rust
async fn submit_cmd(args: &[String]) -> Result<(), BoxError> {
    let input_path = flag(args, "--input").ok_or("submit: --input <file> required")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let context = flag(args, "--context");
    let agent = flag(args, "--agent");
    // Skill is the first NON-flag positional, and is optional when --agent is given
    // (no skill → Local route to --agent / the default agent).
    let skill = args.iter().find(|a| !a.starts_with("--")
        && Some(a.as_str()) != context && Some(a.as_str()) != agent
        && a.as_str() != input_path && a.as_str() != url).cloned();
    let text = std::fs::read_to_string(input_path)?;

    let mut metadata = serde_json::Map::new();
    if let Some(s) = &skill { metadata.insert("a2a-bridge.skill".into(), s.clone().into()); }
    if let Some(a) = agent { metadata.insert("a2a-bridge.agent".into(), a.into()); }

    let mut message = serde_json::Map::new();
    message.insert("text".into(), text.into());
    message.insert("metadata".into(), serde_json::Value::Object(metadata));
    if let Some(c) = context { message.insert("contextId".into(), c.into()); }

    let params = serde_json::json!({ "message": serde_json::Value::Object(message) });
    let v = rpc_call(url, a2a::methods::SEND_MESSAGE, params).await?;
    if let Some(err) = v.get("error") { return Err(format!("submit failed: {err}").into()); }
    // Warm/Local sends return a message/result, not necessarily a task — print whatever id is present.
    let out = v["result"]["task"]["id"].as_str()
        .or_else(|| v["result"]["messageId"].as_str())
        .unwrap_or("ok");
    println!("{out}");
    Ok(())
}
```

(The positional-skill heuristic is intentionally simple; if the repo has an arg parser pattern, prefer it.)

- [ ] **Step 2: Add `session_cmd`** — near `task_cmd` (main.rs:2618):

```rust
async fn session_cmd(args: &[String]) -> Result<(), BoxError> {
    let sub = args.first().map(|s| s.as_str())
        .ok_or("session: missing subcommand (status|release|cancel)")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let ctx = args.get(1).cloned().ok_or("session: missing <contextId>")?;
    let method = match sub {
        "status" => "SessionStatus",
        "release" => "SessionRelease",
        "cancel" => "SessionCancel",
        other => return Err(format!("session: unknown subcommand {other:?}").into()),
    };
    let v = rpc_call(url, method, serde_json::json!({ "contextId": ctx })).await?;
    if let Some(err) = v.get("error") { return Err(format!("session {sub} failed: {err}").into()); }
    println!("{}", serde_json::to_string_pretty(&v["result"])?);
    Ok(())
}
```

- [ ] **Step 3: Register the subcommand** — in the match (main.rs:3287), add after the `"task"` arm:

```rust
        Some("session") => return session_cmd(&raw_args[2..]).await,
```

and add `session` to the unknown-subcommand error list (main.rs:3306) and `TOP_USAGE`.

- [ ] **Step 4: Build**

Run: `cargo build -p a2a-bridge`
Expected: builds.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(cli): submit --context/--agent (Local route) + session status|release|cancel"
```

---

## Task 13: Config + wire `SessionManager` into `serve` boot

**Files:**
- Modify: the serve boot path (where `InboundServer::new(...).with_*(...)` is assembled — find via
  `rg "InboundServer::new|with_task_store" bin/ crates/`) and the config struct that reads `[server]`/
  `[sessions]` (find via `rg "warm_idle|RegistryConfig|ServerConfig|deserialize" bin/a2a-bridge/src`)
- Test: a config-parse test for `warm_idle_ttl_secs` default

- [ ] **Step 1: Write the failing test** — in the config module's test area, assert the default + override:

```rust
    #[test]
    fn warm_idle_ttl_defaults_to_1800_and_parses_override() {
        let cfg: ServerConfig = toml::from_str("").unwrap();           // adjust to the real config type
        assert_eq!(cfg.warm_idle_ttl_secs, 1800);
        let cfg2: ServerConfig = toml::from_str("warm_idle_ttl_secs = 5").unwrap();
        assert_eq!(cfg2.warm_idle_ttl_secs, 5);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p a2a-bridge warm_idle_ttl_defaults` (or the crate that owns the config)
Expected: FAIL — field missing.

- [ ] **Step 3: Add the config field** — on the `[server]` config struct:

```rust
    /// Idle TTL (seconds) before a warm session is reaped. Slice 0 default 30 min.
    #[serde(default = "default_warm_idle_ttl_secs")]
    pub warm_idle_ttl_secs: u64,
```

```rust
fn default_warm_idle_ttl_secs() -> u64 { 1800 }
```

- [ ] **Step 4: Instantiate + wire `SessionManager` in serve boot** — where `InboundServer` is built, after
  the registry is constructed:

```rust
    let session_manager = std::sync::Arc::new(
        bridge_a2a_inbound::session_manager::SessionManager::new(
            registry.clone(),
            std::time::Duration::from_secs(server_cfg.warm_idle_ttl_secs),
        ),
    );
    // ... .with_task_store(...).with_session_manager(session_manager.clone())
    // Spawn the idle reaper.
    {
        let sm = session_manager.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            loop { tick.tick().await; sm.reap_idle().await; }
        });
    }
```

- [ ] **Step 5: Run test + build the whole workspace**

Run: `cargo test -p a2a-bridge warm_idle_ttl_defaults && cargo build --workspace`
Expected: PASS; workspace builds.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(serve): warm_idle_ttl_secs config + wire SessionManager + idle reaper into serve boot"
```

---

## Task 14: Full workspace gate + live-gate (DoD)

**Files:** none (verification task). The live-gate is a manual/scripted run, recorded in the PR.

- [ ] **Step 1: Workspace fmt/clippy/test**

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: clean; all tests pass. (If a system-integration test needs docker/PID-1, scope it out as the
verify examples do — see [[containerized-agents-slice-b2b2-shipped]] for the `--exclude`/`--skip` pattern.)

- [ ] **Step 2: Build the release binary**

Run: `cargo build --release -p a2a-bridge`

- [ ] **Step 3: Live-gate (real serve + codex).** In one terminal, start serve with a codex agent config and
  `warm_idle_ttl_secs = 5`. In another, run the DoD scenarios (record output in the PR):

```bash
# DoD-1/2 Continuation + latency (warm reuse, no cold spawn on the 2nd call):
printf 'Remember the codeword ZEBRA. Reply OK.' > /tmp/s0a.txt
printf 'What codeword did I give you THIS session? One word.' > /tmp/s0b.txt
# watcher: pgrep -f codex-acp should NOT grow between the two calls, and the shared process STAYS alive.
./target/release/a2a-bridge submit --agent codex --context c1 --input /tmp/s0a.txt
./target/release/a2a-bridge submit --agent codex --context c1 --input /tmp/s0b.txt   # → must recall ZEBRA

# DoD-3 Isolation:
./target/release/a2a-bridge submit --agent codex --context c2 --input /tmp/s0b.txt   # → NONE (no cross-talk)

# DoD-4 Back-compat: no --context → legacy forget-after (unchanged)
./target/release/a2a-bridge submit --agent codex --input /tmp/s0a.txt

# DoD-5/7 status/release/cancel + reaper (host-ACP: shared process stays; sessions[id] gone):
./target/release/a2a-bridge session status c1
./target/release/a2a-bridge session release c1
./target/release/a2a-bridge session status c1   # → not found

# DoD-6 Config-mismatch (different effort on the same context → typed ConfigMismatch):
./target/release/a2a-bridge submit --agent codex --context c3 --input /tmp/s0a.txt
# resend c3 with a2a-bridge.effort override differing from the first → expect a ConfigMismatch error

# DoD-8 Idle-TTL: leave c2 idle > 5s → next `session status c2` → not found (reaped).
```

- [ ] **Step 4: Record the gate results** in the PR description (the `pgrep` no-growth observation, the ZEBRA
  recall, the isolation NONE, the typed ConfigMismatch, the reaper eviction). For ContainerRw, if a `:rw`
  agent is configured, additionally show `docker ps` →0 for the released session.

- [ ] **Step 5: Final commit (if any verification fixups)** and proceed to
  `superpowers:finishing-a-development-branch`.

---

## Self-review notes (done during planning)

- **Spec coverage:** every Slice-0 IN item maps to a task — ids (T1), DTOs+Usage (T2/T4), errors (T3),
  release_session ACP/ContainerRw/API (T4 default + T5 + T6), fingerprint (T7), SessionManager (T8),
  contextId parse + reject (T9), warm dispatch + guard=None (T10), session methods (T11), CLI (T12), config +
  reaper wiring (T13), live-gate DoD 1–9 (T14). SEQ-AUTHORITY: the non-Local rejection (T9) + intra-manager
  `HandleBusy` — **note:** `HandleBusy` (refuse mint on a live handle of a *different* in-flight op) is
  subsumed by Slice-0's single-turn model (one contextId = one handle; concurrent same-context turns are out
  of scope) — if concurrent-same-context arrives, T8's `mint_or_resume` returns the existing handle; add an
  explicit `Running`-state reject only if a test surfaces a need.
- **Type consistency:** `HandleRef`/`WarmHandle`/`SessionState` names are stable across T8/T10/T11;
  `LocalDispatch.warm_session` added in T10 is used at the `Translator::run` sites in the same task.
- **No placeholders:** the only `unimplemented!()` is in a test scaffold (T8 `mgr()`), to be filled with the
  fake doubles during implementation — flagged explicitly, not shipped.
