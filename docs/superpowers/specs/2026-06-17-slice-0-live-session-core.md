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
3. **Status/release/cancel = new JSON-RPC methods** `session/status` + `session/release` + `session/cancel`
   (+ CLI `session status|release|cancel <contextId>`), all keyed by `contextId`. (A2A `tasks/cancel` is
   task-id-keyed and the server ignores the standard `id` field today — `server.rs:2867` reads
   `taskId`/`task_id`; a contextId-native `session/cancel` is cleaner than retrofitting cancel onto a task id.)

> **v2 (2026-06-17): dual spec-review fixes folded.** codex-xhigh + Opus both `fix-then-ship` (no redesign).
> Folded: release does NOT reap the shared ACP backend (only evicts the bridge session; only ContainerRw
> reaps its per-session container); held lease force-retires after grace → typed `SessionExpired`; cost is
> `{amount,currency}` not USD; fingerprint includes cwd + agent; the warm path must suppress the existing
> per-task `BindingGuard` forget (guard=None); SessionManager wiring (gate parses → dispatch consults);
> SEQ-AUTHORITY scoped to SessionManager-internal (TaskRecord has no contextId) + reject contextId on
> detached routes; submit must drive the **Local** route (not a workflow skill); split live-gates.

## Findings (grounded in the code)

- **Inbound `contextId` EXISTS and is ignored today.** `a2a-lf-0.3.0` `Message` carries
  `context_id: Option<String>` (wire `contextId`, `#[serde(default, skip_serializing_if)]`, `types.rs:318`).
  `gate()` parses `params` as a raw `Value` and derives `session = "session-{task}"` (`server.rs:348`),
  **never reading `contextId`**. SSE uses task-id AS contextId (`server.rs:660`). → reading
  `params.message.contextId` is cheap new wiring; the `metadata["a2a-bridge.context"]` channel is a **NEW
  compatibility fallback** (current metadata parsing covers skill/agent/model/effort/mode `server.rs:2924` +
  cwd `:2889`, NOT context) — add a `context_id_from_params` parse test.
- **The warm primitive already exists.** `AcpBackend.sessions` (the field decl `acp_backend.rs:341`)
  multiplexes many ACP sessions over one warm connection, lazy-mints once via `ensure_session` (`:1184`),
  serializes turns with `turn_lock` (`:282`); the registry `OnceCell` keeps the process warm for `serve`'s
  lifetime (`registry.rs:39`). SPIKE A proved a fresh `session/new` on a live connection is sub-second.
  **A1 ≈ done.**
- **`forget_session` only drops the config stash** (`acp_backend.rs:1810`) — it does NOT remove
  `sessions[id]`. Per-session `OnceCell`s `agent_id`/`minted_cwd` (`:269/277`) are non-resettable. So a warm
  session must NOT be forgotten (keep it live) and a real `release_session` (removing BOTH `session_cfg` AND
  `sessions[id]`) is needed for eviction.
- **The registry lease keeps the backend alive — but is NOT a hard pin, and release does NOT reap it.**
  `Resolved { entry, backend, lease }` (`ports.rs:132`). The lease only increments a counter
  (`crates/bridge-registry/src/registry.rs:73`); **retirement runs ONLY for removed/replaced slots during
  `apply()`** (`registry.rs:403`) and **force-retires after a grace deadline even with leases held**
  (`registry.rs:248`, asserted by the test at `:1146`). The shared `AcpBackend` PROCESS is retired only by
  `retire()` (`acp_backend.rs:1816`). **CONSEQUENCE (codex BLOCKER):** the shared codex-acp process is warm
  for serve's whole lifetime and is shared across all sessions/agents — `session/release` must **evict the
  SessionManager record + `backend.release_session(id)`** (drop the bridge session), and must **NOT** reap
  the shared backend process. Only `ContainerRwBackend` has a **per-session** container to reap.
- **Today's Local path ALREADY warms per-task via `TaskBinding`** (`server.rs:67`, keyed by `TaskId`, holds
  `backend+eff+lease`), evicted by `BindingGuard::Drop`→`forget_session` (`:96-114`). **CONSEQUENCE (Opus
  M2):** SessionManager introduces a SECOND, contextId-keyed lease+warmth owner. A contextId-bearing Local
  message MUST be dispatched with **`guard = None`** (no `BindingGuard`) so the per-task drop never
  `forget_session`s the warm session out from under the handle; SessionManager owns the lease.

