# Slice 2 — Usage telemetry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Plumb the ACP `usage_update` notification — today received and dropped at TWO gates — end-to-end:
map → `Update::Usage` → through the bridge-acp turn pipeline → through the translator event stream → record
on the warm `SessionManager` handle → expose in `session/status` → a configurable pre-task threshold warn,
WITHOUT ever putting usage on the A2A wire.

**Architecture:** (1) bridge-acp maps `SessionUpdate::UsageUpdate` and carries it through a new
`TurnEvent::Usage` so it reaches the `BackendStream` as `Update::Usage`; (2) bridge-core's `Translator`
surfaces it as a new `EventKind::Usage` event carrying a `UsageSnapshot`; (3) bridge-a2a-inbound's producers
RECORD it on the warm handle (`record_usage`, latest-wins) and explicitly FILTER it before every A2A output
channel (SSE/unary/fan-out) so the wire contract is byte-identical; (4) `session/status` gains a `usage`
block (used/size/windowFraction/cost/atMs/overThreshold); (5) a pre-task, level-triggered, advisory threshold
warn computed at `checkout_turn` from the carried usage (serve WARN log + `overThreshold` in status).

**Tech Stack:** Rust workspace — bridge-acp, bridge-core, bridge-a2a-inbound, bin/a2a-bridge; the
`agent-client-protocol` SDK (`agent-client-protocol-schema 0.13.2`) with feature `unstable_session_usage`
ENABLED (`crates/bridge-acp/Cargo.toml`), which models `SessionUpdate::UsageUpdate(UsageUpdate{used:u64,
size:u64,cost:Option<Cost{amount:f64,currency:String}>})`.

**Spec:** `docs/superpowers/specs/2026-06-18-slice-2-usage-telemetry.md` (v2, dual-reviewed — FIX-1..10
binding). **Slicing authority:** `2026-06-17-orchestration-slicing.md` (Slice 2). Built on Slice 0+1 (shipped,
main).

**Implementor:** codex gpt-5.5/high host (`run-workflow slice0-impl --session-cwd <repo>`), test+impl together,
controller verifies+commits (the `_dyld_start` flake). **Gate enum additions with `cargo test --workspace
--no-run` (`--all-targets`)** — adding `EventKind::Usage` / `TurnEvent::Usage` breaks exhaustive `match`es in
test targets that `cargo build` misses (Slice-0/1 lesson).

**Grounded seams (verbatim-verified):**
- `map_session_update(notif) -> Option<Update>` `acp_backend.rs:1622-1629` (the pure mapper; corpus-replay
  driven `:1615-1620`); SDK `UsageUpdate` `agent-client-protocol-schema-0.13.2/src/v1/client.rs:271-287`,
  `SessionUpdate::UsageUpdate` gated `:120-121`. Corpus frames: `tests/corpus/codex-acp.jsonl` (usage, no cost),
  `tests/corpus/claude-agent-acp.jsonl:12` (usage with used/size).
- `enum TurnEvent { Text(String), Done(Update), Failed(BridgeError) }` `acp_backend.rs:211-223`; the
  notification handler routes only `Update::Text` `:973-981`; the prompt `unfold` maps only Text/Done/Failed
  `:1834-1848`; the scripted-agent test harness `enum ScriptedUpdate` `:2416-2423` + its `SessionUpdate`
  builder `:2950-2964`.
- `Update::Usage(crate::orch::UsageSnapshot)` `ports.rs:24`; `UsageSnapshot{used:Option<u64>,size:Option<u64>,
  cost:Option<UsageCost>,at_ms:i64}` + `UsageCost{amount,currency}` `orch.rs:31-43`.
- `enum EventKind { Status, Artifact, Terminal }` `translator.rs:24-29`; `struct Event{kind,text,source,
  outcome}` `:39-44` + ctors `status`/`artifact`/`terminal` `:~60-90`; the `Ok(Update::Usage(_)) => continue;`
  drop `translator.rs:157`.
- `sse.rs` — `event_to_streamresponse` exhaustive `match ev.kind()` `:46-108`; `event_to_sse` exhaustive name
  `match` `:113-123`.
- `session_manager.rs` — `struct WarmHandle` `:31-48` (no usage), `struct WarmTurn{backend,session}` `:51-54`,
  `struct SessionStatusInfo{state,agent,generation,idle_age_ms,capabilities}` `:57-63`, `checkout_turn`
  `:95-251` (resume fast-return `:124`, post-reconcile return `:190`, mint return `:229`), `record_*`/`status`
  `:254-276`. Epoch helper `crate::workflow_sink::now_ms() -> i64` `workflow_sink.rs:61`.
- `server.rs` — warm producer loop `spawn_local_producer` `:1126-1200` (forwards every event via `tx.send(ev)`
  `:1177`), the unary Local arm `.collect().await` `:2318-2329`, `local_kiro_source` fan-out swallow
  `:1376-1388` (Terminal swallow `:1382`), `WarmTurnGuard{sm,ctx}` `:452-465`, `warm_local_dispatch`
  `:553-579`, `session_status` JSON `:2842-2858`.
- `bin/a2a-bridge/src/config.rs` — `struct ServerConfig{addr,warm_idle_ttl_secs}` `:44-50`; serve wiring
  `bin/a2a-bridge/src/main.rs:3657-3662` (`SessionManager::new(registry, Duration::from_secs(warm_ttl))`).

---

## v2 — dual plan-review fixes folded (codex-xhigh + Opus, both `fix-then-execute`)

