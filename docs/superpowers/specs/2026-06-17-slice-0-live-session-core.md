# Slice 0 — Live Session Core (warm continue) — design spec

**Status:** design (2026-06-17). First slice of the orchestration roadmap. Governed by
`2026-06-17-orchestration-slicing.md` (Slice 0) over the converged architecture
`2026-06-17-orchestration-architecture.md` (S1 Session Resource + the minimal S2 types). ACP grounding:
`docs/references/acp-protocol-v1.md`.

## Goal

Make a bridge-driven agent **warm across tasks**: a second A2A message on the same `contextId` **reuses the
same warm ACP session** (process + context intact — no cold spawn, no re-read), while a message with **no
`contextId` behaves exactly as today** (forget-after). This is the felt-pain latency win (≈27s cold-start →
sub-second), delivered on the A2A `message/send` path, with a **minimal, real (non-throwaway)** result/event
schema that later slices extend additively.

This slice deliberately ships the warm SessionManager + the minimal DTOs **only** — NOT the rich journal,
NOT the 4-path event rewrite, NOT reconcile/clear/compact/telemetry (those are Slices 1–4/6/7).

## Decisions (from the 2026-06-17 brainstorm)

1. **Warm trigger = `contextId` presence.** Any `message/send` carrying a `contextId` keeps its session warm
   until TTL/idle/`release`. No `contextId` → today's per-task forget-after (back-compat, opt-in to warmth).
2. **Identity = `contextId` only.** The A2A `contextId` the client generates is the public key; the internal
   `SessionHandleId` is not surfaced. Nothing new is returned for the client to track.
3. **Status/release = new JSON-RPC methods** `session/status` + `session/release` (+ CLI `session
   status|release <contextId>`); reuse A2A `tasks/cancel` for in-flight turn cancel.

## Findings (grounded in the code)

- **Inbound `contextId` EXISTS and is ignored today.** `a2a-lf-0.3.0` `Message` carries
  `context_id: Option<String>` (wire `contextId`, `#[serde(default)]`, `types.rs:318`). `gate()` parses
  `params` as a raw `Value` and derives `session = "session-{task}"` (`server.rs:348`), **never reading
  `contextId`**. SSE uses task-id AS contextId (`server.rs:661`). → reading `params.message.contextId` is
  cheap new wiring; the `metadata["a2a-bridge.context"]` channel (`server.rs:2933`) is a documented backup.
- **The warm primitive already exists.** `AcpBackend.sessions` multiplexes many ACP sessions over one warm
  connection (`acp_backend.rs:337`), lazy-mints once via `ensure_session` (`:1184`), serializes turns with
  `turn_lock` (`:1578`); the registry `OnceCell` keeps the process warm for `serve`'s lifetime
  (`registry.rs:39`). SPIKE A proved a fresh `session/new` on a live connection is sub-second. **A1 ≈ done.**
- **`forget_session` only drops the config stash** (`acp_backend.rs:1805/1810`) — it does NOT remove
  `sessions[id]`. Per-session `OnceCell`s `agent_id`/`minted_cwd` (`:266/269/277`) are non-resettable. So
  a warm session must NOT be forgotten (keep it live) and a real `release_session` (removing BOTH
  `session_cfg` AND `sessions[id]`) is needed for eviction.
- **The registry lease pins the backend.** `Resolved { entry, backend, lease }` (`ports.rs:132`); registry
  retirement drains on leases (`registry.rs:248`). Holding the lease in the warm record keeps the backend
  alive for free.
- **Today's teardown is binding-scoped.** The inbound `TaskBinding` drop calls `forget_session` (memory
  `server.rs:78`); the executor forgets per node (`executor.rs:152`). Warm = don't tear down the
  binding/session for a contextId-bearing message.

## Architecture — `SessionManager` (serve-side, in-memory)

A new serve-side component, **sibling to the registry and `TaskStore`** (NOT in `TaskStore`, NOT keyed by
task id). It owns the warm-session table + the backend lease; it is consulted by `gate()`/dispatch to map a
`contextId` to a warm `SessionId` instead of minting `session-{task}`.

```rust
// crates/bridge-a2a-inbound/src/session_manager.rs  (new; serve-side — holds tokio + leases)
pub struct SessionManager {
    registry: Arc<dyn AgentRegistry>,
    by_context: Mutex<HashMap<ContextId, SessionHandleId>>,
    by_handle:  Mutex<HashMap<SessionHandleId, Arc<WarmHandle>>>,
    idle_ttl: Duration,            // configurable default (see Config)
}

pub struct WarmHandle {
    pub id: SessionHandleId,       // internal stable id (not surfaced)
    pub context_id: ContextId,
    pub agent: AgentId,
    pub backend: Arc<dyn AgentBackend>,
    lease: Box<dyn Lease>,         // pins the registry slot's backend warm
    pub backend_session: SessionId,// the key into AcpBackend.sessions
    pub generation: SessionGeneration, // 0 in Slice 0; bumped by reset in Slice 3
    pub config_fingerprint: EffectiveConfigFingerprint, // frozen-at-mint; mismatch => typed error
    pub state: Mutex<SessionState>,
    last_used: Mutex<Instant>,
}

pub enum SessionState { Idle, Running { op: OperationId }, Releasing }
```

