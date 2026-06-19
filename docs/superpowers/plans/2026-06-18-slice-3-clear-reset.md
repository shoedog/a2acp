# Slice 3 — Clear / reset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add **`clear`** — reset a warm session's CONTEXT to empty while keeping the PROCESS warm — via a new
bridge `SessionId` per generation (DIVERGENCE-1), with a GENERATION-MONOTONICITY guard so a force-cancelled
old-generation turn can't corrupt the new generation.

**Architecture:** `SessionManager::reset_session(ctx, ResetOpts{force})` composes the shipped
`release_session(old)` + `configure_session(new)` + a generation bump (`ctx-{ctx}-g{N}`); the next turn's lazy
`session/new` gives fresh context on the same warm process. A new `Resetting` state owns the handle across the
async window (mirrors Slice-1 `Reconciling`). `finish_turn`/`record_usage` become generation-scoped (no-op
unless `gen == handle.generation && state == Running`). Wire method `SessionClear` + CLI `session clear`.

**Tech Stack:** Rust workspace — bridge-core, bridge-a2a-inbound, bin/a2a-bridge. Reuses Slice-0
`release_session` (ACP + ContainerRw) + `configure_session` + the lazy ACP `session/new` mint.

**Spec:** `docs/superpowers/specs/2026-06-18-slice-3-clear-reset.md` (v2, dual-reviewed — FIX-1..11 binding).
**Slicing authority:** `2026-06-17-orchestration-slicing.md` (Slice 3). Built on Slices 0–2 (shipped, main).

**Implementor:** codex gpt-5.5/high host (`run-workflow slice0-impl --session-cwd <repo>`), test+impl together,
controller verifies+commits (the `_dyld_start` flake). **Gate enum/signature changes with `cargo test
--workspace --no-run` (`--all-targets`).**

**Grounded seams (verbatim-verified):**
- `session_manager.rs`: `SessionState` (`:18`, Idle/Running/Reconciling/Expiring); `WarmHandle` (`:31`, has
  `generation: SessionGeneration`/`backend_session`/`usage`/`fingerprint`/`op`/`state`); `WarmTurn` (`:52`);
  `checkout_turn` (mint `:271-309` builds `ctx-{ctx}-g0` + `generation: SessionGeneration::new(0)`; the THREE
  `Ok(WarmTurn{..})` returns at the fast-resume / post-reconcile `:246` / mint `:286`); `finish_turn` (`:313`,
  ctx-keyed, unconditional); `record_usage` (`:343`, ctx-keyed, unconditional); `status()` (`:321`, matches all
  4 `SessionState` arms); `release` (`:350`, deferral check `:356`); `cancel` (`:369`, deferral check `:378`);
  `reap_idle` (`:390`); the `checkout_turn` busy-check (`~:161` "Running/Reconciling/Expiring → HandleBusy").
- `bridge-core`: `SessionGeneration(u64)` (`ids.rs:41`, `new`/`get`, NO increment helper); `AgentBackend::
  release_session` (`ports.rs:55`) + `configure_session` (`ports.rs:42`, returns `Result`); `EffectiveConfig`
  carries model/effort/mode (`domain.rs:161`); `SessionSpec{config,cwd}` (`domain.rs:167`); `SessionCwd::parse`;
  `BridgeError::{ConfigInvalid,SessionNotFound,HandleBusy,SessionExpired}` (`error.rs`).
- `acp_backend.rs`: `AgentSession.agent_id`/`minted_cwd` OnceCells (`:275/283`); `AcpBackend::release_session`
  removes `session_cfg`+`sessions[id]` + re-cancels (`~:2048`); `ensure_session` lazy `session/new` mint.
- `bridge-container/src/lib.rs`: `release_session` (`:553`) → `release_warm` (`:424`, cancels + reaps).
- `server.rs`: dispatch match (`:672-684`, string-literal `"SessionStatus"/"SessionRelease"/"SessionCancel"`);
  `session_release` handler (`:2900`); `session_status` usage block (`:2884`); `WarmTurnGuard{sm,ctx}` (`:452`,
  Drop→`finish_turn(&ctx)`); `warm_local_dispatch` builds the guard (`~:571`); `spawn_local_producer` usage tap
  (`~:1164` `w.sm.record_usage(&w.ctx, snap.clone())`); unary usage tap (`~:2340`); `context_id_arg` helper.
- `bin/a2a-bridge/src/main.rs`: `session_cmd` (`:2724`, maps `status|release|cancel` → `Session*`); the CLI
  help (`:104`).

---

> **POST-MERGE NOTE (whole-branch review FIX-12 + Deferred hardening):** after the per-task plan executed, a
> whole-branch codex-xhigh review added **FIX-12** — `finish_turn`/`record_usage` ALSO require the
> `OperationId` (`gen == generation && op == Some(op) && state == Running`) and `WarmTurn`/`WarmTurnGuard`
> carry it. So the `finish_turn`/`record_usage` pseudocode below (`(ctx, gen)` only) is STALE — see the SPEC's
> FIX-12 + "## Deferred hardening" for the shipped guard + the two PRE-EXISTING races (task-derived op
> collision; `force`-clear vs producer-start) that are DEFERRED to a "warm-turn cancellation tokens"
> follow-up. Do NOT re-implement `finish_turn`/`record_usage` from the pseudocode below without the `op`.