All findings verified against the code. These resolutions AMEND the tasks below; where they conflict, THESE
win. codex (correctness lead) caught the cfg + ordering blockers; Opus caught the corpus-test inversion +
borrow/guard details. No rework — the seams hold.

- **PF-1 (BLOCKER, both) — T1 must REWRITE three existing `corpus_replay.rs` tests, not just add assertions.**
  Once `map_session_update` returns `Some(Update::Usage)` for `usage_update`, these EXISTING tests fail (two via
  `panic!`): (a) `usage_update_is_modeled_and_dropped_not_a_minus_32602` (`tests/corpus_replay.rs:~367` asserts
  `.is_none()`) → repurpose to `..._is_modeled_and_SURFACED` asserting `Some(Update::Usage{used:Some(55011),
  size:Some(1000000),cost:Some(_)})`, KEEPING the `-32602` "deser-doesn't-fail" regression note; (b)
  `codex_real_capture_replays_pong_and_drops_unmodeled` (`:~245` `panic!` on unexpected modeled outcome; `:~270`
  `assert_eq!(modeled, 3)`) → add a `ReplayOutcome::Update(Update::Usage(s))` arm that counts `usage_seen`, keep
  `modeled == 3`; (c) `claude_agent_acp_real_capture_replays_through_backend` (`:~401` `panic!`) → same usage
  arm. This is the home for FIX-4/FIX-10's "proven across BOTH ACP agents" assertion. (Gemini corpus has no
  usage frame — unaffected.) **List these three fn names in T1 so the red bar is expected, and correct T1
  Step-5's "PASS" claim — it only passes AFTER these rewrites.**
- **PF-2 (BLOCKER, codex; CORRECTS Opus) — remove ALL `#[cfg(feature = "unstable_session_usage")]` from
  bridge-acp code/tests; use UNCONDITIONAL code.** `bridge-acp` enables `unstable_session_usage` only as an
  `agent-client-protocol` DEPENDENCY feature (`crates/bridge-acp/Cargo.toml:~19`), NOT as a `bridge-acp` crate
  `[features]` entry — so `cfg(feature="unstable_session_usage")` is NEVER set for bridge-acp and every gated
  arm/test/variant **compiles out** (silently dead), and `clippy -D warnings` trips `unexpected_cfgs`. Affects
  the T1 map arm + its unit test, and the T2 `TurnEvent::Usage` variant + handler arm + `unfold` arm + the
  `map_session_update_maps_usage` / `usage_update_reaches_prompt_stream` tests. **Fix: delete those `#[cfg]`s
  (the SDK `UsageUpdate`/`SessionUpdate::UsageUpdate` types are available because the dep feature is
  hard-enabled — referencing them needs no cfg).** (Optional alternative: add a real forwarding feature
  `[features] default=["unstable_session_usage"]; unstable_session_usage=["agent-client-protocol/unstable_session_usage"]`
  — NOT needed; unconditional is simpler.)