### Lifecycle (Slice 0 subset)
`mint(contextId, spec)` → `Idle` → `Running{op}` (a turn) → `Idle` (turn end, **no forget**) → `Released`
(TTL/idle reap OR manual `session/release`). On release: remove both maps, `backend.release_session`, drop
the lease → registry retirement reaps the backend (and any `:ro`/`:rw` container via the existing reaper).
`Resetting`/reconcile/compact are explicitly out of Slice 0.

## Data flow

1. **`message/send` arrives.** `gate()` parses `contextId` from `params.message.contextId` (fallback
   `metadata["a2a-bridge.context"]`).
2. **No contextId:** unchanged — derive `session-{task}`, today's forget-after path (back-compat).
3. **New contextId:** `SessionManager.mint(contextId, spec)` → resolve the agent (hold the lease), allocate
   `SessionHandleId` + `backend_session = SessionId("ctx-{contextId}-g0")`, stash config fingerprint, insert
   maps. Dispatch the turn against `backend_session`; on turn end, **keep warm** (no forget).
4. **Known contextId:** look up the `WarmHandle`; verify the effective-config fingerprint
   (**mismatch → typed `ConfigMismatch` error** telling the caller this needs reconcile/clear — Slice 1/3,
   NOT a silent reset); dispatch against the existing `backend_session` (context intact).
5. **`session/status {contextId}`:** return `{ state, agent, generation, idle_age_ms }` (usage added Slice 2).
6. **`session/release {contextId}`:** evict (above). **`tasks/cancel`:** cancel the in-flight turn
   (`backend.cancel(backend_session)`), state → `Idle`.

## Minimal types (S2 subset — bridge-owned DTOs, Ser+De, versioned)

Added to `bridge-core` (new `orch` module + `ids.rs` newtypes). Bridge-owned, NOT raw SDK enums.

```rust
// ids.rs (macro newtypes)
SessionHandleId(String);  OperationId(String);  ContextId(String);   // string newtypes
pub struct SessionGeneration(pub u64);

// orch.rs (minimal — only what a single warm turn emits; rich variants are Slice 6/7)
pub const ORCH_V: u16 = 1;
#[derive(Serialize, Deserialize)] pub struct UsageSnapshot { pub used: Option<u64>, pub size: Option<u64>, pub cost_usd: Option<f64>, pub at_ms: i64 }
#[derive(Serialize, Deserialize)] pub struct OrchEvent { pub v: u16, pub seq: i64, pub ts_ms: i64, pub operation_id: OperationId, #[serde(flatten)] pub kind: OrchEventKind }
#[derive(Serialize, Deserialize)] #[serde(tag="kind", rename_all="snake_case")]
pub enum OrchEventKind { Progress { text: String }, Usage(UsageSnapshot), Terminal { status: TerminalStatus } }
#[derive(Serialize, Deserialize)] #[serde(tag="status", rename_all="snake_case")]
pub enum TerminalStatus { Completed, Failed { reason: String }, Canceled }     // from StopReason: end_turn->Completed, cancelled->Canceled, refusal/max_*->Failed, unknown->Failed
#[derive(Serialize, Deserialize)] pub struct OrchResult { pub v: u16, pub operation_id: OperationId, pub status: TerminalStatus, pub wall_clock_ms: u64, pub usage: UsageSnapshot, pub output: String }
```

- **`Update::Usage(UsageSnapshot)`** is added to `ports.rs` (UPDATE-MINIMAL: a single-turn emission). The
  `map_session_update` *plumbing* to populate it is **Slice 2** — Slice 0 only adds the variant + the DTO so
  the schema is real-from-day-one. `Usage` is mapped to `None`/dropped at dispatch until Slice 2.
- **Versioned + `#[serde(flatten)] kind`** → later variants (Plan/ToolCall/…) are additive.

## Backend contract change (all backends)

```rust
// ports.rs — AgentBackend (additive; default delegates to forget_session so non-warm paths are unchanged)
async fn release_session(&self, session: &SessionId) { self.forget_session(session).await; }
```
- **`AcpBackend::release_session`** removes BOTH `session_cfg[id]` AND `sessions[id]` (today's
  `forget_session` removes only the former, `acp_backend.rs:1810`).
- **`ContainerRwBackend::release_session`** must reap the warm container (else it leaks on release).
- **`bridge-api` (kind=api)** implements the default (no per-session process) — confirm no leak.

## SEQ-AUTHORITY (enforced now, mechanism not just assertion)

