# Slice 4 — Compact — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `compact` — summarize a warm session's context, reset it to a fresh generation, and seed the
summary as the next turn's first input — keeping the agent process warm while shedding raw token weight.

**Architecture:** Compose over the SHIPPED Slice-3 `reset_session` under ONE new `Compacting` claim:
`SessionManager::compact_session(ctx, summarize_fn)` claims `Idle→Compacting`, runs a handler-provided
summarize closure on the claimed gen-N session (time-bounded), then — on a good summary — runs the reset
internals (release old → configure new) and stashes the summary as `pending_seed`; on ANY summarize failure it
EXPIRES the handle (the old context is already mutated; no rollback). The seed is taken-and-cleared on the next
checkout and prepended at both dispatch sites. `SessionCompact` wire + `session compact` CLI.

**Tech Stack:** Rust (tokio, async-trait), the bridge workspace (`bridge-a2a-inbound`, `bridge-core`,
`bin/a2a-bridge`). Spec: `docs/superpowers/specs/2026-06-18-slice-4-compact.md` (v2, FIX-1..14 BINDING).

**Binding constraints (from the spec):** FIX-1 bad-summary EXPIRES (never restore-Idle); FIX-3 configure-fail
returns the configure error; FIX-4 `Update::Permission`→failure + no-tools prompt; FIX-5 summarize timeout;
FIX-6 the two test-fake extensions; FIX-7 `MAX_SUMMARY_BYTES=32*1024` enforced during drain; FIX-8 wrap seed;
FIX-9 cwd-parse before the flip; FIX-10 seed taken only at the two resume returns; FIX-12 `MessageTooLarge`→-32603.

## v2 — dual plan-review fixes folded (BINDING — harness/fixture corrections)

Both reviewers returned `fix-then-execute`; both endorsed the production logic (`compact_session`,
`summarize_collect`, seed plumbing, the EXPIRE keystone) as SOUND. The defects are all in test fixtures /
field-plumbing. These PFIX corrections are binding and are applied inline in the tasks below:

- **PFIX-1 (BLOCKER) — `manager()` is a 3-TUPLE** `(SessionManager, Arc<FakeBackend>, Arc<FakeRegistry>)`
  (`session_manager.rs:814`). ALL compact tests destructure THREE: `let (m, fake, _r) = manager();` /
  `let (m, _f, _r) = manager();`. `manager_with_timeout` (T3) returns the same 3-tuple.
- **PFIX-2 (MAJOR) — `LocalDispatch` has THREE construction sites.** Adding `seed` requires `seed: None` at the
  two legacy `resolve_configure_bind` sites (`server.rs:511`, `:547`) AND `seed: turn.seed` at the warm site
  (`:581`); else T6 fails `cargo test --workspace --no-run` (missing-field).
- **PFIX-3 (MAJOR) — T3 configure-fail uses the EXISTING `set_configure_result`, set AFTER warm+finish.** The
  fake returns the configured result on EVERY `configure_session` (incl. the warm-up `g0` mint,
  `session_manager.rs:305/681`), so setting it before checkout panics the warm-up `unwrap()`. Do: checkout →
  finish → `fake.set_configure_result(Err(BridgeError::ConfigInvalid { reason: "test".into() }))` (the method
  exists, `:626`; pattern used at `:1020`) → compact. NO new `fail_configure` method.
- **PFIX-4 (MAJOR) — the seed-prepend tests record on the WARM fake** (`WarmRecordingBackend`, `server.rs:5804`),
  NOT the generic `FakeBackend` (`:3630`). The WARM-UP turn prompts the backend FIRST, so `prompted_parts[0]`
  is the warm-up turn — assert `prompted_parts.last()` (or `clear_prompted_parts()` after the warm-up/idle).
- **PFIX-5 (MINOR) — `Update` is NOT `Clone`** (`ports.rs:20`). `ScriptedBackend` stores
  `Mutex<Option<Vec<Update>>>`, one-shot `take()` + `into_iter().map(Ok)` (no clone).
- **PFIX-6 (MINOR) — `PermissionRequest::read()`** (imported at `server.rs:3364`), not a nonexistent
  `test_permission_request()`.
- **PFIX-7 (MINOR) — import `Update`** into the `use bridge_core::ports::{...}` at `server.rs` module scope
  (`:42-45`) — `Part`/`AgentBackend`/`SessionId`/`BridgeError` are already there; `Update` is not.
- **PFIX-8 (MINOR) — `#[allow(dead_code)]`** on `summarize_collect`/`SUMMARIZE_PROMPT`/`MAX_SUMMARY_BYTES` in
  T4 (no non-test caller until T7's handler); remove it in T7. (`--workspace --no-run` doesn't pass
  `-D warnings`, so T4–T6 stay green; T8 is the first `-D warnings` gate, by which point T7 wired the caller.)
- **PFIX-9 (MINOR) — T8 live-gate:** launch serve as an ISOLATED process; capture its `codex-acp` child PID
  set before AND after compact and require equality; assert `usage.used`/`usage.size` are `null` specifically
  (not the whole `usage` object).
- **PFIX-10 (traceability) — oversize is tested at BOTH levels:** the collector
  (`summarize_collect_oversize_is_message_too_large`, T4) AND the manager EXPIRE path
  (`compact_oversize_summary_expires`, T3 — injects `Err(MessageTooLarge)`, asserts `status()==None` + old
  released). Closes codex's traceability note.

---

## File Structure