- **PF-3 (MAJOR, both — the ordering defect) — move the SSE defensive handling from T5 into T3.** Adding
  `EventKind::Usage` (T3) breaks the only two EXHAUSTIVE `match ev.kind()` sites in the workspace —
  `event_to_streamresponse` (`sse.rs:46`) and `event_to_sse` (`sse.rs:115`) — so T3's `cargo test --workspace
  --no-run` cannot pass while the SSE fix is deferred to T5; and between T3 and T5 a warm turn's usage event
  would reach SSE. **Fix: T3 does the full SSE-safe change** (`event_to_sse → Option<SseEvent>` returning `None`
  for `Usage`; `EventKind::Usage => unreachable!(..)` in `event_to_streamresponse`; the server SSE-stream
  wrapper → `filter_map` per PF-5; update the sse.rs tests that call `event_to_sse` to `.unwrap()`). After T3
  the wire is already safe (usage dropped, not yet recorded). **T5 shrinks to producer RECORDING**:
  `spawn_local_producer` taps `EventKind::Usage` → `record_usage` iff warm → `continue`; the unary `.collect()`→
  loop records + excludes; `local_kiro_source` swallows usage. All `.kind()` uses outside sse.rs are `==`/filter
  predicates (translator tests, `fanout.rs:216/264/282`, `server.rs:1171/1382/2481/2486/2562/2578`) → compile
  unchanged, treat Usage as a no-op. `executor.rs:143` matches `Update::Usage(_)` (already wildcarded). `delegate`
  + reattach carry PEER frames (outbound maps remote → `Event::status/artifact`), so local Usage cannot
  originate there — no leak path, leave untouched.
- **PF-4 (MAJOR, codex) — T3 must update the raw `Event { … }` literals, not only the named constructors.**
  `Translator::run` builds events as struct literals at `translator.rs:130, 137, 164, 177, 191, 199` (Status/
  Artifact flushes). Adding the `usage` field breaks every one. **Fix:** replace each with `Event::status(chunk)`
  / `Event::artifact(payload)`, or add `usage: None` to every literal. (T3 Step 4's `cargo test -p bridge-core`
  catches this immediately.)
- **PF-5 (MAJOR, Opus) — the SSE wrapper is NOT `.map(event_to_sse)`.** It is (server.rs ~2249)
  `.map(|item| { let frame = match item { Ok(ev) => event_to_sse(&ev, …), Err(e) => <error frame> }; Ok(frame)})`.
  With `event_to_sse → Option`, specify the exact shape so the ERROR frame survives:
  `Ok(ev) => event_to_sse(&ev, …).map(Ok)` (None → filtered), `Err(e) => Some(Ok(<error frame>))`, and switch
  `.map(..)` → `.filter_map(..)`. (Defensive/compile-only on the warm path since T5's producer filter precedes
  it, but it must keep error frames.)
- **PF-6 (MAJOR detail, Opus) — T5 unary: keep BOTH guards.** The arm has `let _guard = dispatch.guard;`
  (`server.rs:2316`, its Drop runs `finish_turn`/eviction) AND `let _warm = dispatch.warm_guard;` (`:2317`).
  Rename ONLY `_warm`→`warm`; keep `_guard`. The in-loop `record_usage(...).await` completes before `warm`
  drops at the end of the arm (correct as written) — state it so the implementor doesn't reorder.
- **PF-7 (MINOR, both) — T4 must ASSERT FIX-7 (no idle bump), not just comment it.** Add a `ManualClock` test:
  `checkout_turn` → `finish_turn` → `clock.advance(> ttl)` → `record_usage(...)` → `reap_idle()` → assert the
  handle is REAPED (gone). Proves `record_usage` does not refresh `last_used` (`session_manager.rs:323` reaps on
  `last_used`; `finish_turn:254` is the legitimate refresh).
- **PF-8 (MINOR, codex) — T7 binary test command is invalid.** `a2a-bridge` is bin-only (no lib target) → use
  `cargo test -p a2a-bridge --bin a2a-bridge warm_usage` (or `cargo test -p a2a-bridge warm_usage`). Also extend
  the existing `warm_idle_ttl_defaults_and_overrides` config test (`config.rs:~2255`) to cover
  `warm_usage_warn_fraction` default(None)+override.
- **PF-9 (MINOR, Opus) — T2 keep `let session_id = notif.session_id.clone();`** (the line before the consuming
  `map_session_update(notif)`, `acp_backend.rs:972`) — the routing snippet uses `session_id` after `notif` is
  moved.
- **PF-10 (MINOR, Opus) — T7 FIX-9 borrow:** compute `let warn = self.eval_warn(&h.usage);` while `h` is
  borrowed (≈`session_manager.rs:119`, a shared borrow alongside `&self.warn_fraction` — no conflict with the
  `tab` guard); the owned `Option<UsageWarning>` survives `drop(tab)`/re-lock and attaches at BOTH resume
  returns (`:124`, `:190`); mint (`:229`) hardcodes `usage_warning: None`.
- **PF-11 (MINOR, Opus) — T6 assert `windowFraction` with tolerance** (`14584/258400 = 0.05644…`):
  `(wf - 0.0564).abs() < 1e-3`, not float equality.

> **Net effect on tasks:** **T1** grows (rewrite the 3 corpus tests PF-1; drop cfg PF-2). **T2** drops cfgs
> (PF-2) + keeps the `session_id` clone (PF-9). **T3** grows: raw-literal fix (PF-4) + the FULL SSE-safe change
> (PF-3/PF-5) — it now OWNS sse.rs; its gate `cargo test --workspace --no-run` passes. **T4** adds the
> idle-bump reap test (PF-7). **T5** shrinks to producer recording (sse moved to T3) + keep-both-guards (PF-6).
> **T6** float tolerance (PF-11). **T7** binary cmd + config test (PF-8) + the borrow note (PF-10). **T8**
> unchanged.

---

## Task 1: bridge-acp — map `usage_update` → `Update::Usage` (pure mapper) + corpus assertions

**Files:** Modify `crates/bridge-acp/src/acp_backend.rs:1622-1629`. Test: same file's `#[cfg(test)]` + the
corpus-replay test (`crates/bridge-acp/tests/corpus_replay.rs`).

- [ ] **Step 1: Write failing unit test** (in `acp_backend.rs` tests):

