# Slice 9 — Turn Channel + E2 permission — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (or
> executing-plans) to implement task-by-task. Steps use checkbox (`- [ ]`) syntax. The proven loop:
> codex-HIGH implements (no commit) / Opus verifies in the clean host env + commits / codex-xhigh reviews /
> live-gate vs real codex.

**Goal:** Give the orchestrator a bidirectional Turn Channel: (A) queued-inject (content drained into the
context's NEXT turn) and (B) E2 pending-permission (a real agent permission surfaces as an event; the
orchestrator decides Approve/Deny/Modify/Escalate under a bounded timeout; cancel resolves a pending
permission), with ACP handlers nonblocking and the auto-policy path byte-identical.

**Architecture:** Inject lives in `SessionManager` (mirrors `pending_seed`). Permission keeps the existing
`cx.spawn` offload in `AcpBackend`; on an opt-in `Defer` policy it publishes a `PermissionRequest` event,
registers a gen-keyed pending oneshot in a bridge-owned `PermissionRegistry`, awaits it with a bounded timeout
(default Deny), and maps the resolved decision onto an ACP option select.

**Tech stack:** Rust. `bridge-core` (domain/ports/orch), `bridge-coordinator` (SessionManager, Coordinator,
registry, params), `bridge-acp` (permission path), `bridge-a2a-inbound` (wire), `bin/a2a-bridge` (CLI).

**BINDING:** spec `docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md` — the **`## v2`** section
(SPIKE-1 RESOLVED + SF-1..9 + D1-D5) supersedes the draft. SPIKE-1 confirmed E2 is reachable
(codex `approval_policy="untrusted"` + `sandbox_mode="read-only"` + a sandbox-blocked write).

---

## File structure
- `crates/bridge-core/src/domain.rs` — extend `PermissionDecision`; add `InjectMode`/`QueuedInject`/`InjectRequest`.
- `crates/bridge-core/src/ports.rs` — `PolicyOutcome` + the defaulted `PolicyEngine::interactive_decide`.
- `crates/bridge-core/src/orch.rs` — `OrchEventKind::PermissionRequest` variant + `PermissionOptionView`.
- `crates/bridge-coordinator/src/session_manager.rs` — `pending_injects` + `inject()` + drains + new-gen clears.
- `crates/bridge-coordinator/src/dispatch.rs` — `LocalDispatch.injects`.
- `crates/bridge-coordinator/src/turn_parts.rs` (NEW) — the shared `assemble_turn_parts` helper.
- `crates/bridge-coordinator/src/permission_registry.rs` (NEW) — gen-keyed exact-once registry.
- `crates/bridge-coordinator/src/coordinator.rs` — `inject()` + `permit()` ops; wire `resolve_context`.
- `crates/bridge-coordinator/src/detached.rs` — skip the new event in `frame_from_orch`/flush (no panic).
- `crates/bridge-coordinator/src/params.rs` — `InjectParams` + `PermitParams`.
- `crates/bridge-acp/src/acp_backend.rs` — route carries `{ctx,gen,op}`; interactive permission path.
- `crates/bridge-a2a-inbound/src/server.rs` — `SessionInject`/`SessionPermit` methods; inject in both producers.
- `bin/a2a-bridge/src/main.rs` — `session inject` / `session permit` CLI.

---

### Task 1: Domain + ports + orch types (bridge-core, additive, dead-safe)

**Files:**
- Modify: `crates/bridge-core/src/domain.rs`
- Modify: `crates/bridge-core/src/ports.rs:151`
- Modify: `crates/bridge-core/src/orch.rs:62`
- Test: inline `#[cfg(test)]` in each.

- [ ] **Step 1: Write failing tests** — serde round-trip + the dead-safe default.