- `crates/bridge-a2a-inbound/src/session_manager.rs` — `SessionState::Compacting`, `WarmHandle.pending_seed`,
  `WarmTurn.seed`, `compact_summarize_timeout` field + builder, `compact_session` + `expire_after_summarize`
  helper, `checkout_turn` seed take-and-clear, `reset_session` seed-drop, `status()` mapping. Test fake gains a
  scriptable `prompt` only where needed (the manager tests inject a closure, so its fake does NOT need a
  scriptable prompt — see T2).
- `crates/bridge-a2a-inbound/src/server.rs` — `SUMMARIZE_PROMPT`/`MAX_SUMMARY_BYTES` consts, `summarize_collect`
  helper, `session_compact` handler + dispatch arm, `LocalDispatch.seed` + carry, seed prepend at
  `spawn_local_producer` + the unary collect. Test fakes gain a scriptable + a parts-recording `prompt`.
- `bin/a2a-bridge/src/main.rs` — `session compact` CLI arm + help + missing-subcommand string.
- `bin/a2a-bridge/src/config.rs` + `main.rs` serve wiring — `compact_summarize_timeout_secs` knob.

Each task ends GREEN under `cargo test -p bridge-a2a-inbound --lib` (or the named target) AND
`cargo test --workspace --no-run` (catch match-exhaustiveness in test targets — the slice-2/3 lesson).

---

### Task 1: `SessionState::Compacting` + handle/turn seed fields + ripple

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/session_manager.rs` (`:19` enum, `:32` is_claimed, `:39` WarmHandle,
  `:60` WarmTurn, `:352` status, all `WarmTurn { .. }` construction sites, all `match SessionState` sites)

- [ ] **Step 1: Write the failing test** (append to the session_manager `#[cfg(test)]` module)

```rust
#[test]
fn is_claimed_includes_compacting() {
    assert!(super::is_claimed(super::SessionState::Compacting));
}
```

- [ ] **Step 2: Run it — expect FAIL** (no `Compacting` variant)

Run: `cargo test -p bridge-a2a-inbound --lib is_claimed_includes_compacting`
Expected: compile error — no variant `Compacting`.

- [ ] **Step 3: Add the variant + is_claimed + status mapping + the two seed fields**

In `SessionState` (`:19`) add `Compacting,` after `Resetting`. In `is_claimed` (`:32`) add it to the `matches!`:
```rust
fn is_claimed(s: SessionState) -> bool {
    matches!(
        s,
        SessionState::Reconciling | SessionState::Expiring | SessionState::Resetting | SessionState::Compacting
    )
}
```
In `WarmHandle` (`:39`) add `pending_seed: Option<String>,` (after `op`). In `WarmTurn` (`:60`) add
`pub seed: Option<String>,`. In `status()` (`:352`) — find the `state` string match and add
`SessionState::Compacting => "compacting",`.

- [ ] **Step 4: Fix the compile ripple**

Every `WarmTurn { .. }` literal (checkout_turn `:198`, `:268`, `:310`) gets `seed: None,` (the seed is filled
in T5). Every `WarmHandle { .. }` literal (the mint, `:317-333`) gets `pending_seed: None,`. Any non-wildcard
`match SessionState` the compiler flags gets a `Compacting` arm (defer/claimed behavior — treat like
`Resetting`).

- [ ] **Step 5: Run — expect PASS + workspace compiles**

Run: `cargo test -p bridge-a2a-inbound --lib is_claimed_includes_compacting && cargo test --workspace --no-run`
Expected: PASS; workspace builds (no match-exhaustiveness breaks).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/session_manager.rs
git commit -m "feat(compact): SessionState::Compacting + pending_seed/seed fields (T1)"
```

---

### Task 2: `compact_session` (happy path + require-Idle + NotFound) + timeout field

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/session_manager.rs` (struct `:108`, `new_with_clock` `:122`, after
  `reset_session` `:520`)

- [ ] **Step 1: Add the `compact_summarize_timeout` field + builder**

In the struct (`:108`) add `compact_summarize_timeout: Duration,`. In `new_with_clock` (`:122`) initialize
`compact_summarize_timeout: Duration::from_secs(120),`. Add a builder next to `with_warn_fraction` (`:137`):
```rust
pub fn with_compact_summarize_timeout(mut self, d: Duration) -> Self {
    self.compact_summarize_timeout = d;
    self
}
```

- [ ] **Step 2: Write the failing tests** (append to the test module; reuse `manager()`/`FakeBackend`/`ctx()`)

```rust
#[tokio::test]
async fn compact_advances_generation_and_seeds() {
    let (m, fake, _r) = manager();
    let c = ctx("c1");
    // Warm + idle a session at gen 0.
    let turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
    m.finish_turn(&c, turn.generation, &turn.op).await;
    let out = m
        .compact_session(&c, |_b, _s| async { Ok("THE SUMMARY".to_string()) })
        .await
        .unwrap();
    assert_eq!(out, ResetOutcome::Cleared { generation: 1 });
    let st = m.status(&c).await.unwrap();
    assert_eq!(st.generation, 1);
    assert_eq!(st.state, "idle");
    assert!(fake.releases().iter().any(|s| s == "ctx-c1-g0")); // old released
    assert!(fake.configured().iter().any(|s| s == "ctx-c1-g1")); // new configured
    // seed is delivered to the NEXT checkout (asserted in T5); here assert it advanced.
}

#[tokio::test]
async fn compact_on_running_is_handle_busy() {
    let (m, _f, _r) = manager();
    let c = ctx("c2");
    let _turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap(); // Running
    let err = m
        .compact_session(&c, |_b, _s| async { Ok("x".to_string()) })
        .await
        .unwrap_err();
    assert_eq!(err, BridgeError::HandleBusy);
}

#[tokio::test]
async fn compact_unknown_ctx_is_not_found() {
    let (m, _f, _r) = manager();
    let out = m
        .compact_session(&ctx("nope"), |_b, _s| async { Ok("x".to_string()) })
        .await
        .unwrap();
    assert_eq!(out, ResetOutcome::NotFound);
}
```
(Match the EXISTING test helpers' signatures — `manager()`, `agent()`, `op()`, `ctx()` — adjust arg names to
the real harness at `session_manager.rs:~600+`.)

