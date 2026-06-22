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

---

## v2 — Plan revision (post codex-xhigh plan-review + Opus verification) — BINDING

> This section SUPERSEDES the draft tasks where it conflicts. It folds the codex-xhigh plan-review (verdict:
> needs-revision; 3 BLOCKERs + 8 MAJORs) AFTER each finding was re-verified against ground-truth `file:line`
> (every anchor confirmed). Tasks below are renumbered v2.T1..v2.T9; where a v2 task only amends a draft task,
> it says so. The draft's TDD step rhythm (failing test → FAIL → impl → PASS → commit, staging ONLY each task's
> files) is unchanged.

### Verified findings (all confirmed against real code)
| # | Sev | Claim | Ground truth | Resolution |
|---|-----|-------|--------------|------------|
| B1 | BLOCKER | cancel-resolve not wired to real A2A paths; Coordinator wrapper too late | `server.rs:2923/2952/3034` call `sm.release_with_children`/`clear_with_children`/`cancel_with_children` DIRECTLY; `cancel_inner` awaits `backend.cancel` at `session_manager.rs:731` | Registry lives in **`SessionManager`** (Arc, shared-in); `resolve_context(ctx, Cancelled)` called SYNCHRONOUSLY at the top of `cancel_inner`/`reset_session_inner`/`release_inner`, BEFORE any backend await. v2.T3+T4. |
| B2 | BLOCKER | `{ctx,gen,op}` not available at ACP route registration | `AgentBackend::prompt` gets only `SessionId`+`Vec<Part>` (`ports.rs:44`); `TurnRoute{tx,watch}` (`acp_backend.rs:218`); registered from `session`/`agent_id` (`:1986`) | New defaulted `AgentBackend::configure_turn(session, TurnMeta)` (no-op default); producers call it before `prompt`; AcpBackend stashes per-session, copies into `TurnRoute`. v2.T5. |
| B3 | BLOCKER | permission event won't reach normal turns (rich dropped w/o sink) | `acp_backend.rs:2182` drops `TurnEvent::Rich` when `sink` is None; `prompt` passes `None` (`:2216`); A2A uses `translator.rs:133` `backend.prompt` | **Visibility via `session/status` enumerating the registry** (pull, sink-independent) — NOT RichEventSink, NOT a new stream variant. Detached-node E2 + push/journal visibility DEFERRED. v2.T6. |
| M4 | MAJOR | registry in bridge-coordinator but bridge-acp uses it | `bridge-acp` deps = only `bridge-core` (`Cargo.toml:23`) | `PermissionRegistry`+`PermKey`+`PermissionResolution`+`PermitDecision`+`TurnMeta` all live in **`bridge-core`**. v2.T1/T3. |
| M5 | MAJOR | event lacks `context_id` | `OrchEvent` has none; `RichEventSink::record` takes only `OrchEventKind` (`ports.rs:32`) | MOOT — no `OrchEventKind::PermissionRequest` in-slice (B3 → session/status). The registry holds full `PendingPermissionView` keyed by `PermKey` (which carries ctx). v2.T3. |
| M6 | MAJOR | `Escalate` must not consume the sender (D4) | decision map at `acp_backend.rs:1264` | `Escalate` = registry `reap`-and-leave-pending NO-OP (never `resolve`); pending entry then times out → default-Deny. v2.T6/T8. |
| M7 | MAJOR | `assemble_turn_parts(seed,injects,input:&str)` doesn't fit producers | both producers build from `routed.parts: Vec<Part>`+`dispatch.seed` (`server.rs:1376/2311`) | Signature → `assemble_turn_parts(seed: Option<&str>, injects: &[QueuedInject], base: Vec<Part>) -> Vec<Part>`. v2.T2. |
| M8 | MAJOR/CRITICAL | `PermissionDecision` reshape breaks ~12 sites + contradicts dead-safe | `PermissionDecision` IS `enum { Approve }` (`domain.rs:283-285`); constructed/matched at coordinator.rs:758, acp:173/1266/3316, api:64/257/283, policy:18/36, translator:353, sse:309, server:3882, ports:157 | **Leave `PermissionDecision{Approve}` UNTOUCHED.** Introduce a DISTINCT operator-decision type **`PermitDecision`** (`Approve{option_id?}`/`Deny{option_id?,reason?}`/`Modify{option_id,note?}`/`Escalate{reason?}`). Zero construction-site churn. v2.T1. |
| M9 | MAJOR | registry API inconsistent (`register` vs `register_guard`) | draft `:267` vs `:325` | One API: `register(key, view) -> (Receiver<PermissionResolution>, PermitGuard)`. v2.T3. |
| M10 | MAJOR | `Defer` live-gate unreachable from config | serve builds `AutoPolicy` at `main.rs:1913/2210/2627`; `AgentEntryToml` has no policy field (`config.rs:174`) | New `v2.T7` adds `permission_policy="defer"` config + a `DeferPolicy` in `bridge-policy`; serve constructs it when set. |
| M11 | MAJOR | detached `frame_from_orch` panics on new variant | `unreachable!` at `detached.rs:131`, called in `flush` (`:405`); `fold_journal_to_snapshot` exhaustive-no-wildcard (`task_store.rs:278`) | MOOT — no new `OrchEventKind` in-slice (B3). `detached.rs`/`task_store.rs` UNTOUCHED. Draft Task 6 DELETED. |
| M12 | MINOR | `finish_turn_by_ctx` test helper doesn't exist | only in plan (`grep`); `manager()` real at `session_manager.rs:1511` | Tests store the `WarmTurn` and call `finish_turn(&ctx, turn.generation, &turn.op)`. v2.T2. |

