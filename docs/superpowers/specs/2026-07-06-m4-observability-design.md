# M4 — Operational Observability Design (v2, post dual-review)

**Date:** 2026-07-06
**Status:** Draft — revised after codex gpt-5.5 (correctness, ×2) + fable/`claude-fable-5[1m]` (architecture) review. Both REVISE; all findings folded.
**Roadmap:** strategic-analysis §10 M4. Builds on the #10 Coordinator migration (one lifecycle owner) and the detached-task journal.

**Goal:** Give the long-running `serve` operational observability that actually serves **debugging** and **eval** — an aggregate metrics endpoint, turn latency/outcome/queue signals, a **durable per-turn record** (the substrate both jobs depend on), per-task cost, drill-down links, and bounded storage — behind a product-neutral seam so Prometheus, OTLP, or both can back it.

**Why v2:** the first draft had two sinks (Prometheus + the detached-only journal) and **no durable per-turn record**, so warm inline turns (the primary serve path) were undrillable and the eval dimensions (`prompt_id`/`model`/`effort`) rode events no consumer persisted — "decoration." v2 adds the **turn-log sink** that both jobs require; the reviewers confirmed the seam is right, just under-populated.

## Slices (each its own plan → ship)

- **Slice 1 — Metrics seam + turn-log sink + `/metrics`.** The foundation; pure-additive; independently valuable. *(Fully specified here.)*
- **Slice 2 — Drill-down HTTP** (turn/task read routes + DTO trace refs, `[traces]` config). *(Scoped here; spec expands at slice start.)*
- **Slice 3 — Retention** under `[storage]` (artifact-purge default; never deletes resumable TaskRecords). *(Scoped here.)*

## Global Constraints

- Toolchain `1.94.0`; CI gates fmt (`-D warnings`) + clippy + full `--workspace` test. Local triad = fmt+clippy+test (`-j 1`).
- Metrics/trace surfaces **opt-in, default OFF**; no new unauthenticated HTTP (existing loopback bearer auth).
- `prometheus` types never leak into `bridge-core` ports/DTOs — confined to `bridge-observ`.
- High-cardinality ids (`task_id`/`context_id`/`turn_id`/`prompt_id`) are **never** Prometheus labels — they live in the turn-log/trace surfaces.

---

## Verified facts (unchanged from v1)

- No metrics/otel/prometheus deps today. Ports in `bridge-core/src/ports.rs`; `bridge-observ` exists (tracing setup).
- Cost model (`bridge-core/src/orch.rs`): `UsageSnapshot { used, size, cost: Option<UsageCost{amount,currency}>, terminal: Option<TerminalUsage>, at_ms }`; currency NOT guaranteed USD.
- Per-session usage on `session/status`; `TaskStatusDto` has none. **OrchEvents journaled ONLY on the detached path** (`detached.rs`); warm inline (`checkout_turn`) never journals.
- Shared usage-recording boundary for ALL turns: `coordinator.rs:356–423`. Batch admission: `batch.rs:587–605`. Node usage folded: `batch.rs:334`. Serve is loopback + bearer; router in `bridge-a2a-inbound::InboundServer::router()`. Store is SQLite (task store).

## Architecture (shared across slices): "A-seam + durable turn-log"

A single **event enum** recorded through one observer port; adapters interpret. Three adapters in slice 1: Prometheus (aggregate), the **turn-log** (durable per-turn record — the debug/eval substrate), Noop (default). The journal is reused as the *workflow* drill-down target (slice 2); the turn-log makes **warm** turns drillable and is the source-of-truth that reconciles metric and displayed cost and rebuilds counters on restart.

### The port — one enum, one method (`bridge-core/src/ports.rs`)

```rust
pub struct TraceParent { pub trace_id: [u8; 16], pub span_id: [u8; 8], pub flags: u8 }

pub struct TurnContext {
    pub turn_id: TurnId,               // globally-unique random 128-bit (not session-monotonic)
    pub session_id: ContextId,
    pub task_id: Option<TaskId>,
    pub workflow: Option<String>,
    pub node: Option<String>,
    pub attempt: u32,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub prompt_id: Option<String>,
    pub traceparent: Option<TraceParent>,   // inbound W3C trace-context off the A2A request
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FailureClass { AgentCrashed, TimedOut, Overloaded, Config, Transport, Other }

#[derive(Clone, PartialEq, Eq)]
pub enum TurnOutcome { Success, Failed(FailureClass), Canceled }   // timeout = Failed(TimedOut); ONE encoding

/// Which lifecycle moment finalized this usage — disambiguates the counter trigger.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UsageFinalization { TurnFinal, TaskFinal, Partial }

pub enum ObsEvent<'a> {
    TaskStarted   { ctx: &'a TurnContext },
    TaskFinished  { ctx: &'a TurnContext, outcome: &'a TurnOutcome },
    NodeStarted   { ctx: &'a TurnContext },
    NodeFinished  { ctx: &'a TurnContext, outcome: &'a TurnOutcome },
    TurnStarted   { ctx: &'a TurnContext },
    TurnFinished  { ctx: &'a TurnContext, latency: Duration, ttft: Option<Duration>, outcome: &'a TurnOutcome },
    QueueChanged  { in_flight: u64, queued: u64, wait: Option<Duration> },
    UsageFinalized{ ctx: &'a TurnContext, usage: &'a UsageSnapshot, fin: UsageFinalization },
}

pub trait Observer: Send + Sync { fn record(&self, e: &ObsEvent); }
```