- [ ] **Step 2b: Run — expect FAIL** (`compact_session` does not exist)

Run: `cargo test -p bridge-a2a-inbound --lib compact_`
Expected: compile error — no method `compact_session`.

- [ ] **Step 3: Implement `compact_session` + `expire_after_summarize`** (after `reset_session`, `:520`)

```rust
/// Compact: summarize the gen-N context, reset to N+1, and seed the summary for the next turn.
/// require-Idle (no force). On ANY summarize failure the handle is EXPIRED (the old context is already
/// mutated by the failed summarize exchange — no rollback). [Slice 4, FIX-1..14]
pub async fn compact_session<F, Fut>(
    &self,
    ctx: &ContextId,
    summarize: F,
) -> Result<ResetOutcome, BridgeError>
where
    F: FnOnce(Arc<dyn AgentBackend>, SessionId) -> Fut,
    Fut: std::future::Future<Output = Result<String, BridgeError>>,
{
    // (1) Claim Idle -> Compacting under one lock; capture incl. the fallible cwd parse BEFORE the flip (FIX-9).
    let (backend, old_id, claimed_id, new_gen, new_id, spec) = {
        let mut tab = self.by_context.lock().await;
        let Some(h) = tab.get_mut(ctx) else {
            return Ok(ResetOutcome::NotFound);
        };
        if h.state != SessionState::Idle {
            return Err(BridgeError::HandleBusy);
        }
        let backend = h.backend.clone();
        let old_id = h.backend_session.clone();
        let claimed_id = h.id.clone();
        let new_gen = SessionGeneration::new(h.generation.get() + 1);
        let new_id = SessionId::parse(format!("ctx-{}-g{}", ctx.as_str(), new_gen.get()))
            .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
        let cwd = match h.fingerprint.cwd.as_deref() {
            Some(s) => Some(SessionCwd::parse(s).map_err(|_| BridgeError::ConfigInvalid {
                reason: "session cwd".into(),
            })?),
            None => None,
        };
        let spec = SessionSpec { config: h.fingerprint.config.clone(), cwd };
        h.state = SessionState::Compacting;
        h.expire_after_reconcile = false;
        (backend, old_id, claimed_id, new_gen, new_id, spec)
    };

    // (2) Summarize on the gen-N session, TIME-BOUNDED, claim held (FIX-5).
    let summarized = tokio::time::timeout(
        self.compact_summarize_timeout,
        summarize(backend.clone(), old_id.clone()),
    )
    .await;

    // (3) Bad summary (Err / empty / timeout) -> EXPIRE (FIX-1/2). Never restore Idle.
    let summary = match summarized {
        Ok(Ok(s)) if !s.trim().is_empty() => s,
        bad => {
            let err = match bad {
                Ok(Ok(_)) => BridgeError::AgentCrashed { reason: "compact summary was empty".into() },
                Ok(Err(e)) => e,
                Err(_) => BridgeError::AgentCrashed { reason: "compact summarize timed out".into() },
            };
            self.expire_after_summarize(ctx, &claimed_id, backend.as_ref(), &old_id).await;
            return Err(err);
        }
    };

    // (4) Good summary -> reset tail under Compacting (mirrors reset_session:475-519), stash seed on commit.
    backend.release_session(&old_id).await;
    let cfg = backend.configure_session(&new_id, &spec).await;
    let mut tab = self.by_context.lock().await;
    let still_ours = matches!(tab.get(ctx), Some(h) if h.id == claimed_id && h.state == SessionState::Compacting);
    let new_stashed = cfg.is_ok();
    if !still_ours {
        drop(tab);
        if new_stashed { backend.release_session(&new_id).await; }
        return Err(BridgeError::SessionExpired);
    }
    let deferred = tab.get(ctx).map(|h| h.expire_after_reconcile).unwrap_or(true);
    if cfg.is_err() || deferred {
        drop(tab);
        if new_stashed { backend.release_session(&new_id).await; }
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.remove(ctx) { drop(h.lease); }
        return match cfg {
            Err(e) => Err(e),               // FIX-3: configure error, NOT SessionExpired
            Ok(()) => Err(BridgeError::SessionExpired),
        };
    }
    let h = tab.get_mut(ctx).expect("still_ours");
    h.backend_session = new_id;
    h.generation = new_gen;
    h.usage = UsageSnapshot::default();
    h.op = None;
    h.pending_seed = Some(summary);
    h.state = SessionState::Idle;
    h.last_used = (self.now)();
    Ok(ResetOutcome::Cleared { generation: new_gen.get() })
}

/// EXPIRE a Compacting handle after a failed summarize: tombstone -> release old -> remove + drop lease.
/// Mirrors the non-clean tail of `checkout_turn` (:276-292).
async fn expire_after_summarize(
    &self,
    ctx: &ContextId,
    claimed_id: &SessionHandleId,
    backend: &dyn AgentBackend,
    old_id: &SessionId,
) {
    {
        let mut tab = self.by_context.lock().await;
        let still_ours = matches!(
            tab.get(ctx),
            Some(h) if h.id == *claimed_id && h.state == SessionState::Compacting
        );
        if !still_ours {
            return;
        }
        tab.get_mut(ctx).expect("still_ours").state = SessionState::Expiring;
    }
    backend.release_session(old_id).await;
    let mut tab = self.by_context.lock().await;
    if let Some(h) = tab.remove(ctx) {
        drop(h.lease);
    }
}
```
(Ensure `Duration`, `SessionCwd`, `SessionSpec`, `SessionGeneration`, `UsageSnapshot`, `SessionHandleId`,
`AgentBackend` are already in scope — they are, used by `reset_session`/`checkout_turn`.)

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p bridge-a2a-inbound --lib compact_ && cargo test --workspace --no-run`
Expected: the three tests PASS; workspace compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/session_manager.rs
git commit -m "feat(compact): SessionManager::compact_session happy path + require-Idle + timeout field (T2)"
```

