# Warm Loop Session (B2b-3c) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Give the `a2a-bridge implement` review→tweak loop a WARM `:rw` agent session across its edit+fix
turns (one container + one ACP session, continuity + no per-turn cold start), reaped at loop end.

**Architecture:** Slice 1 adds a warm lifecycle to `ContainerRwBackend` (a `Lifecycle::Warm` mode with
SEPARATE `*_warm` method bodies, sharing an extracted `open_inner` with the untouched per-turn path; warm
keeps an authoritative `warm` cache + a `turn_active` marker, and reaps ONLY at `retire()`). Slice 2 takes the
impl turns off the workflow executor: `implement_cmd` builds a warm `ContainerRwBackend` from the `impl`
agent entry, mints ONE stable `SessionId`, and drives the edit turn + `ProdEffects::fix` as prompts on it,
reaping via `retire()` + an `RwSweepGuard`. Review/verify are unchanged.

**Tech Stack:** Rust (workspace). `crates/bridge-container`, `crates/bridge-core/src/sandbox.rs`,
`bin/a2a-bridge/src/{main,tweak}.rs`, `prompts/`. Docker for the live gate only.

**Spec:** `docs/superpowers/specs/2026-06-06-warm-loop-session-design.md` (rev3, dual-reviewed; idle-survival
spike PASSED — `docs/superpowers/spikes/2026-06-07-warm-session-idle-survival.md`).

**Design note (refines spec-decision #3, still in-place):** the spec said "single injected reap-trigger,
no `Lifecycle`-branching in cancel/retire." Realized concretely as: a one-line `if self.is_warm() { return
self.<op>_warm(...).await }` dispatch at the top of `prompt`/`cancel`/`retire`, delegating to SEPARATE warm
method bodies. The warm bodies never call the reap fn except in `retire_warm` — so the never-reap invariant
lives in isolated code a future per-turn edit cannot reach (the reviewer's BLOCKER-3 concern), while staying
one type sharing `open_inner` (the clean-room's reuse point). This is NOT scattered per-line branching.

**Conventions:** TDD green-per-task (red step first); task/code commits do NOT carry the `Co-Authored-By`
trailer (doc commits do). Branch `feat/warm-loop-session` off `main`. Coverage after `cargo llvm-cov clean
--workspace`; floors per **ci.yml** (workspace 85; bridge-core/acp/api/workflow 90 — bridge-container has no
per-crate floor but keep the new warm code well-covered). Serialize heavy Docker jobs at the live gate.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/bridge-container/src/lib.rs` | modify | `Lifecycle` + `new_warm`/`new_warm_with_hooks`; `WarmInner`; extracted `open_inner`; `warm`+`turn_active` fields; `prompt_warm`/`cancel_warm`/`retire_warm` + the dispatch guards; warm tests. |
| `crates/bridge-core/src/sandbox.rs` | modify | `rw_sweep_filter_argv` (sibling of `ro_sweep_filter_argv`). |
| `bin/a2a-bridge/src/main.rs` | modify | `container_rw_cfg_from_entry`; `rw_sweep_targets` + `RwSweepGuard`; `drain_turn`; warm backend build + config-identity asserts + `impl_session`; edit turn + `ProdEffects::fix` off-executor; hand-off→print→`retire`. |
| `bin/a2a-bridge/src/tweak.rs` | modify | slim `build_fix_input` (self-sufficient). |
| `prompts/implement-fix.md` | modify | reword to a continuation. |

---

## SLICE 1 — warm lifecycle in `ContainerRwBackend`

### Task 1: Extract `open_inner` + `WarmInner` (per-turn stays behaviorally identical)

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: add `WarmInner` + `open_inner`; refactor per-turn `prompt` to call it**

Add the struct (near `InflightTurn`):

```rust
/// A spawned, configured inner backend + its container identity. Shared shape for per-turn (promoted to
/// `InflightState::Live`) and warm (cached in `warm`). `rw_canon` is the canonicalized :rw target the
/// session was configured with (re-applied on a warm reuse turn).
struct WarmInner {
    inner: Arc<dyn AgentBackend>,
    name: String,
    reaped: Arc<AtomicBool>,
    rw_canon: SessionCwd,
}
```

Add the extracted spawn/compose/configure block as an inherent method (single-sources naming, compose,
spawn-failure reap, configure — used by BOTH paths). It does NOT touch `inflight`/`warm`/`turn_active` (the
callers own their cache):

```rust
    /// Spawn + configure ONE inner container for `session`. On ANY failure the just-started container is
    /// reaped by name (the `docker run` client can be up before the handshake fails) and `Err` is returned.
    /// Caller owns the cache bookkeeping + the cwd strict-reject. `session/new` is lazy (inside the inner's
    /// first `prompt`), so this method does NOT mint the ACP session.
    async fn open_inner(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<WarmInner, BridgeError> {
        let runtime = self.cfg.sandbox.runtime().to_string();
        let cwd = spec.cwd.clone().ok_or(BridgeError::ConfigInvalid {
            reason: "missing session cwd".into(),
        })?;
        let rw_canon = self.resolve_rw_target(&cwd)?;
        let n = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        let name = format!("a2a-rw-{}-{}", self.owner, n);
        let (program, argv) = compose_container_rw(
            &self.cfg.sandbox, &rw_canon, &name, &self.cfg.cmd, &self.cfg.args,
        );
        let acp = AcpConfig {
            cwd: PathBuf::from(rw_canon.as_str()),
            model: self.cfg.model.clone(),
            mode: self.cfg.mode.clone(),
            auth_method: self.cfg.auth_method.clone(),
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
            container: None,
        };
        let inner = match self.spawn.spawn(&program, &argv, acp).await {
            Ok(i) => i,
            Err(e) => {
                (self.reap_fn)(runtime.clone(), name.clone()); // spawn-failure reap (never inserted)
                return Err(e);
            }
        };
        let reaped = Arc::new(AtomicBool::new(false));
        // The inner prefers the stashed SessionSpec.cwd over AcpConfig.cwd → configure with the CANONICAL cwd.
        let mut spec_canon = spec.clone();
        spec_canon.cwd = Some(rw_canon.clone());
        if let Err(e) = inner.configure_session(session, &spec_canon).await {
            reap_once(&self.reap_fn, &runtime, &name, &reaped);
            return Err(e);
        }
        Ok(WarmInner { inner, name, reaped, rw_canon })
    }
```

Refactor the per-turn `prompt` (current `lib.rs:135-238`) to use `open_inner` while preserving the
reserve→Live→stream behavior EXACTLY. Replace the body from the `// Atomic check-and-reserve` block through
the `inner_stream` match with:

```rust
        // Atomic check-and-reserve: reject a second concurrent prompt on a live session under ONE lock.
        {
            let mut m = self.inflight.lock().await;
            if m.contains_key(session) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("session {} already has an in-flight turn", session.as_str()),
                });
            }
            m.insert(session.clone(), InflightState::Reserving);
        }
        // From here every error path must remove the reservation (open_inner already reaps on its own failure).
        let runtime = self.cfg.sandbox.runtime().to_string();
        let wi = match self.open_inner(session, &spec).await {
            Ok(wi) => wi,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                return Err(e);
            }
        };
        // Promote the reservation to Live (the cancel handle), sharing the `reaped` bool.
        self.inflight.lock().await.insert(
            session.clone(),
            InflightState::Live(InflightTurn {
                inner: wi.inner.clone(),
                name: wi.name.clone(),
                reaped: wi.reaped.clone(),
            }),
        );
        let inner_stream = match wi.inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                reap_once(&self.reap_fn, &runtime, &wi.name, &wi.reaped);
                return Err(e);
            }
        };
        let reaper = ContainerReaper {
            runtime,
            name: wi.name,
            reap_fn: self.reap_fn.clone(),
            reaped: wi.reaped,
            inflight: self.inflight.clone(),
            session: session.clone(),
        };
        Ok(wrap_with_reaper(wi.inner, inner_stream, reaper))
```

Note: the strict-reject `let spec = ...; let cwd = ...` lines at the top of `prompt` (`lib.rs:140-148`) STAY
(per-turn keeps the early cwd check before reserving). `open_inner` re-reads `spec.cwd` — harmless (same
value). The `_ = cwd;` if unused: keep the existing `spec`/`cwd` resolution as-is; pass `&spec` to `open_inner`.

- [ ] **Step 2: run the existing per-turn tests (must stay green)**

Run: `cargo test -p bridge-container 2>&1 | tail -15`
Expected: ALL existing tests pass unchanged (`prompt_spawns_once_with_rw_mount_and_name`,
`prompt_spawn_failure_reaps_and_errors`, `stream_completion_reaps_once_and_clears_inflight`,
`cancel_reaches_inner_and_reaps_once`, `retire_cancels_and_reaps`, etc.). This proves the extraction is
behavior-preserving.

- [ ] **Step 3: commit**

```bash
git add crates/bridge-container/src/lib.rs
git commit -m "container: extract open_inner + WarmInner (per-turn unchanged) (b2b3c)"
```

---

### Task 2: warm constructor + fields + `prompt_warm`

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: write the failing tests** (append to `mod tests`)

Extend `StubInner` to count prompts + sessions, then add warm tests. First extend the stub:

```rust
    // (extend StubInner) — record prompt count + the distinct sessions it newSession'd.
    struct StubInner {
        canceled: AtomicBool,
        prompts: AtomicUsize,
        sessions: Mutex<std::collections::HashSet<String>>,
        fail_prompt: bool,
    }
```

Update `StubInner::prompt` to `self.prompts.fetch_add(1, SeqCst); self.sessions.lock().await.insert(s.as_str().to_string()); if self.fail_prompt { return Err(BridgeError::agent_crashed("prompt boom")); } Ok(...one Done...)`,
and `CountingSpawn` to build `StubInner { canceled: false, prompts: 0, sessions: default, fail_prompt: self.fail_prompt }`
(add a `fail_prompt: bool` field to `CountingSpawn`, defaulting false in `new`; add `CountingSpawn::new_prompt_fail()`).
Add a warm test backend helper:

```rust
    async fn warm_backend(mount: &str, spawn: Arc<dyn ContainerSpawn>, reap: ReapFn) -> ContainerRwBackend {
        ContainerRwBackend::new_warm_with_hooks(
            cfg_with_mount(mount), spawn, "inst".into(), reap, noop_sweep(),
        ).await.unwrap()
    }
```

Tests:

```rust
    #[tokio::test]
    async fn warm_reuses_one_inner_and_one_session_across_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        { let mut a = be.prompt(&s, vec![]).await.unwrap(); while a.next().await.is_some() {} } // turn 1
        { let mut b = be.prompt(&s, vec![]).await.unwrap(); while b.next().await.is_some() {} } // turn 2
        assert_eq!(spawn.count.load(Ordering::SeqCst), 1, "ONE container across both turns");
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "NOT reaped between turns");
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 2, "both turns hit the SAME inner");
        assert_eq!(inner.sessions.lock().await.len(), 1, "one ACP session/new");
    }

    #[tokio::test]
    async fn warm_reuse_turn_error_clears_turn_active_and_does_not_reap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false); // turn 1 ok
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        { let mut a = be.prompt(&s, vec![]).await.unwrap(); while a.next().await.is_some() {} }
        // make the cached inner fail its NEXT prompt
        spawn.last_inner.lock().await.as_ref().unwrap().fail_prompt.store_relaxed();
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("prompt boom"), "got {err:?}");
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "a transient reuse error must NOT reap the warm container");
        assert!(be.warm.lock().await.contains_key(&s), "warm entry retained");
        assert!(!be.turn_active.lock().await.contains(&s), "turn_active cleared after the error");
        // a subsequent good turn still works on the SAME container (set fail_prompt=false first in the stub)
    }

    #[tokio::test]
    async fn warm_rejects_second_concurrent_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, _) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap(); // hold the stream
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("in-flight turn"), "got {err:?}");
    }
```

(`fail_prompt` is an `AtomicBool` on `StubInner` so the test can flip it after turn 1; `store_relaxed()` is a
tiny helper or inline `.store(true, SeqCst)`.)