**Why the enum (both reviewers):** every future signal (compact, container spawn, MCP call, deeper node nesting) adds a *variant*, not a trait method touching every adapter; `FanoutObserver` stays trivial; and — decisively — the enum **serializes to the turn-log for free**. Borrowed `&ctx` is fine; buffering adapters (turn-log, a future OTLP batch exporter) clone the fields they persist. Task/Node lifecycle events ship now (default-ignored by adapters that don't need them) so a future OTLP **span-tree** adapter has parents to open/close — without reopening `bridge-core`.

### Adapters (`bridge-observ`)

- `NoopObserver` — `record` is empty; the **default** (zero-cost when disabled).
- `PrometheusObserver` — matches variants → instruments on an owned `prometheus::Registry`.
- `TurnLogObserver` — on `TurnFinished`+`UsageFinalized`, writes one row to the `turn_log` SQLite table (below).
- `FanoutObserver(Vec<Arc<dyn Observer>>)` — forwards to each (prometheus **and** otel/turn-log).

`Coordinator` holds `Arc<dyn Observer>` (default Noop). The `/metrics` **exposition** is a separate `MetricsEndpoint` adapter (present only when a Prometheus exporter is configured) — emission ≠ exposition.

---

## SLICE 1 — Metrics seam + turn-log sink + `/metrics`

### §1.1 The durable turn-log (the linchpin)

A new SQLite table `turn_log` in the existing store DB, one row per finished turn:

| column | source |
|---|---|
| `turn_id` (PK), `session_id`, `task_id?`, `workflow?`, `node?`, `attempt` | `TurnContext` |
| `agent`, `model?`, `effort?`, `mode?`, `prompt_id?` | `TurnContext` (eval dims) |
| `started_ms`, `latency_ms`, `ttft_ms?`, `outcome`, `failure_class?` | `TurnFinished` |
| `input_tokens`, `output_tokens`, `thought/cached_*`, `cost_amount?`, `cost_currency?` | `UsageFinalized` |
| `traceparent?` | `TurnContext` |

Serves all three purposes the reviewers required: **debugging** (warm turns are now drillable — a `turn_id` resolves to a row even with no journal), **eval** (`SELECT … GROUP BY prompt_id, model, effort` yields per-prompt/model precision/cost — the joined tuple finally persists somewhere), and **restart-safe counters** (rebuild on boot from this table with dedupe on `turn_id`).

### §1.2 Metric catalog

| Metric | Type | Bounded labels |
|---|---|---|
| `bridge_turns_total` | counter | `agent`, `model`, `effort`, `outcome` |
| `bridge_turn_duration_seconds` | histogram | `agent`, `model` (buckets .05,.1,.25,.5,1,2.5,5,10,30,60,120,300) |
| `bridge_turn_ttft_seconds` | histogram | `agent` |
| `bridge_turns_in_flight` | gauge | — |
| `bridge_queue_depth` | gauge | — |
| `bridge_queue_wait_seconds` | histogram | — |
| `bridge_turn_cost_total` | counter | `agent`, `model`, `currency` |
| `bridge_turn_tokens_total` | counter | `agent`, `kind` (input/output/thought/cached_read/cached_write) |

Cost/token counters are **per-turn** (renamed from `task_cost` — they count real spend on every finalized turn incl. warm inline), **idempotency-keyed** `(session_id, turn_id, attempt)`, and **rebuilt from `turn_log` on boot** (dedupe by key) so a restart doesn't lose history or double-count on replay. Every label whose value is user/config-defined (`agent`, `model`, `effort`, `workflow`, `currency`, `kind`, `outcome`) is normalized against a bounded vocabulary; unknown → `"other"`. Ids/`prompt_id`/`traceparent` are turn-log-only, never labels.

### §1.3 Hook points (exactly-once, explicit taxonomy)

**Drive-path taxonomy** — enumerate and hook every one; tests prove exactly-once `TurnStarted`/`TurnFinished` per turn on each:
1. warm inline A2A send, 2. detached task turn, 3. workflow-node turn, 4. `implement` turn, 5. `review` turn, 6. `batch` fan-out child turn.

- **Turn latency/outcome/ttft:** at the shared usage boundary (`coordinator.rs:356–423`), stamp `Instant` at entry, capture `ttft` at first streamed event, map result → `TurnOutcome` on exit.
- **Usage/cost:** emit `UsageFinalized{ fin }` at the same boundary for **all** turn types. `fin = TurnFinal` on every finished turn (drives the per-turn cost/token counters + a turn-log row, idempotency-keyed). `TaskFinal`/`Partial` are informational for adapters (e.g. a future OTLP task span). **Count every finalized turn attempt** (real spend), keyed so a crash-resume replay of the same `(session_id,turn_id,attempt)` is deduped.
- **Queue:** an RAII guard with an explicit state machine `Waiting → Admitted → Released`. `Drop` in `Waiting` decrements the waiter count (cancellation mid-`acquire_owned().await` can't leak `bridge_queue_depth`); `Waiting→Admitted` atomically `waiter--,in_flight++`; `Drop` in `Admitted` decrements `in_flight` (normal release AND cancellation). `bridge_queue_wait_seconds` observed `Waiting→Admitted`.

### §1.4 Config (slice 1 only)

```toml
[metrics]
enabled   = false
exporters = ["prometheus"]   # "otel" later; both => fanout
turn_log  = true             # persist the per-turn record table (enables eval + restart-safe counters)
```
`/metrics` on the existing serve port + bearer auth; 404 when disabled or when no Prometheus exporter is configured; 401 without bearer.

### §1.5 Testing (slice 1)

- Adapter units: `PrometheusObserver` exposition per instrument (counter/histogram bucket+`_sum`+`_count`/gauge); label normalization → `"other"`; `TurnLogObserver` writes the expected row per turn; `NoopObserver` no-op; `FanoutObserver` forwards to N.
- Port-contract (`RecordingObserver`): exactly-once `TurnStarted`/`TurnFinished` with correct outcome on success/`Failed(class)`/cancel **on each of the six drive paths**; `UsageFinalized` at the boundary with correct `fin`.
- Cancellation: cancel mid-`acquire` → `bridge_queue_depth` returns to baseline (RAII state machine).
- Idempotency: replay a terminal transition (simulated resume) → cost/token counters do **not** double.
- Restart: boot with a populated `turn_log` + empty in-memory counters → counters rebuilt, no double-count.
- HTTP: `/metrics` exposition when enabled; 404 disabled / no-prom-exporter; 401 no bearer.

### §1.6 Acceptance (slice 1)

1. `Observer`/`ObsEvent` in `bridge-core`, `prometheus`-free; adapters in `bridge-observ`; domain has no Prometheus reference.
2. `/metrics` returns valid exposition covering §1.2 after a workflow run; disabled/no-exporter → 404; no bearer → 401.
3. Turn counters/histograms increment exactly once per turn across all six drive paths (tested).
4. `bridge_queue_depth` cancellation-safe (tested); `bridge_queue_wait_seconds` records wait.
5. `bridge_turn_cost_total` covers warm inline turns, dedupes across simulated resume, rebuilds from `turn_log` on restart (tested).
6. `turn_log` row per finished turn with the eval columns; an eval query over `prompt_id`×`model` returns per-group cost/outcome.
7. fmt+clippy+full suite green.

---

## SLICE 2 — Drill-down HTTP (scoped)

Routes on `InboundServer::router()`, behind bearer + a **separate** `[traces].enabled` flag (independent of metrics):

| Route | Returns |
|---|---|
| `GET /turns/{turn_id}` | the `turn_log` row (JSON) — drillable for **warm** turns too |
| `GET /tasks/{id}/journal.jsonl` | detached task OrchEvent journal, streamed |
| `GET /tasks/{id}/artifacts/{node}` | a node's output |

`TaskStatusDto` gains `usage: Option<UsageSnapshot>` + `trace: Option<TraceRefs>` (relative route URLs; `skip_serializing_if`). **Guards:** `{id}`/`{turn_id}` are store KEYS, never path segments; `{node}` validated against the task's node set; artifacts served by **opening the store-owned handle before/without a separate path check** (no canonicalize-then-open TOCTOU); response size/stream limits; safe content-types; audit-log per fetch. Redaction out of scope (personal tier: bearer+loopback+audit). Reconciliation: `turn_log` is the single source for both the cost *metric* and the displayed per-turn cost (no dual-source drift).

## SLICE 3 — Retention under `[storage]` (scoped)

```toml
[storage]
artifact_retention_days   = 14   # purge journal / node-outputs / transcripts + turn_log rows past TTL
artifact_retention_max_bytes = 0 # 0 = off; else evict oldest-by-completion-time until under cap
purge_terminal_tasks_days = 0    # 0 = NEVER delete TaskRecords (default); opt-in only, terminal-only
```
Retention is a **storage** concern, not observability — **not** gated on `[traces]`. Default purges **artifacts** (the space the drill-down creates) but **never** deletes `TaskRecord`s (which back `tasks/get`, crash-resume, merge). Deleting aged **terminal** task records is a separate explicit opt-in that refuses non-terminal/resumable tasks. Ordering is oldest-by-**completion-time**. Boot + periodic sweep (reuses reaper primitives). Retention never rewrites metric counters (rebuilt from surviving `turn_log`; document that purged turns drop from rebuilt totals — acceptable, and identical to Prometheus retention).

## Non-goals

OTLP adapter (port is now genuinely ready — lifecycle events + traceparent present — adapter is a real follow-up), separate metrics bind port, transcript redaction, multi-user auth/quotas, a metrics UI.