---

### Task 3: `compact_session` failure paths — EXPIRE (the keystone, FIX-1/3/5)

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/session_manager.rs` (tests only — the impl from T2 already EXPIREs)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn compact_bad_summary_expires_handle() {
    for bad in ["__ERR__", "   "] {
        let (m, fake, _r) = manager();
        let c = ctx("c");
        let turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        let b = bad.to_string();
        let err = m
            .compact_session(&c, move |_b, _s| {
                let b = b.clone();
                async move {
                    if b == "__ERR__" { Err(BridgeError::AgentCrashed { reason: "boom".into() }) }
                    else { Ok(b) } // whitespace-only -> empty
                }
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::AgentCrashed { .. }));
        assert!(m.status(&c).await.is_none(), "handle EXPIRED (removed), not restored Idle");
        assert!(fake.releases().iter().any(|s| s == "ctx-c-g0"), "old session released");
    }
}

#[tokio::test]
async fn compact_summary_timeout_expires() {
    let (m, _f, _r) = manager_with_timeout(std::time::Duration::from_millis(10));
    let c = ctx("c");
    let turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
    m.finish_turn(&c, turn.generation, &turn.op).await;
    let err = m
        .compact_session(&c, |_b, _s| async {
            futures::future::pending::<()>().await; // never resolves
            Ok(String::new())
        })
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::AgentCrashed { .. }));
    assert!(m.status(&c).await.is_none());
}

#[tokio::test]
async fn compact_oversize_summary_expires() {
    // PFIX-10: a MessageTooLarge from the closure EXPIRES the handle (FIX-1/7) — explicit manager-level test.
    let (m, fake, _r) = manager();
    let c = ctx("c");
    let turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
    m.finish_turn(&c, turn.generation, &turn.op).await;
    let err = m
        .compact_session(&c, |_b, _s| async { Err(BridgeError::MessageTooLarge) })
        .await
        .unwrap_err();
    assert_eq!(err, BridgeError::MessageTooLarge);
    assert!(m.status(&c).await.is_none());
    assert!(fake.releases().iter().any(|s| s == "ctx-c-g0"));
}

#[tokio::test]
async fn compact_configure_failure_returns_configure_error() {
    // PFIX-3: set the configure failure AFTER the warm-up (the fake fails EVERY configure incl. g0).
    let (m, fake, _r) = manager();
    let c = ctx("c");
    let turn = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap(); // configures g0 OK
    m.finish_turn(&c, turn.generation, &turn.op).await;
    fake.set_configure_result(Err(BridgeError::ConfigInvalid { reason: "test".into() })); // g1 will fail
    let err = m
        .compact_session(&c, |_b, _s| async { Ok("good summary".to_string()) })
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::ConfigInvalid { .. })); // FIX-3: configure error, NOT SessionExpired
    assert!(m.status(&c).await.is_none()); // handle EXPIRED (removed)
}
```

- [ ] **Step 2: Add the one needed helper**

Add `fn manager_with_timeout(d: Duration) -> (SessionManager, Arc<FakeBackend>, Arc<FakeRegistry>)` mirroring
`manager()` (`:814`) but chaining `.with_compact_summarize_timeout(d)` — same **3-tuple** return (PFIX-1). NO
`fail_configure` is needed: `FakeBackend::set_configure_result(Err(..))` already exists (`:626`, used at
`:1020`) and the test sets it AFTER warm-up (PFIX-3).

- [ ] **Step 3: Run — expect PASS** (the T2 impl already EXPIREs; these lock it in)

Run: `cargo test -p bridge-a2a-inbound --lib compact_ && cargo test --workspace --no-run`
Expected: all PASS. If `compact_bad_summary_expires_handle` shows a surviving handle, the T2 impl restored Idle
somewhere — fix to EXPIRE.

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-a2a-inbound/src/session_manager.rs
git commit -m "test(compact): bad-summary/timeout/configure-fail EXPIRE the handle (T3, FIX-1/3/5)"
```

---

### Task 4: `summarize_collect` helper + scriptable server fake (FIX-4/7/14)

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (a new helper + consts; the test `FakeBackend`)

- [ ] **Step 1: Write the failing tests** (server test module)

```rust
#[tokio::test]
async fn summarize_collect_accumulates_multichunk() {
    let b = Arc::new(ScriptedBackend::with_updates(vec![
        Update::Text("AL".into()), Update::Text("PHA".into()), Update::Done { stop_reason: "end_turn".into() },
    ]));
    let s = super::summarize_collect(b, SessionId::parse("s").unwrap()).await.unwrap();
    assert_eq!(s, "ALPHA"); // NOT truncated to the last chunk
}