- [ ] **Step 2: run — expect RED** (`new_warm_with_hooks`/`warm`/`turn_active`/`prompt_warm` don't exist).

Run: `cargo test -p bridge-container warm_ 2>&1 | tail -15` → FAIL (compile).

- [ ] **Step 3: implement**

Add the lifecycle + fields + constructors. Add to the struct:

```rust
#[derive(Clone, Copy, PartialEq)]
enum Lifecycle { PerTurn, Warm }
```

`ContainerRwBackend` gains: `lifecycle: Lifecycle`, `warm: Mutex<HashMap<SessionId, WarmInner>>`,
`turn_active: Arc<Mutex<std::collections::HashSet<SessionId>>>`. `new_with_hooks`/`new` set
`lifecycle: Lifecycle::PerTurn` + empty warm/turn_active. Add:

```rust
    pub async fn new_warm_with_hooks(
        cfg: ContainerRwConfig, spawn: Arc<dyn ContainerSpawn>, owner: String,
        reap_fn: ReapFn, sweep_fn: SweepFn,
    ) -> Result<Self, BridgeError> {
        let mut be = Self::new_with_hooks(cfg, spawn, owner, reap_fn, sweep_fn).await?;
        be.lifecycle = Lifecycle::Warm;
        Ok(be)
    }
    pub async fn new_warm(
        cfg: ContainerRwConfig, spawn: Arc<dyn ContainerSpawn>, owner: String,
    ) -> Result<Self, BridgeError> {
        let runtime = cfg.sandbox.runtime().to_string();
        Self::new_warm_with_hooks(cfg, spawn, owner, production_reap_fn(), production_sweep_fn(runtime)).await
    }
    fn is_warm(&self) -> bool { self.lifecycle == Lifecycle::Warm }
```

Add the dispatch guard at the TOP of `prompt` (before the per-turn body):

```rust
        if self.is_warm() {
            return self.prompt_warm(session, parts).await;
        }
```

Add `prompt_warm` + a `TurnGuard`:

```rust
    /// Warm turn: reuse one cached container/session across prompts. Concurrency-reject via `turn_active`,
    /// cleaned up by an RAII guard on EVERY path. Open-or-reuse the cached inner; a REUSE-turn error
    /// (configure/prompt) clears `turn_active`, does NOT reap, and returns Err (a transient error must not
    /// nuke the warm container). The stream's `TurnGuard` clears `turn_active` on end/drop — it NEVER reaps;
    /// the only warm reap site is `retire_warm`.
    async fn prompt_warm(
        &self, session: &SessionId, parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let spec = self.session_cfg.lock().await.get(session).cloned().ok_or(
            BridgeError::ConfigInvalid { reason: "missing session cwd".into() })?;
        // Atomic concurrency reject + mark active; the RAII guard owns cleanup from here.
        {
            let mut ta = self.turn_active.lock().await;
            if ta.contains(session) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("session {} already has an in-flight turn", session.as_str()),
                });
            }
            ta.insert(session.clone());
        }
        let guard = TurnGuard { turn_active: self.turn_active.clone(), session: session.clone(), armed: true };

        // Open once (cache miss) or reuse (re-configure with the cached canonical cwd).
        let inner = {
            let mut w = self.warm.lock().await;
            match w.get(session) {
                Some(wi) => wi.inner.clone(),
                None => {
                    drop(w); // don't hold the lock across the async open
                    let wi = self.open_inner(session, &spec).await?; // open failure: guard clears turn_active
                    let inner = wi.inner.clone();
                    self.warm.lock().await.insert(session.clone(), wi);
                    inner
                }
            }
        };
        // Reuse turn: re-apply the canonical cwd (deterministic; minted_cwd guard passes).
        if let Some(wi) = self.warm.lock().await.get(session) {
            let mut spec_canon = spec.clone();
            spec_canon.cwd = Some(wi.rw_canon.clone());
            // configure_session error on a reuse turn → return Err (guard clears turn_active; NO reap).
            inner.configure_session(session, &spec_canon).await?;
        }
        let inner_stream = inner.prompt(session, parts).await?; // prompt Err (pre-stream) → guard clears; NO reap
        Ok(wrap_with_turn_guard(inner, inner_stream, guard))
    }
```

```rust
/// Clears `turn_active` on stream end OR early drop. NEVER reaps (warm reaps only at `retire`).
struct TurnGuard {
    turn_active: Arc<Mutex<std::collections::HashSet<SessionId>>>,
    session: SessionId,
    armed: bool,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        if !self.armed { return; }
        let ta = self.turn_active.clone();
        let s = self.session.clone();
        spawn_detached(async move { ta.lock().await.remove(&s); });
    }
}
fn wrap_with_turn_guard(
    inner: Arc<dyn AgentBackend>, inner_stream: BackendStream, mut guard: TurnGuard,
) -> BackendStream {
    Box::pin(async_stream::stream! {
        let _inner = inner;
        let mut s = inner_stream;
        while let Some(item) = s.next().await { yield item; }
        // clear synchronously on normal completion so a sequential next turn isn't spuriously rejected.
        guard.turn_active.lock().await.remove(&guard.session);
        guard.armed = false; // prevent the Drop double-clear (harmless but tidy)
    })
}
```

Note: `?` in `prompt_warm` (open/configure/prompt errors) returns Err — `guard` is still in scope and drops
→ clears `turn_active` (no reap). For a REUSE prompt error specifically, the warm entry is NOT removed (only
`?`-propagated) → the container stays cached. For a cache-MISS open failure, `open_inner` already reaped the
just-started container and nothing was inserted into `warm`. Both correct.

- [ ] **Step 4: run to verify pass** — `cargo test -p bridge-container 2>&1 | tail -20` → PASS (warm + per-turn).

- [ ] **Step 5: commit**

```bash
git add crates/bridge-container/src/lib.rs
git commit -m "container: warm prompt (open-or-reuse, reuse-error no-reap, TurnGuard) (b2b3c)"
```

---

### Task 3: warm `cancel` + warm `retire` (the sole reap site)

**Files:** Modify `crates/bridge-container/src/lib.rs`.

- [ ] **Step 1: write the failing tests**

```rust
    #[tokio::test]
    async fn warm_retire_is_the_sole_reap_site() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        { let mut a = be.prompt(&s, vec![]).await.unwrap(); while a.next().await.is_some() {} }
        { let mut b = be.prompt(&s, vec![]).await.unwrap(); while b.next().await.is_some() {} }
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "no reap across turns");
        be.retire().await.unwrap();
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert!(inner.canceled.load(Ordering::SeqCst), "retire cancels the inner");
        assert_eq!(reaps.load(Ordering::SeqCst), 1, "reaped exactly once at retire");
        assert!(be.warm.lock().await.is_empty(), "warm cache drained");
    }

    #[tokio::test]
    async fn warm_cancel_clears_turn_active_without_reaping() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap();
        be.cancel(&s).await.unwrap();
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "warm cancel does NOT reap");
        assert!(!be.turn_active.lock().await.contains(&s), "cancel cleared turn_active");
        be.retire().await.unwrap(); // retire still reaps the cached container
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }
```

- [ ] **Step 2: run — RED.**

- [ ] **Step 3: implement** the dispatch guards + warm bodies.

Top of `cancel`: `if self.is_warm() { return self.cancel_warm(session).await; }`
Top of `retire`: `if self.is_warm() { return self.retire_warm().await; }`

```rust
    /// Warm cancel: cancel the inner's current turn + clear `turn_active`. Does NOT reap (retire owns that).
    async fn cancel_warm(&self, session: &SessionId) -> Result<(), BridgeError> {
        let inner = self.warm.lock().await.get(session).map(|wi| wi.inner.clone());
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        self.turn_active.lock().await.remove(session);
        Ok(())
    }

    /// Warm retire — the SOLE warm reap site. Drain the cache; per entry: cancel the inner, then reap once.
    async fn retire_warm(&self) -> Result<(), BridgeError> {
        let entries: Vec<(SessionId, WarmInner)> = {
            let mut w = self.warm.lock().await;
            w.drain().collect()
        };
        let runtime = self.cfg.sandbox.runtime().to_string();
        for (s, wi) in entries {
            let _ = wi.inner.cancel(&s).await;
            reap_once(&self.reap_fn, &runtime, &wi.name, &wi.reaped);
        }
        Ok(())
    }
```

- [ ] **Step 4: run to verify pass** — `cargo test -p bridge-container 2>&1 | tail -20` → PASS.

- [ ] **Step 5: clippy + commit**

```bash
cargo clippy -p bridge-container --all-targets -- -D warnings 2>&1 | tail -3
git add crates/bridge-container/src/lib.rs
git commit -m "container: warm cancel (no reap) + retire (sole reap site) (b2b3c)"
```

---

## SLICE 2 — impl turns off the executor

### Task 4: `rw_sweep_filter_argv` (sandbox) + `rw_sweep_targets`/`RwSweepGuard` (main)

**Files:** Modify `crates/bridge-core/src/sandbox.rs`, `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: failing test for `rw_sweep_filter_argv`** (in `sandbox.rs` tests, beside the `:ro` one)

```rust
    #[test]
    fn rw_sweep_filter_argv_is_owner_scoped() {
        let (prog, argv) = rw_sweep_filter_argv("docker", "abc");
        assert_eq!(prog, "docker");
        assert_eq!(argv, vec!["ps", "-aq", "--filter", "name=a2a-rw-abc-"]);
    }
```

- [ ] **Step 2: RED**, then implement (sibling of `ro_sweep_filter_argv`, `sandbox.rs:146`):

```rust
/// PURE. `(program, argv)` for the owner-scoped `:rw` sweep: `ps -aq --filter name=a2a-rw-<owner>-`.
pub fn rw_sweep_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (runtime.to_string(), vec!["ps".into(), "-aq".into(), "--filter".into(),
        format!("name=a2a-rw-{owner}-")])
}
```

- [ ] **Step 3: add `rw_sweep_targets` + `RwSweepGuard` in `main.rs`** (beside `ro_sweep_targets`/`RoSweepGuard`)

```rust
/// `(runtime, owner)` sweep targets for THIS instance's `:rw` (ContainerRw) agents — mirrors `ro_sweep_targets`.
fn rw_sweep_targets(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    config_path: &std::path::Path,
) -> Vec<(String, String)> {
    use bridge_core::domain::AgentKind;
    let mut targets = Vec::new();
    for entry in &snapshot.entries {
        let Some(sb) = entry.sandbox.as_ref() else { continue };
        if entry.kind != AgentKind::ContainerRw { continue; }
        targets.push((sb.runtime().to_string(), container_owner(config_path, &sb.mount, entry.id.as_str())));
    }
    targets
}

