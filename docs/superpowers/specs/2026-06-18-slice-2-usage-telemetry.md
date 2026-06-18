# Slice 2 — Usage telemetry — design spec

**Status:** design (2026-06-18). Third orchestration slice. Governed by
`2026-06-17-orchestration-slicing.md` (Slice 2) over the converged architecture
(`2026-06-17-orchestration-architecture.md`: CORRECTION-2, P-4, UPDATE-MINIMAL). Builds on Slice 0
(`2026-06-17-slice-0-live-session-core.md`, shipped — `SessionManager` + warm Local dispatch + the minimal
`OrchEvent`/`OrchResult`/`UsageSnapshot` DTOs + the `Update::Usage` port variant) and Slice 1
(`2026-06-17-slice-1-config-reconcile.md`, shipped — `reconcile_config`, `AgentSessionCaps` in
`session/status`). ACP grounding: `docs/references/acp-protocol-v1.md`.

## Goal

Make a warm session's **context budget visible** so the orchestrator can decide keep / compact / clear /
release with real numbers. Today the bridge **receives** the ACP `usage_update` notification on every turn
and **drops it** (`map_session_update`, `acp_backend.rs:1628` returns `None` for non-text updates). This slice
plumbs it end-to-end: map `usage_update` → `Update::Usage`, surface it through the translator's event stream
to the warm `SessionManager` handle, snapshot it at task **start + end**, expose it in **`session/status`**,
and emit a configurable **pre-task threshold warn** (advisory — computed before the turn, never mid-turn,
never blocks).