```rust
// domain.rs tests
#[test]
fn permission_decision_variants_round_trip() {
    for d in [
        PermissionDecision::Approve { option_id: None },
        PermissionDecision::Deny { option_id: None, reason: Some("nope".into()) },
        PermissionDecision::Modify { option_id: "approved-execpolicy-amendment".into(), note: None },
        PermissionDecision::Escalate { reason: None },
    ] {
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<PermissionDecision>(&s).unwrap(), d);
    }
}
#[test]
fn inject_request_defaults_prepend() {
    let r = InjectRequest { context: ContextId::parse("c").unwrap(), text: "hi".into(),
        mode: InjectMode::PrependNextTurn, dedupe_key: None };
    assert_eq!(r.mode, InjectMode::PrependNextTurn);
}
// ports.rs test — the 14 existing impls inherit "never Defer"
#[test]
fn default_policy_engine_never_defers() {
    struct OldStyle;
    impl PolicyEngine for OldStyle {
        fn decide(&self, _: &PermissionRequest, _: &SessionContext)
            -> Result<PermissionDecision, BridgeError> { Ok(PermissionDecision::Approve { option_id: None }) }
    }
    let out = OldStyle.interactive_decide(&PermissionRequest::read(), &SessionContext);
    assert!(matches!(out, PolicyOutcome::Decide(Ok(PermissionDecision::Approve { .. }))));
}
```