## Architecture — `SessionManager` (serve-side, in-memory)

A new serve-side component, **sibling to the registry and `TaskStore`** (NOT in `TaskStore`, NOT keyed by
task id). It owns the warm-session table + the backend lease.

**Wiring (Opus M3):** `gate()` is sync and has no manager in scope, so it ONLY parses the contextId into the
`RoutedCall` (add `context_id: Option<ContextId>`). The async **dispatch layer** (`unary_message` /
`stream_message`, the `RouteTarget::Local` arm) consults `srv.session_manager`: a present contextId →
resolve/lookup the `WarmHandle` and dispatch against its `(backend, backend_session)` with **`guard =
None`** (so the per-task `BindingGuard` never forgets the warm session); absent → today's `session-{task}`
forget-after path. `SessionManager` is added to `InboundServer` via a `with_session_manager(..)` builder
(mirroring `with_workflows`/`with_task_store`). **`RouteTarget::{Workflow,Delegate,Fanout}` reject a
contextId in Slice 0** (those are detached/non-warm; see SEQ-AUTHORITY).

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
    pub fingerprint: SessionSpecFingerprint, // frozen-at-mint = agent + effective_config + session_cwd
    pub state: Mutex<SessionState>,
    last_used: Mutex<Instant>,
}

pub enum SessionState { Idle, Running { op: OperationId }, Releasing, Expired }
```

### Lifecycle (Slice 0 subset)
`mint(contextId, spec)` → `Idle` → `Running{op}` (a turn) → `Idle` (turn end, **no forget**) → `Released`
(manual `session/release` OR TTL/idle reap). **On release: evict both maps + `backend.release_session(id)` +
drop the lease.** This drops the *bridge session* (and, for `ContainerRwBackend`, reaps **that session's
container** via a per-session `release_warm` — see Backend contract). It does **NOT** reap the shared
`AcpBackend` process (warm for serve's lifetime, shared across sessions) — the lease drop only decrements
the slot counter. `Expired`: an `apply()` config-reload that removes/replaces the agent slot force-retires
the backend after grace even with the lease held (`registry.rs:248`) → the handle transitions to `Expired`
and the next `continue` returns typed `SessionExpired` (cold re-mint on a fresh contextId). `Resetting`/
reconcile/compact are out of Slice 0.

## Data flow

1. **`message/send` arrives.** `gate()` parses `contextId` from `params.message.contextId` (fallback
   `metadata["a2a-bridge.context"]`) into `RoutedCall`. A contextId on a non-`Local` route
   (`Workflow`/`Delegate`/`Fanout`) is **rejected** (`InvalidRequest`) in Slice 0.
2. **No contextId (Local):** unchanged — derive `session-{task}`, today's forget-after path (back-compat).
3. **New contextId (Local):** `SessionManager.mint(contextId, spec)` → resolve the agent (hold the lease),
   allocate `SessionHandleId` + `backend_session = SessionId("ctx-{contextId}-g0")`, stash the fingerprint,
   insert maps. Dispatch the turn against `(handle.backend, backend_session)` with **`guard = None`** (no
   `BindingGuard` — SessionManager owns the lease); on turn end, **keep warm** (no forget).
4. **Known contextId (Local):** look up the `WarmHandle`; if `Expired` → typed `SessionExpired`. Verify the
   `SessionSpecFingerprint` (agent + effective_config + session_cwd); **mismatch → typed `ConfigMismatch
   {field}`** (needs reconcile/clear — Slice 1/3, NOT a silent reset; cwd mismatch would otherwise hit
   ACP's `InvalidStateTransition` `acp_backend.rs:1292`). Dispatch against the existing `backend_session`
   (context intact), `guard = None`.
5. **`session/status {contextId}`:** return `{ state, agent, generation, idle_age_ms }` (usage added Slice 2).
6. **`session/release {contextId}`:** evict (Lifecycle). **`session/cancel {contextId}`:** cancel the
   in-flight turn — SessionManager maps the handle's `Running{op}` → `backend.cancel(backend_session)`, state
   → `Idle`, **session stays warm** (distinct from release). Also parse the standard A2A `id` field so a
   future task-keyed `tasks/cancel` interops.

## Minimal types (S2 subset — bridge-owned DTOs, Ser+De, versioned)

Added to `bridge-core` (new `orch` module + `ids.rs` newtypes). Bridge-owned, NOT raw SDK enums.

```rust
// ids.rs (macro newtypes)
SessionHandleId(String);  OperationId(String);  ContextId(String);   // string newtypes
pub struct SessionGeneration(pub u64);

