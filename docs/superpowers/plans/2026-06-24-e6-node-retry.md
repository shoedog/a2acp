# E6 — Node Retry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A workflow node that fails with a TRANSIENT agent error (crash / `_dyld_start` startup flake / overload / watchdog timeout) is automatically retried within the run — bounded attempts + overflow-safe backoff, with a clean respawn between attempts — before degrading to `ok=false`. Opt-in per node; default off (zero behavior change).

**Architecture:** A `BridgeError::is_transient()` classifier (bridge-core) gates a retry loop in the cold executor's `run_node`. The loop spans `resolve → configure → prompt → drain`; on a transient failure it `release_session`s the prior attempt, `invalidate`s the agent's cached backend (a NEW `AgentRegistry` seam that atomically replaces the slot so the next `resolve` RESPAWNS a fresh process), backs off (cancel-abortable), and re-attempts. Per-node `RetryPolicy` rides the durable spec snapshot (like Slice-10 `panel`). Last-attempt usage; `tracing` observability; resume-compatible for free.

**Tech Stack:** Rust (bridge-core error.rs, bridge-workflow graph.rs/executor.rs, bridge-registry registry.rs + bridge-core ports.rs, bin/a2a-bridge config.rs), tokio (select/sleep), serde, arc-swap.

**Binding spec:** `docs/superpowers/specs/2026-06-24-e6-node-retry.md` — the `## v2` (SR-FIX-1..6) + `## v3` (RR-FIX-1..4) sections supersede the v1 body. Base = `main` `d274177`. Branch `feat/e6-node-retry`.

---

## Reference facts (verified — do not re-derive)
- `BridgeError` variants (`crates/bridge-core/src/error.rs:22-72`); `is_resumable()` (`:127`), `disposition()` (`:107`). Transient set (v3 D4) = `AgentCrashed | AgentOverloaded | AgentTimedOut`; everything else non-transient.
- `WorkflowNode { id, agent, prompt_template, inputs }` (`crates/bridge-workflow/src/graph.rs:20-25`); `WorkflowGraph.panel: Option<PanelConfig>` (`:16`) is the additive `#[serde(default, skip_serializing_if="Option::is_none")]` precedent. The durable snapshot is plain serde JSON of `WorkflowGraph` via `encode_workflow_spec` (resume restores it).
- `WorkflowNodeToml { id, agent, prompt_file, inputs }` (`bin/a2a-bridge/src/config.rs`, `#[derive(Debug, serde::Deserialize)]`). Mapped to `WorkflowNode` at the graph build in `into_snapshot`/the workflows loader.
- `AgentRegistry` trait (`crates/bridge-core/src/ports.rs:197-213`): `resolve`, `default_id`, `apply`, `list`, `mcp_advertisement` (default). Add `invalidate` here (default no-op).
- Registry impl (`crates/bridge-registry/src/registry.rs`): `Slot { entry: ArcSwap<AgentEntry>, backend: OnceCell<Arc<dyn AgentBackend>> }` (`:32-34`); `State { slots: HashMap<AgentId, Arc<Slot>>, ... }` in `state: ArcSwap<State>` (`:93`); `resolve` lazy-spawns via `slot.backend.get_or_try_init` (`:308`, uninitialized on spawn failure — `:305`); `apply` rebuilds + atomically `state.store`s the slot map (`:366-412`); `retire()` (`:486`) drains leases then retires.
- `run_node` (`crates/bridge-workflow/src/executor.rs:158-388`): cold session id `workflow-{wf}-{node}-{run}` (`:250`); resolve cancel-select (`:266-273`); configure fail-on-error (T6, `:275-291`, regression test `:1549`); prompt cancel-select + error site (`:299-332`); drain loop + `Some(Err)` site (`:337-359`); `STOP_REASON_CANCELLED` (`:352`); rich-flush + `forget_session` on exit (`:363-388`); 3-tuple return (`:167`). Existing cold test harness: `cold_configure_error_fails_node_without_prompting` + `single_node_configures_renders_concatenates` (`~:1480/1519`); `Rec { configured, prompts: Mutex<Vec<String>>, forgets, cancels }` (`:646`).
- `release_session` (`crates/bridge-acp/src/acp_backend.rs:2705`): cancels + drops the bridge `sessions` map + config stash; process-preserving (does NOT retire). Watchdog can KILL the process past grace yet still report `AgentTimedOut` (`:2376/2435`) — hence the invalidate seam.
- W3b resume: `crates/bridge-coordinator/src/detached.rs` `resume_working_tasks` builds the seed from `node_checkpoints` (incl. `ok=false`); `run_from` skips seeded nodes; checkpoints/watch key on `NodeId`, not the cold `SessionId`. `SessionId::parse` rejects only empty (`crates/bridge-core/src/ids.rs:10`).