/// SYNCHRONOUS owner-scoped reap of `a2a-rw-<owner>-` containers (best-effort) — `:rw` sibling of `ro_sweep`.
fn rw_sweep(targets: &[(String, String)]) {
    for (runtime, owner) in targets {
        let (prog, argv) = bridge_core::sandbox::rw_sweep_filter_argv(runtime, owner);
        if let Ok(out) = std::process::Command::new(&prog).args(&argv).output() {
            for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                let _ = std::process::Command::new(runtime).args(["rm", "-f", id]).output();
            }
        }
    }
}

struct RwSweepGuard(Vec<(String, String)>);
impl Drop for RwSweepGuard {
    fn drop(&mut self) { rw_sweep(&self.0); }
}
```

- [ ] **Step 4: run** `cargo test -p bridge-core sandbox 2>&1 | tail -6` (PASS) + `cargo build -p a2a-bridge`
(the main.rs additions are unused until Task 8 — dead-code warning OK).

- [ ] **Step 5: commit**

```bash
git add crates/bridge-core/src/sandbox.rs bin/a2a-bridge/src/main.rs
git commit -m "sweep: rw_sweep_filter_argv + rw_sweep_targets/RwSweepGuard (b2b3c)"
```

---

### Task 5: `container_rw_cfg_from_entry` (factor out of `make_spawn_fn`)

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: extract** the `ContainerRwConfig` build (currently inline at `main.rs:289-299`) into:

```rust
/// Build a `ContainerRwConfig` from a ContainerRw agent entry (shared by `make_spawn_fn` + the warm
/// `implement` path so the per-turn and warm containers compose identically).
fn container_rw_cfg_from_entry(
    entry: &AgentEntry,
) -> Result<bridge_container::ContainerRwConfig, BridgeError> {
    let sb = entry.sandbox.clone().ok_or(BridgeError::ConfigInvalid {
        reason: format!("container_rw agent {} requires sandbox", entry.id.as_str()),
    })?;
    let cmd = entry.cmd.clone().ok_or(BridgeError::ConfigInvalid {
        reason: format!("container_rw agent {} requires cmd", entry.id.as_str()),
    })?;
    Ok(bridge_container::ContainerRwConfig {
        sandbox: sb,
        cmd,
        args: entry.args.clone(),
        model: entry.model.clone(),
        mode: entry.mode.clone(),
        auth_method: entry.auth_method.clone(),
        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default().handshake_timeout,
        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
    })
}
```

Replace the `make_spawn_fn` ContainerRw arm body (`main.rs:289-304`) to use it:

```rust
                    let owner = container_owner(&owner_config_path, &sb.mount, entry.id.as_str());
                    let ccfg = container_rw_cfg_from_entry(entry)?;
                    let cspawn: Arc<dyn bridge_container::ContainerSpawn> =
                        Arc::new(AcpContainerSpawn { policy: Arc::clone(&policy) });
                    let be = bridge_container::ContainerRwBackend::new(ccfg, cspawn, owner).await?;
```

(`sb` is still needed for the `owner`; keep the `let sb = entry.sandbox...` line or read `entry.sandbox` for
the owner before calling the helper.)

- [ ] **Step 2: run** `cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -6` + `cargo build` → green (pure
refactor, behavior unchanged).

- [ ] **Step 3: commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: factor container_rw_cfg_from_entry (b2b3c)"
```