- [ ] **Step 2: Run — expect FAIL** (types don't exist). `cargo test -p bridge-core`.

- [ ] **Step 3: Implement.**

```rust
// domain.rs — REPLACE the Approve-only enum
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    Approve { #[serde(default, skip_serializing_if = "Option::is_none")] option_id: Option<String> },
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")] option_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<String>,
    },
    Modify { option_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] note: Option<String> },
    Escalate { #[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectMode { PrependNextTurn, AppendNextTurn }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInject { pub text: String, pub mode: InjectMode, pub dedupe_key: Option<String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectRequest { pub context: ContextId, pub text: String, pub mode: InjectMode,
    pub dedupe_key: Option<String> }
```
NOTE: grep every `PermissionDecision::Approve` match site (acp_backend.rs:1271 etc.) and update to
`Approve { .. }`; add `Deny { .. }`/`Modify { .. }`/`Escalate { .. }` arms where the verdict is mapped.

```rust
// ports.rs — additive, DEAD-SAFE (no change to the 14 decide() impls)
pub enum PolicyOutcome { Decide(Result<PermissionDecision, BridgeError>), Defer }
pub trait PolicyEngine: Send + Sync {
    fn decide(&self, req: &PermissionRequest, ctx: &SessionContext)
        -> Result<PermissionDecision, BridgeError>;
    /// Interactive decision (Slice 9). DEFAULT = never `Defer` → byte-identical to the auto path.
    /// An opt-in interactive policy overrides this to return `Defer`.
    fn interactive_decide(&self, req: &PermissionRequest, ctx: &SessionContext) -> PolicyOutcome {
        PolicyOutcome::Decide(self.decide(req, ctx))
    }
}
```

```rust
// orch.rs — additive struct variant on OrchEventKind
PermissionRequest {
    request_id: String,
    tool_call_id: String,
    generation: u64,
    op: String,
    title: String,
    options: Vec<PermissionOptionView>,
    #[serde(skip_serializing_if = "Option::is_none")] raw_input: Option<String>,
    timeout_ms: u64,
},
// + new struct (cap title/raw_input like slice-7a, e.g. 4 KB)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionOptionView { pub option_id: String, pub name: String, pub kind: String }
```

- [ ] **Step 4: Run — expect PASS.** `cargo test -p bridge-core`; `cargo build --workspace` (the match-site sweep compiles).
- [ ] **Step 5: Commit** `feat(slice-9): domain+ports+orch types (PermissionDecision/PolicyOutcome/Inject/event)`.

---

### Task 2: Queued-inject in SessionManager + the shared parts helper

**Files:**
- Create: `crates/bridge-coordinator/src/turn_parts.rs`
- Modify: `crates/bridge-coordinator/src/session_manager.rs` (WarmHandle, WarmTurn, inject(), 3 drains, new-gen clears)
- Modify: `crates/bridge-coordinator/src/dispatch.rs:71` (LocalDispatch.injects)
- Modify: `crates/bridge-coordinator/src/coordinator.rs:215` (collect_turn uses the helper) + `inject()` op
- Modify: `crates/bridge-a2a-inbound/src/server.rs:1376,2311` (both producers use the helper)

- [ ] **Step 1: Failing tests** (session_manager.rs):
```rust
#[tokio::test]
async fn inject_queues_and_drains_once_fifo() {
    let (m, _b, _r) = manager();
    let c = ctx("inj");
    m.checkout_turn(&c, agent(), None, None).await.unwrap(); // Running
    m.finish_turn_by_ctx(&c).await; // helper: drain gen+op → Idle  (or finish_turn with stored gen/op)
    m.inject(InjectRequest { context: c.clone(), text: "A".into(), mode: InjectMode::PrependNextTurn, dedupe_key: None }).await.unwrap();
    m.inject(InjectRequest { context: c.clone(), text: "B".into(), mode: InjectMode::AppendNextTurn, dedupe_key: None }).await.unwrap();
    let t = m.checkout_existing_turn(&c).await.unwrap();
    assert_eq!(t.injects.len(), 2);
    m.finish_turn(&c, t.generation, &t.op).await;
    let t2 = m.checkout_existing_turn(&c).await.unwrap();
    assert!(t2.injects.is_empty(), "injects drain once");
}
#[tokio::test]
async fn inject_dedupe_replaces_in_place() {
    let (m, _b, _r) = manager(); let c = ctx("inj2");
    m.checkout_turn(&c, agent(), None, None).await.unwrap();
    m.inject(InjectRequest{context:c.clone(),text:"v1".into(),mode:InjectMode::PrependNextTurn,dedupe_key:Some("k".into())}).await.unwrap();
    m.inject(InjectRequest{context:c.clone(),text:"v2".into(),mode:InjectMode::PrependNextTurn,dedupe_key:Some("k".into())}).await.unwrap();
    assert_eq!(m.pending_inject_count(&c).await, 1);
}
#[tokio::test]
async fn clear_drops_injects() { /* inject → reset_session(force) → checkout → injects empty */ }
```
And `turn_parts.rs`:
```rust
#[test]
fn assemble_orders_seed_prepend_input_append() {
    let injects = vec![
        QueuedInject{text:"P".into(),mode:InjectMode::PrependNextTurn,dedupe_key:None},
        QueuedInject{text:"A".into(),mode:InjectMode::AppendNextTurn,dedupe_key:None}];
    let parts = assemble_turn_parts(Some("S"), &injects, "U");
    let texts: Vec<&str> = parts.iter().map(|p| p.text.as_str()).collect();
    // seed, prepend, input, append (each wrapped) — assert relative order of S, P, U, A
    assert!(idx(&texts,"S") < idx(&texts,"P") && idx(&texts,"P") < idx(&texts,"U") && idx(&texts,"U") < idx(&texts,"A"));
}
```

- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Implement.**
  - `turn_parts.rs`: `pub fn assemble_turn_parts(seed: Option<&str>, injects: &[QueuedInject], input: &str) -> Vec<Part>` — push seed (wrapped `"[Summary of earlier context in this session]\n{seed}"` to match coordinator.rs:217), then Prepend injects (FIFO, wrapped `"[Injected context]\n{text}"`), then `Part{text: input}`, then Append injects. (Pure, total, unit-tested.)
  - `WarmHandle`: add `pending_injects: Vec<QueuedInject>` (init `Vec::new()` at the fresh-mint insert + everywhere a handle is built). `WarmTurn`: add `pub injects: Vec<QueuedInject>`.
  - `inject(&self, req)`: lock `by_context`; `SessionNotFound` if absent; if `dedupe_key` matches an existing entry, REPLACE in place (preserve position) else push; cap at 32 entries / 64 KB total (`HandleBusy`-style `BridgeError` beyond); return depth. Allowed in Idle OR Running (D5).
  - Drain at the 3 checkout sites (`:306,:353,:447`): `let injects = std::mem::take(&mut h.pending_injects);` ALONGSIDE `pending_seed.take()`; put `injects` in the returned `WarmTurn`.
  - New-gen tails: `reset_session_inner` (:964) + `compact_session` — for clear (reset force/non-force) `h.pending_injects.clear()` (D3 drop). For **compact: REJECT if `!h.pending_injects.is_empty()`** at the claim (mirror the `pending_seed.is_some()` reject at :1010) → `HandleBusy`.
  - `LocalDispatch.injects: Vec<QueuedInject>` (`dispatch.rs`); populate from `turn.injects` in `warm_local_dispatch` (`server.rs:512`); cold-bind = `Vec::new()`.
  - `Coordinator::collect_turn` (:215): replace the inline seed-prepend with `assemble_turn_parts(turn.seed.as_deref(), &turn.injects, &input)`.
  - BOTH A2A producers (`spawn_local_producer` parts assembly + the unary Local parts at `:2311`): replace their inline seed handling with `assemble_turn_parts(dispatch.seed.as_deref(), &dispatch.injects, &input)`.
  - `Coordinator::inject(p: InjectParams)` → `session_manager.inject(...)`.

- [ ] **Step 4: Run — PASS** (`-p bridge-coordinator -p bridge-a2a-inbound`).
- [ ] **Step 5: Commit** `feat(slice-9): queued-inject (SessionManager + shared assemble_turn_parts in all 3 producers)`.

---

### Task 3: PermissionRegistry (gen-keyed, exact-once, drop-guard)

**Files:** Create `crates/bridge-coordinator/src/permission_registry.rs`; wire into Coordinator + cancel/release/clear/reset.

- [ ] **Step 1: Failing tests.**
```rust
#[tokio::test]
async fn register_resolve_exactly_once() {
    let reg = PermissionRegistry::new();
    let key = PermKey { ctx: ctx("c"), generation: 1, op: op("turn-1"), request_id: "r".into() };
    let rx = reg.register(key.clone());
    assert!(reg.resolve(&key, PermissionResolution::Decided(approve())));
    assert!(!reg.resolve(&key, PermissionResolution::Decided(approve())), "second resolve no-ops");
    assert!(matches!(rx.await.unwrap(), PermissionResolution::Decided(_)));
}
#[tokio::test]
async fn resolve_context_cancels_all_for_ctx() {
    let reg = PermissionRegistry::new();
    let k1 = key("c",1,"turn-1","r1"); let k2 = key("c",1,"turn-1","r2");
    let (rx1, rx2) = (reg.register(k1), reg.register(k2));
    let n = reg.resolve_context(&ctx("c"), PermissionResolution::Cancelled);
    assert_eq!(n, 2);
    assert!(matches!(rx1.await.unwrap(), PermissionResolution::Cancelled));
    assert!(matches!(rx2.await.unwrap(), PermissionResolution::Cancelled));
}
#[tokio::test]
async fn stale_generation_permit_rejected() {
    let reg = PermissionRegistry::new();
    let live = key("c",2,"turn-2","r"); let _rx = reg.register(live);
    assert!(!reg.resolve(&key("c",1,"turn-1","r"), PermissionResolution::Decided(approve())));
}
#[tokio::test]
async fn drop_guard_reaps_on_handler_exit() { /* register, drop the guard, entry gone */ }
```

- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Implement.**
```rust
pub enum PermissionResolution { Decided(PermissionDecision), Cancelled }
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PermKey { pub ctx: ContextId, pub generation: u64, pub op: OperationId, pub request_id: String }
pub struct PermissionRegistry { inner: Mutex<HashMap<PermKey, oneshot::Sender<PermissionResolution>>> }
impl PermissionRegistry {
    pub fn new() -> Arc<Self> { Arc::new(Self { inner: Mutex::new(HashMap::new()) }) }
    pub fn register(&self, key: PermKey) -> oneshot::Receiver<PermissionResolution> { /* insert sender, return rx; returns a PermitGuard too — see below */ }
    pub fn resolve(&self, key: &PermKey, res: PermissionResolution) -> bool { /* take(key) under lock, send; true iff found+sent */ }
    pub fn resolve_context(&self, ctx: &ContextId, res: /*Clone*/ ) -> usize { /* drain all entries with key.ctx==ctx, send each; return count */ }
    pub fn reap(&self, key: &PermKey) { /* remove without sending — drop-guard path */ }
}
// PermitGuard { reg, key }: on Drop, reg.reap(&key) (no-op if already resolved). The handler holds it across
// the await so EVERY exit (decision/timeout/cancel/responder-fail/task-drop) reaps.
```
(`resolve_context` needs a Clone-able resolution; `Cancelled` is Copy-like; for `Decided` it is only used by
`resolve`. Make `resolve_context` take `PermissionResolution` and only `Cancelled` is ever passed — assert/doc.)
  - Coordinator OWNS `Arc<PermissionRegistry>`; `Coordinator::permit(p: PermitParams)` → build `PermKey` from the echoed ctx/gen/op/request_id → `registry.resolve(&key, Decided(decision))` → `bool`.
  - Wire `resolve_context(ctx, Cancelled)` into `SessionManager::cancel` / `reset_session(force)` / `clear_with_children` / `release` — SF-1: call it DIRECTLY/synchronously where the handle is held (the registry is shared into SessionManager OR the Coordinator wraps these and calls it after the SM op). PREFER: the Coordinator's `clear`/`cancel_*` wrappers call `registry.resolve_context` immediately after invoking the SM op (SM stays registry-free; the Coordinator owns the registry). Verify the A2A `SessionCancel`/`SessionClear` handlers route through a Coordinator path that resolves — if they call SM directly (`server.rs:2952`), thread the registry there too.

- [ ] **Step 4: Run — PASS.** **Step 5: Commit** `feat(slice-9): gen-keyed PermissionRegistry (exact-once + drop-guard + resolve_context)`.

---

### Task 4: Route carries {ctx, gen, op} (acp plumbing, SF-3)

**Files:** `crates/bridge-acp/src/acp_backend.rs` (route entry struct ~:1986 + its registration at turn start).

- [ ] **Step 1: Failing test** — a unit test that the route map entry for a checked-out turn exposes ctx/gen/op
  (construct a backend, register a route with a bridge context, assert the reverse-handler lookup sees it).
- [ ] **Step 2: FAIL.**
- [ ] **Step 3: Implement.** Extend the route entry (today `{tx, watch}`) with `bridge_ctx: Option<ContextId>`,
  `generation: u64`, `op: OperationId` (or a `TurnRouteMeta`); set them where the producer registers its route
  for the turn (the `prompt`/`prompt_observed` path that inserts into the `updates` map keyed by
  `AgentSessionId`). The reverse permission handler reads them via `map.get(&req.session_id)`. This is the seam
  that lets the handler build a gen-stamped `PermissionRequest` + `PermKey` WITHOUT parsing a formatted id.
  NOTE: the bridge `ContextId`↔ACP `AgentSessionId` mapping must be available at route-registration time —
  confirm the producer knows the bridge `ContextId`/gen/op when it calls prompt (it does: the WarmTurn carries
  `generation`+`op`; thread the `ContextId` too).
- [ ] **Step 4: PASS.** **Step 5: Commit** `feat(slice-9): thread bridge {ctx,gen,op} onto the acp turn route`.

---

### Task 5: Interactive permission path (acp, SF-7 dead-safe + SPIKE-1 shape)

**Files:** `crates/bridge-acp/src/acp_backend.rs` (the `on_receive_request` handler ~:1051 + `decide_permission` ~:1227).

- [ ] **Step 1: Failing tests.**
  - `auto_path_byte_identical`: a default policy (never Defer) → the handler responds via `decide_permission`
    with NO event published, NO registry entry (assert the sink got 0 PermissionRequest events).
  - `defer_publishes_event_and_awaits_then_maps`: a `Defer` policy → assert (a) a `PermissionRequest` event is
    published to the sink carrying the real options (mirror the SPIKE-1 shape: `approved`/AllowOnce etc.); (b)
    resolving the registry with `Approve` selects the AllowOnce option in the ACP response; with
    `Modify{option_id:"approved-execpolicy-amendment"}` selects THAT option; with `Deny` selects RejectOnce.
  - `timeout_defaults_deny`: a `Defer` policy + no resolution within the (short, test-injected) timeout →
    responds with a reject option (Deny).
- [ ] **Step 2: FAIL.**
- [ ] **Step 3: Implement.** In the `cx.spawn` task:
  ```
  match policy.interactive_decide(&perm_req, &SessionContext) {
      PolicyOutcome::Decide(verdict) => responder.respond(map_verdict_to_outcome(verdict, &req.options)),  // today's path
      PolicyOutcome::Defer => {
          let route = lookup(req.session_id);            // {ctx,gen,op} from Task 4
          let key = PermKey { ctx, generation, op, request_id };
          sink.record(OrchEventKind::PermissionRequest { ...SPIKE-1 fields..., options: views(&req.options) });
          let _guard = registry.register_guard(key.clone());   // returns (rx, guard)
          let res = tokio::select! {
              biased;
              r = rx => r.unwrap_or(PermissionResolution::Cancelled),
              _ = sleep(Duration::from_millis(timeout_ms)) => PermissionResolution::Decided(deny()),
          };
          let outcome = match res {
              PermissionResolution::Decided(d) => map_decision_to_outcome(d, &req.options),
              PermissionResolution::Cancelled => RequestPermissionOutcome::Cancelled,
          };
          let _ = responder.respond(RequestPermissionResponse::new(outcome));
      }
  }
  ```
  `map_decision_to_outcome` extends today's `select(&[kinds])` (`:1264`): Approve→[AllowOnce,AllowAlways];
  Deny→[RejectOnce,RejectAlways]; Modify{option_id}→that exact `option_id` (verify it's in `req.options`, else
  Cancelled); Escalate→(unreached in-slice). The registry/sink/policy are threaded into the handler at backend
  construction (like `with_policy`); the `timeout_ms` is configurable (default e.g. 30_000), test-injectable.
  **Dead-safe:** the `Decide` arm is byte-identical to today (responds, no event/register).
- [ ] **Step 4: PASS** (`-p bridge-acp`). **Step 5: Commit** `feat(slice-9): interactive permission path (Defer→event+oneshot+timeout; auto byte-identical)`.

---

### Task 6: Detached sink skips the new event (SF-6, no panic)

**Files:** `crates/bridge-coordinator/src/detached.rs:398` (`frame_from_orch`) + flush (:88/:131).

- [ ] **Step 1: Failing test** — record a `PermissionRequest` event into a `DetachedRichSink`, call `flush()`,
  assert NO panic and no frame emitted for it (journal-only).
- [ ] **Step 2: FAIL** (today panics). **Step 3:** add an arm in `frame_from_orch` (or its caller) that returns
  `None`/skips for `OrchEventKind::PermissionRequest` (and defensively any non-frameable kind). **Step 4: PASS.**
- [ ] **Step 5: Commit** `fix(slice-9): detached sink skips PermissionRequest event (journal-only, no panic)`.

---

### Task 7: Wire + CLI + MCP (SessionInject / SessionPermit) + params

**Files:** `params.rs` (InjectParams/PermitParams); `server.rs:698` (2 dispatch arms + handlers); `main.rs` (CLI); MCP tools (bridge-mcp).

- [ ] **Step 1: Failing tests** — `server.rs` tests: `SessionInject {contextId,text,mode?,dedupeKey?}` → queues
  (status/next-turn reflects it); `SessionPermit {requestId, decision{...}, contextId, generation, op}` → resolves
  (returns `{resolved:bool}`); unknown requestId → `{resolved:false}`. CLI flag-parse tests for `session inject`/
  `session permit`.
- [ ] **Step 2: FAIL.**
- [ ] **Step 3: Implement.**
  - `InjectParams { context, text, mode, dedupe_key }` + `PermitParams { context, generation, op, request_id,
    decision: PermissionDecision }` in `params.rs`, each with `from_a2a_metadata`/`from_cli`/`from_mcp`.
  - `server.rs`: add `"SessionInject" => session_inject(...)` + `"SessionPermit" => session_permit(...)` arms;
    handlers call `coordinator.inject(...)` / `coordinator.permit(...)`.
  - CLI `main.rs`: `session inject <ctx> --input <f> [--append] [--dedupe <k>]`; `session permit <requestId>
    --context <c> --generation <n> --op <o> (--approve [--option <id>] | --deny [--reason ..] | --modify <id> |
    --escalate)`. (The orchestrator gets ctx/gen/op/requestId from the `PermissionRequest` event.)
  - MCP tools `inject` + `permit` in bridge-mcp (mirror the existing tool dispatch → `OpParams`-style).
- [ ] **Step 4: PASS** (`-p bridge-a2a-inbound -p a2a-bridge -p bridge-mcp`). **Step 5: Commit** `feat(slice-9): SessionInject/SessionPermit wire+CLI+MCP`.

---

### Task 8: DoD — dead-safe byte-identity + full gate + live-gate

**Files:** a dead-safe test; the live-gate config + driver.

- [ ] **Step 1:** `dead_safe_auto_policy_unchanged` — a full turn through an auto-policy backend emits ZERO
  PermissionRequest events and behaves identically (the existing bridge-a2a-inbound/coordinator/acp suites stay
  green: assert the pre-slice counts hold).
- [ ] **Step 2: Full gate** — `cargo test -p bridge-core -p bridge-coordinator -p bridge-acp -p
  bridge-a2a-inbound -p bridge-mcp` + `--workspace --no-run` + `clippy --workspace --all-targets` + `fmt --all
  --check` clean.
- [ ] **Step 3: Live-gate** (`examples/a2a-bridge.slice-9-livegate.toml` = codex `approval_policy="untrusted"`
  `sandbox_mode="read-only"` + a `Defer` policy wired in; port 8125). Driver (CLI):
  - **Inject:** `session inject <ctx> --input codeword.txt` → a follow-up `submit` recalls the injected codeword.
  - **Permission:** `submit` a write-prompt → a `PermissionRequest` event appears (`task watch`); route
    `session permit <reqid> --deny` → the agent's write is rejected; re-run + `--approve` → it proceeds.
  - **Cancel mid-permission:** `submit` a write-prompt (pending permission) → `session cancel <ctx>` → the turn
    ends promptly (Canceled), no hang.
- [ ] **Step 4: Commit** `test(slice-9): DoD — dead-safe byte-identity + live-gate (inject/permit/cancel vs codex)`.

---

## Self-review (done)
- **Spec coverage:** SF-1 (T3/T5 cancel-resolves-immediate + select on rx not turn_kill) · SF-2 (T3 gen-keyed
  PermKey) · SF-3 (T4 route plumbing) · SF-4 (T3 exact-once + drop-guard) · SF-5 (T2 helper in all 3 producers)
  · SF-6 (T6 skip) · SF-7 (T1 defaulted method + T5 Decide-byte-identical) · SF-8 (residual documented, no task)
  · SF-9 (T2 bounds + clear-drop/compact-reject). D1-D5 all realized. ✅
- **Type consistency:** `PermissionDecision`/`PolicyOutcome`/`PermissionResolution`/`PermKey`/`QueuedInject`/
  `InjectMode` used identically across tasks. `assemble_turn_parts` signature stable.
- **Risk:** Task 4 (route plumbing) is the least-pinned — the bridge `ContextId` must be known at ACP
  route-registration time; if the producer does NOT currently carry it to the prompt call, Task 4 grows a small
  threading change. Flag for plan review.

## Open items for plan review
- P1: does the A2A `SessionCancel`/`SessionClear` path go through a Coordinator wrapper that can call
  `resolve_context`, or does it call `SessionManager` directly (`server.rs:2952`)? If direct, the registry must
  be threaded to that handler (or those ops move behind the Coordinator). Confirm the cleanest wiring.
- P2: is the ACP `ContextId` reliably available at route registration (Task 4)? If not, what threads it?
- P3: timeout default value + whether it's per-config; Escalate's in-slice no-op must not consume the sender.
- P4: MCP `inject`/`permit` — confirm bridge-mcp's tool-dispatch seam matches.