## File Structure
| File | Responsibility | Tasks |
|---|---|---|
| `crates/bridge-core/src/error.rs` | `is_transient()` classifier | T1 |
| `crates/bridge-workflow/src/graph.rs` | `RetryPolicy` + `WorkflowNode.retry` (rides spec snapshot) | T2 |
| `bin/a2a-bridge/src/config.rs` | `WorkflowNodeToml.retry` + map → `WorkflowNode.retry` | T3 |
| `crates/bridge-core/src/ports.rs` | `AgentRegistry::invalidate` trait method (default no-op) | T4 |
| `crates/bridge-registry/src/registry.rs` | `invalidate` impl (atomic Slot replace + retire old) | T4 |
| `crates/bridge-workflow/src/executor.rs` | the retry loop in `run_node` + resume-compat test | T5, T6 |

Bottom-up order keeps the tree green per task: classifier (T1) → policy type + snapshot (T2) → config wiring (T3) → registry invalidate seam (T4) → the retry loop (T5) → resume-compat + workspace gate (T6).

---

## Task 1: `BridgeError::is_transient()` classifier

**Files:**
- Modify: `crates/bridge-core/src/error.rs`

- [ ] **Step 1: Write the failing test** — in `error.rs` tests:

```rust
#[test]
fn is_transient_covers_every_variant() {
    use BridgeError::*;
    // transient (retryable): the agent process died / is overloaded / hung
    for e in [AgentCrashed { reason: "x".into() }, AgentOverloaded, AgentTimedOut] {
        assert!(e.is_transient(), "{e:?} must be transient");
    }
    // NON-transient: needs human/config action, protocol/state/persistence bug, or user intent
    for e in [
        A2aVersionMismatch, InvalidRequest { field: "x" }, TaskNotFound, SessionNotFound,
        ConfigMismatch { field: "x" }, ConfigReseedRequired { field: "x" }, SessionExpired, HandleBusy,
        AuthRequired { request_id: "r".into() }, PermissionRequired { request_id: "r".into() },
        PermissionDenied, AgentNotAuthenticated, ModelNotAvailable, CancelTimeout, FrameError,
        MessageTooLarge, UpstreamA2aError, StoreFailure, InvalidStateTransition,
        UnknownAgent { id: "a".into() }, ConfigInvalid { reason: "x".into() }, AgentOverloaded_is_transient_marker_skip(),
    ] {} // (see note below — enumerate the real non-transient set without the marker)
}
```
(Write the test enumerating the REAL non-transient variants — drop the placeholder `AgentOverloaded_..._skip()`; assert each `!is_transient()`. The point: EVERY variant is classified, so a future variant forces a decision.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core is_transient`
Expected: FAIL — `is_transient` undefined.

- [ ] **Step 3: Implement** — in `impl BridgeError`, beside `is_resumable()`:

```rust
/// True for failures that a workflow node MAY retry (the agent crashed, is overloaded, or hung) —
/// the single source of truth for E6 retry. COLD-workflow-only; do NOT reuse the warm-respawn
/// classifier (`resilient.rs`, deliberately different). Everything else needs human/config action,
/// indicates a protocol/state/persistence bug, or is user intent (cancel) → fail fast.
pub fn is_transient(&self) -> bool {
    matches!(
        self,
        BridgeError::AgentCrashed { .. } | BridgeError::AgentOverloaded | BridgeError::AgentTimedOut
    )
}
```

- [ ] **Step 4: Run** — `cargo test -p bridge-core is_transient` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/error.rs
git commit -m "feat(core): T1 — BridgeError::is_transient() classifier (E6)"
```

---

## Task 2: `RetryPolicy` + `WorkflowNode.retry` (rides the durable spec snapshot)

**Files:**
- Modify: `crates/bridge-workflow/src/graph.rs`

- [ ] **Step 1: Write the failing test** — in `graph.rs` tests:

```rust
#[test]
fn retry_policy_rides_the_spec_snapshot_round_trip() {
    let node = WorkflowNode {
        id: NodeId::parse("n1").unwrap(),
        agent: AgentId::parse("codex").unwrap(),
        prompt_template: "p".into(),
        inputs: vec![],
        retry: Some(RetryPolicy { max_attempts: 3, backoff_ms: 500, backoff_cap_ms: Some(30_000) }),
    };
    let json = serde_json::to_string(&node).unwrap();
    let back: WorkflowNode = serde_json::from_str(&json).unwrap();
    assert_eq!(back.retry, Some(RetryPolicy { max_attempts: 3, backoff_ms: 500, backoff_cap_ms: Some(30_000) }));
    // additive: a node serialized WITHOUT retry deserializes to None
    let no_retry: WorkflowNode = serde_json::from_str(r#"{"id":"n1","agent":"codex","prompt_template":"p","inputs":[]}"#).unwrap();
    assert_eq!(no_retry.retry, None);
}
```
(Match the REAL `NodeId`/`AgentId` constructors + the existing serde field names — verify against `graph.rs`. If `WorkflowNode` isn't `Serialize`/`Deserialize` directly, test via the `encode_workflow_spec`/`WorkflowGraph` round-trip the panel test uses.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-workflow retry_policy_rides`
Expected: FAIL — `RetryPolicy` / `retry` field undefined.

- [ ] **Step 3: Implement** — in `graph.rs`:

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_cap_ms: Option<u64>,
}
impl RetryPolicy {
    /// Total attempts (>=1). `max_attempts == 0` is treated as 1 (defensive).
    pub fn attempts(&self) -> u32 { self.max_attempts.max(1) }
    /// Overflow-safe backoff for `attempt` (1-based): min(backoff_ms * 2^(attempt-1), cap).
    pub fn backoff_for(&self, attempt: u32) -> std::time::Duration {
        let shift = attempt.saturating_sub(1).min(63);
        let base = self.backoff_ms.checked_shl(shift).unwrap_or(u64::MAX);
        let cap = self.backoff_cap_ms.unwrap_or(30_000);
        std::time::Duration::from_millis(base.min(cap))
    }
}
```
Add `retry: Option<RetryPolicy>` to `WorkflowNode` with `#[serde(default, skip_serializing_if = "Option::is_none")]` (mirror `WorkflowGraph.panel`). Update EVERY `WorkflowNode { .. }` literal in the crate (construction sites + tests) to add `retry: None` — `cargo build -p bridge-workflow` surfaces them.

- [ ] **Step 4: Run** — `cargo test -p bridge-workflow` → PASS (round-trip + backoff math; add a `backoff_for` unit assert: `attempt=1 → 500ms`, `attempt=10 → capped 30_000ms`, no overflow panic).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/graph.rs
git commit -m "feat(workflow): T2 — RetryPolicy + WorkflowNode.retry (rides spec snapshot)"
```

---

## Task 3: `WorkflowNodeToml.retry` + map → `WorkflowNode.retry`

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test** — in `config.rs` tests (reuse `AGENTS_HEADER`/`SERVER_FOOTER`):

```rust
#[test]
fn workflow_node_retry_parses_and_maps() {
    let toml = format!(
        "{AGENTS_HEADER}\n[[workflows]]\nid = \"wf1\"\n\
         [[workflows.nodes]]\nid = \"only\"\nagent = \"codex\"\nprompt_file = \"p.md\"\ninputs = []\n\
         retry = {{ max_attempts = 3, backoff_ms = 250 }}\n{SERVER_FOOTER}"
    );
    let cfg: RegistryConfig = toml::from_str(&toml).unwrap();
    let n = &cfg.workflows[0].nodes[0];
    let r = n.retry.as_ref().unwrap();
    assert_eq!((r.max_attempts, r.backoff_ms), (3, 250));
}
```
(If the workflows→graph mapping is reachable in a test, also assert the mapped `WorkflowNode.retry` is `Some`. Else the graph-build mapping is covered by T5's integration.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge workflow_node_retry`
Expected: FAIL — `retry` field unknown / undefined.

- [ ] **Step 3: Implement** — in `config.rs`:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct RetryToml {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    #[serde(default)]
    pub backoff_cap_ms: Option<u64>,
}
```
Add `#[serde(default)] pub retry: Option<RetryToml>,` to `WorkflowNodeToml`. At the graph-build site that constructs `WorkflowNode` from `WorkflowNodeToml` (grep `WorkflowNode {` in config.rs / the workflows loader), map `retry: n.retry.as_ref().map(|r| bridge_workflow::graph::RetryPolicy { max_attempts: r.max_attempts, backoff_ms: r.backoff_ms, backoff_cap_ms: r.backoff_cap_ms })`.

- [ ] **Step 4: Run** — `cargo test -p a2a-bridge workflow_node_retry && cargo build --workspace` → PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(workflow): T3 — [[workflows.nodes]].retry config + map to WorkflowNode.retry"
```

---

## Task 4: `AgentRegistry::invalidate(agent)` seam (atomic Slot replace + retire old)

**Files:**
- Modify: `crates/bridge-core/src/ports.rs` (trait method, default no-op)
- Modify: `crates/bridge-registry/src/registry.rs` (impl + test)

**Design (RR-FIX-1):** `invalidate(agent)` atomically REPLACES that agent's `Slot` with a fresh one (new empty `OnceCell`) via an `ArcSwap` state store (mirroring `apply`'s swap) so the NEXT `resolve()` RESPAWNS. The OLD backend is RETIRED (process killed) so respawn-every-retry does NOT leak processes; concurrent resolvers that already hold an `Arc` clone keep using it (their turn already failed). Idempotent + best-effort (unknown agent → no-op).

- [ ] **Step 1: Write the failing test** — in `registry.rs` tests (mirror `apply_*`):

```rust
#[tokio::test]
async fn invalidate_replaces_slot_so_next_resolve_respawns() {
    // a registry whose spawn fn counts spawns + hands out a fresh fake backend each call
    let spawns = Arc::new(AtomicUsize::new(0));
    let reg = registry_with_counting_spawn(spawns.clone()); // helper mirroring the apply tests' setup
    let id = AgentId::parse("a").unwrap();
    let _b1 = reg.resolve(&id).await.unwrap();        // spawn #1
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    let _b2 = reg.resolve(&id).await.unwrap();        // cached — no new spawn
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    reg.invalidate(&id).await;                         // drop the cached backend
    let _b3 = reg.resolve(&id).await.unwrap();        // RESPAWN
    assert_eq!(spawns.load(Ordering::SeqCst), 2, "invalidate forces a respawn");
    reg.invalidate(&AgentId::parse("ghost").unwrap()).await; // unknown → no-op, no panic
}
```
(Build `registry_with_counting_spawn` from the existing test scaffolding — the `apply_*` tests already construct a `Registry` with a custom `SpawnFn`; thread an `AtomicUsize` + a trivial fake `AgentBackend`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-registry invalidate_replaces_slot`
Expected: FAIL — `invalidate` undefined.

- [ ] **Step 3: Implement**

`ports.rs` — add to the `AgentRegistry` trait (default no-op so mocked/test registries need no change):
```rust
/// Drop the cached backend for `agent` so the next `resolve` RESPAWNS a fresh process (E6 retry
/// reset). Best-effort + idempotent; unknown agent ⇒ no-op. Default: no-op (non-spawning registries).
async fn invalidate(&self, _agent: &crate::ids::AgentId) {}
```
`registry.rs` — impl on the real `Registry`: under the same discipline as `apply`, build a `next` slot map that REPLACES `agent`'s `Arc<Slot>` with a fresh `Slot::new(entry)` (new `OnceCell`), `state.store` it atomically; then RETIRE the old slot's backend if initialized:
```rust
async fn invalidate(&self, agent: &AgentId) {
    let old = {
        let st = self.state.load();
        let Some(old_slot) = st.slots.get(agent).cloned() else { return; }; // unknown → no-op
        let entry = old_slot.entry.load_full();
        let mut next = st.slots.clone();
        next.insert(agent.clone(), Arc::new(Slot::new((*entry).clone())));   // fresh OnceCell
        self.state.store(Arc::new(State { slots: next, default: st.default.clone() /* match State fields */ }));
        old_slot
    };
    // retire the replaced backend (kill the process) so respawn-every-retry doesn't leak; best-effort.
    if let Some(be) = old.backend.get() {
        let _ = be.retire().await;
    }
}
```
(VERIFY the exact `State` fields + `Slot::new` signature; match `apply`'s store shape. Note the apply-vs-invalidate ArcSwap race: a concurrent `apply` may overwrite the swap — acceptable since `apply` rebuilds from desired config; the lost invalidate just means one stale-backend resolve, self-healing on the next failure→invalidate. Document this.)

- [ ] **Step 4: Run** — `cargo test -p bridge-registry && cargo build --workspace` → PASS (incl. the default-no-op for any mocked `AgentRegistry` impls — grep `impl AgentRegistry` to confirm none break).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/ports.rs crates/bridge-registry/src/registry.rs
git commit -m "feat(registry): T4 — AgentRegistry::invalidate(agent) seam (atomic slot replace + retire old)"
```

---

## Task 5: the retry loop in `run_node`

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs`

**Design (the core, v3):** wrap `resolve → configure → prompt → drain` in `for attempt in 1..=node.retry.attempts()`. Classify each attempt's outcome: `Ok` / `Canceled` / `Transient(err)` / `Fatal`. On `Transient` with `attempt < attempts` AND not cancelled → `release_session(session)` + `registry.invalidate(&node.agent)` + `tracing::warn!(node, attempt, ?err, "node retry")` + a CANCEL-ABORTABLE backoff sleep → `continue` (the next iteration re-`resolve`s → respawn). On `Ok`/`Canceled`/`Fatal`/exhausted → the current behavior (`forget_session` + return), `ok=false` carrying a `[node N failed after K attempts: <err>]` marker on exhaustion. **Usage = the LAST attempt's** (do NOT sum). When `node.retry` is `None`, `attempts()==1` ⇒ exactly today's single-attempt path (zero behavior change).

- [ ] **Step 1: Write the failing tests** — in `executor.rs` tests, a configurable `FlakyBackend` (mirror `cold_configure_error_fails_node`'s registry harness) whose `prompt` fails transiently a set number of times then succeeds (or always-transient / non-transient), counting prompts:

```rust
// FlakyBackend modes: FailThenOk(n) → first n prompts return Err(AgentOverloaded), then a normal stream;
//                     AlwaysTransient → every prompt Err(AgentTimedOut);
//                     Fatal → prompt Err(PermissionDenied).
#[tokio::test]
async fn retry_succeeds_after_transient_failures() {
    // node with retry { max_attempts: 3, backoff_ms: 0 }, FlakyBackend::FailThenOk(2)
    // → node finishes ok=true, prompt called 3x, registry.invalidate called 2x.
}
#[tokio::test]
async fn retry_exhausts_then_degrades() {
    // retry { max_attempts: 2, backoff_ms: 0 }, AlwaysTransient
    // → ok=false, marker contains "after 2 attempts", prompt called 2x.
}
#[tokio::test]
async fn non_transient_fails_without_retry() {
    // retry { max_attempts: 3, backoff_ms: 0 }, Fatal → ok=false on attempt 1, prompt called 1x, invalidate 0x.
}
#[tokio::test]
async fn no_retry_policy_is_single_attempt() {
    // retry: None, AlwaysTransient → ok=false, prompt called 1x (today's behavior).
}
#[tokio::test]
async fn cancel_mid_backoff_aborts_retry() {
    // retry { max_attempts: 5, backoff_ms: 60_000 }, AlwaysTransient; cancel the token during the first
    // backoff → returns canceled marker promptly, prompt NOT called again.
}
```
(Thread an invalidate-counter through the test registry — wrap the real `Registry` or a fake `AgentRegistry` that counts `invalidate`. Use `backoff_ms: 0` so the non-cancel tests don't sleep.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p bridge-workflow retry_`
Expected: FAIL — no retry loop yet (FlakyBackend's first failure degrades immediately).

- [ ] **Step 3: Implement** — restructure `run_node` (`:158-388`): extract the per-attempt core (resolve→configure→prompt→drain→rich-flush) into the loop body, returning an attempt classification. The loop:

```rust
let attempts = node.retry.as_ref().map(|r| r.attempts()).unwrap_or(1);
let mut last: (String, bool, Option<UsageSnapshot>) = (
    format!("[node {} not run]", node.id.as_str()), false, None,
);
for attempt in 1..=attempts {
    if cancel.is_cancelled() { return (format!("[node {} canceled]", node.id.as_str()), false, None); }
    // === one attempt: resolve (cancel-select) → configure → prompt (cancel-select) → drain ===
    //   - a resolve/configure/prompt/drain error e: if e.is_transient() classify Transient(e), else Fatal(e).
    //   - cancellation (any cancel arm / STOP_REASON_CANCELLED / canceled_during_drain) → return canceled NOW.
    //   - success → forget_session; return (text, true, usage).
    // (Reuse every existing cancel-select + the rich-sink flush + the existing markers.)
    let outcome = /* (text, ok, usage, class) where class ∈ {Ok, Canceled, Transient(e), Fatal} */;
    match class {
        Canceled => return outcome_canceled,
        Ok => { resolved.backend.forget_session(&session).await; return (text, true, usage); }
        Transient(e) if attempt < attempts => {
            last = (format!("[node {} failed (attempt {attempt}/{attempts}): {e:?}]", node.id.as_str()), false, usage);
            resolved.backend.release_session(&session).await;       // bridge-side drop + cancel
            self.registry.invalidate(&node.agent).await;             // respawn next resolve
            tracing::warn!(node = node.id.as_str(), attempt, error = ?e, "node retry");
            // CANCEL-ABORTABLE backoff
            let backoff = node.retry.as_ref().unwrap().backoff_for(attempt);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return (format!("[node {} canceled]", node.id.as_str()), false, None),
                _ = tokio::time::sleep(backoff) => {}
            }
            continue;
        }
        Transient(e) => { // exhausted
            resolved.backend.forget_session(&session).await;
            return (format!("[node {} failed after {attempts} attempts: {e:?}]", node.id.as_str()), false, usage);
        }
        Fatal(_) => { resolved.backend.forget_session(&session).await; return (text, false, usage); }
    }
}
last
```
Notes for the implementer: (a) `self.registry` is the executor's registry handle — confirm it's `Arc<dyn AgentRegistry>` reachable in `run_node`; (b) `resolved` must be re-bound each iteration (resolve INSIDE the loop — SR-FIX-1, so an invalidated agent respawns); (c) keep `forget_session` on the non-retry exits (Ok/Fatal/exhausted) exactly as today; on the retry path use `release_session` + `invalidate` instead (the respawn supersedes the bridge session); (d) preserve the T6 configure fail-fast — a `ConfigInvalid` configure error is `Fatal` (non-transient) → no retry, the regression test stays green; (e) `tokio::time` must be available (the crate already uses tokio; add the `time` feature to `bridge-workflow`'s tokio dep if `cargo build` complains).

- [ ] **Step 4: Run** — `cargo test -p bridge-workflow` (the 5 new + ALL existing incl. `cold_configure_error_fails_node` + the panel/usage tests) → PASS. Then `cargo test --workspace --all-targets` (ripple).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "feat(workflow): T5 — run_node retry loop (is_transient-gated, release+invalidate+respawn reset, cancel-abortable backoff)"
```

---

## Task 6: resume-compat assertion + workspace gate

**Files:**
- Modify: `crates/bridge-workflow/src/executor.rs` tests (or the detached resume tests)

- [ ] **Step 1: Write the test** — assert a retrying node interrupted before finishing is NOT checkpointed/seeded (so W3b re-runs it fresh):

```rust
#[tokio::test]
async fn interrupted_retrying_node_is_not_seeded_on_resume() {
    // run a one-node graph with retry { max_attempts: 5, backoff_ms: 60_000 }, AlwaysTransient;
    // cancel during the first backoff (mid-retry, before NodeFinished) → assert NO NodeFinished /
    // no checkpoint was emitted for the node (the sink/recorder saw none), so resume would re-run it.
}
```
(Reuse the cancel harness from T5 + the existing checkpoint/sink recorder used by the W3b/detached tests to assert no `NodeFinished` for the node.)

- [ ] **Step 2: Run** — `cargo test -p bridge-workflow interrupted_retrying_node` → PASS (no checkpoint on a mid-retry cancel).

- [ ] **Step 3: Full workspace gate** (controller, clean host env):

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --all-targets`
Expected: clean fmt, no clippy warnings, all pass (watch the pre-existing server.rs `warm_streaming_records_usage…` flake → re-run if it trips; the `a2a_bridge` bin-test sandbox stall is codex-only — the controller runs clean).

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-workflow/src/executor.rs
git commit -m "test(workflow): T6 — resume-compat (mid-retry interrupt not seeded) + workspace gate"
```

---

## After the tasks (process — not plan steps)
1. **Whole-branch dual-lens review** (codex xhigh + Opus) over the full diff vs `main`. Fold blockers/majors.
2. **Live-gate** (spec v3): a config with a node `retry = { max_attempts = 3, backoff_ms = 200 }` + a flaky/forced-transient real agent → assert (1) transient-then-success completes ok; (2) always-transient exhausts to `ok=false` after `max_attempts`; (3) non-transient fails on attempt 1; (4) cancel mid-retry aborts; (5) a `tracing` retry line per attempt. A scripted-flaky backend proves the wiring; a real agent pointed at a transiently-failing endpoint proves the classifier end-to-end.
3. **Merge** `--no-ff` + push; update memory (`e6-node-retry-shipped`) + the orchestration handoff.

**Staging discipline:** stage ONLY each task's files. Pre-existing untracked `examples/*.toml`/`prompts/*.md` + `M examples/a2a-bridge.slicing-analysis.toml` — NEVER fold them.

## Self-Review (against spec v3)
**1. SR/RR coverage:** `is_transient` D4 (T1); `RetryPolicy`+snapshot SR-FIX-1-plumbing + Q1/D1 overflow-safe backoff (T2); config wiring (T3); `invalidate` seam RR-FIX-1 (T4); the retry loop SR-FIX-1 (resolve-in-loop) + SR-FIX-2/RR-FIX-1/2 (release+invalidate+respawn reset, same SessionId) + SR-FIX-3 (is_transient-gates all sites, T6/ConfigInvalid fail-fast) + SR-FIX-4 (last-attempt usage) + SR-FIX-6 (tracing) + cancel-abortable backoff (T5); resume-compat (T6). SR-FIX-5 (read-only-recommended) = doc/config, not code. ✅
**2. Placeholders:** each step has real test + impl code + commands. The T1 test note (enumerate the real non-transient set) + the T4 `State`-fields verify + the T5 attempt-extraction are flagged inline for the implementer to bind to the real types. ✅
**3. Type consistency:** `RetryPolicy { max_attempts: u32, backoff_ms: u64, backoff_cap_ms: Option<u64> }` + `attempts()`/`backoff_for()` + `WorkflowNode.retry` + `RetryToml` + `AgentRegistry::invalidate(&AgentId)` consistent across T2/T3/T4/T5. ✅
**4. Open items for plan-review:** (a) does `invalidate` retiring the old backend (T4) block (retire drains leases) — should it be best-effort/non-blocking? (b) the apply-vs-invalidate ArcSwap race (T4) — acceptable or needs compare-swap? (c) is `self.registry` actually reachable + `Arc<dyn AgentRegistry>` in `run_node` (T5)? (d) does `release_session` THEN `invalidate`+respawn double-cancel harmlessly, or is `release` redundant given respawn (drop it)? (e) the FlakyBackend + invalidate-counter harness shape (T5). Confirm in the dual plan-review.

---

## v2 — dual plan-review folded (codex xhigh: 4 BLOCKER + 6 MAJOR; Opus lens) — BINDING

> Supersedes the task bodies above where it conflicts. The ARCHITECTURE + task list HOLD; the MECHANICS of T4 (the
> registry seam) + T5 (the attempt model) + T6 (the resume test) needed real correction. Apply each in its named
> task. CONFIRMED (do NOT re-litigate): `self.registry: Arc<dyn AgentRegistry>` IS reachable in `run_node`
> (`executor.rs:119/120`); `tokio::time` IS available (workspace `tokio` = full features, `Cargo.toml:11`);
> `release_session` STAYS in the reset (clears BRIDGE-side session/config state — `acp_backend.rs:2705`) while
> `invalidate` handles PROCESS replacement (they are NOT redundant — plan open-item (d) resolved).

### PR-FIX-1 (BLOCKER-1, T2) — `WorkflowNode.retry` ripples WORKSPACE-WIDE (~32 literals), not just the crate
`WorkflowNode` is constructed far outside `bridge-workflow`: `bin/a2a-bridge/src/main.rs:4729`,
`crates/bridge-a2a-inbound/src/server.rs:6296`, `crates/bridge-coordinator/src/coordinator.rs:886`,
`detached.rs:1080`, `crates/bridge-mcp/tests/mcp_client.rs:222`, the inbound/integration tests, etc. (~32 literals).
T2 must add `retry: None` to EVERY workspace construction site; the gate is **`cargo build --workspace`** (not
`-p bridge-workflow`). (Optional churn-cut: add a `WorkflowNode::new(id, agent, prompt_template, inputs)` ctor that
defaults `retry: None` and migrate literals — but the field add + `retry: None` is the minimum.)

### PR-FIX-2 (BLOCKER-2, T4) — close the `apply`-vs-`invalidate` ArcSwap race (no slot resurrection)
`apply` does a stale `load_full` → build `next` → `state.store` with NO writer lock (`registry.rs:376-414`) — fine
as the SOLE writer. `invalidate` adding a second load→modify→store RACES it: an `invalidate` that wins after a
concurrent `apply` would RESURRECT removed/old slots or the old `default`. **Add a shared writer `Mutex` (e.g.
`write_lock: tokio::sync::Mutex<()>`) held across the load→modify→store in BOTH `apply` AND `invalidate`** (serialize
the two writers). `invalidate` must NO-OP if the agent is absent from the CURRENT state (re-load under the lock; don't
re-insert a vanished agent).

### PR-FIX-3 (BLOCKER-3, T4) — do NOT await `be.retire()`; reuse the DETACHED lease-draining retirement
Direct `backend.retire()` KILLS the process (`acp_backend.rs:2728`) out from under concurrent sessions still holding
that `Arc` (a fan-out workflow can have node B mid-turn on the SAME agent). Mirror `apply` (`registry.rs:416-425`):
after the slot swap, mark the OLD slot `retired = true` SYNCHRONOUSLY (closes resolve's spawn/retire race —
`resolve` re-checks `retired` at `:322`) and hand it to the DETACHED lease-draining retirement task (awaits
leases==0 or the grace deadline, THEN `retire()`). **`invalidate` NEVER awaits process teardown.** Extract apply's
retirement-of-one-slot into a reusable helper (`spawn_retirement(old_slot, self.grace)` or similar) and call it from
both. (This makes the retry path non-blocking AND concurrency-safe — the old process drains its leases before dying.)

### PR-FIX-4 (MAJOR-4, T4) — `Slot::new` already returns `Arc<Slot>` (no double-Arc)
`Slot::new(entry) -> Arc<Slot>` (`registry.rs:43`). The plan's `Arc::new(Slot::new(...))` would be `Arc<Arc<Slot>>`.
Use `next.insert(agent.clone(), Slot::new((*entry).clone()))`. `State { slots, default }` (`:55-57`) — match it.

### PR-FIX-5 (MAJOR-5, T5) — a resolve-time failure has NO `resolved` backend
T5's reset can't call `resolved.backend.release_session(...)` on a resolve-time `AgentCrashed` (the startup flake) —
there is no `resolved`. Model the attempt outcome as e.g. `enum Attempt { Ok(text,usage), Canceled(text),
Transient { err: BridgeError, backend: Option<Arc<dyn AgentBackend>> }, Fatal(text,usage) }`. On `Transient`: if
`backend.is_some()` → `release_session` it; ALWAYS `invalidate(&node.agent)` + backoff + re-resolve. A resolve
failure → `backend: None` → skip release, still invalidate (so the uninitialized/failed `OnceCell` is replaced) +
backoff + re-resolve.

### PR-FIX-6 (MAJOR-6, T5) — the test must PROVE resolve-in-loop + invalidate (no tautology)
`FlakyBackend::FailThenOk(n)` passes even if `resolve` stays OUTSIDE the loop (same backend re-prompted). The test
registry MUST: (a) count `resolve` calls, (b) count `invalidate` calls, (c) make SUCCESS depend on a POST-`invalidate`
FRESH backend instance (e.g. the spawn fn hands out a backend that fails until `invalidate` swaps in a fresh one).
Assert `resolve_count == attempts`, `invalidate_count == attempts-1`, and success only after the fresh resolve.

### PR-FIX-7 (BLOCKER-7, T6) — resume-compat: test a DROPPED runner future (crash), NOT cancellation
Cancellation is the WRONG proxy: `run_node` emits `NodeFinished` on EVERY return incl. canceled (`executor.rs:615`),
and detached mode checkpoints it (`detached.rs:310`) — so a canceled retry IS seeded. Resume-compat holds only for a
CRASH = the retry future DROPPED before returning. T6 must DROP/abort the runner task while a node's retry future is
still pending (mid-backoff) and assert NO `NodeFinished`/checkpoint was emitted for the node (so W3b re-runs it).
(Use the detached/run harness; abort the spawned run task mid-flight, or drop the `run_from` future.)

### PR-FIX-8 (MAJOR-8) — prove `ConfigInvalid` stays fatal WITH retry enabled
The existing configure fail-fast test (`executor.rs:1519` via `one_node_graph()` `:761`) has NO retry policy, so it
doesn't prove fail-fast when retry is ON. Add/extend a test with `retry: Some(RetryPolicy{max_attempts:3,..})` + a
backend whose `configure_session` returns `ConfigInvalid` → assert exactly ONE configure attempt, ZERO prompts, ZERO
invalidates (non-transient ⇒ no retry).

### PR-FIX-9 (MAJOR-9, T3) — test the GRAPH MAPPING, not just TOML deser
T3's deser-only test doesn't prove `WorkflowNodeToml.retry` reaches `WorkflowNode.retry`. The mapping is reachable via
`RegistryConfig::load_workflows` (the `WorkflowNode {` build at `config.rs:970`). Make a mapping assertion MANDATORY:
a temp prompt file + `load_workflows` → `WorkflowGraph.nodes[0].retry == Some(RetryPolicy{..})`.

### PR-FIX-10 (MAJOR-10, T5) — test last-attempt usage (SR-FIX-4)
Add a retry test where a FAILED attempt emits `Update::Usage(A)` and the SUCCESSFUL final attempt emits
`Update::Usage(B)` → assert the node's reported usage `== B` (last attempt), NOT `A+B`.

### Revised task structure (net of the folds)
T1 `is_transient` (unchanged) → **T2 `RetryPolicy` + `WorkflowNode.retry` + `retry: None` at ALL ~32 workspace
literals (PR-FIX-1) + overflow-safe `backoff_for`** → **T3 config `retry` + the load_workflows graph-mapping test
(PR-FIX-9)** → **T4 `AgentRegistry::invalidate`: shared write-lock with `apply` (PR-FIX-2) + detached lease-drained
retirement of the old slot (PR-FIX-3) + the `Slot::new`/`State` type fix (PR-FIX-4) + the resolve/invalidate-counting
test (toward PR-FIX-6)** → **T5 the retry loop: `Attempt` enum with `Option<backend>` (PR-FIX-5), reset =
release(if backend)+invalidate+backoff+re-resolve, cancel-abortable overflow-safe backoff, last-attempt usage; tests:
fail-then-ok proving resolve-in-loop+invalidate counts (PR-FIX-6), exhaust, non-transient, no-policy,
cancel-mid-backoff, retry-enabled-ConfigInvalid-fail-fast (PR-FIX-8), last-attempt-usage (PR-FIX-10)** → **T6
resume-compat via a DROPPED runner future (PR-FIX-7) + workspace gate**. After folding PR-FIX-1..10 the plan is
**ready-to-implement**.