### v2 architecture (the 5 reconciled decisions)
1. **Shared core registry.** `PermissionRegistry` (+ `PermKey`, `PermissionResolution`, `PermitDecision`,
   `TurnMeta`, `PendingPermissionView`) live in **`bridge-core`**. Constructed ONCE in serve composition;
   `Arc<PermissionRegistry>` injected into BOTH `AcpBackend` (`with_permission_registry`) AND `SessionManager`
   (`with_permission_registry`). No dep-direction problem (both depend on `bridge-core`).
2. **Synchronous cancel-resolve inside SessionManager** (B1). `cancel_inner`/`reset_session_inner`/`release_inner`
   call `perm_registry.resolve_context(ctx, Cancelled)` at the TOP, before any `backend.cancel().await`. The
   A2A handlers (`server.rs:2923/2952/3034`) need NO change — they already funnel through these SM methods.
   `turn_kill`/abort-token stays a backstop only.
3. **`PermitDecision` is distinct from `PermissionDecision`** (M8). The PolicyEngine `decide()` path is unchanged
   (returns `PermissionDecision{Approve}`). `PermitDecision` is produced ONLY by the operator (`SessionPermit`)
   and consumed ONLY by the ACP handler's outcome-mapping. `PolicyOutcome::Decide` carries the EXISTING
   `Result<PermissionDecision, BridgeError>`; `Defer` carries nothing. Defaulted `interactive_decide` returns
   `Decide(self.decide(req,ctx))` → 14 impls unchanged, dead-safe by construction.
4. **Visibility via `session/status`** (B3, M5, M11). The registry stores `PendingPermissionView{request_id,
   generation, op, title, options, timeout_ms}` alongside each sender. `session/status` enumerates pending
   permissions for the ctx (pull, sink-independent). NO new `OrchEventKind`, NO stream/journal change in-slice.
   Push/SSE + detached-node interactive permission are DEFERRED (tracked, not built).
5. **TurnMeta threading** (B2). `TurnMeta{context_id, generation, op}` (bridge-core). Producers call a defaulted
   `AgentBackend::configure_turn(session, meta)` immediately before `prompt`/`prompt_observed`; AcpBackend
   stashes it per-session (the turn lock guarantees one-at-a-time) and copies it into `TurnRoute`; the reverse
   permission handler reads `route.turn_meta` via `map.get(&req.session_id)` to build the gen-stamped `PermKey`.