#[tokio::test]
async fn summarize_collect_oversize_is_message_too_large() {
    let big = "x".repeat(40 * 1024);
    let b = Arc::new(ScriptedBackend::with_updates(vec![
        Update::Text(big), Update::Done { stop_reason: "end_turn".into() },
    ]));
    let err = super::summarize_collect(b, SessionId::parse("s").unwrap()).await.unwrap_err();
    assert_eq!(err, BridgeError::MessageTooLarge);
}

#[tokio::test]
async fn summarize_collect_permission_fails() {
    use bridge_core::domain::PermissionRequest; // PFIX-6: real ctor, imported at server.rs:3364
    let b = Arc::new(ScriptedBackend::with_updates(vec![
        Update::Permission(PermissionRequest::read()),
    ]));
    let err = super::summarize_collect(b, SessionId::parse("s").unwrap()).await.unwrap_err();
    assert!(matches!(err, BridgeError::AgentCrashed { .. }));
}
```

- [ ] **Step 2: Add a one-shot `ScriptedBackend` test fake** (PFIX-5: `Update` is NOT `Clone`)

```rust
struct ScriptedBackend { updates: std::sync::Mutex<Option<Vec<Update>>> }
impl ScriptedBackend {
    fn with_updates(u: Vec<Update>) -> Self { Self { updates: std::sync::Mutex::new(Some(u)) } }
}
#[async_trait::async_trait]
impl AgentBackend for ScriptedBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        use futures::StreamExt;
        let u = self.updates.lock().unwrap().take().unwrap_or_default(); // one-shot, no clone
        Ok(futures::stream::iter(u.into_iter().map(Ok)).boxed())
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { Ok(()) }
}
```
(Confirm the trait's `#[async_trait]` usage + `BackendStream` alias against `ports.rs:33-38/:82`; only `prompt`
+ `cancel` are non-defaulted.)

- [ ] **Step 3: Implement the helper + consts** (near the other server helpers)

```rust
const SUMMARIZE_PROMPT: &str = "Summarize the conversation so far into a faithful, self-contained summary that \
a fresh session could continue from. Preserve durable facts, decisions, and identifiers; exclude any values \
explicitly marked temporary or throwaway. Do NOT use tools, read files, or run commands — reply with the \
summary text only.";
const MAX_SUMMARY_BYTES: usize = 32 * 1024;

/// Drive a single summarize turn on `session` and collect the FULL text (routes around the unary
/// last-chunk truncation). Bounds bytes during the drain; treats a permission update as a failure. [Slice 4]
async fn summarize_collect(
    backend: Arc<dyn AgentBackend>,
    session: SessionId,
) -> Result<String, BridgeError> {
    use futures::StreamExt;
    let mut stream = backend
        .prompt(&session, vec![Part { text: SUMMARIZE_PROMPT.to_string() }])
        .await?;
    let mut out = String::new();
    while let Some(update) = stream.next().await {
        match update? {
            Update::Text(t) => {
                if out.len() + t.len() > MAX_SUMMARY_BYTES {
                    return Err(BridgeError::MessageTooLarge);
                }
                out.push_str(&t);
            }
            Update::Usage(_) => {} // FIX-14: intentionally not recorded
            Update::Permission(_) => {
                return Err(BridgeError::AgentCrashed {
                    reason: "compact summarize requested a permission".into(),
                });
            }
            Update::Done { .. } => break,
        }
    }
    Ok(out)
}
```
(PFIX-7: add `Update` to the module-level `use bridge_core::ports::{...}` at `server.rs:42-45` — `Part`/
`AgentBackend`/`BridgeError`/`SessionId` are already there, only `Update` is missing. Confirm `Part` is
`{ text }` — `domain.rs:7`. PFIX-8: until T7 wires the handler, tag `summarize_collect` + the two consts
`#[allow(dead_code)]` to keep T4–T6 clippy-clean; remove the attribute in T7.)

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p bridge-a2a-inbound --lib summarize_collect_ && cargo test --workspace --no-run`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(compact): summarize_collect helper (full-text drain, byte/permission bounds) (T4)"
```

---