// orch.rs (minimal — only what a single warm turn emits; rich variants are Slice 6/7)
pub const ORCH_V: u16 = 1;
// cost is {amount,currency} per ACP UsageCost (NOT guaranteed USD — agent-client-protocol-schema client.rs:329)
#[derive(Serialize, Deserialize)] pub struct UsageCost { pub amount: f64, pub currency: String }
#[derive(Serialize, Deserialize)] pub struct UsageSnapshot { pub used: Option<u64>, pub size: Option<u64>, pub cost: Option<UsageCost>, pub at_ms: i64 }
// envelope: session/source fields are DEFERRED (S6) but the schema is forward-additive — re-adding them as
// Option later is non-breaking; Slice 0 omits them.
#[derive(Serialize, Deserialize)] pub struct OrchEvent { pub v: u16, pub seq: i64, pub ts_ms: i64, pub operation_id: OperationId, #[serde(flatten)] pub kind: OrchEventKind }
#[derive(Serialize, Deserialize)] #[serde(tag="kind", rename_all="snake_case")]
pub enum OrchEventKind { Progress { text: String }, Usage { #[serde(flatten)] usage: UsageSnapshot }, Terminal { status: TerminalStatus } } // struct variants only — internally-tagged serde rejects bare tuple variants
#[derive(Serialize, Deserialize)] #[serde(tag="status", rename_all="snake_case")]
pub enum TerminalStatus { Completed, Failed { reason: String }, Canceled }     // from StopReason: end_turn->Completed, cancelled->Canceled, refusal/max_*->Failed, unknown->Failed
#[derive(Serialize, Deserialize)] pub struct OrchResult { pub v: u16, pub operation_id: OperationId, pub status: TerminalStatus, pub wall_clock_ms: u64, pub usage: UsageSnapshot, pub output: String }
```

- **`Update::Usage(UsageSnapshot)`** is added to `ports.rs` (UPDATE-MINIMAL: a single-turn emission). The
  `map_session_update` *plumbing* to populate it is **Slice 2** — Slice 0 only adds the variant + the DTO so
  the schema is real-from-day-one. `Usage` is mapped to `None`/dropped at dispatch until Slice 2.
- **Versioned + `#[serde(flatten)] kind`** → later variants are additive. Note: the architecture's "net
  effect on slices" says Slice 0 adds Plan/ToolCall; the **slicing spec (authoritative) narrows Slice 0 to
  Progress/Usage/Terminal** and defers Plan/ToolCall + the `tool_call_id` correlation field to S6/S7 — the
  versioned/flattened envelope makes that addition non-breaking (no envelope re-cut).

## Backend contract change (all backends)

```rust
// ports.rs — AgentBackend (additive; default delegates to forget_session so non-warm paths are unchanged)
async fn release_session(&self, session: &SessionId) { self.forget_session(session).await; }
```
- **`AcpBackend::release_session`** removes BOTH `session_cfg[id]` AND `sessions[id]` (today's
  `forget_session` removes only the former, `acp_backend.rs:1810`). It does **NOT** call `retire()` — the
  shared process stays warm.
- **`ContainerRwBackend`** needs a NEW **per-session** `release_warm(session)` (reap that one session's
  container + remove it from the `warm` map + clear `turn_active`) — distinct from `retire_warm()` which
  drains ALL warm sessions on lease→0 (`bridge-container/src/lib.rs:412/534`). The default trait
  `release_session` = `forget_session` is **stash-only and would silently leak the container** → ContainerRw
  MUST override `release_session` to call `release_warm`.
- **`bridge-api` (kind=api)** keeps per-session state in a `sessions` map cleared via `forget_session` → the
  default delegate is correct (no process), but confirm the default removes the api `sessions` entry.

## SEQ-AUTHORITY (scoped to what's implementable in Slice 0)

A `contextId` is stamped by exactly one authority. **`TaskRecord` has NO `context_id` field today**
(`task_store.rs:49`) and detached `submit` returns `context_id = task id` (`server.rs:2356`) — so a
cross-authority "contextId has a Working detached task" guard is **not queryable without a TaskStore change
(OUT of Slice 0).** Therefore Slice 0:
- **Rejects a `contextId` on `Workflow`/`Delegate`/`Fanout` routes** (`InvalidRequest`) → the two stamping
  namespaces never intersect in Slice 0.
- `SessionManager.mint` **refuses** (`HandleBusy`) a `contextId` with a live, non-`Released`/`Expired`
  `WarmHandle` (intra-SessionManager mutual exclusion).
- Warm-turn `OrchEvent`s are stamped by `SessionManager` (per-handle seq); detached workflow streams keep
  TaskStore stamping (untouched). **Cross-authority exclusion (warm-handle ⊥ detached-task on one contextId)
  is deferred to S6** when the journal unifies seq + TaskRecord gains a contextId.

## Config

`[server]` (or `[sessions]`) gains `warm_idle_ttl_secs` (default **1800** = 30 min) + an optional
`warm_max_secs` hard cap. Conservative defaults; documented. No auto-compaction (deferred).
- **`SessionSpecFingerprint` = `agent` + `EffectiveConfig{model,effort,mode}` + `session_cwd`** (cwd is
  separate from `EffectiveConfig` in `SessionSpec` `domain.rs:167` and is immutable post-`session/new`). A
  `continue` whose computed *effective* (post-override) fingerprint differs → typed **`BridgeError::
  ConfigMismatch{field}`** (NEW variant; map to a JSON-RPC error code; ensure `client_message()` doesn't
  leak per [[inbound-hardening-shipped]]). Compare the *effective* config (not the entry default — the
  effort-silently-dropped gotcha, [[warm-loop-session-b2b3c-shipped]]).
- **`SessionExpired`** (NEW typed error / `SessionState::Expired`): a config-reload (`apply()`) that
  removes/replaces the agent slot force-retires the backend after grace even with the lease held — the warm
  handle expires; the next `continue` returns `SessionExpired` (cold re-mint), NOT a silent failure.

## Scope

**IN:** contextId parse in `gate()` + dispatch-layer SessionManager consult (`with_session_manager` builder,
`guard=None` on the warm Local path); `SessionManager` (mint/continue/status/release/cancel, lease
ownership, TTL/idle reap, `SessionSpecFingerprint` + typed `ConfigMismatch`, `SessionExpired` on slot
retire); `release_session` on ACP (drop `sessions[id]`) + `ContainerRwBackend::release_warm` (per-session
reap) + API; `BridgeError::ConfigMismatch`/`SessionExpired` variants; minimal `OrchEvent`/`OrchResult`/ids/
`UsageSnapshot`/`UsageCost` DTOs + `Update::Usage` variant; `session/status`+`session/release`+`session/cancel`
JSON-RPC + CLI; **`submit --context` + agent-targeted (non-workflow) send** for the live-gate; the
SEQ-AUTHORITY scoping (reject contextId on detached routes + intra-manager `HandleBusy`).

**OUT (later slices):** `reconcile_config` (S1); usage plumbing/telemetry/threshold (S2); reset/clear (S3);
compact (S4); `run-workflow --serve --context` + executor keep-warm policy (S5); the 4-path journal rewrite
+ dual-store + rich Plan/ToolCall variants (S6/7); MCP surface (S8); Turn Channel/permission (S9).

## Definition of Done + LIVE-GATE (real serve + real codex)

All via `submit --context C --agent codex` (the **Local** route — NOT a workflow skill, which routes detached).
1. **Continuation:** two `submit --context C` calls to one `serve` → the 2nd **recalls** a codeword set in
   the 1st (context intact; the agent does NOT re-read) — proven via a "remember X" probe.
2. **Latency:** the 2nd call pays **no cold codex-acp spawn + ACP handshake** (cold 1st ≈ tens of seconds;
   warm 2nd sub-second to first token, modulo model time) — proven by a `pgrep -f codex-acp` watcher showing
   **no process growth** on the 2nd call.
3. **Isolation:** distinct `contextId`s get distinct warm sessions; no cross-talk.
4. **Back-compat:** no `contextId` → unchanged per-task forget-after behavior.
5. **Release (split gate):** `session/status C` shows `Idle/Running`; `session/release C` evicts —
   **(a) host-ACP:** a `pgrep` watcher shows **no codex-acp process growth and the shared process STAYS
   alive** (release is not a process reap) + a fake-backend assert that `sessions[id]` is removed;
   **(b) ContainerRw:** `docker ps` shows the released session's `:rw` container reaped **to 0**. A
   subsequent `--context C` mints fresh.
6. **Config-mismatch:** a `continue` on C with a different model/effort/cwd → typed `ConfigMismatch{field}`
   (NOT a silent reset, NOT a silent drop).
7. **Cancel keeps warm:** `session/cancel C` cancels an in-flight turn (state→`Idle`) but the session stays
   warm (a follow-up `--context C` still recalls).
8. **Idle-TTL reap:** with a gate config `warm_idle_ttl_secs=5`, an idle warm session is reaped after the
   TTL (host: `sessions[id]` gone / ContainerRw: `docker ps`→0); the registry lease invariants hold.
9. **SessionExpired:** a config-reload removing the agent slot expires the warm handle → the next `continue`
   returns typed `SessionExpired` (not a silent hang/crash).

## Risks

- **contextId wire:** confirm `params.message.contextId` deserializes on the actual inbound `message/send`
  (the field exists in `a2a-lf-0.3.0`; add the metadata fallback). Pin with a `context_id_from_params` test.
- **Release ≠ process reap (codex BLOCKER):** the shared `AcpBackend` process is warm for serve's lifetime;
  `release` must only evict the bridge session, NOT `retire()` the backend. The DoD gate asserts the shared
  process STAYS alive on release.
- **`BindingGuard` collision (Opus M2):** the contextId-warm path must dispatch with `guard=None`; if the
  per-task `BindingGuard::Drop` fires `forget_session`, the warm session dies on first producer exit and the
  feature is silently dead. The single most important integration point.
- **Lease force-retire:** a held warm lease does NOT pin forever — `apply()` force-retires after grace
  (`registry.rs:248`, test `:1146`). Handle this as `SessionExpired`, don't assume the lease blocks reload.
- **Container per-session reap:** `ContainerRwBackend` reaps only at `retire_warm` (drain-all, lease→0) —
  Slice 0 needs a NEW per-session `release_warm`; gate with `docker ps`→0 for the released session.
- **SEQ-AUTHORITY:** un-aliasing contextId from task id must not break the `session-{task}` no-contextId
  path (back-compat test); reject contextId on detached routes (no TaskRecord contextId column today).

## Testing approach

- **Unit:** `OrchEvent`/`OrchResult` ser/de round-trip + version field + `UsageCost{amount,currency}`;
  `Usage` struct-variant serializes under the internal `kind` tag; `TerminalStatus` from each `StopReason`
  (incl. `unknown`→Failed); `SessionManager` mint/lookup/fingerprint-mismatch(`ConfigMismatch`)/TTL-eviction
  /`HandleBusy`/`Expired`→`SessionExpired` (fake backend + fake clock); `release_session` removes
  `sessions[id]` for ACP, default-delegates to `forget_session` for non-warm backends, and ContainerRw
  overrides to `release_warm`; `context_id_from_params` (field + metadata fallback).
- **Integration (in-crate, mocked backend):** `gate()` parses contextId into `RoutedCall`; dispatch consults
  SessionManager — new contextId mints (guard=None, warm kept), known resumes, no contextId = legacy
  forget-after path; contextId on a `Workflow` route rejected; `session/status`/`release`/`cancel` methods;
  a mocked `BindingGuard` does NOT fire `forget_session` on the warm path.
- **Live-gate (real serve + codex):** DoD 1–9 via `submit --context C --agent codex` (Local route) + a
  `pgrep -f codex-acp` watcher (shared process stays alive + no growth on continue/release) and, for
  ContainerRw, a `docker ps` watcher (per-session container →0 on release/TTL) — the SPIKE-A / B2b-3c
  watcher pattern. DoD-8 uses a `warm_idle_ttl_secs=5` gate config.

## Constraints (carried)

sonnet implementor; codex high-risk + final, Opus arch; `max_attempts=3`; reviewers judge **intent, not
verbatim**. **Dual spec-review (codex xhigh + Opus) before planning**; **LIVE-GATED before merge.**