---

### Task 6: `drain_turn` (STRICTER than the executor)

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: failing tests** (in `main.rs` tests)

```rust
    #[tokio::test]
    async fn drain_turn_outcomes() {
        use bridge_core::ports::Update;
        let done = |sr: &str| Ok(Update::Done { stop_reason: sr.into() });
        // end_turn → complete
        let s: bridge_core::ports::BackendStream = Box::pin(tokio_stream::iter(vec![done("end_turn")]));
        assert!(drain_turn(s).await);
        // cancelled → incomplete
        let s: bridge_core::ports::BackendStream = Box::pin(tokio_stream::iter(vec![done("cancelled")]));
        assert!(!drain_turn(s).await);
        // clean end without Done → incomplete (the executor-divergence guard)
        let s: bridge_core::ports::BackendStream = Box::pin(tokio_stream::iter(Vec::new()));
        assert!(!drain_turn(s).await);
        // stream error → incomplete
        let s: bridge_core::ports::BackendStream =
            Box::pin(tokio_stream::iter(vec![Err(bridge_core::error::BridgeError::agent_crashed("x"))]));
        assert!(!drain_turn(s).await);
    }
```

- [ ] **Step 2: RED**, then implement (beside `drain_impl`):

```rust
/// Drain a warm-session turn's raw `Update` stream → `completed`. STRICTER than the executor (which leaves
/// ok=true on a clean end): complete IFF a `Done { stop_reason != CANCELLED }` arrived; a stream `Err(_)`
/// or a clean end without `Done` → incomplete. Polls to the end so the inner runs its cancel cleanup.
async fn drain_turn(mut stream: bridge_core::ports::BackendStream) -> bool {
    use bridge_core::ports::{Update, STOP_REASON_CANCELLED};
    use futures::StreamExt;
    let mut completed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Update::Done { stop_reason }) => {
                completed = stop_reason != STOP_REASON_CANCELLED;
            }
            Ok(_) => {}
            Err(e) => { eprintln!("[implement] turn: stream error: {e:?}"); completed = false; }
        }
    }
    completed
}
```

- [ ] **Step 3: run** `cargo test -p a2a-bridge --bin a2a-bridge drain_turn 2>&1 | tail -6` → PASS.

- [ ] **Step 4: commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: drain_turn (raw Updates, stricter than executor) (b2b3c)"
```

---

### Task 7: slim `build_fix_input` (self-sufficient)

**Files:** Modify `bin/a2a-bridge/src/tweak.rs`.

- [ ] **Step 1: update the existing `build_fix_input_keeps_task_and_sections` test** — assert the new framing:
the output still contains the task, the verify/review sections, the `git add` mandate, but DROPS the "it
already has your prior commit" phrasing. Add `assert!(!i.contains("prior commit"));` and
`assert!(i.contains("git add"));`.

- [ ] **Step 2: RED** (the current header says "has your prior commit"), then update the `header` in
`build_fix_input` (`tweak.rs:97-101`):

```rust
    let header = format!(
        "{task}\n\nThe previous attempt did not pass. FIX the issues below; re-stage your fixes with \
         `git add` (the bridge folds ONLY staged changes); do NOT run `git commit` and do NOT write a commit \
         message.\n"
    );
```

(The budget/section logic is unchanged. Keeping the task reminder + digest makes it self-sufficient — a
future cold re-open stays sound.)

- [ ] **Step 3: run** `cargo test -p a2a-bridge --bin a2a-bridge tweak:: 2>&1 | tail -6` → PASS.

- [ ] **Step 4: commit**

```bash
git add bin/a2a-bridge/src/tweak.rs
git commit -m "tweak: slim build_fix_input for warm continuity (self-sufficient) (b2b3c)"
```

---

### Task 8: wire the warm session into `implement_cmd`

**Files:** Modify `bin/a2a-bridge/src/main.rs`.

This is integration glue; it is validated by the existing fake-executor `run_tweak_loop` tests staying green
(seam unchanged), the build, and the live gate (Task 11). Make these edits in `implement_cmd`:

- [ ] **Step 1: config-identity asserts + resolve the impl agent (pre-clone or pre-commit)**

After `wf_map` + `fix_graph` are resolved (the B2b-3b block ~`main.rs:671-682`), add:

```rust
    // B2b-3c: the warm session drives BOTH edit + fix turns on ONE container, so edit & fix MUST name the
    // SAME single-node ContainerRw agent. Validate pre-first-commit (fail-loud).
    let edit_node = match graph.nodes.as_slice() {
        [n] => n.clone(),
        _ => return Err("implement: edit workflow must be single-node for the warm session".into()),
    };
    let impl_agent_id = edit_node.agent.clone();
    if let Some(fg) = &fix_graph {
        match fg.nodes.as_slice() {
            [n] if n.agent == impl_agent_id => {}
            [_] => return Err("implement: fix workflow agent must match the edit agent (one warm session)".into()),
            _ => return Err("implement: fix workflow must be single-node".into()),
        }
    }
    let impl_entry = snapshot.entries.iter().find(|e| e.id == impl_agent_id)
        .ok_or("implement: impl agent not found in snapshot")?
        .clone();
    if impl_entry.kind != bridge_core::domain::AgentKind::ContainerRw {
        return Err("implement: warm session requires a container_rw impl agent".into());
    }
```

(NOTE: `into_snapshot` consumes `cfg`; resolve `impl_entry` from `snapshot` AFTER it's built, and capture the
edit/fix `prompt_template`s from the graphs BEFORE — they're `Arc<WorkflowGraph>`, so clone `edit_node` and
the fix node. The exact ordering: build `snapshot`, then resolve `impl_entry`; the graphs are already in hand.)

- [ ] **Step 2: build the warm backend + `impl_session`; declare `RwSweepGuard` BEFORE it**

Replace the per-turn registry/executor build for the IMPL agent (the warm path does NOT use the registry for
impl; review still does). After `snapshot` + the owner config path:

```rust
    let rw_targets = rw_sweep_targets(&snapshot, &owner_config_path);
    rw_sweep(&rw_targets);                       // boot-sweep (crash recovery)
    let _rw_guard = RwSweepGuard(rw_targets);    // declared BEFORE `warm` → drops AFTER it (synchronous backstop)

    let warm_owner = container_owner(&owner_config_path, &impl_entry.sandbox.as_ref().unwrap().mount, impl_agent_id.as_str());
    let warm = bridge_container::ContainerRwBackend::new_warm(
        container_rw_cfg_from_entry(&impl_entry)?,
        Arc::new(AcpContainerSpawn { policy: Arc::clone(&policy_for_spawn_or_equivalent) }),
        warm_owner,
    ).await?;
    let impl_session = bridge_core::ids::SessionId::parse(format!("implement-{task_id}"))
        .map_err(|e| format!("implement: session id: {e:?}"))?;
    warm.configure_session(&impl_session, &SessionSpec { config: Default::default(), cwd: Some(clone_cwd.clone()) }).await?;