```rust
#[test]
fn map_session_update_maps_usage_to_update_usage_clock_free() {
    use agent_client_protocol::schema::UsageUpdate;
    let notif = SessionNotification::new(
        AgentSessionId::from("s"),
        SessionUpdate::UsageUpdate(UsageUpdate::new(14584, 258400)),
    );
    let u = AcpBackend::map_session_update(notif).expect("usage maps");
    match u {
        Update::Usage(s) => {
            assert_eq!(s.used, Some(14584));
            assert_eq!(s.size, Some(258400));
            assert_eq!(s.cost, None);     // corpus codex frame carries no cost
            assert_eq!(s.at_ms, 0);       // FIX-8/§4: clock-free; stamped downstream
        }
        other => panic!("expected Update::Usage, got {other:?}"),
    }
}
```
(If `AgentSessionId::from`/`SessionNotification::new` arg shapes differ, mirror the construction used by the
existing `map_session_update` tests / corpus_replay.)

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-acp --lib map_session_update_maps_usage` →
FAIL (mapper returns `None` for usage today).

- [ ] **Step 3: Rewrite `map_session_update`** (fold into ONE by-value `match` per FIX-8 — the current
`if let` consumes `notif.update`, so a second `&notif.update` branch won't compile):

```rust
#[must_use]
pub fn map_session_update(notif: SessionNotification) -> Option<Update> {
    match notif.update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(t) = chunk.content {
                return Some(Update::Text(t.text));
            }
            None
        }
        // FIX-1/CORRECTION-2: surface usage. Clock-free (at_ms stamped at record_usage) so the
        // corpus-replay conformance test stays deterministic. UNCONDITIONAL (PF-2): the SDK usage types
        // are available because bridge-acp hard-enables the dependency feature — no crate `#[cfg]` here.
        SessionUpdate::UsageUpdate(u) => Some(Update::Usage(bridge_core::orch::UsageSnapshot {
            used: Some(u.used),
            size: Some(u.size),
            cost: u
                .cost
                .map(|c| bridge_core::orch::UsageCost { amount: c.amount, currency: c.currency }),
            at_ms: 0,
        })),
        _ => None, // tolerant reader: unmodeled variants / non-text chunk content
    }
}
```

- [ ] **Step 4: Add corpus assertions** (in `tests/corpus_replay.rs`): assert the captured `usage_update`
frames map to `Update::Usage`. Mirror the existing per-frame replay; for the codex frame
(`tests/corpus/codex-acp.jsonl`) assert `used`/`size` present + `cost == None`; for the claude frame
(`tests/corpus/claude-agent-acp.jsonl`) assert `used`/`size` present. (FIX-4/FIX-10: proves the un-drop across
BOTH ACP agents and pins "ACP always carries used+size".)

- [ ] **Step 5: Run + build** — `cargo test -p bridge-acp --lib map_session_update_maps_usage &&
cargo test -p bridge-acp --test corpus_replay && cargo build -p bridge-acp` → PASS. (This commit is a behavior
NO-OP end-to-end: the handler at `:973` still drops `Update::Usage` until Task 2 — the mapper change alone is
safe.)

- [ ] **Step 6: Commit** — `feat(acp): map usage_update -> Update::Usage (pure, clock-free) + corpus assertions`

---

## Task 2: bridge-acp — widen the `TurnEvent` pipeline so usage reaches the BackendStream (FIX-1)

**Files:** Modify `crates/bridge-acp/src/acp_backend.rs` — `TurnEvent` `:211`, handler `:973`, `unfold` `:1838`,
`ScriptedUpdate` `:2416` + its builder `:2950`. Test: same-file scripted prompt-stream test.

- [ ] **Step 1: Write failing test** (mirror the nearest scripted prompt-stream test, e.g. the one driving
`ScriptedUpdate::Text`/`Plan` around `acp_backend.rs:~3400`): script
`[ScriptedUpdate::Usage(100, 1000), ScriptedUpdate::Text("hi")]`, drive `prompt`, and assert the returned
`BackendStream` yields an `Update::Usage` with `used == Some(100)` BEFORE the terminal `Update::Done`:

```rust
#[tokio::test]
async fn usage_update_reaches_prompt_stream() {
    // ... stand up the scripted agent with prompt_updates = [Usage(100,1000), Text("hi")] ...
    let mut items = Vec::new();
    while let Some(it) = stream.next().await { items.push(it); }
    let saw_usage = items.iter().any(|it| matches!(it, Ok(Update::Usage(s)) if s.used == Some(100)));
    assert!(saw_usage, "usage must traverse the TurnEvent pipeline to the BackendStream");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL (`ScriptedUpdate::Usage` undefined / usage never yielded).

- [ ] **Step 3a: Add the `TurnEvent` carrier** (`acp_backend.rs:211`, after `Text`):

```rust
    /// A streamed context-window usage snapshot (ACP `usage_update`). Non-terminal,
    /// routed exactly like `Text`. [Slice 2] (UNCONDITIONAL — PF-2, no crate `#[cfg]`.)
    Usage(bridge_core::orch::UsageSnapshot),
```

- [ ] **Step 3b: Route it in the notification handler** (`:973`, replace the `if let` with a routed value):

```rust
    let te = match Self::map_session_update(notif) {
        Some(Update::Text(text)) => Some(TurnEvent::Text(text)),
        Some(Update::Usage(snap)) => Some(TurnEvent::Usage(snap)),
        _ => None, // unmodeled / non-text (tolerant reader)
    };
    if let Some(te) = te {
        if let Ok(map) = updates.lock() {
            if let Some(tx) = map.get(&session_id) {
                let _ = tx.send(te);
            }
        }
    }
```

- [ ] **Step 3c: Yield it from the stream `unfold`** (`:1838`, alongside the `Text` arm, NON-terminal):

```rust
                Some(TurnEvent::Usage(snap)) => Some((Ok(Update::Usage(snap)), (rx, false))),
```

- [ ] **Step 3d: Extend the test harness** — add `Usage(u64, u64)` to `ScriptedUpdate` (`:2416`) and a builder
arm (`:2950`):

```rust
        // enum ScriptedUpdate { ... }
        Usage(u64, u64),
        // ... in the `match u` builder:
        ScriptedUpdate::Usage(used, size) => SessionUpdate::UsageUpdate(
            agent_client_protocol::schema::UsageUpdate::new(used, size),
        ),
```
(PF-2: ALL of T2 is unconditional — `unstable_session_usage` is a hard-enabled DEPENDENCY feature, not a
bridge-acp crate feature, so a `#[cfg(feature=...)]` would never be set and would silently compile the code
out. No `#[cfg]` anywhere in T1/T2.)

- [ ] **Step 4: Run + build** — `cargo test -p bridge-acp --lib usage_update_reaches_prompt_stream &&
cargo build -p bridge-acp` → PASS. (Note: `crates/bridge-workflow/src/executor.rs:143` already has
`Some(Ok(Update::Usage(_))) => { /* ignore */ }` — it now actually receives usage and keeps ignoring it; no
change, executor keep-warm is S5.)

- [ ] **Step 5: Commit** — `feat(acp): carry usage through TurnEvent pipeline to the BackendStream (FIX-1)`

---

## Task 3: bridge-core translator — `EventKind::Usage` + emit instead of drop (FIX-8)