### v2 task list (renumbered)
- **v2.T1 — Domain/ports/orch (AMENDS draft T1).** In `bridge-core/src/domain.rs`: ADD `PermitDecision`
  (`#[serde(tag="decision", rename_all="snake_case")]`, the 4 variants from draft `:95-104` but RENAMED type)
  + `InjectMode`/`QueuedInject`/`InjectRequest` (unchanged). **DO NOT touch `PermissionDecision{Approve}`.** In
  `ports.rs`: `enum PolicyOutcome { Decide(Result<PermissionDecision, BridgeError>), Defer }` + defaulted
  `interactive_decide(&self, req, ctx) -> PolicyOutcome { PolicyOutcome::Decide(self.decide(req,ctx)) }`. In
  `orch.rs`: **NOTHING** (no new event variant). Add `TurnMeta`, `PermKey`, `PermissionResolution{Decided(PermitDecision),
  Cancelled}`, `PendingPermissionView`, `PermissionRegistry`, `PermitGuard` to bridge-core (a new
  `bridge-core/src/permission.rs` module, `pub mod permission;`). Tests: `permit_decision_variants_round_trip`;
  `default_policy_engine_never_defers` (asserts `matches!(out, PolicyOutcome::Decide(Ok(PermissionDecision::Approve)))`).
- **v2.T2 — Queued-inject + shared parts helper (AMENDS draft T2).** Helper signature →
  `assemble_turn_parts(seed: Option<&str>, injects: &[QueuedInject], base: Vec<Part>) -> Vec<Part>` (prepend
  seed-part + Prepend injects ONTO `base`, then the base parts, then Append injects — preserve `base` order).
  All 3 sites pass their existing `Vec<Part>` as `base`: `collect_turn` (the assembled input parts),
  `spawn_local_producer:1376` (`routed.parts`), unary `:2311` (`routed.parts`). Tests use the real `manager()`
  (`session_manager.rs:1511`) and `finish_turn(&ctx, turn.generation, &turn.op)` (NOT `finish_turn_by_ctx`).
  `turn_parts.rs` lives in bridge-coordinator (uses only `bridge_core::domain::Part`).
- **v2.T3 — PermissionRegistry in bridge-core (AMENDS draft T3).** `register(key, view) -> (oneshot::Receiver
  <PermissionResolution>, PermitGuard)`; `resolve(&key, PermissionResolution) -> bool` (atomic take-under-lock);
  `resolve_context(&ctx, PermissionResolution) -> usize` (drain+send all entries with `key.context_id==ctx`;
  only `Cancelled` is ever passed — assert); `reap(&key)` (remove w/o send — drop-guard + Escalate); `pending(&ctx)
  -> Vec<PendingPermissionView>` (for session/status). Entry = `{sender, view}`. `PermitGuard{reg,key}` reaps on
  Drop. Tests: exact-once, resolve_context-cancels-all, stale-generation-no-op, drop-guard-reaps, pending-lists.
- **v2.T4 — Wire resolve into SessionManager (NEW; realizes B1/SF-1).** `SessionManager::with_permission_registry
  (Arc<PermissionRegistry>)`; store `Option<Arc<PermissionRegistry>>`. At the TOP of `cancel_inner`,
  `reset_session_inner`, `release_inner`: `if let Some(r)=&self.perm_registry { r.resolve_context(ctx,
  PermissionResolution::Cancelled); }` BEFORE any backend await. Tests: a registered pending perm for ctx is
  resolved `Cancelled` synchronously when cancel/clear/release runs (assert the rx resolves before a paused
  fake-backend cancel returns).