A `contextId` is stamped by exactly one authority. Slice 0 enforces the mutual-exclusion **mechanism**:
- `SessionManager.mint` **refuses** (`HandleBusy`) a `contextId` that already has a `Working` detached task.
- The detached `submit` path **refuses** a `contextId` that has a live warm handle.

Warm-turn `OrchEvent`s in Slice 0 are stamped by `SessionManager` (a per-handle seq counter); detached
workflow streams keep TaskStore stamping (untouched). The full journal unification is Slice 6.

## Config

`[server]` (or `[sessions]`) gains `warm_idle_ttl_secs` (default **1800** = 30 min) + an optional
`warm_max_secs` hard cap. Conservative defaults; documented. No auto-compaction (deferred).

## Scope

**IN:** contextId routing in `gate()`; `SessionManager` (mint/continue/status/release/cancel, lease
ownership, TTL/idle reap, frozen config fingerprint + typed `ConfigMismatch`); `release_session` on
ACP+ContainerRw+API; minimal `OrchEvent`/`OrchResult`/ids/`UsageSnapshot` DTOs + `Update::Usage` variant;
`session/status`+`session/release` JSON-RPC + CLI; `submit --context` for the live-gate; SEQ-AUTHORITY
mutual-exclusion guard.

**OUT (later slices):** `reconcile_config` (S1); usage plumbing/telemetry/threshold (S2); reset/clear (S3);
compact (S4); `run-workflow --serve --context` + executor keep-warm policy (S5); the 4-path journal rewrite
+ dual-store + rich Plan/ToolCall variants (S6/7); MCP surface (S8); Turn Channel/permission (S9).

## Definition of Done + LIVE-GATE (real serve + real codex)

1. **Continuation:** two `submit --context C` calls to one `serve` → the 2nd **recalls** a codeword set in
   the 1st (context intact; the agent does NOT re-read) — proven via a "remember X" probe.
2. **Latency:** the 2nd call pays **no cold codex-acp spawn + ACP handshake** (cold 1st ≈ tens of seconds;
   warm 2nd sub-second to first token, modulo model time).
3. **Isolation:** distinct `contextId`s get distinct warm sessions; no cross-talk.
4. **Back-compat:** no `contextId` → unchanged per-task forget-after behavior.
5. **Status/release:** `session/status C` shows `Idle/Running`; `session/release C` evicts → **reaper to 0**
   (no leaked codex-acp process / `:rw` container); a subsequent `--context C` mints fresh (cold).
6. **Config-mismatch:** a `continue` on C with a different model/effort → typed `ConfigMismatch` (NOT a
   silent reset, NOT a silent drop).
7. **No leak across release/TTL:** the registry lease/reaper invariants hold; idle TTL reaps a warm session.

## Risks

- **contextId wire:** confirm `params.message.contextId` deserializes on the actual inbound `message/send`
  (the field exists in `a2a-lf-0.3.0`; add the metadata fallback). Pin with a parse test.
- **Lease lifetime:** a held warm lease must not deadlock config-reconcile retirement — verify the existing
  drain (`registry.rs:248`) tolerates a long-held lease (it blocks reconcile of THAT agent until release —
  acceptable; document).
- **Container backend release:** `ContainerRwBackend` warm mode has its own cache/retire
  (`bridge-container/src/lib.rs:410`) — `release_session` must reap, gated by a `docker ps`/reaper→0 check.
- **SEQ-AUTHORITY guard:** the un-aliasing of contextId from task id must not break the existing
  `session-{task}` no-contextId path (back-compat test).
- **Cancel vs keep-warm:** `tasks/cancel` must cancel the turn but KEEP the warm session (state→Idle), not
  evict it — distinct from `release`.

## Testing approach

- **Unit:** `OrchEvent`/`OrchResult` ser/de round-trip + version field; `TerminalStatus` from each
  `StopReason` (incl. `unknown`→Failed); `SessionManager` mint/lookup/fingerprint-mismatch/TTL-eviction
  (with a fake backend + fake clock); SEQ-AUTHORITY guard (handle-create refused on Working task & vice
  versa); `release_session` default-delegates to `forget_session` for non-warm backends.
- **Integration (in-crate, mocked backend):** `gate()` reads contextId; new contextId mints, known resumes,
  no contextId = legacy path; `session/status`/`session/release` methods.
- **Live-gate (real serve + codex):** the DoD 1–7 scenarios via `submit --context` + a `docker ps`/`pgrep`
  watcher for the reaper→0 and warm-process-reuse assertions (the SPIKE-A / B2b-3c watcher pattern).

## Constraints (carried)

sonnet implementor; codex high-risk + final, Opus arch; `max_attempts=3`; reviewers judge **intent, not
verbatim**. **Dual spec-review (codex xhigh + Opus) before planning**; **LIVE-GATED before merge.**