**Files:** Modify `crates/bridge-core/src/translator.rs` (`EventKind` `:24`, `Event` struct + ctors `:39-91`,
the `Update::Usage` arm `:157`). Test: same file.

- [ ] **Step 1: Write failing tests:**

```rust
#[tokio::test]
async fn translator_emits_usage_event_and_no_artifact_text() {
    // FakeBackend yields: Usage(used:7,size:9), Text("body"), Done(end_turn)
    let evs = collect_translator_events(vec![
        Ok(Update::Usage(UsageSnapshot { used: Some(7), size: Some(9), cost: None, at_ms: 0 })),
        Ok(Update::Text("body".into())),
        Ok(Update::Done { stop_reason: "end_turn".into() }),
    ]).await;
    // A Usage event is emitted carrying the snapshot.
    let usage = evs.iter().find(|e| e.kind() == &EventKind::Usage).expect("usage event");
    assert_eq!(usage.usage_snapshot().and_then(|s| s.used), Some(7));
    assert!(usage.text().is_empty(), "usage events carry no artifact text");
    // The final artifact text is exactly the agent body (usage contributes nothing).
    let artifact = evs.iter().rev().find(|e| e.kind() == &EventKind::Artifact).unwrap();
    assert_eq!(artifact.text(), "body");
}
```
(Reuse/define a small `collect_translator_events(updates)` helper like the existing `sse.rs` `FakeBackend`.)

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-core --lib translator_emits_usage` → FAIL
(`EventKind::Usage`/`usage_snapshot` undefined).

- [ ] **Step 3a: Add the variant + field + ctor + accessor** in `translator.rs`:

```rust
// EventKind (:24)
pub enum EventKind { Status, Artifact, Terminal, Usage }

// Event struct (:39) — add the field
pub struct Event { kind: EventKind, text: String, source: Option<String>, outcome: Option<TaskOutcome>,
                   usage: Option<crate::orch::UsageSnapshot> }

// add `usage: None` to the status/artifact/terminal constructors, and:
impl Event {
    pub fn usage(snap: crate::orch::UsageSnapshot) -> Self {
        Self { kind: EventKind::Usage, text: String::new(), source: None, outcome: None, usage: Some(snap) }
    }
    pub fn usage_snapshot(&self) -> Option<&crate::orch::UsageSnapshot> { self.usage.as_ref() }
}
```

- [ ] **Step 3b: Emit instead of drop** (`translator.rs:157`):

```rust
                    Ok(Update::Usage(snap)) => {
                        yield Event::usage(snap);   // was: continue;
                    }
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-core --lib translator_emits_usage &&
cargo test --workspace --no-run` (catch exhaustive-match breaks from the new `EventKind` variant — fix any
`_`-less `match ev.kind()` in test targets) → PASS.

- [ ] **Step 5: Commit** — `feat(core): translator surfaces Update::Usage as EventKind::Usage (no artifact text)`

---

## Task 4: bridge-a2a-inbound — `WarmHandle.usage` + `SessionManager::record_usage` (FIX-7)

**Files:** Modify `crates/bridge-a2a-inbound/src/session_manager.rs` (`WarmHandle` `:31`, `SessionStatusInfo`
`:57`, mint `:235`, `status` `:262`, + a new `record_usage`).

- [ ] **Step 1: Write failing tests** (in the session_manager test module):

```rust
#[tokio::test]
async fn record_usage_latest_wins_stamps_at_ms_and_no_idle_bump() {
    let (manager, _b, _r) = manager();
    let c = ctx("u");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap();
    manager.record_usage(&c, UsageSnapshot { used: Some(10), size: Some(100), cost: None, at_ms: 0 }).await;
    manager.record_usage(&c, UsageSnapshot { used: Some(42), size: Some(100), cost: None, at_ms: 0 }).await;
    let s = manager.status(&c).await.unwrap();
    assert_eq!(s.usage.used, Some(42));            // latest-wins
    assert!(s.usage.at_ms > 0);                    // FIX/§4: stamped at record time
    // FIX-7: usage does not resurrect/refresh idle (no last_used bump asserted via reap behavior elsewhere).
}

#[tokio::test]
async fn record_usage_noops_unknown_ctx() {
    let (manager, _b, _r) = manager();
    manager.record_usage(&ctx("nope"), UsageSnapshot::default()).await; // must not panic
    assert!(manager.status(&ctx("nope")).await.is_none());
}
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p bridge-a2a-inbound --lib record_usage` → FAIL.

- [ ] **Step 3a: Add the field + status field:**

```rust
// WarmHandle (:31) — add:
    usage: UsageSnapshot,
// at the mint insert (:235) — add:
    usage: UsageSnapshot::default(),
// SessionStatusInfo (:57) — add:
    pub usage: UsageSnapshot,
```
Import: `use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome, UsageSnapshot};` (extend the existing
`orch` import).

- [ ] **Step 3b: Add `record_usage`** (FIX-7 — updates ONLY `usage`, never `last_used`):

```rust
    /// Record the latest usage snapshot for a warm handle (latest-wins). Stamps `at_ms`
    /// here (the inbound layer has a wall clock; SessionManager.now is monotonic). Does NOT
    /// touch last_used (FIX-7) and no-ops a missing/removed handle. [Slice 2]
    pub async fn record_usage(&self, ctx: &ContextId, mut snap: UsageSnapshot) {
        snap.at_ms = crate::workflow_sink::now_ms();
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            h.usage = snap;
        }
    }