### Task 5: Seed take-and-clear in `checkout_turn` + drop in `reset_session` (FIX-10)

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/session_manager.rs` (`checkout_turn` `:198`/`:268`, `reset_session`
  commit `:510-516`)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn checkout_consumes_seed_once() {
    let (m, _f, _r) = manager();
    let c = ctx("c");
    let t = m.checkout_turn(&c, agent(), None, None, op("t0")).await.unwrap();
    m.finish_turn(&c, t.generation, &t.op).await;
    m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) }).await.unwrap();
    // First checkout after compact carries the seed; clear it; second sees None.
    let t1 = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
    assert_eq!(t1.seed.as_deref(), Some("SUMMARY"));
    m.finish_turn(&c, t1.generation, &t1.op).await;
    let t2 = m.checkout_turn(&c, agent(), None, None, op("t2")).await.unwrap();
    assert_eq!(t2.seed, None);
}

#[tokio::test]
async fn seed_delivered_on_reconcile_checkout() {
    // FIX-10: the seed is ALSO taken at the post-reconcile clean resume return (:261-275), not only clean-diff.
    // Mirror the clean-reconcile setup in `model_override_change_reconciles_and_advances_fingerprint` (:1277).
    let (m, fake, _r) = manager();
    let c = ctx("c");
    let t = m.checkout_turn(&c, agent(), None, None, op("t0")).await.unwrap();
    m.finish_turn(&c, t.generation, &t.op).await;
    m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) }).await.unwrap();
    // A model-override checkout takes the reconcile path; the seed must still be delivered.
    let t1 = m
        .checkout_turn(&c, agent(), Some(model_override("m1")), None, op("t1"))
        .await
        .unwrap();
    assert_eq!(t1.seed.as_deref(), Some("SUMMARY"));
    assert!(!fake.reconciled().is_empty(), "exercised the reconcile resume path");
}

#[tokio::test]
async fn clear_drops_pending_seed() {
    let (m, _f, _r) = manager();
    let c = ctx("c");
    let t = m.checkout_turn(&c, agent(), None, None, op("t0")).await.unwrap();
    m.finish_turn(&c, t.generation, &t.op).await;
    m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) }).await.unwrap();
    m.reset_session(&c, ResetOpts { force: false }).await.unwrap(); // plain clear after compact
    let t1 = m.checkout_turn(&c, agent(), None, None, op("t1")).await.unwrap();
    assert_eq!(t1.seed, None, "clear drops the pending seed");
}
```

- [ ] **Step 2: Run — expect FAIL** (seed always None — not taken yet)

- [ ] **Step 3: Take-and-clear at the two resume returns; drop on reset**

In `checkout_turn`, the clean-diff success (`:193-205`): before building the returned `WarmTurn`, add
`let seed = h.pending_seed.take();` and set `seed,` in the returned `WarmTurn`. Do the SAME at the
post-reconcile clean success (`:261-275`). Do NOT touch the mint path (`:295-335`, leaves `seed: None`) nor any
error/HandleBusy/reseed return.
In `reset_session`'s commit block (`:510-516`), add `h.pending_seed = None;` (clear = empty context).

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p bridge-a2a-inbound --lib 'checkout_consumes_seed_once|clear_drops_pending_seed|seed_delivered_on_reconcile_checkout' && cargo test -p bridge-a2a-inbound --lib && cargo test --workspace --no-run`
Expected: PASS (and no regression — the existing checkout/reset tests still green).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/session_manager.rs
git commit -m "feat(compact): seed take-and-clear on resume checkout; drop on clear (T5, FIX-10)"
```

---

### Task 6: Seed prepend at both dispatch sites + recording server fake (FIX-8)

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`LocalDispatch`, `warm_local_dispatch` `:557`,
  `spawn_local_producer` `:1138/:1150`, the unary collect `~:2354`; the test `FakeBackend`)

- [ ] **Step 1: Write the failing tests** (server warm-test harness — model on `session_clear_dispatch`)

```rust
// Build the warm-session test server (mirrors session_clear_dispatch :6510-6536).
fn seed_test_server(
) -> (Arc<InboundServer>, Arc<crate::session_manager::SessionManager>, Arc<WarmRecordingBackend>) {
    let backend = WarmRecordingBackend::new();
    let registry = FakeRegistry::with_entries(
        "a",
        vec![(bare_entry("a"), backend.clone() as Arc<dyn AgentBackend>)],
    );
    let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
    let sm = Arc::new(crate::session_manager::SessionManager::new(
        registry.clone() as Arc<dyn AgentRegistry>,
        std::time::Duration::from_secs(60),
    ));
    let srv = Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            store,
            Arc::new(AutoApprove),
            Arc::new(RegistryRoute { default: AgentId::parse("a").unwrap() }),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "a",
        )
        .with_session_manager(sm.clone()),
    );
    (srv, sm, backend)
}

async fn wait_idle(sm: &crate::session_manager::SessionManager, ctx: &ContextId) {
    for _ in 0..50 {
        if matches!(sm.status(ctx).await.as_ref().map(|s| s.state), Some("idle")) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("session did not reach idle");
}

fn warm_msg(text: &str) -> serde_json::Value {
    json!({ "message": { "contextId": "c1", "text": text, "metadata": { "a2a-bridge.agent": "a" } } })
}

#[tokio::test]
async fn seed_prepended_unary() {
    let (srv, sm, backend) = seed_test_server();
    let ctx = ContextId::parse("c1").unwrap();
    let r = router(srv.clone())
        .oneshot(post_request(methods::SEND_MESSAGE, warm_msg("go"), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let _ = body_string(r).await;
    wait_idle(&sm, &ctx).await;
    sm.compact_session(&ctx, |_b, _s| async { Ok("S".to_string()) }).await.unwrap();
    backend.clear_prompted_parts(); // drop the warm-up turn; only the seeded turn remains
    let r = router(srv)
        .oneshot(post_request(methods::SEND_MESSAGE, warm_msg("hello"), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let _ = body_string(r).await;
    let parts = backend.prompted_parts();
    let turn = parts.last().expect("a seeded turn was prompted");
    assert_eq!(turn[0], "[Summary of earlier context in this session]\nS");
    assert_eq!(turn[1], "hello");
}

#[tokio::test]
async fn seed_prepended_streaming() {
    let (srv, sm, backend) = seed_test_server();
    let ctx = ContextId::parse("c1").unwrap();
    let r = router(srv.clone())
        .oneshot(post_request(methods::SEND_STREAMING_MESSAGE, warm_msg("go"), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let _ = collect_sse_frames(r).await;
    wait_idle(&sm, &ctx).await;
    sm.compact_session(&ctx, |_b, _s| async { Ok("S".to_string()) }).await.unwrap();
    backend.clear_prompted_parts();
    let r = router(srv)
        .oneshot(post_request(methods::SEND_STREAMING_MESSAGE, warm_msg("hello"), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let _ = collect_sse_frames(r).await;
    let parts = backend.prompted_parts();
    let turn = parts.last().expect("a seeded turn was prompted");
    assert_eq!(turn[0], "[Summary of earlier context in this session]\nS");
    assert_eq!(turn[1], "hello");
}
```
(The `seed_test_server`/`wait_idle`/`warm_msg` helpers mirror `session_clear_dispatch` `server.rs:6509-6536`;
`bare_entry`/`FakeRegistry::with_entries`/`FakeStore`/`AutoApprove`/`RegistryRoute`/`AlwaysGrant`/`NoDelegation`/
`body_string`/`collect_sse_frames`/`post_request`/`router` are all already in the server test module.)