- **v2.T5 — TurnMeta onto the ACP route (NEW; realizes B2/SF-3).** `AgentBackend::configure_turn(&self, session:
  &SessionId, meta: TurnMeta)` defaulted no-op (`ports.rs`). AcpBackend stashes `meta` in the session entry;
  `prompt_inner` moves it into `TurnRoute{tx, watch, turn_meta: Option<TurnMeta>}`. Producers (warm dispatch +
  Translator callers) call `configure_turn` before `prompt`. Confirm `LocalDispatch`/`WarmTurn` carry
  `generation`+`op`+`ContextId` to populate `TurnMeta` (add fields to `LocalDispatch` if absent). Test: the
  reverse-handler lookup `map.get(&agent_session_id)` exposes the `{ctx,gen,op}`.
- **v2.T6 — Interactive permission path (AMENDS draft T5; DELETES draft T6).** In the `cx.spawn` handler task:
  `match policy.interactive_decide(&perm_req, &SessionContext)`: `Decide(verdict)` → today's byte-identical
  `decide_permission` mapping (NO registry, NO view). `Defer` → read `route.turn_meta` → build `PermKey` →
  `let (rx, _guard) = registry.register(key, view)` → `biased; select!{ r=rx => …, _=sleep(timeout_ms) =>
  Decided(Deny) }` → map `PermitDecision`→`RequestPermissionOutcome` (`Approve`→[AllowOnce,AllowAlways];
  `Deny`→[RejectOnce,RejectAlways]; `Modify{option_id}`→that exact id if in `req.options` else Cancelled;
  `Escalate`→leave pending, do NOT resolve, let it time-out) → `responder.respond`. `Cancelled`→`Cancelled`
  outcome. `detached.rs`/`task_store.rs`/`orch.rs` UNTOUCHED (no new event). Tests: auto-byte-identical (no
  registry entry), defer-publishes-view-to-registry + maps each PermitDecision, timeout-default-deny,
  escalate-leaves-pending-then-times-out.
- **v2.T7 — Config Defer policy (NEW; realizes M10).** `DeferPolicy` in `bridge-policy/src/permission.rs`
  (`decide`→`Ok(PermissionDecision::Approve)` fallback; `interactive_decide`→`PolicyOutcome::Defer`). Config:
  `permission_policy: Option<String>` (top-level `[server]` or registry section); `into_snapshot` maps
  `"defer"`→DeferPolicy else AutoPolicy. Serve composition (`main.rs:1913/2210/2627`) selects the policy. Test:
  config parse → DeferPolicy `interactive_decide` returns `Defer`.
- **v2.T8 — session/status surfaces pending permissions (NEW; realizes B3 visibility).** `Coordinator::
  pending_permissions(&ctx) -> Vec<PendingPermissionView>` (→ `registry.pending`); `session/status` handler
  includes a `pendingPermissions` block. Test: a registered pending perm appears in status; resolved/cancelled
  ones don't.
- **v2.T9 — Wire + CLI + MCP (AMENDS draft T7).** `InjectParams`/`PermitParams{context,generation,op,request_id,
  decision: PermitDecision}` in `params.rs`. `server.rs` `SessionInject`/`SessionPermit` arms → `coordinator.
  inject`/`permit`. **CLI parser SPLIT** (P4): `session` at `main.rs:3009` assumes `<contextId>` positional —
  give `permit` its own shape `session permit <requestId> --context <c> --generation <n> --op <o> (--approve
  [--option <id>]|--deny [--reason ..]|--modify <id>|--escalate)`; `inject <ctx> --input <f> [--append] [--dedupe
  <k>]`. **MCP** (P4): add `inject`+`permit` tools at BOTH `bridge-mcp` schema (`transport.rs:68`) AND dispatch
  (`server.rs:70`). The operator reads `{ctx,gen,op,request_id}` from `session/status`.
- **v2.T10 — DoD + live-gate (AMENDS draft T8).** Live-gate visibility is `session/status` (NOT `task watch`):
  `submit` a write-prompt under untrusted+read-only+DeferPolicy → `session status <ctx>` shows the pending perm
  + options + request_id → `session permit <reqid> --context <c> --generation <n> --op <o> --deny` rejects the
  write; re-run `--approve` allows; `session cancel <ctx>` mid-permission ends promptly. Plus inject codeword
  recall + the dead-safe auto-policy byte-identity gate.