```

- [ ] **Step 3c: Surface `usage` in `status()`** (`:262`, add to the returned `SessionStatusInfo`):

```rust
            usage: h.usage.clone(),
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-a2a-inbound --lib record_usage && cargo build -p bridge-a2a-inbound` → PASS.
- [ ] **Step 5: Commit** — `feat(inbound): WarmHandle.usage + SessionManager::record_usage (latest-wins, no idle bump)`

---

## Task 5: bridge-a2a-inbound — record usage on the warm path + FILTER it off every A2A output channel (FIX-2/FIX-3)

**Files:** Modify `crates/bridge-a2a-inbound/src/sse.rs` (`event_to_sse`/`event_to_streamresponse`),
`crates/bridge-a2a-inbound/src/server.rs` (`spawn_local_producer` `:1126-1200`, the unary Local arm
`:2318-2329`, the SSE stream wrapper, `local_kiro_source` `:1382`). Tests: sse.rs + server.rs.

- [ ] **Step 1: Write failing tests:**
  - `sse.rs`: `event_to_sse(&Event::usage(snap), "t","c")` returns `None` (telemetry never a frame).
  - `server.rs` (streaming): a warm turn whose backend yields `Update::Usage` records it on the handle
    (`sm.status(C).usage.used == Some(..)`) AND the SSE frame list contains NO usage/extra frame (identical to
    the no-usage run) — DoD-5.
  - `server.rs` (unary): with a `Update::Usage` in the stream, the artifact text equals the agent body
    (usage excluded) AND the warm handle records the usage.
  - `server.rs` (fan-out): `local_kiro_source` does not yield a `Usage` event.

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3a: Make the SSE converter defensive** (`sse.rs`): change `event_to_sse` to return
`Option<SseEvent>` (FIX-2 — usage is never a wire frame):

```rust
pub fn event_to_sse(ev: &Event, task_id: &str, context_id: &str) -> Option<SseEvent> {
    if matches!(ev.kind(), EventKind::Usage) { return None; } // telemetry: recorded, never wired
    let sr = event_to_streamresponse(ev, task_id, context_id);
    let event_name = match ev.kind() {
        EventKind::Status => EVENT_STATUS,
        EventKind::Artifact => EVENT_ARTIFACT,
        EventKind::Terminal => EVENT_STATUS,
        EventKind::Usage => unreachable!("usage filtered above"),
    };
    let data = serde_json::to_string(&sr).expect("a2a::StreamResponse always serializes");
    Some(SseEvent::default().event(event_name).data(data))
}
```
and add `EventKind::Usage => unreachable!("usage events are telemetry; filtered before SSE conversion")` to the
`event_to_streamresponse` match (`:91`-ish). Update the existing sse.rs tests that call `event_to_sse(...)`
(`:201`, `:408-409`) to `.unwrap()` (they pass Status/Artifact). **Update the SSE stream wrapper** (the
`.map(event_to_sse)` over pipeline events, `server.rs:~2249`) to a `filter_map` that drops `None`.

- [ ] **Step 3b: Tap + filter in the streaming producer** (`spawn_local_producer`, `server.rs:1126-1200`).
Keep `warm` usable (rename `let _warm = warm;` → `let warm = warm;`), and intercept BEFORE the `tx.send`:

```rust
            // Slice 2: usage is telemetry — record it on the warm handle, never forward to SSE.
            if let Ok(e) = &ev {
                if e.kind() == &EventKind::Usage {
                    if let (Some(snap), Some(w)) = (e.usage_snapshot(), warm.as_ref()) {
                        w.sm.record_usage(&w.ctx, snap.clone()).await;
                    }
                    continue;
                }
            }
```
(`WarmTurnGuard { sm, ctx }` fields are private but in the same module — accessible. `warm` still drops at the
end of the spawn → `finish_turn`.)

- [ ] **Step 3c: Tap + filter in the unary producer** (`server.rs:2318-2329`). Replace the blind
`.collect().await` with a recording loop (FIX-3 — the unary path has no event loop today). Keep `warm` usable
(`let warm = dispatch.warm_guard;`):

```rust
            let translator = Translator::new();
            let mut events = translator.run(
                dispatch.backend.as_ref(), srv.store.as_ref(), srv.policy.as_ref(),
                &routed.task, &dispatch.session, routed.parts,
            );
            let mut collected: Vec<Result<Event, BridgeError>> = Vec::new();
            while let Some(ev) = events.next().await {
                if let Ok(e) = &ev {
                    if e.kind() == &EventKind::Usage {
                        if let (Some(snap), Some(w)) = (e.usage_snapshot(), warm.as_ref()) {
                            w.sm.record_usage(&w.ctx, snap.clone()).await;
                        }
                        continue; // exclude from the unary output (no artifact/status corruption)
                    }
                }
                collected.push(ev);
            }
            collected