This is the second half of the felt-pain ask (`docs/orchestration-improvements-2026-06-17.md` **A4** context-
budget awareness + **C4** per-run telemetry — the latter also feeds B2's future cost weighting). It is pure
plumbing of an already-received event onto Slice 0's already-shipped `UsageSnapshot` field; **no new wire
contract, no reset, no rich journal.**

## Decisions (settled by the architecture; carried in)

1. **Telemetry is FEASIBLE — plumbing only** (SPIKE B / CORRECTION-2). codex-acp 0.15.0 emits
   `{"sessionUpdate":"usage_update","used":N,"size":M}` — **token count + window size**, not cost-only. The
   SDK models it as `SessionUpdate::UsageUpdate(UsageUpdate{used,size,cost?})` (gated `unstable_session_usage`,
   **enabled** in `crates/bridge-acp/Cargo.toml`). No protocol fix needed.
2. **UPDATE-MINIMAL holds.** `Update` grows **only** the `Usage` variant Slice 0 already added — a single ACP
   turn can physically emit it. Plan / ToolCall / config / mode / commands stay journal-level for S6/S7.
3. **Per-backend degrade is automatic via `Option` fields** (premise CORRECTED by FIX-4). The pinned ACP SDK
   makes `UsageUpdate.used`/`size` **required `u64`**, so **every ACP agent — codex AND claude — yields
   `Some(used)+Some(size)`** (window fraction always computable on the ACP path; cost optional). The
   `Option`-None / cost-only degrade applies **only to non-ACP (api) backends** emitting `Update::Usage`
   directly. The threshold check still requires `used` AND `size` present and `size > 0`.
4. **The warn is advisory + pre-task only** (P-4 context). It is computed at `checkout_turn` from the
   *carried* (prior-turn-end) usage, **before** the prompt is sent; it never blocks and never fires mid-turn.
   `max_tokens` is its failure mode (window exhausted mid-turn) — the warn exists precisely to forewarn it.
5. **No new A2A wire frames in Slice 2.** Usage is surfaced via the **queryable `session/status`** (the
   orchestrator's budget-visibility surface) + a serve-side warn log; streaming usage to the A2A client and
   journaling it are deferred to S6/S7. This preserves the existing unary-artifact / SSE contract exactly.

## v2 — dual spec-review fixes folded (codex-xhigh + Opus, both `fix-then-ship`)

Both lenses converged (same two blockers, same file:line). These resolutions are BINDING and SUPERSEDE any
conflicting detail in the sections below. **No redesign** — the seam (widen `Event`) is endorsed; the gaps
were under-counted pipeline gates + a wrong degrade premise.

- **FIX-1 (BLOCKER, both) — the un-drop is a 3-site bridge-acp change, NOT "one branch."** Mapping
  `usage_update → Update::Usage` in `map_session_update` is necessary but **insufficient**: the live
  notification handler forwards ONLY `Update::Text` (`acp_backend.rs:973`, drops the rest), the per-turn
  channel `enum TurnEvent` has only `Text/Done/Failed` (`acp_backend.rs:211`), and the `prompt` stream's
  `unfold` maps only those back to `Update`s (`acp_backend.rs:~1838`). So a mapped `Update::Usage` dies inside
  bridge-acp and never reaches the translator → the entire downstream (§2/§3/§5/§6) is dead code. **Fix
  (folds into §1 / Scope IN):** widen the turn pipeline — (a) `TurnEvent` gains a **`Usage(UsageSnapshot)`**
  carrier (mirrors `Text`; codex's more general `TurnEvent::Update(Update)` is an accepted alternative — pick
  in the plan, default to the minimal `Usage` arm); (b) the handler (`:973`) routes a mapped
  `Some(Update::Usage(snap))` → `tx.send(TurnEvent::Usage(snap))` on the same session id, like `Text`;
  (c) the `unfold` (`:~1838`) yields `Ok(Update::Usage(snap))` **non-terminal** (`done=false`), like `Text`.
  **Prove it at the `TurnEvent` layer:** the pure-`map_session_update` unit test goes green while the live
  path fails — add a **`ScriptedUpdate::Usage`** variant to the scripted-update harness (`acp_backend.rs:~2952`)
  and a corpus assertion that a real `usage_update` frame reaches `Update::Usage` through the prompt stream
  (not just the mapper).
- **FIX-2 (MAJOR — no-regression blocker, both) — `EventKind::Usage` MUST be filtered before every A2A
  output channel; my "tolerant `_` arm covers it" claim is FALSE.** `sse.rs` `event_to_sse`/
  `event_to_streamresponse` (`sse.rs:46/115`) are **exhaustive, total** `match`es (no `_`) — adding the
  variant is a hard compile break AND any arm there is a wire mapping. The streaming producer forwards every
  event unconditionally (`server.rs:1177`); fan-out forwards all source events (`fanout.rs:126`) and the local
  fan-out source swallows only `Terminal` (`server.rs:1382`). **Fix:** explicitly drop `EventKind::Usage` at
  EVERY producer boundary **before** `tx.send` — streaming local: record-iff-warm then `continue` (no send);
  unary local: see FIX-3; local fan-out source: `EventKind::Usage => continue` beside the Terminal swallow;
  SSE converter: a defensive `Usage =>` no-frame arm (or make it return `Option`) WITH a unit test asserting it
  is never reached with `Usage`. DoD-5 (byte-identical wire) becomes a hard merge gate.
- **FIX-3 (MAJOR, both) — the unary path has NO event loop to "tap."** `unary_message` does
  `translator.run(...).collect().await` (`server.rs:~2319`) then `.find(Artifact)`/`.filter(Status)`.
  **Fix:** replace the blind `collect()` (or post-iterate the collected vec) to record `EventKind::Usage`
  **synchronously, iff warm, before** the response is built and before the `WarmTurnGuard` Drop runs
  `finish_turn` (the Drop is a detached `tokio::spawn`, `server.rs:457`); exclude usage events from the artifact
  accumulator. §3's "both producers tap in their event loop" is corrected to this asymmetric shape.
- **FIX-4 (MAJOR, codex — sharper than the spec) — degrade premise is wrong: ACP `usage_update` ALWAYS
  carries `used`+`size`.** The pinned SDK makes `UsageUpdate.used: u64` + `size: u64` **required** (only
  `cost` optional, `client.rs:271-279`); the real `claude-agent-acp.jsonl` corpus also carries `used`/`size`.
  So **every ACP agent (codex AND claude) yields `Some(used)+Some(size)`** → window fraction is always
  computable on the ACP path. The `Option`-None / **cost-only degrade applies ONLY to non-ACP (api) backends**
  emitting `Update::Usage` directly. **Fix:** delete the "claude → cost only" line; reframe degrade as
  "ACP → exact used+size (+optional cost); api/non-ACP → whatever it emits, Options absorb absence." **DoD-4**
  becomes a fake/non-ACP backend emitting `UsageSnapshot{used:None,size:None,cost:Some(..)}`, NOT an ACP
  cost-only `usage_update`. Add a **claude corpus** usage assertion too (the un-drop proven across both ACP
  agents).
- **FIX-5 (MINOR, both) — `overThreshold` = computed `Option<bool>`, never a stored flag.** Compute at
  status-assembly time from the CURRENT `usage` + `warn_fraction`: **`None`** when `windowFraction` is unknown
  OR no `warn_fraction` is configured; **`Some(false)`** below; **`Some(true)`** at/above. This disambiguates
  "unconfigured" from "can't-tell" from "under" for a polling orchestrator. No `last_warning` field on the
  handle.
- **FIX-6 (MINOR, codex) — plumb `warm_usage_warn_fraction` explicitly.** Today serve wires only
  `warm_idle_ttl_secs` into `SessionManager`; the new knob needs config parse + default + a `SessionManager`
  constructor parameter (alongside `idle_ttl`).
- **FIX-7 (MINOR, Opus) — drop the idle-refresh in `record_usage`.** Bumping `last_used` on usage races
  `reap_idle`'s read for ~no benefit (a turn's `finish_turn` already refreshes idle). `record_usage` updates
  ONLY `handle.usage` (latest-wins); it never touches `last_used` and never resurrects a removed handle.
- **FIX-8 (MINOR, Opus) — the §1 snippet won't compile + name the accessor.** The existing match consumes
  `notif.update` by value (`acp_backend.rs:1623`); the `&notif.update` usage branch placed "before the final
  `None`" reads a moved value. Match `notif.update` once (handle the usage arm in the same `match`/by value).
  Add `Event::usage_snapshot() -> Option<&UsageSnapshot>` (the accessor §2's test needs).
- **FIX-9 (MINOR, Opus) — set `usage_warning` on ALL `WarmTurn` construction sites.** `checkout_turn` returns
  on the fast in-place resume (`session_manager.rs:~124`), the post-reconcile path (`~190`), AND mint
  (`~229`). Compute the warn once from the carried `handle.usage` before the diff branch (so both resume
  returns carry it); mint sets `usage_warning: None` (no carried usage).
- **FIX-10 (MINOR, Opus/codex) — corpus assertions pin the shape.** Assert the codex corpus `usage_update`
  frame (`tests/corpus/codex-acp.jsonl`, `used`/`size`, no cost) maps to `cost == None`; assert the claude
  frame maps with `used`/`size` present.

**Scope/no-regression verdicts (both):** scope **in-bounds** (no reset/clear/compact/watchdog/`session/close`
creep) with the single creep FIX-4-adjacent removal (`OrchResult.usage`, below); **no-regression at risk until
FIX-2** (the wire-leak guard) is in; **live-gate provable after FIX-1**. Both: `SPEC VERDICT: fix-then-ship`.

## Findings (grounded in the code)

- **The drop is one branch.** `AcpBackend::map_session_update` (`acp_backend.rs:1622-1629`) maps only
  `SessionUpdate::AgentMessageChunk` (text) → `Update::Text`; **everything else, including
  `SessionUpdate::UsageUpdate`, falls through to `None`.** This is a **necessary** branch but **NOT the only
  gate** (corrected by FIX-1: the live handler `:973` forwards only `Update::Text`, `TurnEvent` `:211` has no
  usage carrier, and the `unfold` `:1838` maps only Text/Done/Failed — the turn pipeline must be widened too). The
  function is `#[must_use] pub fn` and is driven by the **corpus-replay conformance test** (`map_session_update`
  is the production mapping the corpus feeds real frames through — `acp_backend.rs:1615-1620`,
  `tests/corpus/codex-acp.jsonl` already contains a `usage_update` frame), so it must stay **pure** (no clock).
- **The port + DTOs already exist (Slice 0).** `Update::Usage(crate::orch::UsageSnapshot)` (`ports.rs:24`);
  `UsageSnapshot { used: Option<u64>, size: Option<u64>, cost: Option<UsageCost>, at_ms: i64 }` +
  `UsageCost { amount: f64, currency: String }` (`orch.rs:32-43`). The SDK shapes map **1:1**:
  `UsageUpdate.used: u64 → Some`, `.size: u64 → Some`, `.cost: Option<Cost{amount,currency}> →
  Option<UsageCost>`. `OrchResult.usage` (`orch.rs:98`) is already typed `UsageSnapshot`.
- **Usage is currently ignored by both downstream consumers:** the domain translator
  `Ok(Update::Usage(_)) => { continue; }` (`translator.rs:157`) and the workflow executor
  `Some(Ok(Update::Usage(_))) => { /* Slice 0: ignore */ }` (`executor.rs:143`). Slice 2 changes the
  translator (warm Local path); the executor stays ignoring (executor keep-warm is S5).
- **The recording site must be the inbound producer, not the translator.** `Translator::run(backend,…)`
  (`bridge-core`) OWNS the `backend.prompt()` stream and yields `Event`s; it has **no** `SessionManager`
  (which lives in `bridge-a2a-inbound`). The producers that drive the translator — `spawn_local_producer`
  (streaming, `server.rs:1102`) and `unary_message` (unary, `server.rs:2266`) — hold the
  `WarmTurnGuard { sm, ctx }` (`server.rs:452`, present **only** on the warm path). So usage must ride the
  translator's `Event` out to the producer, which records it on the handle.
- **`WarmHandle`/`SessionStatusInfo` have no usage today.** `WarmHandle` (`session_manager.rs:31-48`) and
  `SessionStatusInfo` (`session_manager.rs:57-63`) carry state/agent/generation/idle/caps but no usage;
  Slice 0's status spec explicitly noted "usage added Slice 2". The `session/status` JSON
  (`server.rs:2842-2858`) assembles `{contextId,state,agent,generation,idleAgeMs,capabilities}`.
- **`finish_turn` is the natural end-snapshot boundary.** `WarmTurnGuard::Drop` → `sm.finish_turn(ctx)`
  (`server.rs:457-465`, `session_manager.rs:254`) already runs on producer exit. The last `usage_update`
  recorded during the turn IS the end snapshot (no separate end call needed — latest-wins covers it).

## Architecture

### 1. Map `usage_update` → `Update::Usage` (bridge-acp, the un-drop)

In `map_session_update` add a branch (kept **pure** — `at_ms` is stamped downstream, see §4). **Per FIX-1 this
branch alone does not surface usage** — the bridge-acp `TurnEvent` pipeline (`:211`/`:973`/`:1838`) must be
widened in lockstep. **Per FIX-8, fold this into the single by-value `match notif.update`** (the existing
`AgentMessageChunk` arm consumes `notif.update`; a separate `&notif.update` if-let after it won't compile):

```rust
// acp_backend.rs, inside map_session_update — additive, BEFORE the final `None`.
#[cfg(feature = "unstable_session_usage")]
if let SessionUpdate::UsageUpdate(u) = &notif.update {
    return Some(Update::Usage(UsageSnapshot {
        used: Some(u.used),
        size: Some(u.size),
        cost: u.cost.as_ref().map(|c| UsageCost { amount: c.amount, currency: c.currency.clone() }),
        at_ms: 0, // placeholder — stamped at the recording site (keeps this fn clock-free for corpus replay)
    }));
}
```

`unstable_session_usage` is already enabled, so the `#[cfg]` is satisfied in this build; gating it keeps the
mapping honest if the feature is ever dropped. Corpus replay continues to assert the existing text mapping and
gains a usage-frame assertion.

### 2. Surface usage through the translator `Event` (bridge-core)

`Event` (`translator.rs:39`) is `{ kind, text, source, outcome }`. Add a typed usage payload + an
`EventKind::Usage`, and stop dropping:

```rust
// translator.rs — additive; all existing Event constructors default usage = None.
pub struct Event { kind: EventKind, text: String, source: Option<String>, outcome: Option<TaskOutcome>,
                   usage: Option<UsageSnapshot> }   // NEW field
// EventKind gains `Usage`. Event::usage(snap) constructor sets kind=Usage + usage=Some(snap).
// In Translator::run:
Ok(Update::Usage(snap)) => { yield Event::usage(snap); }   // was: continue;
```

A `Usage` event carries **no artifact/status text** and is **not** a terminal — it is a side-band telemetry
event. Existing consumers that match on `EventKind` must treat `Usage` as a no-op for output. **CORRECTED by
FIX-2:** there is NO tolerant `_` arm to rely on — `sse.rs` (`:46/115`) is an exhaustive total converter and
the producers forward unconditionally, so `Usage` MUST be explicitly filtered before every A2A output channel
(see FIX-2 / §3); otherwise it is a compile break and/or a wire leak.

> **Design note (for review):** alternative considered — thread a usage sink (channel/closure) into
> `Translator::run` instead of widening `Event`. Rejected: it couples `bridge-core` to an inbound callback and
> duplicates the event pipeline the translator already owns. Widening `Event` with one `Option` field (additive,
> all constructors default `None`) is the smaller, idiomatic change and matches how the translator already
> projects backend `Update`s into `Event`s.

### 3. Record on the handle + keep it off the A2A wire (bridge-a2a-inbound)

`SessionManager` gains a recorder; `WarmHandle` gains a `usage` field:

```rust
// session_manager.rs
struct WarmHandle { /* … */ usage: UsageSnapshot }   // default-empty at mint

pub async fn record_usage(&self, ctx: &ContextId, mut snap: UsageSnapshot) {
    snap.at_ms = now_epoch_ms();                      // stamp here (inbound has a wall clock)
    if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
        h.usage = snap;                               // latest-wins (end snapshot = last update of the turn)
        // FIX-7: do NOT touch h.last_used — usage during a turn is covered by Running + finish_turn's
        // refresh; bumping it here only races reap_idle for no benefit.
    }
}
```

Both producers tap `EventKind::Usage` in their event loop: **if a `warm` guard is present**, call
`w.sm.record_usage(&w.ctx, snap)`; in **all** cases **do not forward** the usage event to `tx` (streaming SSE)
or into the unary artifact accumulator — Slice 2 keeps usage internal (status-queryable), preserving the A2A
contract. Non-warm / workflow / fan-out / delegate paths simply have no `warm` guard → the event is consumed
and dropped (unchanged external behavior).

- **Start snapshot** = `handle.usage` *before* this turn (the carried prior-turn-end value), read at
  `checkout_turn`. **End snapshot** = `handle.usage` after the turn (the last `usage_update` recorded). No
  separate start/end calls — the carried field + latest-wins gives both, and `session/status` reads the
  current value any time.

### 4. `at_ms` stamping

`map_session_update` stays clock-free (corpus-replay determinism) and emits `at_ms: 0`; **`record_usage`
stamps `at_ms = now_epoch_ms()`** (the inbound layer has wall-clock; `SessionManager.now` is a monotonic
`Instant` used for idle math, so a separate epoch-ms helper is used for `at_ms`). `OrchResult.usage.at_ms`
likewise gets stamped where the result is built.

### 5. `session/status` exposure (bridge-a2a-inbound)

`SessionStatusInfo` (`session_manager.rs:57`) gains `usage: UsageSnapshot` + a derived
`usage_window_fraction: Option<f64>` (`used/size` when both present and `size>0`, else `None`). The
`session/status` JSON (`server.rs:2842`) gains a `usage` block (additive — existing fields unchanged):

```json
"usage": {
  "used": 14584, "size": 258400, "windowFraction": 0.0565,
  "cost": { "amount": 0.12, "currency": "USD" },   // omitted when absent
  "atMs": 1718700000000,
  "overThreshold": false                            // null when fraction is unknown
}
```

`windowFraction`/`cost`/`overThreshold` are `null`/omitted under degrade (claude cost-only → `used/size` null →
`windowFraction` null; api → all null). CLI `session status <contextId>` renders the block.

### 6. Pre-task threshold warn (advisory, non-blocking)

- **Config:** `[server]` (or `[sessions]`) gains `warm_usage_warn_fraction: Option<f64>` (e.g. `0.85`).
  Absent / `0` / out-of-(0,1] → disabled. Conservative default **disabled** (opt-in) to avoid noise; the
  live-gate sets it low to demonstrate a crossing. Plumbed onto `SessionManager` alongside `idle_ttl`.
- **Mechanism (level-triggered, pre-task):** in `checkout_turn`, on a **resume** (known contextId), after the
  fingerprint/reconcile checks pass and **before** returning the `WarmTurn`, read the *carried* `handle.usage`;
  if `warn_fraction` is set and `used/size >= warn_fraction`, build a `UsageWarning { used, size, fraction,
  threshold }`. `checkout_turn` returns it on `WarmTurn`:

  ```rust
  pub struct WarmTurn { pub backend: Arc<dyn AgentBackend>, pub session: SessionId,
                        pub usage_warning: Option<UsageWarning> }   // NEW
  ```

- **Surface (no new wire frame):** when `usage_warning` is `Some`, the producer (a) logs a structured serve
  WARN line (`usage_threshold_warn ctx=… used=… size=… fraction=… threshold=…`) and (b) the handle records
  `last_warning`, reflected in `session/status` as `usage.overThreshold = true`. The orchestrator polling
  `session/status` sees the elevated `windowFraction` + `overThreshold`; the serve log gives the live-gate its
  observable. Mint (fresh contextId) never warns (no carried usage). The warn is recomputed each checkout from
  the latest carried usage (level-triggered) — simple and matches "pre-task."

## Scope

**IN:**
- `map_session_update` maps `SessionUpdate::UsageUpdate` → `Update::Usage(UsageSnapshot)` (clock-free, folded
  into the single by-value `match`, `unstable_session_usage`-gated); codex + claude corpus-replay gain
  usage-frame assertions (FIX-4/FIX-10).
- **(FIX-1) Widen the bridge-acp turn pipeline:** `TurnEvent::Usage(UsageSnapshot)` carrier (`:211`); handler
  routes mapped usage → `TurnEvent::Usage` (`:973`); `unfold` yields `Ok(Update::Usage)` non-terminal
  (`:1838`); `ScriptedUpdate::Usage` harness variant (`:~2952`) + a prompt-stream assertion proving the un-drop
  end-to-end (not just the pure mapper).
- `Event` gains `usage: Option<UsageSnapshot>` + `EventKind::Usage` + `Event::usage(..)` + accessor
  `Event::usage_snapshot()` (FIX-8); `Translator::run` emits it instead of dropping.
- **(FIX-2) Explicit `EventKind::Usage` filter at every A2A output boundary:** streaming local producer +
  unary local (FIX-3) + local fan-out source (`server.rs:1382`) drop/record-before-send; SSE converter
  (`sse.rs:46/115`) gets a defensive no-frame arm. Unit/integration gates prove no SSE frame / unchanged
  artifact (DoD-5).
- `WarmHandle.usage` + `SessionManager::record_usage` (latest-wins, `at_ms` stamp, idle refresh); both
  producers tap `EventKind::Usage` (record iff warm; never forward to A2A output).
- `SessionStatusInfo.usage` + derived `windowFraction`; `session/status` JSON `usage` block; CLI render.
- `warm_usage_warn_fraction` config; `WarmTurn.usage_warning` + `UsageWarning`; pre-task evaluation in
  `checkout_turn`; serve WARN log + `usage.overThreshold` in status.
- ~~`OrchResult.usage` populated where an `OrchResult` is built~~ **REMOVED (FIX-4/M2): `OrchResult` has no
  production construction site in Slice 2 (first producer is S5/S6); usage is surfaced via `session/status`
  only.** Deferred to the slice that builds an `OrchResult`.

**OUT (later slices):** clear/reset (S3) + compact (S4); `run-workflow --serve --context` + executor
keep-warm/usage (S5); streaming usage to the A2A client + the rich journal / `OrchEvent` Usage on the wire +
4-path rewrite (S6/S7); E9 watchdog (S7); MCP surface (S8); Turn Channel (S9). **No** auto-compaction on
threshold (manual levers only — the orchestrator reacts to the warn). **No** mid-turn warn.

## Definition of Done + LIVE-GATE (real serve + real codex)

1. **Usage visible:** a `submit --context C --agent codex` turn, then `session status C`, shows a `usage` block
   with **exact** `used`/`size` and a `windowFraction` in (0,1) — proving the previously-dropped `usage_update`
   is now plumbed. (codex emits `used`+`size`.)
2. **End snapshot updates across turns:** a 2nd `submit --context C` (longer context) raises `used` in a
   subsequent `session status C` (latest-wins; carried across the warm session).
3. **Threshold warn (pre-task):** with `warm_usage_warn_fraction` set low enough to be crossed by turn 1's
   usage, the **2nd** checkout on C logs `usage_threshold_warn …` on serve and `session status C` shows
   `usage.overThreshold = true` — and the turn **still runs** (advisory, non-blocking).
4. **Degrade (unit/fixture-gated; reframed per FIX-4):** a **non-ACP/fake** backend emitting
   `Update::Usage(UsageSnapshot{used:None,size:None,cost:Some(..)})` → `windowFraction` null, `cost` present,
   `overThreshold` null, no spurious warn; an api backend emitting nothing → empty `usage`. (ALL ACP agents —
   codex AND claude — always carry `used`+`size`, so cost-only is NOT an ACP `usage_update`; it is only
   reachable via a non-ACP backend or a fixture. The live codex gate always shows exact used+size.)
5. **No wire regression:** the unary artifact text and the SSE frame sequence for a normal turn are **byte-for-
   byte unchanged** vs Slice 1 (usage events are recorded, never forwarded). Slice 0/1 DoD (warm continue,
   isolation, release, idle reap, reconcile, caps in status) all still green.
6. **Purity preserved:** `map_session_update` remains clock-free (corpus replay deterministic; `at_ms` stamped
   only at `record_usage`).

## Risks

- **`Event` widening blast radius (primary):** adding a field + variant to the core `Event` touches every
  `match EventKind` site. Mitigate: additive field defaulting `None`; audit all `EventKind` matches
  (translator, both producers, fan-out synth, delegate, workflow projection) for an explicit or `_` arm so
  `Usage` compiles + is a no-op for output. `cargo test --workspace --no-run` (`--all-targets`) catches the
  match-exhaustiveness breaks `cargo build` misses.
- **Usage leaking onto the A2A wire:** if a producer forwards a `Usage` event to `tx`/artifact, the client
  contract changes (DoD-5 regression). Gate: a unary test asserting the artifact equals the agent text with
  usage events present; an SSE test asserting no extra frame.
- **`at_ms` / clock purity:** stamping in `map_session_update` would break corpus determinism. Keep it at
  `record_usage`; assert the map emits `at_ms: 0` and the recorder overwrites it.
- **Latest-wins vs multi-`usage_update`:** codex emits several per turn as context grows; the LAST is the end
  snapshot — latest-wins is correct, but assert ordering isn't reversed (record in stream order).
- **Threshold false-positive under degrade:** a cost-only backend has no `size` → the warn MUST NOT fire
  (no fraction). Unit-assert the guard (`used.is_some() && size.is_some() && size>0`).
- **Idle-refresh-on-usage:** `record_usage` touching `last_used` is deliberate (usage = liveness) but must not
  resurrect a `Released`/reaping handle; only mutate an existing live handle (the `get_mut` already no-ops a
  removed handle).

## Testing approach

- **Unit (bridge-acp):** `map_session_update(UsageUpdate{used,size,cost})` → `Some(Update::Usage(snapshot))`
  with `used/size/cost` populated + `at_ms == 0`; non-usage updates still map as before. **Corpus replay**
  asserts the captured `usage_update` frame produces a `Usage` update (deserialize-then-MAP, not -drop).
- **Unit (bridge-core):** `Translator` emits `EventKind::Usage` carrying the snapshot (was dropped); a usage
  event contributes **no** text to the coalesced artifact; `Event::usage` round-trips the payload.
- **Unit (bridge-a2a-inbound / SessionManager):** `record_usage` sets `handle.usage` (latest-wins across two
  calls), stamps `at_ms`, refreshes idle, no-ops a missing ctx; `status().usage` + `windowFraction` (exact;
  `None` when `size` absent); `checkout_turn` returns `usage_warning` when carried `used/size >= fraction` and
  `None` when below / when `size` absent / when disabled; mint never warns.
- **Integration (in-crate, mocked backend emitting `Update::Usage`):** warm turn records usage → `session/status`
  shows it; a 2nd turn raises `used`; usage events are NOT in the SSE frame list / unary artifact; threshold
  crossing sets `overThreshold` + (capture) the warn log; non-warm path drops usage with no panic.
- **Live-gate (real serve + codex):** DoD 1-3, 5 via `submit --context C --effort …` + `session status C` +
  a serve-log grep for `usage_threshold_warn`; DoD-4 degrade via unit/fixture.

## Constraints (carried)

codex gpt-5.5/high implementor (host, via `run-workflow slice0-impl`-style; controller verifies + commits —
the `_dyld_start` flake); codex high-risk/final + Opus arch review; `max_attempts=3`; reviewers judge
**intent, not verbatim**. **Dual spec-review (codex xhigh + Opus) before planning** + **dual plan-review** +
**LIVE-GATED** before merge.