```

(Reuse the existing `policy`/`AutoPolicy` already built for the review executor; the review executor +
registry are still built for `run_review_step`. The `_ro_guard` (review `:ro`) stays; add `_rw_guard` for the
warm `:rw`, BOTH declared before the backends they must outlive in drop.)

- [ ] **Step 3: edit turn via the warm session** — replace the first-edit `drain_impl(executor.run_with_context(
graph, a.task.clone(), …))` (B2b-3b `main.rs:565-571`) with:

```rust
    use std::collections::HashMap;
    // `bridge_workflow::template::render(&str, &HashMap<&str,&str>)` — mirror the executor: the only var a
    // single-node `inputs=[]` workflow needs is `{{input}}`.
    let edit_vars: HashMap<&str, &str> = HashMap::from([("input", a.task.as_str())]);
    let edit_input = bridge_workflow::template::render(&edit_node.prompt_template, &edit_vars);
    let completed = match warm.prompt(&impl_session, vec![Part { text: edit_input }]).await {
        Ok(stream) => drain_turn(stream).await,
        Err(e) => { eprintln!("[implement] edit turn failed: {e:?}"); false } // pre-commit → decide() aborts
    };
```

- [ ] **Step 4: `ProdEffects` drives fix turns on the warm session** — change `ProdEffects` to hold the warm
backend + session + fix template (per spec decision 6); `ProdEffects::fix`:

```rust
    async fn fix(&mut self, _attempt: u32, input: &str) -> bool {
        let vars: std::collections::HashMap<&str, &str> = std::collections::HashMap::from([("input", input)]);
        let parts = vec![Part { text: bridge_workflow::template::render(&self.fix_template, &vars) }];
        match self.impl_backend.prompt(self.impl_session, parts).await {
            Ok(stream) => drain_turn(stream).await,
            Err(e) => { eprintln!("[implement] fix turn failed: {e:?}"); false } // → FixIncomplete
        }
    }
```

`ProdEffects` fields become: `impl_backend: &'a dyn AgentBackend`, `impl_session: &'a SessionId`,
`fix_template: String` (the fix node's `prompt_template`), plus the unchanged verify/review fields. The
`verify`/`review` methods are UNCHANGED. Pass `&warm` as `impl_backend`.

- [ ] **Step 5: hand-off → print → retire** — after `run_tweak_loop` returns (the B2b-3b `Action::Commit`
arm tail), keep computing + printing the hand-off, then:

```rust
    println!("{handoff}");
    let _ = warm.retire().await; // log-only; never alters the result (post-commit, no `?`)
    Ok(())
```

(If `retire` errors, swallow it — the hand-off already printed.)

- [ ] **Step 6: build + bin tests + clippy**

```bash
cargo build -p a2a-bridge 2>&1 | tail -8
cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | tail -8   # fake-executor run_tweak_loop tests stay green
cargo clippy -p a2a-bridge --all-targets --all-features -- -D warnings 2>&1 | tail -4
```
Expected: clean build, all bin tests pass, clippy clean. If `Part`/`SessionSpec`/`template` imports are
missing, add them.

- [ ] **Step 7: commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "implement: drive edit+fix turns on a warm :rw session (b2b3c)"
```

---

### Task 9: reword `prompts/implement-fix.md`

- [ ] Reword the opening from "you have a prior commit" to a continuation, KEEPING the firm `git add` mandate:

```markdown
You are the SAME coding agent continuing in the SAME session on this clone — you already made the prior
edit. A build/test verify and/or a code review found problems. CONTINUE and FIX them.
```
(Keep the rest of the contract — `git add` mandatory, no `git commit`, no message, no branch switch — verbatim
from the B2b-3b version.)

- [ ] **Commit:**
```bash
git add prompts/implement-fix.md
git commit -m "implement-fix: reword for warm-session continuity (b2b3c)"
```

---

### Task 10: workspace gate

- [ ] Serialized: `cargo fmt --all`; `cargo build --workspace`; `cargo test --workspace --exclude
bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip
process::tests::term_ignoring_loop_forces_group_sigkill` (note: bridge-container's OWN unit tests run via
`cargo test -p bridge-container` since the workspace test excludes it for the hermetic verify — run BOTH);
`cargo clippy --workspace --all-targets --all-features -- -D warnings`. All green.
- [ ] Coverage: `cargo llvm-cov clean --workspace` then `cargo llvm-cov -p bridge-container -p a2a-bridge
2>&1 | tail`; keep the new warm code well-covered (≥90 on lib.rs warm paths). Floors per ci.yml.
- [ ] Commit any fmt-only changes (scoped `git add $(git diff --name-only)`).

---

### Task 11: live gate (operator-run; Docker)

Fresh containerized claude creds first (`cp ~/.claude/.credentials.json ~/.config/a2a-creds/claude/.credentials.json`).
Throwaway clone under `allowed_cwd_root`. Serialize heavy jobs.

- [ ] **Right-first-try:** `implement` a trivially-correct task → warm opened, ONE prompt, converged, hand-off
prints, `docker ps -a | grep a2a-rw` → 0 after retire.
- [ ] **Converge-via-fix with continuity + identity:** the acceptance-orthogonal `clippy::ptr_arg` task (B2b-3b
gotcha) → attempt-1 clippy FAIL → fix turn continues the SAME session → converge. **Assert the SAME container
id across the edit and fix turns** (capture `docker ps -q --filter name=a2a-rw-` during each turn; it must be
the same id + nonzero in the verify/review gap) — the falsifiable per-turn-regression guard. One amended commit.
- [ ] **Reaper:** warm `:rw` → 0 after the run; `:ro` review + verify containers unaffected.
- [ ] Record the hand-off + the container-id-across-turns evidence for the ADR.

---

## After the build

Plan dual-review (containerized `plan-review` primary — refresh claude creds first — + a2a-local codex
backstop) BEFORE building; fold a rev2 if needed. Then inline TDD build (Tasks 1–10), live gate (Task 11),
merge + push, memory, ADR-0024.