## v2 — dual plan-review fixes folded (codex-xhigh + Opus, both `fix-then-execute`)

Both lenses verified the ALGORITHM is sound (Opus independently confirmed: the gen-guard closes the
Resetting-window hole; the claim revalidation has no ABA; the new-generation id scheme structurally removes the
backend_session-reuse race; harness + imports all exist). The gaps are test authoring + one missed call site.
These resolutions AMEND the tasks below; where they conflict, THESE win.

- **PF-1 (BLOCKER, both) — T1's call-site grep is too narrow.** The `finish_turn`/`record_usage` signature
  change ripples to a direct `record_usage` call in `server.rs` too (`server.rs:6366`, the Slice-2
  `session_status_release_cancel_dispatch` test), plus the direct session-manager test calls at
  `session_manager.rs:1252/1285/1316`. Step 3d must grep `\.finish_turn(&` / `\.record_usage(&` across the
  **whole `bridge-a2a-inbound` crate** (NOT just `session_manager.rs`), thread `SessionGeneration::new(0)` at
  each (the handle is freshly minted → gen 0), AND **update the Slice-2 usage tests to record WHILE RUNNING**
  (the new `state==Running` guard makes a record-after-`finish_turn` a no-op): `checkout_turn` →
  `record_usage(turn.generation, snap)` (Running) → `finish_turn(turn.generation)`. (No `bin/` callers exist —
  crate scope suffices.)
- **PF-2 (MAJOR, both) — the T2 "usage zeroed" test is mis-authored and proves nothing.** As written it
  records usage AFTER `finish_turn` (Idle → no-op under T1), so `usage.used` is already `None` and the
  post-reset assert passes trivially (never exercises FIX-11). **Author-correct it:** record `used:Some(7)`
  WHILE Running, `finish_turn`, THEN `reset_session`, then assert `usage.used == None`.
- **PF-3 (MAJOR, codex) — the keystone stale-write test must exercise the `Resetting` WINDOW, not the
  post-commit case.** The v1 `stale_finish_turn_after_reset` calls `finish_turn(gen0)` after reset commits to
  gen1 — a generation-only guard would also pass, so it does NOT prove the `&& state==Running` half. Replace
  with a **blocked-reset** test: extend `FakeBackend` with a `block_next_configure` (or `block_next_release`)
  oneshot gate (mirror the existing `block_next_reconcile` at `session_manager.rs:~468`); (1) checkout gen0
  Running; (2) `tokio::spawn` `reset_session(force:true)`; (3) wait until `status().state == "resetting"`;
  (4) `finish_turn(ctx, gen0)` + `record_usage(ctx, gen0, ..)` → assert they NO-OP (state stays `"resetting"`,
  usage unchanged) **even though `gen0 == handle.generation`** — this is what the `state` predicate buys;
  (5) unblock → assert commit to gen1 Idle + empty usage.