- [ ] **Step 2: Add parts-RECORDING to the WARM test backend** (FIX-6 / PFIX-4)

Edit `WarmRecordingBackend` (`server.rs:5788`) — the warm fake the seed tests use (NOT the generic `FakeBackend`
`:3630`):
```rust
// struct WarmRecordingBackend (:5788) — add:
    prompted_parts: Arc<Mutex<Vec<Vec<String>>>>,
// fn new() (:5794) — add:
    prompted_parts: Arc::new(Mutex::new(Vec::new())),
// impl AgentBackend::prompt (:5804) — rename `_p` -> `p`, FIRST line of the body:
    self.prompted_parts.lock().unwrap().push(p.iter().map(|x| x.text.clone()).collect());
// impl WarmRecordingBackend (:5793) — add accessors:
    fn prompted_parts(&self) -> Vec<Vec<String>> { self.prompted_parts.lock().unwrap().clone() }
    fn clear_prompted_parts(&self) { self.prompted_parts.lock().unwrap().clear(); }
```
(The warm-up turn records `prompted_parts[0]`; after `clear_prompted_parts()` the seeded turn is the only/last
entry — PFIX-4.)

- [ ] **Step 3: Thread the seed through dispatch + prepend at both sites**

Add `seed: Option<String>` to `LocalDispatch`. **PFIX-2: fix ALL THREE construction sites** — `seed: None` at
the legacy `resolve_configure_bind` literals (`server.rs:511` and `:547`) and `seed: turn.seed` at
`warm_local_dispatch` (`:581-591`); else `cargo test --workspace --no-run` fails (missing-field). In
`spawn_local_producer` (`:1138`), after `let parts = routed.parts;`, add:
```rust
let mut parts = parts;
if let Some(seed) = dispatch.seed {
    parts.insert(0, Part { text: format!("[Summary of earlier context in this session]\n{seed}") });
}
```
Apply the IDENTICAL prepend in the unary collect path (`~:2354`, just before its `Translator::run`).

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p bridge-a2a-inbound --lib 'seed_prepended' && cargo test --workspace --no-run`
Expected: PASS; workspace compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(compact): prepend wrapped seed at streaming + unary dispatch sites (T6, FIX-8)"
```

---

### Task 7: `SessionCompact` wire + `session compact` CLI + config knob

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (dispatch `:691`, a `session_compact` handler near
  `session_clear` `:2932`), `bin/a2a-bridge/src/main.rs` (`session_cmd` `:2724`, help `:104`),
  `bin/a2a-bridge/src/config.rs` + the serve wiring (`main.rs:3667`)

- [ ] **Step 1: Write the failing tests** (server)

```rust
// Reuses `seed_test_server`, `wait_idle`, `warm_msg` from T6 (same test module).
#[tokio::test]
async fn session_compact_dispatch() {
    let (srv, sm, _backend) = seed_test_server();
    let ctx = ContextId::parse("c1").unwrap();
    let r = router(srv.clone())
        .oneshot(post_request(methods::SEND_MESSAGE, warm_msg("go"), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let _ = body_string(r).await;
    wait_idle(&sm, &ctx).await;
    // The handler's summarize_collect drives WarmRecordingBackend::prompt -> "warm" (non-empty) -> compacts.
    let r = router(srv)
        .oneshot(post_request("SessionCompact", json!({ "contextId": "c1" }), "1.0"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = serde_json::from_str(&body_string(r).await).unwrap();
    assert_eq!(
        v["result"],
        json!({ "contextId": "c1", "compacted": true, "generation": 1 })
    );
}

#[tokio::test]
async fn session_compact_unknown_ctx_is_not_found() {
    let (srv, _sm, _backend) = seed_test_server();
    let r = router(srv)
        .oneshot(post_request("SessionCompact", json!({ "contextId": "nope" }), "1.0"))
        .await
        .unwrap();
    // SessionNotFound is RejectRequest -> HTTP 400 (matches session_clear_unknown_ctx_is_not_found :6617-6621).
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_str(&body_string(r).await).unwrap();
    assert_eq!(v["error"]["code"], JSONRPC_INVALID_REQUEST);
    assert_eq!(v["error"]["message"], "session not found");
}
```