### v2 open items for the 2nd review pass
- R1: `configure_turn` vs threading `TurnMeta` through a new `prompt` arg — is the per-session stash race-free
  given the turn lock? Confirm one producer owns the session at `configure_turn`→`prompt` (no interleave).
- R2: does `LocalDispatch` already carry `generation`+`op` (for `TurnMeta`), or must v2.T5 add them? (cancel-tokens
  added `abort`; confirm gen/op presence.)
- R3: `session/status` for a ctx with NO warm handle but a pending perm — can a permission be pending without a
  live warm session? (Handler holds the turn; the warm handle exists during the turn. Confirm.)
- R4: is scoping interactive permission to WARM sessions (detached-node deferred) acceptable for the slice DoD,
  or must a detached workflow node also support interactive permit in-slice?

---

## v3 — Dual-review reconciliation (Opus architecture lens + codex-xhigh 2nd pass) — BINDING

> Both reviews returned **needs-revision** but agreed the v2 five-decision architecture is SOUND — the fixes
> are precise edits, NOT re-architecture. This section supersedes v2 where it conflicts. Every corrected
> `file:line` below was re-verified against the working tree. The two lenses were complementary (Opus =
> lifecycle/placement/coherence; codex = wiring gaps + a wrong anchor that v2 AND the Opus lens both shared).

### Corrected facts (verified — these change v2's anchors)
| Fact | v2 said | TRUTH (verified) |
|------|---------|------------------|
| Serve policy construction | `main.rs:1913/2210/2627` | those are `implement_cmd`/resume/`run-workflow`. **Serve = `main.rs:3909` (inside `main`), MCP = `main.rs:3706` (`mcp_cmd`).** The live-gate uses SERVE → wire Defer at `:3909`+`:3706`. |
| `RegistrySnapshot` carries policy | implied | NO — it has only `default`/`entries`/`allowed_cmds` (`domain.rs:217`). Policy selection goes on **`ServerConfig` (`config.rs:44`)** via a `make_policy(&cfg.server)` helper. |
| `InboundServer` has a Coordinator | v2.T8/T9 call `coordinator.*` | NO coordinator field — only `registry`/`store`/`policy`/`task_store`/`session_manager` (`server.rs:71`). Wire `Arc<PermissionRegistry>` + `sm.inject` DIRECTLY into `InboundServer`; `SessionStatus` (`server.rs:2871`) reads `registry.pending`. |
| `ContainerRwBackend` forwards turn config | unaddressed | It forwards `configure_session` (`bridge-container/src/lib.rs:97`) but NOT `configure_turn`; `AcpContainerSpawn` only `.with_policy` (`main.rs:296`). Must add a `configure_turn` forward + thread the registry into the inner `AcpBackend`. |
| `LocalDispatch` carries gen/op | "confirm/add if absent" | ABSENT — only backend/session/seed/guard/warm_guard/abort (`dispatch.rs:71`). `WarmTurn` HAS gen+op (`session_manager.rs:95`); `WarmTurnGuard` carries ctx/gen/op. Add `turn_meta: Option<TurnMeta>` to `LocalDispatch`. |
| `prompt_inner` turn-lock protects the stash | v2.T5 assumed | `ensure_session().await` runs at `:1955` BEFORE `turn_lock` at `:1959` — a failed `ensure_session` leaves stale stashed meta. `prompt_inner` must `take` pending `TurnMeta` at entry + clear on every early error + move into `TurnRoute` at `:1986`. |