- **PF-4 (MAJOR, codex) — add the FIX-4/FIX-7 path tests** (the spec's unit checklist): (a) `configure_session`
  returns `Err` → the handle is EXPIRED (removed + lease dropped) and `reset_session` returns the ORIGINAL
  error; (b) a `checkout_turn` while `Resetting` → `HandleBusy`; (c) a `release`/`cancel` while `Resetting`
  sets the deferred flag → `reset_session` returns `SessionExpired` + the handle is removed. Use the
  `block_next_*` gate to hold the handle in `Resetting` for (b)/(c).
- **PF-5 (MINOR, codex) — the reset commit must clear `op`.** Add `h.op = None;` before `h.state =
  SessionState::Idle;` in the commit block (matches `finish_turn:315` / `cancel:383`).
- **PF-6 (MINOR, codex) — one cargo test filter per command.** Split T1 Step-2's
  `cargo test ... finish_turn_ record_usage_noops_on_stale` into two runs (or one broader filter).
- **PF-7 (MINOR, Opus) — the deferred-EXPIRE path leaks the freshly-configured `new_id` stash** (benign — one
  HashMap entry; `configure_session` is pure stash for ACP + ContainerRw, no process/container leak). In the
  `cfg.is_ok() && deferred` sub-case, `drop(tab); backend.release_session(&new_id).await;` BEFORE removing the
  handle (mirror the Slice-1 `Expiring`-across-release shape), then re-acquire to remove + drop the lease.
- **PF-8 (MINOR, Opus) — drop the redundant `force` pre-`cancel`.** `release_session(old_id)` already
  re-cancels (`acp_backend.rs:2052`), so `if opts.force { backend.cancel(old_id) }` is dead; remove it (or keep
  with a `// release re-cancels; this only narrows cancel→release latency` comment).
- **PF-9 (MINOR, Opus) — an `is_claimed(state)` helper guards FIX-7 drift.** Add
  `fn is_claimed(s: SessionState) -> bool { matches!(s, Reconciling | Expiring | Resetting) }` and use it at
  the `release`/`cancel` deferral checks (the `checkout` busy-check stays `if state != Idle` — adding the enum
  variant auto-covers it). Keeps "every claim-state defers" un-droppable when S4 adds a 4th claim state.

> **Note (Opus MN2, no change):** the `new_id` `SessionId::parse` failure maps to `InvalidRequest{field:
> "contextId"}` (not `ConfigInvalid`) DELIBERATELY — it mirrors the shipped mint (`session_manager.rs:280`);
> only the cwd parse → `ConfigInvalid` (FIX-10). Keep both as-is for parity.

### v3 — round-2 codex re-review fixes folded (`fix-then-execute`; core algorithm/wire/guard/deferral CONFIRMED)

- **PF-10 (MAJOR, T1) — `cancel` must refresh `last_used` on `Running→Idle`, or T1 introduces a TTL-reap
  regression.** Today `cancel` (`session_manager.rs:369`) idles the handle WITHOUT touching `last_used`,
  relying on the producer's `WarmTurnGuard::Drop`→`finish_turn` to refresh it. After T1's guard the Drop
  `finish_turn` no-ops (state already `Idle`), so a cancelled session's `last_used` stays at turn-start and
  `reap_idle` (`:390`) can evict it early. **Fix (fold into T1):** in `cancel`'s `Running→Idle` arm:
  ```rust
  let was_running = h.state == SessionState::Running;
  h.state = SessionState::Idle;
  h.op = None;
  if was_running { h.last_used = (self.now)(); }   // PF-10: cancel now owns the idle refresh
  ```
  + a `ManualClock` test: checkout → advance past TTL → `cancel` → stale `finish_turn(turn.generation)` →
  `reap_idle` KEEPS the now-idle session (cancel refreshed `last_used`); advance TTL again → reap removes it.
- **PF-11 (MAJOR, T2) — the keystone test must assert usage is UNCHANGED *during* the `Resetting` window.**
  Asserting after commit is moot (`UsageSnapshot::default()` clears it regardless). **Fix:** seed `used:Some(7)`
  while Running (before the reset), write `used:Some(99)` during the `Resetting` window, and assert
  `status().usage.used == Some(7)` (the `Some(99)` write was DROPPED) — that is what proves `record_usage`'s
  `&& state==Running` half. (Then the post-commit assert `used == None` proves the zeroing — both matter.)
- **PF-12 (MINOR, T2) — add `cancel_during_resetting_is_deferred`.** The PF-4 path test covers `release` during
  `Resetting`; add the sibling for `cancel` (separate deferral site `:378`): same `block_next_configure` setup,
  `manager.cancel(&c).await.unwrap()` during `"resetting"`, unblock, assert `SessionExpired` + `status` is
  `None`.

> Round-2 codex CONFIRMED: the core reset algorithm, `SessionClear` wire shape, generation guard, `Resetting`
> deferral model, usage reset all match v2; the whole-crate PF-1 grep is correct; `Resetting` is
> compiler-guided through `status()`. Residuals are the cancel-TTL refresh (PF-10) + two test-rigor items.

### v4 — round-3 codex re-review fix folded (REVERSES PF-8)

- **PF-13 (MAJOR, round-3) — RESTORE the `force` pre-cancel; PF-8 was wrong for non-ACP backends.** Dropping
  the pre-cancel assumed `release_session(old)` always re-cancels — true for ACP (`acp_backend.rs:2052`) +
  ContainerRw, but the trait-DEFAULT `release_session` = `forget_session` (`ports.rs:55`), and `ApiBackend`'s
  `forget_session` (`bridge-api/src/backend.rs:244`) only drops the slot — it does NOT cancel the running
  stream (`ApiBackend::cancel` `:222` is the cancel). `serve` wires `SessionManager` for ALL agents incl.
  `AgentKind::Api` (`main.rs:3490/3656`). So `clear --force` on an API agent would reset without cancelling the
  running turn (violates FIX-2/DoD-4). **Fix:** KEEP `if opts.force { let _ = backend.cancel(&old_id).await; }`
  before `release_session(old_id)` (done in the §reset_session code). + a test (add a `FakeBackend::cancels()`
  accessor mirroring `releases()`, `:397`): on `force` reset of a Running handle, `old_id` appears in BOTH
  `cancels()` AND `releases()`, and the generation advances.

> Round-3 codex CONFIRMED: core reset algorithm, generation guard, deferral model, wire method, CLI shape all
> correct after the force-cancel fix; PF-13 was the sole finding.

---

## Task 1: Generation-scoped `finish_turn`/`record_usage` + thread `generation` through the warm turn (FIX-3)

**Files:** Modify `crates/bridge-a2a-inbound/src/session_manager.rs` (`WarmTurn`, `finish_turn`, `record_usage`,
the 3 `WarmTurn` returns, + the direct test call sites); `crates/bridge-a2a-inbound/src/server.rs`
(`WarmTurnGuard`, `warm_local_dispatch`, the two `record_usage` taps).

- [ ] **Step 1: Write failing tests** (session_manager test module):

```rust
#[tokio::test]
async fn finish_turn_applies_on_matching_generation_and_running() {
    let (manager, _b, _r) = manager();
    let c = ctx("ft");
    let turn = manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.finish_turn(&c, turn.generation).await;           // gen matches, state was Running
    assert_eq!(manager.status(&c).await.unwrap().state, "idle");
}

#[tokio::test]
async fn finish_turn_noops_on_stale_generation() {
    let (manager, _b, _r) = manager();
    let c = ctx("ft-stale");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap(); // Running, gen 0
    manager.finish_turn(&c, SessionGeneration::new(99)).await;  // stale gen -> NO-OP
    assert_eq!(manager.status(&c).await.unwrap().state, "running");
}

#[tokio::test]
async fn record_usage_noops_on_stale_generation() {
    let (manager, _b, _r) = manager();
    let c = ctx("ru-stale");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.record_usage(&c, SessionGeneration::new(99),
        UsageSnapshot { used: Some(5), size: Some(9), cost: None, at_ms: 0 }).await;
    assert_eq!(manager.status(&c).await.unwrap().usage.used, None); // stale -> not recorded
}
```

- [ ] **Step 2: Run to verify fail** (PF-6: one filter per run) — `cargo test -p bridge-a2a-inbound --lib
finish_turn_noops_on_stale_generation` then `cargo test -p bridge-a2a-inbound --lib record_usage_noops_on_stale`
(signatures don't take a generation yet).

- [ ] **Step 3a: `WarmTurn` carries the generation** (`session_manager.rs:52`):

```rust
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub usage_warning: Option<UsageWarning>,
    pub generation: SessionGeneration,
}
```
Set `generation: h.generation` on the fast-resume + post-reconcile `Ok(WarmTurn{..})` returns, and
`generation: SessionGeneration::new(0)` on the mint return (`:286`). (Grep `WarmTurn {` — there are exactly 3.)

- [ ] **Step 3b: Generation-scoped, state-guarded completion** (`finish_turn` `:313`, `record_usage` `:343`).
`SessionGeneration` is `Copy`+`PartialEq` (`ids.rs:41`):

```rust
/// Mark the current turn finished -> Idle (keep warm). FIX-3: no-op unless this is the SAME generation
/// AND the handle is Running (a turn only legitimately idles a Running handle); a stale (reset-away or
/// claim-state) completion touches NOTHING.
pub async fn finish_turn(&self, ctx: &ContextId, gen: SessionGeneration) {
    if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
        if h.generation == gen && h.state == SessionState::Running {
            h.state = SessionState::Idle;
            h.op = None;
            h.last_used = (self.now)();
        }
    }
}

pub async fn record_usage(&self, ctx: &ContextId, gen: SessionGeneration, mut snap: UsageSnapshot) {
    snap.at_ms = crate::workflow_sink::now_ms();
    if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
        if h.generation == gen && h.state == SessionState::Running {
            h.usage = snap;
        }
    }
}
```
(FIX-3: key solely on `(ctx, generation)` + `state==Running`; ignore `op`. The mismatch branch mutates
nothing.)

- [ ] **Step 3c: Thread it through the producer** (`server.rs`). `WarmTurnGuard` (`:452`):

```rust
struct WarmTurnGuard {
    sm: std::sync::Arc<crate::session_manager::SessionManager>,
    ctx: bridge_core::ids::ContextId,
    generation: bridge_core::ids::SessionGeneration,
}
impl Drop for WarmTurnGuard {
    fn drop(&mut self) {
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        tokio::spawn(async move { sm.finish_turn(&ctx, generation).await; });
    }
}
```
In `warm_local_dispatch` (`~:571`) build it with `generation: turn.generation`. In BOTH usage taps
(`spawn_local_producer` `~:1164`, unary `~:2340`) change `w.sm.record_usage(&w.ctx, snap.clone())` →
`w.sm.record_usage(&w.ctx, w.generation, snap.clone())`.

- [ ] **Step 3d: Update EVERY direct call site (PF-1 — whole crate, not just session_manager.rs).** Grep
`\.finish_turn(&` and `\.record_usage(&` across **all of `crates/bridge-a2a-inbound/`** — the Slice-0/1/2 tests
call them directly in `session_manager.rs` (incl. `:1252/1285/1316`) AND there is a direct
`sm.record_usage(&ctx, …)` in `server.rs:6366` (the `session_status_release_cancel_dispatch` test). Pass the
handle's generation — after a fresh `checkout_turn` it is `SessionGeneration::new(0)`; capture `turn.generation`
where a `WarmTurn` is in hand, else `SessionGeneration::new(0)`. **Because `record_usage` now no-ops unless
`state==Running`, any test that recorded usage AFTER `finish_turn` must move the `record_usage` to BEFORE
`finish_turn` (while Running)** — e.g. the Slice-2 `record_usage_latest_wins_stamps_at_ms` and the
`server.rs:6366` status test (record while the warm turn is Running, or use a backend that emits
`Update::Usage` during the request).

- [ ] **Step 3e: PF-10 — `cancel` owns the idle refresh.** Because the producer's `WarmTurnGuard::Drop`
  `finish_turn` now no-ops once `cancel` has idled the handle, `cancel` must refresh `last_used` itself or a
  cancelled session reaps early. In `SessionManager::cancel` (`:369`), the `Running→Idle` arm:
  ```rust
      let was_running = h.state == SessionState::Running;
      h.state = SessionState::Idle;
      h.op = None;
      if was_running { h.last_used = (self.now)(); }
  ```
  Add a `ManualClock` test `cancel_refreshes_idle_ttl`: checkout → `clock.advance(> ttl)` → `cancel` → stale
  `finish_turn(turn.generation)` → `reap_idle` KEEPS the session (cancel refreshed `last_used`); then
  `clock.advance(> ttl)` again → `reap_idle` removes it.

- [ ] **Step 4: Run + gate** — `cargo test -p bridge-a2a-inbound --lib && cargo test --workspace --no-run`
(catch every `finish_turn`/`record_usage` call site) → PASS.

- [ ] **Step 5: Commit** — `feat(inbound): generation-scoped finish_turn/record_usage + thread generation through WarmTurn (FIX-3)`

---

## Task 2: `SessionManager::reset_session` + the `Resetting` state + deferral sites (FIX-2/4/5/6/7/8/10/11)

**Files:** Modify `crates/bridge-a2a-inbound/src/session_manager.rs`.

- [ ] **Step 1: Write failing tests:**

**First, extend `FakeBackend` (`session_manager.rs:432`) with a configure gate** (mirror the existing
`block_next_reconcile`/`reconcile_gate`/`reconcile_started` at `~:468`): a `configure_gate:
StdMutex<Option<oneshot::Receiver<()>>>` + `configure_started: Notify` + `configure_started_count: AtomicUsize`;
`block_next_configure() -> oneshot::Sender<()>`; in `configure_session`, after pushing to `configured`, notify
`configure_started` then `if let Some(rx)=gate.take() { let _ = rx.await; }`. (This lets a test hold `reset` in
the `Resetting` window.)

```rust
#[tokio::test]
async fn reset_on_idle_bumps_generation_releases_old_configures_new_zeroes_usage() {
    let (manager, backend, _r) = manager();
    let c = ctx("reset");
    let turn = manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.record_usage(&c, turn.generation,                       // PF-2: record WHILE Running
        UsageSnapshot { used: Some(7), size: Some(9), cost: None, at_ms: 0 }).await;
    manager.finish_turn(&c, turn.generation).await;                // -> Idle, usage = {7,9}
    let out = manager.reset_session(&c, ResetOpts { force: false }).await.unwrap();
    assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
    let s = manager.status(&c).await.unwrap();
    assert_eq!(s.generation, 1);
    assert_eq!(s.usage.used, None);                                 // FIX-11: zeroed (was Some(7))
    assert_eq!(s.state, "idle");
    assert_eq!(backend.releases(), vec!["ctx-reset-g0"]);           // old released
    assert!(backend.configured().contains(&"ctx-reset-g1".to_string())); // new configured
}

#[tokio::test]
async fn reset_on_running_without_force_is_handle_busy() {
    let (manager, _b, _r) = manager();
    let c = ctx("reset-busy");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap(); // Running
    let err = manager.reset_session(&c, ResetOpts { force: false }).await.err().unwrap();
    assert_eq!(err, BridgeError::HandleBusy);
}

#[tokio::test]
async fn reset_unknown_ctx_is_not_found() {
    let (manager, _b, _r) = manager();
    let out = manager.reset_session(&ctx("nope"), ResetOpts { force: false }).await.unwrap();
    assert!(matches!(out, ResetOutcome::NotFound));
}

// PF-3 KEYSTONE: exercise the Resetting WINDOW (gen still old) — proves the `&& state==Running` half.
#[tokio::test]
async fn stale_completion_during_resetting_window_is_dropped() {
    let backend = Arc::new(FakeBackend::new("ok"));
    let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
    let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
    let c = ctx("reset-window");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap(); // gen0 Running
    manager.record_usage(&c, SessionGeneration::new(0),                        // PF-11: seed while Running
        UsageSnapshot { used: Some(7), size: Some(9), cost: None, at_ms: 0 }).await;
    let unblock = backend.block_next_configure();
    let in_flight = {
        let (m, c2) = (manager.clone(), c.clone());
        tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: true }).await })
    };
    // wait until the handle is claimed Resetting (generation STILL 0):
    loop {
        if manager.status(&c).await.map(|s| s.state) == Some("resetting") { break; }
        tokio::task::yield_now().await;
    }
    // stale gen-0 completions arrive DURING the window (gen0 == handle.generation, but state==Resetting):
    manager.finish_turn(&c, SessionGeneration::new(0)).await;
    manager.record_usage(&c, SessionGeneration::new(0),
        UsageSnapshot { used: Some(99), size: Some(100), cost: None, at_ms: 0 }).await;
    let mid = manager.status(&c).await.unwrap();
    assert_eq!(mid.state, "resetting");          // finish_turn did NOT idle (state guard)
    assert_eq!(mid.usage.used, Some(7));         // PF-11: the Some(99) write was DROPPED (proves record_usage half)
    unblock.send(()).unwrap();
    assert!(matches!(in_flight.await.unwrap().unwrap(), ResetOutcome::Cleared { generation: 1 }));
    let s = manager.status(&c).await.unwrap();
    assert_eq!(s.generation, 1);
    assert_eq!(s.state, "idle");
    assert_eq!(s.usage.used, None);              // reset zeroed the carried Some(7)
}

// PF-4: FIX-4 configure-failure -> EXPIRE + original error.
#[tokio::test]
async fn reset_configure_failure_expires_handle_and_returns_error() {
    let (manager, backend, _r) = manager();
    let c = ctx("reset-cfg-fail");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.finish_turn(&c, SessionGeneration::new(0)).await;
    backend.set_configure_result(Err(BridgeError::ConfigInvalid { reason: "boom".into() })); // new gate on FakeBackend
    let err = manager.reset_session(&c, ResetOpts { force: false }).await.err().unwrap();
    assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
    assert!(manager.status(&c).await.is_none());                    // EXPIRED (removed)
    assert_eq!(backend.releases(), vec!["ctx-reset-cfg-fail-g0"]);  // old released, handle gone
}

// PF-4: FIX-7 checkout during Resetting -> HandleBusy; release during Resetting -> deferral -> SessionExpired.
#[tokio::test]
async fn checkout_and_release_during_resetting_are_deferred() {
    let backend = Arc::new(FakeBackend::new("ok"));
    let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
    let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
    let c = ctx("reset-defer");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.finish_turn(&c, SessionGeneration::new(0)).await;
    let unblock = backend.block_next_configure();
    let in_flight = {
        let (m, c2) = (manager.clone(), c.clone());
        tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: false }).await })
    };
    loop {
        if manager.status(&c).await.map(|s| s.state) == Some("resetting") { break; }
        tokio::task::yield_now().await;
    }
    let busy = manager.checkout_turn(&c, agent(), None, None, op("op-2")).await.err().unwrap();
    assert_eq!(busy, BridgeError::HandleBusy);                      // checkout deferred
    manager.release(&c).await;                                     // release defers (expire_after_reconcile)
    unblock.send(()).unwrap();
    assert_eq!(in_flight.await.unwrap().err().unwrap(), BridgeError::SessionExpired);
    assert!(manager.status(&c).await.is_none());                   // handle removed by the deferred release
}

// PF-12: cancel during Resetting defers to the reset's resolve (separate deferral site :378).
#[tokio::test]
async fn cancel_during_resetting_is_deferred() {
    let backend = Arc::new(FakeBackend::new("ok"));
    let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
    let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
    let c = ctx("reset-cancel-defer");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.finish_turn(&c, SessionGeneration::new(0)).await;
    let unblock = backend.block_next_configure();
    let in_flight = {
        let (m, c2) = (manager.clone(), c.clone());
        tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: false }).await })
    };
    loop {
        if manager.status(&c).await.map(|s| s.state) == Some("resetting") { break; }
        tokio::task::yield_now().await;
    }
    manager.cancel(&c).await.unwrap();                              // cancel defers (is_claimed -> flag)
    unblock.send(()).unwrap();
    assert_eq!(in_flight.await.unwrap().err().unwrap(), BridgeError::SessionExpired);
    assert!(manager.status(&c).await.is_none());                   // handle removed by the deferred cancel
}

// PF-13: force reset of a Running handle CANCELS and RELEASES the old id (force-drain even for non-ACP).
#[tokio::test]
async fn force_reset_cancels_and_releases_old_id() {
    let (manager, backend, _r) = manager();
    let c = ctx("reset-force");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap(); // Running
    let out = manager.reset_session(&c, ResetOpts { force: true }).await.unwrap();
    assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
    assert!(backend.cancels().contains(&"ctx-reset-force-g0".to_string()));   // force cancelled old
    assert!(backend.releases().contains(&"ctx-reset-force-g0".to_string()));  // and released old
    assert_eq!(manager.status(&c).await.unwrap().generation, 1);
}
```
(`FakeBackend` test-harness additions: (a) a `set_configure_result(Result<(),BridgeError>)` gate + the
`configure_session` impl returns it — mirror `set_reconcile_result`/`reconcile_result` at `:439/:474`;
(b) a `block_next_configure() -> oneshot::Sender<()>` + the gate await in `configure_session` — mirror
`block_next_reconcile` at `~:468`; (c) a `cancels()` accessor mirroring `releases()` at `:397` — the
`cancels` field already exists at `:436`. The keystone + deferral tests use `block_next_configure` to hold the
`Resetting` window.)

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3a: Add `Resetting` to `SessionState`** (`:18`) + the `status()` arm (`:324`, compiler-forces it):

```rust
pub enum SessionState { Idle, Running, Reconciling, Expiring, Resetting }
// status(): SessionState::Resetting => "resetting",
```

- [ ] **Step 3b: Handle `Resetting` at the deferral sites (FIX-7) via a drift-proof helper (PF-9).** Add a
  free fn (module scope): `fn is_claimed(s: SessionState) -> bool { matches!(s, SessionState::Reconciling |
  SessionState::Expiring | SessionState::Resetting) }`. Then:
  - `release` (`:356`): replace the `Reconciling | Expiring` check with
    `if is_claimed(h.state) { h.expire_after_reconcile = true; return; }`.
  - `cancel` (`:378`): same — `if is_claimed(h.state) { h.expire_after_reconcile = true; return Ok(()); }`.
  - `checkout_turn` busy-check (`~:161`): VERIFY it is written as `if h.state != SessionState::Idle { return
    Err(HandleBusy) }` (Opus confirmed it is) — then adding the `Resetting` enum variant auto-covers it, no
    edit needed. (If it enumerates states instead, add `Resetting`.)

- [ ] **Step 3c: Add the public types + `reset_session`:**

```rust
pub struct ResetOpts { pub force: bool }
#[derive(Debug, PartialEq)]
pub enum ResetOutcome { Cleared { generation: u64 }, NotFound }

/// Clear a warm session's context: NEW generation-scoped backend SessionId, release the old, keep the
/// process/lease/handle warm (DIVERGENCE-1). Require Idle unless `force` (cancel-then-reset). [Slice 3]
pub async fn reset_session(&self, ctx: &ContextId, opts: ResetOpts)
    -> Result<ResetOutcome, BridgeError> {
    // (1)+(2)+(3) claim under ONE lock hold (FIX-2: never bounce through Idle, never call self.cancel).
    // Capture the backend Arc HERE (don't re-lock to fetch it later).
    let (backend, old_id, claimed_id, new_gen, new_id, spec) = {
        let mut tab = self.by_context.lock().await;
        let Some(h) = tab.get_mut(ctx) else { return Ok(ResetOutcome::NotFound); };
        match h.state {
            SessionState::Idle => {}
            SessionState::Running if opts.force => {}     // claim directly from Running
            _ => return Err(BridgeError::HandleBusy),     // Running w/o force, or another claim owns it
        }
        let backend = h.backend.clone();
        let old_id = h.backend_session.clone();
        let claimed_id = h.id.clone();
        let new_gen = SessionGeneration::new(h.generation.get() + 1); // FIX-8: no increment helper
        let new_id = SessionId::parse(format!("ctx-{}-g{}", ctx.as_str(), new_gen.get()))
            .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
        // FIX-6: reconstruct SessionSpec from the fingerprint (superset of SessionSpec's configurable fields).
        let cwd = match h.fingerprint.cwd.as_deref() {
            Some(s) => Some(SessionCwd::parse(s).map_err(|_| BridgeError::ConfigInvalid {  // FIX-10
                reason: "session cwd".into() })?),
            None => None,
        };
        let spec = SessionSpec { config: h.fingerprint.config.clone(), cwd };
        h.state = SessionState::Resetting;
        h.expire_after_reconcile = false;
        (backend, old_id, claimed_id, new_gen, new_id, spec)
    };

    // (4)+(5) async, lock dropped. PF-13: a best-effort cancel BEFORE release — the trait-DEFAULT
    // release_session (e.g. ApiBackend) only `forget_session`s the slot and does NOT cancel the running
    // stream (only ACP/ContainerRw overrides re-cancel); `force` must explicitly cancel a running non-ACP
    // turn. FIX-2: the old producer's late writes target old_id (being released) -> inert.
    if opts.force { let _ = backend.cancel(&old_id).await; }
    backend.release_session(&old_id).await;
    let cfg = backend.configure_session(&new_id, &spec).await; // FIX-4: CAPTURE, do NOT `?`

    // (6) re-acquire + revalidate exact claim; commit or EXPIRE.
    let mut tab = self.by_context.lock().await;
    let still_ours = matches!(tab.get(ctx), Some(h) if h.id == claimed_id && h.state == SessionState::Resetting);
    let new_stashed = cfg.is_ok();   // PF-15: a new_id stash exists iff configure succeeded
    if !still_ours {
        // PF-15: symmetric cleanup — release the stashed new_id even on the (today-unreachable) lost-claim path.
        drop(tab);
        if new_stashed { backend.release_session(&new_id).await; }
        return Err(BridgeError::SessionExpired);
    }
    let deferred = tab.get(ctx).map(|h| h.expire_after_reconcile).unwrap_or(true);
    if cfg.is_err() || deferred {
        // EXPIRE (Slice-1 non-clean path). PF-7: release the stashed new_id (handle still claimed Resetting
        // in the map during this await -> a concurrent checkout stays HandleBusy).
        drop(tab);
        if new_stashed { backend.release_session(&new_id).await; }
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.remove(ctx) { drop(h.lease); }
        return match cfg { Err(e) => Err(e), Ok(()) => Err(BridgeError::SessionExpired) };
    }
    let h = tab.get_mut(ctx).expect("still_ours");
    h.backend_session = new_id;
    h.generation = new_gen;
    h.usage = UsageSnapshot::default();                       // FIX-11: used/size None
    h.op = None;                                              // PF-5: Idle clears op (matches finish_turn/cancel)
    h.state = SessionState::Idle;
    h.last_used = (self.now)();
    Ok(ResetOutcome::Cleared { generation: new_gen.get() })
}
```
Add `use bridge_core::ids::SessionGeneration;` is already imported (`:7`); `SessionCwd` is imported (`:11`).

- [ ] **Step 4: Run + gate** — `cargo test -p bridge-a2a-inbound --lib reset && cargo test --workspace --no-run` → PASS.

- [ ] **Step 5: Commit** — `feat(inbound): SessionManager::reset_session (new-generation clear, Resetting claim, EXPIRE-on-fail)`

---

## Task 3: `SessionClear` wire method + CLI `session clear` (FIX-1/9)

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (dispatch + a `session_clear` handler);
`bin/a2a-bridge/src/main.rs` (`session_cmd` + help).

- [ ] **Step 1: Write failing tests** (server.rs tests — mirror the warm `session_status` test): a warm
contextId, **then POLL for `state=="idle"` (the warm `WarmTurnGuard::Drop`→`finish_turn` is async via
`tokio::spawn`, `server.rs:457`; reuse the existing poll loop at `server.rs:6357`) BEFORE sending a non-force
`SessionClear`** (PF-14 — else it races into `HandleBusy`); assert it returns `{contextId, cleared:true,
generation:1}`; `SessionClear` on an unknown ctx → JSON-RPC error mapping `SessionNotFound` (FIX-9).

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3a: Dispatch** (`server.rs:681`, after `"SessionCancel"`):

```rust
        "SessionClear" => session_clear(srv, headers, id, params).await,
```

- [ ] **Step 3b: Handler** (mirror `session_release` `:2900`):

```rust
async fn session_clear(srv: Arc<InboundServer>, headers: HeaderMap, id: Value, params: Value) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) { return bridge_err_to_jsonrpc(id, &e); }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) { Ok(c) => c, Err(e) => return bridge_err_to_jsonrpc(id, &e) };
    let force = params.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    match sm.reset_session(&ctx, crate::session_manager::ResetOpts { force }).await {
        Ok(crate::session_manager::ResetOutcome::Cleared { generation }) =>
            jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "cleared": true, "generation": generation })),
        Ok(crate::session_manager::ResetOutcome::NotFound) =>           // FIX-9: detached-only ctx
            bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound),
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}
```

- [ ] **Step 3c: CLI** (`main.rs:2724` `session_cmd`): add `"clear" => "SessionClear"` to the `method` match,
and thread `--force` into the params:

```rust
    let force = args.iter().any(|a| a == "--force");
    let params = if sub == "clear" {
        serde_json::json!({ "contextId": ctx, "force": force })
    } else {
        serde_json::json!({ "contextId": ctx })
    };
    let v = rpc_call(url, method, params).await?;
```
Update the subcommand error string to `status|release|cancel|clear` and add `clear` to the CLI help (`:104`).

- [ ] **Step 4: Run + gate** — `cargo test -p bridge-a2a-inbound --lib session_clear && cargo build --workspace` → PASS.

- [ ] **Step 5: Commit** — `feat: SessionClear wire method + CLI session clear (FIX-1)`

---

## Task 4: Workspace gate + live-gate + merge

- [ ] **Step 1: Gate** — `cargo test --workspace --no-run`; then `cargo fmt --all --check && cargo clippy
--workspace --all-targets -- -D warnings && cargo test --workspace` (capture the REAL exit code — redirect, do
NOT pipe to `tail`, which masks cargo's exit).

- [ ] **Step 2: Build release** — `cargo build --release -p a2a-bridge`.

- [ ] **Step 3: Live-gate (real codex; `examples/a2a-bridge.slice2-livegate.toml` reused or a slice3 variant):**
  - **DoD-1 (clear drops context):** `submit --context C` "remember the codeword ZEBRA"; `session clear C`;
    `submit --context C` "what was the codeword?" → the agent does NOT know it. Control (no clear) recalls.
  - **DoD-2 (process stays warm):** a `pgrep -f codex-acp` watcher shows the shared process count UNCHANGED
    across `clear`; the post-clear turn pays no cold start.
  - **DoD-3 (generation advances + usage resets):** `session status C` shows `generation` 0→1→2 across clears;
    `usage.used/size` go null right after a clear, repopulate after the next turn.
  - **DoD-5 (require-Idle):** `session clear C` while a turn is Running → `HandleBusy` (no `--force`).
  - **DoD-4 (force + stale-write):** the precise race is unit-gated (Task 2's `stale_finish_turn_after_reset`);
    live-prove the force path end-to-end: a longer turn in flight + `session clear C --force` cancels + resets,
    and a follow-up recalls NOTHING from before.
  - **DoD-6 (no regression):** Slice 0/1/2 live scenarios (warm continue, reconcile via `--effort`, release,
    idle reap, usage in `session status`, threshold warn) still green across a clear.

- [ ] **Step 4: Record results** + `superpowers:finishing-a-development-branch` (FF-merge to main).

---

## Self-review notes

- **Spec coverage:** generation guard (T1, FIX-3), `reset_session` new-generation clear (T2,
  FIX-2/4/5/6/8/10/11), `Resetting` + deferral sites (T2, FIX-7), `SessionClear` wire + CLI + detached→NotFound
  (T3, FIX-1/9), gate+live (T4). `clear`==`reset_session` primitive; compact/journal/MCP correctly absent.
- **Type consistency:** `WarmTurn.generation`/`WarmTurnGuard.generation`/`finish_turn(ctx,gen)`/
  `record_usage(ctx,gen,snap)` (T1) consumed by the producers + T2/T3; `ResetOpts`/`ResetOutcome`/
  `reset_session` (T2) consumed by T3's handler.
- **Risk hotspots for the plan-review:** T1 (the `finish_turn`/`record_usage` signature change ripples to every
  call site — gate with `--no-run`); T2 (the claim-before-cancel + capture-result-then-EXPIRE concurrency — the
  highest-risk increment; the `stale_finish_turn_after_reset` + `reset_on_running_without_force` tests are the
  keystones). The `force` end-to-end is live-gated; the precise gen-N-completes-after-N+1 race is unit-gated.
- **Open for the plan-review:** whether `force`'s best-effort `backend.cancel(old_id)` before `release_session`
  is needed at all (release re-cancels) or pure noise; whether the deferral set should be a single helper
  `is_claimed(state)` to avoid drift across the 3 sites.