- [ ] **Step 2: Add the dispatch arm + handler**

In the dispatch match (`:691`) add: `"SessionCompact" => session_compact(srv, headers, id, params).await,`.
Add the handler (model on `session_clear` `:2932`):
```rust
async fn session_compact(srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) {
        return bridge_err_to_jsonrpc(id, &e);
    }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) {
        Ok(c) => c,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    match sm.compact_session(&ctx, |backend, session| summarize_collect(backend, session)).await {
        Ok(crate::session_manager::ResetOutcome::Cleared { generation }) => jsonrpc_ok(
            id,
            json!({ "contextId": ctx.as_str(), "compacted": true, "generation": generation }),
        ),
        Ok(crate::session_manager::ResetOutcome::NotFound) => {
            bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound)
        }
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}
```

- [ ] **Step 3: Add the CLI arm + help + missing-subcommand string** (`main.rs`)

In `session_cmd` `match sub` (`:2731-2737`) add `"compact" => "SessionCompact",`. The `params` for `compact`
use `{ "contextId": ctx }` (no force — it falls in the `else` branch already at `:2741`). Update the
missing-subcommand `.ok_or(...)` string (`:2728`) and the help line (`:104`) to include `compact` (FIX-11).

- [ ] **Step 4: Add the `compact_summarize_timeout_secs` config knob**

In `bin/a2a-bridge/src/config.rs` add `compact_summarize_timeout_secs: Option<u64>` to the `[server]` config
struct (serde default `None`). In the serve wiring where the `SessionManager` is built (`main.rs:3667`), chain
`.with_compact_summarize_timeout(Duration::from_secs(cfg.server.compact_summarize_timeout_secs.unwrap_or(120)))`.
**Test (PFIX/round-4):** extend the existing `warm_idle_ttl_defaults_and_overrides` config test
(`config.rs:2258-2277`) to assert `compact_summarize_timeout_secs` parses to default `None` and to `Some(7)`
on override (catches a serde-field/default typo directly).

- [ ] **Step 5: Run — expect PASS + workspace green**

Run: `cargo test -p a2a-bridge warm_idle_ttl_defaults_and_overrides && cargo test -p bridge-a2a-inbound --lib session_compact && cargo test --workspace --no-run`
Expected: PASS; workspace compiles.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/config.rs
git commit -m "feat(compact): SessionCompact wire + session compact CLI + timeout config knob (T7)"
```

---

### Task 8: Workspace gate + live-gate + merge

**Files:** none (verification)

- [ ] **Step 1: Full gate** (CAPTURE THE REAL EXIT CODE — redirect to a file, do NOT pipe to `tail`)

```bash
cargo test --workspace --no-run                                   # match-exhaustiveness
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace > /tmp/s4-test.out 2>&1; echo "EXIT=$?"     # real exit code
```
Expected: all green, `EXIT=0`. (`bridge-container`/system-integration tests may need the established
`--exclude`/`--skip` per the verify-step memory if run in a hermetic context — here we run host-side, so the
full workspace is fine.)

- [ ] **Step 2: Live-gate** (real serve + codex — copy `examples/a2a-bridge.slice3-livegate.toml` → port 8098)

First build the binary: `cargo build --release --bin a2a-bridge` (the steps below invoke
`./target/release/a2a-bridge`).
Per spec §9, on an ISOLATED serve process (PFIX-9): warm a context (plant a durable codeword + an explicitly-
throwaway token), `session status` (gen 0); capture the serve's `codex-acp` CHILD pid set (e.g. `pgrep -P
<serve_pid> -f codex-acp`, not a global `pgrep`); `session compact C`; `session status` (gen 1, idle, and
`usage.used`/`usage.size` specifically `null`); confirm the SAME child pid set; then `submit` a JSON
null-if-absent probe → codeword recalled (seed survived), throwaway token GONE. Re-run if the model is flaky
(FIX-13).

- [ ] **Step 3: Per-increment + whole-branch reviews + merge**

Each task was committed; run the codex-xhigh per-increment reviews as we go (or a final whole-branch
`git diff main...HEAD` codex-xhigh review — the high-value cross-task pass). Fold any BLOCKER/MAJOR. Then
FF-merge to `main` + push (operator authorizes), update `2026-06-17-orchestration-HANDOFF.md` + memory
(Slice 4 ✅, NEXT = Slice 5).

---

## Self-review (run before dispatching)

- **Spec coverage:** T1 (Compacting+fields/FIX-9-state), T2 (compact_session+timeout/FIX-5), T3
  (EXPIRE/FIX-1/3), T4 (collect/FIX-4/7/14), T5 (seed take-clear+drop/FIX-10), T6 (prepend both sites/FIX-8),
  T7 (wire/CLI/config/FIX-11/12), T8 (gate+live-gate/FIX-13). Every FIX-1..14 maps to a task. ✓
- **No placeholders:** T6/T7 tests are concrete bodies over the `seed_test_server`/`wait_idle`/`warm_msg`
  helpers (mirroring `session_clear_dispatch`); all manager tests use the real `agent()`/`ctx`/`op`/
  `model_override`/`set_configure_result`/`reconciled` helpers. ✓ (Round-3 plan-review folded.)
- **Type consistency:** `compact_session<F,Fut>` signature, `WarmTurn.seed: Option<String>`,
  `LocalDispatch.seed`, `pending_seed: Option<String>`, `ResetOutcome::Cleared{generation}` are used
  consistently across T1/T2/T5/T6/T7. ✓