```
(`use futures::StreamExt;` is already in scope for the streaming path.)

- [ ] **Step 3d: Swallow usage in the fan-out local source** (`local_kiro_source`, `server.rs:1382`):

```rust
            if matches!(&ev, Ok(e) if e.kind() == &EventKind::Terminal || e.kind() == &EventKind::Usage) {
                continue;
            }
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-a2a-inbound --lib && cargo test --workspace --no-run`
(catch any remaining exhaustive `EventKind` match) → PASS.
- [ ] **Step 5: Commit** — `feat(inbound): record warm usage + filter EventKind::Usage off SSE/unary/fan-out (FIX-2/3)`

---

## Task 6: bridge-a2a-inbound — `session/status` usage block + windowFraction (FIX-5 part 1)

**Files:** Modify `crates/bridge-a2a-inbound/src/session_manager.rs` (a `window_fraction` helper on
`SessionStatusInfo`), `crates/bridge-a2a-inbound/src/server.rs` (`session_status` JSON `:2842`).

- [ ] **Step 1: Write failing test** — extend the Slice-0/1 `session_status` test (or add one): after a warm
turn + `record_usage(used:14584,size:258400)`, `session/status C` JSON has
`result.usage.used == 14584`, `result.usage.size == 258400`, `result.usage.windowFraction ≈ 0.0565`, and
`result.usage.atMs > 0`; with a `used:None` snapshot, `windowFraction == null`.

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3a: Add a fraction helper** on `SessionStatusInfo` (`session_manager.rs`):

```rust
impl SessionStatusInfo {
    /// used/size when both are known and size>0, else None (degrade-safe).
    pub fn window_fraction(&self) -> Option<f64> {
        match (self.usage.used, self.usage.size) {
            (Some(u), Some(s)) if s > 0 => Some(u as f64 / s as f64),
            _ => None,
        }
    }
}
```

- [ ] **Step 3b: Add the `usage` block** to the `session_status` `json!` (`server.rs:2842`, additive — existing
fields unchanged):

```rust
                "usage": {
                    "used": s.usage.used,
                    "size": s.usage.size,
                    "windowFraction": s.window_fraction(),
                    "cost": s.usage.cost.as_ref().map(|c| serde_json::json!({
                        "amount": c.amount, "currency": c.currency
                    })),
                    "atMs": s.usage.at_ms,
                    // "overThreshold" added in Task 7
                },
```

- [ ] **Step 4: Run + build** — `cargo test -p bridge-a2a-inbound --lib session_status && cargo build -p bridge-a2a-inbound` → PASS.
- [ ] **Step 5: Commit** — `feat(inbound): surface usage + windowFraction in session/status`

---

## Task 7: pre-task threshold warn — config + WarmTurn.usage_warning + overThreshold (FIX-5/FIX-6/FIX-9)

**Files:** Modify `bin/a2a-bridge/src/config.rs` (`ServerConfig`), `bin/a2a-bridge/src/main.rs:3659`
(serve wiring), `crates/bridge-a2a-inbound/src/session_manager.rs` (warn_fraction field + `UsageWarning` +
`WarmTurn.usage_warning` + `checkout_turn` + `over_threshold` in status), `crates/bridge-a2a-inbound/src/
server.rs` (`warm_local_dispatch` serve log + status `overThreshold`).

- [ ] **Step 1: Write failing tests** (session_manager):

```rust
#[tokio::test]
async fn checkout_warns_when_carried_usage_at_or_above_fraction() {
    let backend = Arc::new(FakeBackend::new("ok"));
    let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
    let manager = SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
    let c = ctx("warn");
    manager.checkout_turn(&c, agent(), None, None, op("op-1")).await.unwrap(); // mint: no warn
    manager.finish_turn(&c).await;
    manager.record_usage(&c, UsageSnapshot { used: Some(90), size: Some(100), cost: None, at_ms: 0 }).await;
    let turn = manager.checkout_turn(&c, agent(), None, None, op("op-2")).await.unwrap();
    let w = turn.usage_warning.expect("warn at 0.9 >= 0.8");
    assert_eq!((w.used, w.size), (90, 100));
    // status reflects it
    assert_eq!(manager.status(&c).await.unwrap().over_threshold, Some(true));
}

#[tokio::test]
async fn no_warn_below_or_disabled_or_degraded() {
    // disabled (None) -> usage_warning None; below fraction -> None; size:None -> None + over_threshold None.
}

#[tokio::test]
async fn mint_never_warns() { /* fresh ctx checkout -> usage_warning None */ }
```

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3a: Config knob** (`config.rs:44`):

```rust
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
    #[serde(default = "default_warm_idle_ttl_secs")]
    pub warm_idle_ttl_secs: u64,
    /// Advisory pre-task warn when carried context usage >= this window fraction (0,1]. None = off. [Slice 2]
    #[serde(default)]
    pub warm_usage_warn_fraction: Option<f64>,
}
```

- [ ] **Step 3b: SessionManager warn state + builder** (avoid breaking the ~8 `new(...)` test call sites):

```rust
// fields
    warn_fraction: Option<f64>,
// in new_with_clock: warn_fraction: None,
// builder:
    pub fn with_warn_fraction(mut self, f: Option<f64>) -> Self {
        self.warn_fraction = f.and_then(|v| (v > 0.0 && v <= 1.0).then_some(v));
        self
    }
// helper:
    fn eval_warn(&self, u: &UsageSnapshot) -> Option<UsageWarning> {
        let thr = self.warn_fraction?;
        match (u.used, u.size) {
            (Some(used), Some(size)) if size > 0 && (used as f64 / size as f64) >= thr =>
                Some(UsageWarning { used, size, fraction: used as f64 / size as f64, threshold: thr }),
            _ => None,
        }
    }
    fn over_threshold(&self, u: &UsageSnapshot) -> Option<bool> {
        let thr = self.warn_fraction?;
        match (u.used, u.size) {
            (Some(used), Some(size)) if size > 0 => Some((used as f64 / size as f64) >= thr),
            _ => None, // unknown fraction
        }
    }
```
Add types:
```rust
#[derive(Clone, Debug, PartialEq)]
pub struct UsageWarning { pub used: u64, pub size: u64, pub fraction: f64, pub threshold: f64 }
// WarmTurn:
pub struct WarmTurn { pub backend: Arc<dyn AgentBackend>, pub session: SessionId,
                      pub usage_warning: Option<UsageWarning> }