### v3 task amendments (apply ON TOP of v2)
- **v2.T4 (resolve in SessionManager) — CRITICAL placement fix (Opus).** Call `resolve_context_cancelled(ctx)`
  **AFTER** the `is_claimed`/`Cancelling` early-return guards, before any `backend.cancel().await`:
  `cancel_inner` after `:701-707`; `release_inner` after its claimed guard; `reset_session_inner` after the
  non-force claimed guard `:885-889`. **NOT at the very top** — a ctx whose handle is `Compacting`/`Resetting`
  must NOT have its in-flight claim's pending summarize-permission cancelled (that poisons the summarize →
  EXPIRE → data loss, the cancel-tokens latch lesson). Keep-warm `SessionCancel` DOES resolve a NORMAL turn's
  pending permission — and unlike the abort-token, this is SAFE (the registry is exact-once + gen+op-keyed, not
  a single slot) → add a test `keepwarm_cancel_resolves_pending_perm_without_stranding_next_turn` + a comment on
  the disanalogy. **Drop the "swept on finish" claim** (spec `:211`): a pending permission cannot coexist with a
  returned `PromptResponse` (the agent blocks on it), so finish-with-pending only happens on the abandon path,
  reaped by handler-timeout + the drop-guard. Registry liveness is bounded by `min(operator-resolve,
  permission_timeout_ms)` + drop-guard on EVERY handler exit; `resolve_context` is a prompt-cancel OPTIMIZATION,
  not the authoritative reaper. Use `resolve_context_cancelled(&ctx)` (constructs `Cancelled` per send → no
  `Clone` bound on `PermissionResolution`/`PermitDecision`).
- **v2.T5 (TurnMeta route) — site + race fixes (both lenses).** The producer never calls `prompt` directly —
  `Translator::run` does (`translator.rs:133`). Call `backend.configure_turn(session, meta)` in the THREE
  PRODUCER functions before they drive the turn: `spawn_local_producer` (`server.rs:1376`, before `Translator::
  run`), the unary Local arm (`server.rs:2311`), and the detached workflow `prompt_observed` caller
  (`executor.rs:237`). Source `TurnMeta{context_id, generation, op}` from `dispatch.turn_meta` (NEW field —
  add `turn_meta: Option<TurnMeta>` to `LocalDispatch`; fill `Some(TurnMeta{ctx, turn.generation,
  turn.op.clone()})` in `warm_local_dispatch`, `None` for cold binds). `prompt_inner` takes the stash at entry,
  clears it on every early error, moves it into `TurnRoute{tx, watch, turn_meta}`. `configure_turn` is a
  DEFAULTED no-op trait method (other backends unaffected). `ContainerRwBackend` MUST forward it (see v2.T5b).
  Note (Opus F2): ctx+gen are also derivable from the warm `SessionId` (`ctx-{ctx}-g{gen}`); `op` is the
  load-bearing field — documented escape hatch, but `configure_turn` is the chosen seam. `TurnMeta` is cheap-clone.
- **v2.T5b (NEW) — ContainerRwBackend propagation (codex BLOCKER C-F1).** Add a `configure_turn` forward to
  `ContainerRwBackend`'s `AgentBackend` impl (mirror `configure_session` at `lib.rs:97`) → `inner.configure_turn`
  before `inner.prompt`. Thread the shared `Arc<PermissionRegistry>` through `AcpContainerSpawn` (`main.rs:282-300`)
  into the inner `AcpBackend.with_permission_registry(...)`. (The slice live-gate uses DIRECT codex, so this is
  completeness — but the STANDING implementor config is containerized codex, so wire it.)
- **v2.T6 (interactive handler) — Escalate + byte-identity fixes (codex).** `Escalate` is a TRUE no-op: a
  `SessionPermit` carrying `PermitDecision::Escalate` does NOT `resolve` AND does NOT `reap` — the entry stays
  visible until `permission_timeout_ms` → handler default-denies → the drop-guard reaps. (Fixes the v2
  "reap-and-leave-pending" self-contradiction: `reap` removes-without-send, so it cannot leave-pending.) On the
  `Decide` branch, preserve the EXACT current mapping including `PermissionRequest::with_id(tool_call_id, false)`
  (`acp_backend.rs:1246`) so SF-7 byte-identity holds (`AutoPolicy` denies `interactive=true`, `permission.rs:15`).
  Map `PermitDecision::Deny` and a policy `Err(PermissionDenied)` through the SAME `select(&[RejectOnce,
  RejectAlways])` helper (`:1253`) → identical option; add a test they don't drift. Cold/detached prompt with NO
  `TurnMeta` → immediate default-deny (or Cancelled), NO registry entry (codex R4).