## Self-review (writing-plans)

- **Spec coverage:** in-place Warm mode + isolated warm bodies (T1-3); `open_inner` shared (T1); reuse-turn
  error no-reap (T2); `turn_active` lifecycle + TurnGuard (T2-3); retire sole-reap-site (T3); `RwSweepGuard`
  shared-owner + declared-before-warm + sandbox helper home (T4 + T8); `drain_turn` stricter-than-executor
  table (T6); config-identity asserts + SessionId contract (T8); hand-off→print→`let _ = retire()` (T8);
  slim self-sufficient `build_fix_input` (T7); falsifiable session/new-count + same-container-id (T2 + T11).
  All covered.
- **Type consistency:** `WarmInner`/`open_inner` (T1) used by `prompt_warm` (T2) + `retire_warm` (T3);
  `new_warm` (T2) used in `implement_cmd` (T8); `container_rw_cfg_from_entry` (T5) used by T8 + `make_spawn_fn`;
  `drain_turn` (T6) used by the edit turn + `ProdEffects::fix` (T8); `rw_sweep_*` (T4) used by T8.
- **Resolved (no open items):** `bridge_workflow::template::render(&str, &HashMap<&str,&str>)` (Task 8 uses
  the `{"input": …}` map, mirroring the executor) and `Part { text: String }` are both confirmed against the
  code.
- **No placeholders:** every code step has complete code.

---

## rev2 — dual-review folds (AUTHORITATIVE where it supersedes the tasks above)

Containerized plan-review (claude coverage verified + codex executability) + a2a-local codex backstop, both
fix-before-building. The items below SUPERSEDE the cited steps.

### Fold A — Task 2 Step 1: the `StubInner` stub (compiles)
`fail_prompt` MUST be atomic (mutated through `&self` in `prompt`):
```rust
    struct StubInner {
        canceled: AtomicBool,
        prompts: AtomicUsize,
        sessions: Mutex<std::collections::HashSet<String>>,
        fail_prompt: AtomicBool,
    }
```
`CountingSpawn` gains `fail_prompt: bool` (default false; `new_prompt_fail()` sets true) and builds
`StubInner { canceled: AtomicBool::new(false), prompts: AtomicUsize::new(0), sessions: Mutex::new(HashSet::new()), fail_prompt: AtomicBool::new(self.fail_prompt) }`.
`StubInner::prompt`: `self.prompts.fetch_add(1, SeqCst); self.sessions.lock().await.insert(s.as_str().into());
if self.fail_prompt.load(SeqCst) { return Err(BridgeError::agent_crashed("prompt boom")); } Ok(... one Done ...)`.
The reuse-error test flips it with `inner.fail_prompt.store(true, Ordering::SeqCst)` (no `store_relaxed`).
**Drop** the weak `inner.sessions.lock().len()==1` assertion — assert `spawn.count==1` + `inner.prompts==2` +
same inner instead; the one-`session/new` guarantee is covered at the AcpBackend layer (`new_session_calls`).

### Fold B — Task 2 Step 3: `prompt_warm` (cache-miss vs reuse; SYNCHRONOUS error cleanup; no double-configure)
Construct the `TurnGuard` ONLY on success (so error paths clear `turn_active` synchronously, not via the
detached `Drop`). Cache-miss reaps its just-opened container on a prompt error; reuse does NOT.
```rust
    async fn prompt_warm(&self, session: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        let spec = self.session_cfg.lock().await.get(session).cloned()
            .ok_or(BridgeError::ConfigInvalid { reason: "missing session cwd".into() })?;
        { // concurrency reject + mark active
            let mut ta = self.turn_active.lock().await;
            if ta.contains(session) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("session {} already has an in-flight turn", session.as_str()) });
            }
            ta.insert(session.clone());
        }
        // helper: clear turn_active synchronously on every pre-stream error path.
        macro_rules! fail { ($e:expr) => {{ self.turn_active.lock().await.remove(session); return Err($e); }} }

        let cache_miss = !self.warm.lock().await.contains_key(session);
        if cache_miss {
            let wi = match self.open_inner(session, &spec).await {   // open_inner already reaps on its own failure
                Ok(wi) => wi, Err(e) => fail!(e),
            };
            self.warm.lock().await.insert(session.clone(), wi);
            // NO re-configure on cache-miss: open_inner already configured with the canonical cwd.
        } else {
            // reuse: re-apply the cached canonical cwd (deterministic; minted_cwd guard passes).
            let (inner, rw_canon) = {
                let w = self.warm.lock().await; let wi = w.get(session).unwrap();
                (wi.inner.clone(), wi.rw_canon.clone())
            };
            let mut spec_canon = spec.clone(); spec_canon.cwd = Some(rw_canon);
            if let Err(e) = inner.configure_session(session, &spec_canon).await { fail!(e) } // reuse: no reap
        }
        let (inner, name, reaped) = {
            let w = self.warm.lock().await; let wi = w.get(session).unwrap();
            (wi.inner.clone(), wi.name.clone(), wi.reaped.clone())
        };
        let inner_stream = match inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => {
                if cache_miss { // first-turn failure → reap + remove (no cumulative work to protect)
                    self.warm.lock().await.remove(session);
                    reap_once(&self.reap_fn, self.cfg.sandbox.runtime(), &name, &reaped);
                }
                fail!(e) // reuse: keep the warm entry, do NOT reap
            }
        };
        let guard = TurnGuard { turn_active: self.turn_active.clone(), session: session.clone(), armed: true };
        Ok(wrap_with_turn_guard(inner, inner_stream, guard))
    }
```
`wrap_with_turn_guard` clears `turn_active` synchronously on normal stream end (sets `armed=false`); the
detached `Drop` only covers the early-consumer-drop case (acceptable, matches the per-turn `ContainerReaper`).

### Fold C — Task 3: `retire_warm` clears `turn_active` for drained sessions
After draining `warm`, also clear the markers (a held/raced stream could leave a stale marker):
```rust
    async fn retire_warm(&self) -> Result<(), BridgeError> {
        let entries: Vec<(SessionId, WarmInner)> = { self.warm.lock().await.drain().collect() };
        let runtime = self.cfg.sandbox.runtime().to_string();
        for (s, wi) in entries {
            let _ = wi.inner.cancel(&s).await;
            reap_once(&self.reap_fn, &runtime, &wi.name, &wi.reaped);
            self.turn_active.lock().await.remove(&s);
        }
        Ok(())
    }
```