// SessionStatusInfo:
    pub over_threshold: Option<bool>,
```

- [ ] **Step 3c: Set `usage_warning` on ALL `WarmTurn` sites** (FIX-9). In `checkout_turn`'s resume branch,
compute once from the carried handle usage and attach at the fast-resume return (`:124`) and the post-reconcile
return (`:190`): `usage_warning: self.eval_warn(&h.usage)`. At the **mint** return (`:229`):
`usage_warning: None`. Populate `over_threshold: self.over_threshold(&h.usage)` in `status()` (`:262`).

- [ ] **Step 3d: Serve wiring** (`main.rs:3659`): `...SessionManager::new(registry_for_sessions,
Duration::from_secs(warm_ttl)).with_warn_fraction(cfg.server.warm_usage_warn_fraction)`.

- [ ] **Step 3e: Surface the warn** (`server.rs`): in `warm_local_dispatch` (`:571`, where `turn` is in hand),
before building the `LocalDispatch`, log the serve observable:

```rust
            if let Some(w) = &turn.usage_warning {
                tracing::warn!(target: "a2a_bridge::usage", ctx = %ctx.as_str(),
                    used = w.used, size = w.size, fraction = w.fraction, threshold = w.threshold,
                    "usage_threshold_warn");
            }
```
and add `"overThreshold": s.over_threshold,` to the `session_status` `usage` block (`:2842`).

- [ ] **Step 4: Run + build** — `cargo test -p bridge-a2a-inbound --lib && cargo test -p a2a-bridge --lib warm_usage && cargo build --workspace` → PASS.
- [ ] **Step 5: Commit** — `feat(inbound): pre-task usage threshold warn (config + WarmTurn.usage_warning + overThreshold)`

---

## Task 8: Workspace gate + live-gate + merge

- [ ] **Step 1: Exhaustiveness + gate** — `cargo test --workspace --no-run` (catch every test-target `match`
break from `EventKind::Usage` / `TurnEvent::Usage` / `Update::Usage`); then
`cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

- [ ] **Step 2: Build release** — `cargo build --release -p a2a-bridge`.

- [ ] **Step 3: Live-gate (real codex; serve `examples/a2a-bridge.slice0-livegate.toml` + a low
`warm_usage_warn_fraction`, e.g. add `warm_usage_warn_fraction = 0.01` so turn-1 usage crosses it):**
  - **DoD-1 (usage visible):** `submit --context C --agent codex` (a real turn), then `session status C` shows
    `usage.used`/`size` (exact) and a `windowFraction` in (0,1).
  - **DoD-2 (end snapshot across turns):** a 2nd `submit --context C` with a larger prompt raises `usage.used`
    in a subsequent `session status C`.
  - **DoD-3 (pre-task warn, non-blocking):** with the low fraction, the 2nd checkout logs
    `usage_threshold_warn` on serve, `session status C` shows `usage.overThreshold == true`, and the turn
    STILL returns a normal reply.
  - **DoD-5 (no wire regression):** re-run a Slice-0 warm scenario and confirm the SSE frame sequence + the
    unary artifact are unchanged (no usage frame leaked).
  - **DoD-6 (no regression):** Slice-0/1 live scenarios (warm continue, isolation, release, idle reap,
    reconcile, caps in status) still green.
  - **DoD-4 (degrade)** is unit/fixture-gated (a non-ACP `UsageSnapshot{used:None,size:None,cost:Some}` →
    `windowFraction`/`overThreshold` null, no warn) — NOT live (all ACP agents always carry used+size).

- [ ] **Step 4: Record results** + `superpowers:finishing-a-development-branch` (merge to main; update memory).

---

## Self-review notes

- **Spec coverage:** un-drop map (T1), TurnEvent pipeline / FIX-1 (T2), translator Event / FIX-8 (T3),
  record_usage / FIX-7 (T4), wire-leak filter / FIX-2+FIX-3 (T5), status usage block / FIX-5-part-1 (T6),
  threshold warn + config + overThreshold / FIX-5/6/9 (T7), gate+live / DoD-1..6 (T8). FIX-4 (degrade reframe)
  = corpus assertions (T1) + DoD-4 wording (T8). FIX-10 (corpus shape) = T1. `OrchResult.usage` correctly
  ABSENT (no Slice-2 build site).
- **Type consistency:** `UsageSnapshot`/`UsageCost` (orch) used T1→T4→T6/T7; `TurnEvent::Usage` (T2);
  `EventKind::Usage`+`Event::usage`/`usage_snapshot` (T3) consumed T5; `record_usage`/`WarmTurn.usage_warning`/
  `UsageWarning`/`over_threshold` (T4/T7) consumed by server (T5/T6/T7).
- **Risk hotspots for the plan-review:** T2 (the 3-site `TurnEvent` widening — the load-bearing un-drop; prove
  at the prompt-stream layer, not just the mapper) and T5 (the wire-leak filter — exhaustive `sse.rs` +
  unconditional producer forward; DoD-5 byte-identical is the gate). The `event_to_sse → Option` change ripples
  to ~4 call sites — verify the SSE stream wrapper uses `filter_map`. These two warrant a code-quality pass.
- **Open for the plan-review:** (a) `TurnEvent::Usage(UsageSnapshot)` vs codex's more general
  `TurnEvent::Update(Update)` (chose minimal); (b) `event_to_sse → Option` (defensive) vs an `unreachable!`
  arm behind the producer filter (chose Option + a belt `unreachable!` in `event_to_streamresponse`).