- **v2.T7 (Defer config) — correct sites (codex BLOCKER C-F2).** Add `permission_policy: Option<String>` +
  `permission_timeout_ms: Option<u64>` (default `120_000`) to `[server]` `ServerConfig` (`config.rs:44`). A
  `make_policy(&cfg.server) -> Arc<dyn PolicyEngine>` helper maps `"defer"`→`DeferPolicy` else `AutoPolicy`; call
  it at SERVE (`main.rs:3909`) and MCP (`main.rs:3706`). `DeferPolicy` (bridge-policy): `decide`→`Ok(Approve)`
  fallback, `interactive_decide`→`Defer`. NOTE the policy `Arc` is GLOBAL (one per serve, threaded via
  `.with_policy`) → Defer is all-agents-or-none in-slice; per-agent Defer is a tracked follow-up (no per-agent
  policy seam exists today). Thread `permission_timeout_ms` to `AcpBackend` + the container spawn.
- **v2.T8 (session/status visibility) — direct server wiring (codex BLOCKER C-F3).** `InboundServer` gains a
  `permission_registry: Option<Arc<PermissionRegistry>>` field (no Coordinator exists). The `SessionStatus`
  handler (`server.rs:2871`) appends a `pendingPermissions` block from `registry.pending(&ctx)`. Each entry is a
  `PendingPermissionView{request_id, tool_call_id, generation, op, title, options, raw_input(capped), timeout_ms}`
  (codex C-F9 adds `tool_call_id`+capped `raw_input` per the SPIKE-1 shape, spec `:248-261`). The operator reads
  gen+op+request_id from the CHOSEN pending entry (NOT the status root `generation`, which is the handle's CURRENT
  gen — Opus F3).
- **v2.T9 (wire/CLI/MCP) — direct handlers.** `SessionInject`/`SessionPermit` arms in `server.rs` dispatch
  (`:691`) call `sm.inject(...)` and `registry.resolve(&key, Decided(decision))` DIRECTLY (no Coordinator). CLI
  parser split + MCP `transport.rs:68`/`server.rs:70` as in v2.T9. `PermitParams.decision: PermitDecision`.
- **v2.T10 (DoD/live-gate).** Live-gate = SERVE with `[server] permission_policy="defer"` + direct codex
  `approval_policy="untrusted" sandbox_mode="read-only"` (port 8125). Visibility = `session status <ctx>` shows
  `pendingPermissions` (request_id/op/options); `session permit <reqid> --context <c> --generation <n> --op <o>
  --deny|--approve`; `session cancel <ctx>` mid-permission ends promptly. Poll-contract: `permission_timeout_ms`
  default `120_000` ≫ any sane operator poll interval (Opus F4).

### Deferred — TRACKED (not silently dropped)
- **Push/SSE permission visibility** — in-slice is PULL (`session/status`). A future slice adds a push reader of
  the SAME registry (insertion point: a `registry.subscribe`/journal reader; the registry already holds the
  views — no seam move). Tracked here alongside SF-8.
- **Detached-node INTERACTIVE permit** — detached workflow nodes run unattended; in-slice a Defer policy on a
  detached node TIMES OUT to default-deny (no operator). Interactive detached permit (human-in-the-loop queue)
  is a future slice. v2.T5's `executor.rs:237` `configure_turn` site is still wired so detached-Defer times-out
  cleanly rather than hanging.
- **Per-agent Defer policy** — needs a per-agent policy seam (none today); in-slice Defer is global `[server]`.
- **Producer-join residual single-slot re-mint** (SF-8, inherited from cancel-tokens) — unchanged.

### v3 verdict
Both lenses: needs-revision → **all findings folded above**. No re-architecture; the five v2 decisions stand.
ONE scope question surfaced by both R4 answers (warm-only interactive permit, detached deferred) — both
reviewers call it the correct slice boundary. Plan is now **ready-to-implement** pending that scope confirm.