### Fold D — Task 2/3 extra tests (spec-named; add them)
- **edit-turn open-failure** (cache-miss): `warm_backend` with `CountingSpawn::new(true)` (spawn fails) →
  `prompt_err` → asserts `reaps==1`, `warm` empty, `turn_active` empty.
- **shared-owner equality** (spec §5 silent-leak guard): a unit assertion that
  `container_owner(cfg, mount, "impl") == rw_sweep_targets(&snapshot, cfg)`'s owner for the impl entry.
- **config-identity** (Fold F's `resolve_impl_identity`): unit-test all reject arms (edit multi-node; fix
  multi-node; fix agent != edit; impl not ContainerRw; impl absent) + the happy path.

### Fold E — Task 8 is split: NEW Task 8a (pure helper) + rewritten Task 8b (wiring)
**Task 8a — `resolve_impl_identity` (pure, unit-tested):**
```rust
/// Resolve the single ContainerRw agent that drives BOTH the edit and fix turns of one warm session, or a
/// fail-loud reason. Edit & fix workflows must each be single-node and name the SAME ContainerRw agent.
fn resolve_impl_identity(
    edit_graph: &bridge_workflow::graph::WorkflowGraph,
    fix_graph: Option<&bridge_workflow::graph::WorkflowGraph>,
    snapshot: &bridge_core::domain::RegistrySnapshot,
) -> Result<bridge_core::domain::AgentEntry, String> {
    let edit_node = match edit_graph.nodes.as_slice() {
        [n] => n, _ => return Err("edit workflow must be single-node for the warm session".into()) };
    let id = &edit_node.agent;
    if let Some(fg) = fix_graph {
        match fg.nodes.as_slice() {
            [n] if &n.agent == id => {}
            [_] => return Err("fix workflow agent must match the edit agent (one warm session)".into()),
            _ => return Err("fix workflow must be single-node".into()),
        }
    }
    let entry = snapshot.entries.iter().find(|e| &e.id == id)
        .ok_or_else(|| format!("impl agent {} not found in snapshot", id.as_str()))?.clone();
    if entry.kind != bridge_core::domain::AgentKind::ContainerRw {
        return Err(format!("warm session requires a container_rw impl agent, got {:?}", entry.kind));
    }
    Ok(entry)
}
```
Unit tests cover all five reject arms + happy path (Docker-free; build graphs/snapshot in-test).

**Task 8b — wiring, COMPLETE ORDERING (fixes the borrow-after-move ×2 + the placeholder):** all warm setup
goes ABOVE the `make_spawn_fn`/`Registry::new` moves (beside the existing `ro_sweep_targets`/`_ro_guard`),
because `snapshot` moves into `Registry::new` (~`main.rs:757`) and `owner_config_path` into `make_spawn_fn`
(~`:755`). Order inside `implement_cmd`, right after `snapshot` is built + `owner_config_path` canonicalized:
```rust
    // (already present) let ro_targets = ro_sweep_targets(&snapshot, &owner_config_path); ro_sweep(&ro_targets);
    // (already present) let _ro_guard = RoSweepGuard(ro_targets);
    let _rw_guard = RwSweepGuard(rw_sweep_targets(&snapshot, &owner_config_path)); // declared with _ro_guard → drops after backends
    rw_sweep(&rw_sweep_targets(&snapshot, &owner_config_path));                    // boot-sweep (or reuse one Vec)
    let impl_entry = resolve_impl_identity(&graph, fix_graph.as_deref(), &snapshot)
        .map_err(|e| format!("implement: {e}"))?;                                  // reads &snapshot BEFORE the move
    let edit_template = graph.nodes[0].prompt_template.clone();
    let fix_template: Option<String> = fix_graph.as_ref().map(|g| g.nodes[0].prompt_template.clone());
    let warm_owner = container_owner(&owner_config_path, impl_entry.sandbox.as_ref().unwrap().mount.as_str(), impl_entry.id.as_str());
    let warm = bridge_container::ContainerRwBackend::new_warm(
        container_rw_cfg_from_entry(&impl_entry)?,
        Arc::new(AcpContainerSpawn { policy: Arc::clone(&policy) }) as Arc<dyn bridge_container::ContainerSpawn>,
        warm_owner,
    ).await?;
    let impl_session = bridge_core::ids::SessionId::parse(format!("implement-{task_id}"))
        .map_err(|e| format!("implement: session id: {e:?}"))?;
    warm.configure_session(&impl_session, &SessionSpec { config: Default::default(), cwd: Some(clone_cwd.clone()) }).await?;
    // THEN (unchanged): policy_for_spawn = Arc::clone(&policy) as ...; spawn = make_spawn_fn(policy_for_spawn, owner_config_path);
    //                   registry = Registry::new(snapshot, spawn); executor = ... (review still uses these).
```
- `policy` (the `Arc<AutoPolicy>`) is cloned for BOTH the warm `AcpContainerSpawn` AND the existing
  `policy_for_spawn` — NO `policy_for_spawn_or_equivalent` placeholder.
- Edit turn (replaces the executor edit call): render `edit_template` with `{"input": &a.task}` →
  `warm.prompt(&impl_session, vec![Part{text}])` → `drain_turn`; on `Err` → `completed=false` (decide aborts).
- `ProdEffects`: `{ impl_backend: &'a dyn AgentBackend, impl_session: &'a SessionId, fix_template: Option<String>,
  + the unchanged verify/review fields }`; `fix` renders `self.fix_template.as_deref().expect("fix_available")`
  (run_tweak_loop only calls `fix` when `fix_available==true`); drop the `fix_graph` field.
- **`retire()` on EVERY terminal arm:** the warm container is opened before `decide`, so the `Abort` /
  `NoCommitClean` / `NoCommitDirty` arms must each `let _ = warm.retire().await;` before returning, and the
  `Commit` arm does `println!(handoff); let _ = warm.retire().await; Ok(())` (print BEFORE retire). M4
  retire-error coverage is **by-construction ordering** (print precedes retire; the `let _` swallows) — NOT a
  separate test (no fake-backend seam in `implement_cmd`); state this honestly.

### Fold F — minors
- **lib.rs module doc** (`lib.rs:3-4`): drop "Warm reuse across turns is a separate future slice"; replace
  with a one-line note that warm reuse is now supported via `new_warm` (do this in Task 1 or 3).
- **Task 10 `git add`**: stage an explicit scoped list (the files this slice touches), not `$(git diff --name-only)`.
- **NEW Task 12 — ADR-0024** (promoted from prose): write `docs/adr/0024-warm-loop-session.md` post-merge
  (carries the `Co-Authored-By` trailer).
