# M4 Slice 2 — Drill-down HTTP (design)

**Status:** DESIGN, reviewed. Architected by gpt-5.5 (xhigh) against the shipped Slice-1 surfaces;
adversarial security+architecture review by fable (high). Verdict REVISE → folded here. One review
finding (W1) was **rejected on primary evidence** — see *Review provenance* at the end. Repo
`a2a-bridge`, base `main` (Slice 1 = PR #14, merged `0427b10`).

## Goal

Add the owner-approved drill-down HTTP surface to `InboundServer::router()`:

| Route | Returns |
|---|---|
| `GET /turns/{turn_id}` | one durable `turn_log` row as JSON, including warm inline turns |
| `GET /tasks/{id}/journal.jsonl` | a detached task's orchestration journal as JSONL |
| `GET /tasks/{id}/artifacts/{node}` | one workflow node checkpoint output |

Read-only, bearer-authenticated, gated by a new `[traces].enabled` flag independent of `[metrics]`,
backed by the shipped task-store tables/methods. `TaskStatusDto` gains optional `usage` + `trace`
refs. `turn_log` remains the **single source** for both cost metrics and displayed per-turn cost.

## Global Constraints

- Toolchain `1.94.0`; CI gates fmt (`-D warnings`) + clippy + full `--workspace` test + `cargo deny`. Local triad = fmt+clippy+test (`-j 1` to avoid `--all-targets` OOM).
- Metrics/trace surfaces **opt-in, default OFF**; no new unauthenticated HTTP (existing loopback bearer auth).
- `prometheus` types never leak into `bridge-core` ports/DTOs — confined to `bridge-observ`.
- High-cardinality ids (`task_id`/`context_id`/`turn_id`/`prompt_id`) are **never** Prometheus labels — they live in the turn-log / trace surfaces.

## Route catalog

### Common route wiring

Catalog paths use `{name}`; implement with axum 0.7 path extractors in
`crates/bridge-a2a-inbound/src/server.rs`. Add a trace-HTTP config to `InboundServer`:

```rust
#[derive(Clone, Debug)]
pub struct TraceHttpConfig {
    pub enabled: bool,
    pub journal_max_bytes: usize,
    pub journal_max_events: usize,
    pub artifact_max_bytes: usize,
    pub max_task_turns: usize,   // cap on TaskStatusDto.trace.turns / turn_log_rows_for_task (fold S5)
}

trace_config: TraceHttpConfig,
```

Builder: `pub fn with_trace_http_config(mut self, config: TraceHttpConfig) -> Self;`

Mount all three routes in `router()` so disabled requests return an explicit `404` from the handler
(mirrors the shipped conditional `/metrics` at server.rs:270):

```rust
.route("/turns/:turn_id", get(turn_row))
.route("/tasks/:id/journal.jsonl", get(task_journal_jsonl))
.route("/tasks/:id/artifacts/:node", get(task_artifact))
```

Every handler applies gates in this order:

1. `[traces].enabled`; disabled → `404`.
2. `bearer_token(&headers)`; missing → `401` with `WWW-Authenticate: Bearer`.
3. `srv.auth.authorize(&InboundRequest::with_token(&token))`; failure → `401`.
4. Parse route keys into domain newtypes (`TaskId::parse` / `TurnId::parse` / `NodeId::parse`) and perform store lookups.

Unknown, forbidden, not-yet-materialized, and purged records all return `404` — chosen over
403/410 to avoid object-existence leaks and because Slice 2 adds no retention tombstones. Slice 3
may later upgrade *known-purged* responses to `410 Gone`; **no Slice-2 caller or DTO may assume a
record is immortal.**

> **Accepted trade-off (fable S2):** the disabled-vs-enabled split (disabled→404 before auth,
> enabled→401) lets an unauthenticated loopback client probe whether `[traces].enabled` is on. This
> leaks *config state*, never data, and is identical to the shipped `/metrics` posture. Acceptable at
> this tier (single operator, loopback, bearer, audit). A deployment that cares can flip gate order
> (bearer before feature-gate) — not required here.

### New `TaskStore` read methods

Shipped anchors remain in use: `get`, `turn_log_rows`, `journal_from`, `node_checkpoints`, tables
`turn_log`, `task_journal`, `task_node_checkpoints`. Slice 2 needs bounded single-key reads so HTTP
never buffers an unbounded table or artifact. Add to `crates/bridge-core/src/task_store.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeArtifactMeta { pub node: NodeId, pub finished: bool }

#[derive(Clone, Debug, PartialEq)]
pub enum NodeCheckpointOutput {
    Found { output: String, ok: bool, usage: Option<crate::orch::UsageSnapshot>, bytes: u64 },
    TooLarge { bytes: u64 },
}

/// UNBOUNDED cost/token rollup for a task (SQL SUM/COUNT — O(1) memory, no row buffering).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskUsageAgg {
    pub rows: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub cost: Option<crate::orch::UsageCost>, // Some iff ≥1 costed row AND exactly one distinct currency
    pub at_ms: i64,                            // MAX(completed_ms)
}

/// One turn_log row by key.
async fn turn_log_row(&self, turn_id: &TurnId) -> Result<Option<TurnLogRow>, BridgeError>;

/// Rows for one task, ordered, capped by `limit` — used ONLY to build `trace.turns` ref URLs
/// (fold S5). NEVER used for cost/token accounting: capping the rows that feed the cost total
/// would undercount a task with more turns than the cap (re-review W). Use the aggregate below.
async fn turn_log_rows_for_task(&self, task: &TaskId, limit: usize)
    -> Result<Vec<TurnLogRow>, BridgeError>;

/// UNBOUNDED cost/token rollup, computed in SQL so the cap never truncates the accounting total
/// (fold re-review W). `cost` is Some only when every non-null `cost_currency` for the task agrees.
async fn turn_log_usage_for_task(&self, task: &TaskId)
    -> Result<Option<TaskUsageAgg>, BridgeError>;

/// Newest completed turn for a warm session (warm-turn discoverability).
async fn latest_turn_log_row_for_session(&self, session: &ContextId)
    -> Result<Option<TurnLogRow>, BridgeError>;

/// Read the WHOLE journal for a task under ONE connection guard (fold W3 — no paged streaming).
/// Enforces caps inline: returns TooLarge before assembling the body if over event/byte limit.
async fn journal_jsonl_bounded(&self, task: &TaskId, max_events: usize, max_bytes: usize)
    -> Result<JournalRead, BridgeError>;

/// Node ids that have a checkpoint (metadata-only; no output loaded).
async fn node_checkpoint_nodes(&self, task: &TaskId) -> Result<Vec<NodeId>, BridgeError>;

/// One node's output, size-checked and loaded UNDER ONE connection guard (fold S1).
async fn node_checkpoint_output(&self, task: &TaskId, node: &NodeId, max_bytes: usize)
    -> Result<Option<NodeCheckpointOutput>, BridgeError>;
```

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalRead {
    Body { jsonl: String, events: u64, bytes: u64 }, // full NDJSON, no trailing partial line
    TooLarge { events: u64, bytes: u64 },
}
```

SQLite bindings (`crates/bridge-store/src/sqlite.rs`):

- `turn_log_row`: `SELECT ... FROM turn_log WHERE turn_id=?1`.
- `turn_log_rows_for_task`: `SELECT ... FROM turn_log WHERE task_id=?1 ORDER BY completed_ms, turn_id LIMIT ?2`. (ref-list only)
- `turn_log_usage_for_task`: `SELECT COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), SUM(thought_tokens), SUM(cached_read_tokens), SUM(cached_write_tokens), SUM(cost_amount), COUNT(DISTINCT cost_currency), MIN(cost_currency), MAX(completed_ms) FROM turn_log WHERE task_id=?1` — **no LIMIT**. In Rust: `None` if `COUNT(*)==0`; else `cost = (distinct_currency_count == 1 && sum_cost_amount.is_some()).then(|| UsageCost { amount: sum_cost_amount, currency: min_cost_currency })` (a single distinct currency ⇒ `MIN(cost_currency)` is that currency); mixed currencies ⇒ `cost = None`.
- `latest_turn_log_row_for_session`: `SELECT ... FROM turn_log WHERE session_id=?1 ORDER BY completed_ms DESC, turn_id DESC LIMIT 1`. (Rows are written only at turn finish, where `completed_ms` is always set — `upsert_turn_finished` — so this ordering is sound.)
- `journal_jsonl_bounded`: **one** `BEGIN`-scoped read — `SELECT COUNT(*), COALESCE(SUM(length(CAST(event_json AS BLOB))+1),0) FROM task_journal WHERE task_id=?1`; if over caps → `TooLarge`; else `SELECT seq, event_json FROM task_journal WHERE task_id=?1 ORDER BY seq` and join with `\n`. Both statements execute under the same `Mutex<Connection>` guard, so a concurrent purge/`ON DELETE CASCADE` cannot interleave (**fold W3** — no silently-truncated 200).
- `node_checkpoint_nodes`: `SELECT node_id FROM task_node_checkpoints WHERE task_id=?1 ORDER BY COALESCE(seq,0), node_id`. *(The `seq` column is present on `task_node_checkpoints` — added by `migrate_tasks_columns` at sqlite.rs:252–254 and written by `put_node_checkpoint_sequenced`; rows from the non-sequenced `put_node_checkpoint` leave it NULL, hence `COALESCE`. See Review provenance / W1.)*
- `node_checkpoint_output`: a **single** statement `SELECT (CASE WHEN length(CAST(output AS BLOB)) <= ?3 THEN output END), ok, usage_json, length(CAST(output AS BLOB)) FROM task_node_checkpoints WHERE task_id=?1 AND node_id=?2` — the size gate and the load are atomic; over-limit yields NULL output → `TooLarge{bytes}` without materializing the string (**fold S1**).

The existing `journal_from(task, seq) -> Vec<OrchEvent>` remains for resume/fold logic; HTTP must
**not** call it (it buffers the whole journal without a byte cap).

### `GET /turns/{turn_id}`

- **Auth:** common trace gate + bearer.
- **Input:** `{turn_id}` parsed via `TurnId::parse`; store key only — never a path/URL/SQL-string/outbound target.
- **Binding:** `TaskStore::turn_log_row(&TurnId)`.
- **Response:** `200`, `Content-Type: application/json`, `X-Content-Type-Options: nosniff`. Body mirrors `TurnLogRow`; optional columns → `null`; `traceparent` serialized as W3C text via `TraceParent::to_header_value()` (for future OTLP correlation).

| State | Status |
|---|---:|
| `[traces].enabled = false` | 404 |
| Missing / rejected bearer | 401 |
| Invalid/empty, unknown, forbidden, or purged `turn_id` | 404 |
| Writer has not persisted the row yet | 404 |
| Row exists, usage columns not yet written | 200 (null usage/cost/token columns) |
| Store failure | 500 |

### `GET /tasks/{id}/journal.jsonl`

- **Auth:** common trace gate + bearer.
- **Input:** `{id}` via `TaskId::parse`; store key only.
- **Binding:** `TaskStore::get(&TaskId)` (confirm the durable task row) then `TaskStore::journal_jsonl_bounded(&TaskId, max_events, max_bytes)`.
- **Response:** `200`, `Content-Type: application/x-ndjson`, `X-Content-Type-Options: nosniff`, **`Content-Length` set** (full body assembled under one guard — not a chunked stream). One serialized `OrchEvent` per line, ordered by `seq`.
- **Limit:** `journal_jsonl_bounded` returning `TooLarge` → `413` with an `application/json` explanatory body; no partial body ever emitted.
- **Purged vs empty (fold W2):** after `get` confirms the task row, if `JournalRead::Body{events:0,..}`:
  - task status **terminal** → `404` (journal was purged, or never existed for a completed task — indistinguishable without a tombstone, and both mean "gone");
  - task status **working/non-terminal** → `200` with empty body (events not written yet).

| State | Status |
|---|---:|
| `[traces].enabled = false` | 404 |
| Missing / rejected bearer | 401 |
| Invalid/empty, unknown, or forbidden task id | 404 |
| Terminal task, empty journal (purged/none) | 404 |
| Working task, no events yet | 200 empty |
| Journal over event/byte limit | 413 |
| Store failure | 500 |

### `GET /tasks/{id}/artifacts/{node}`

- **Auth:** common trace gate + bearer.
- **Input:** `{id}` via `TaskId::parse`; `{node}` via strict `NodeId::parse` (`[a-z0-9_-]+`, invalid syntax → 404). Both store keys — neither a filesystem path.
- **Node validation** against the task's actual node set:
  - Preferred: parse `TaskRecord.workflow_spec_json` (the persisted snapshot) and require `{node}` in `graph.nodes`.
  - Legacy task with no snapshot: fall back to `node_checkpoint_nodes(&task)` and allow only nodes with a completed checkpoint.
  - Helper `pub fn workflow_spec_node_ids(spec_json: &str) -> Result<BTreeSet<NodeId>, BridgeError>` lives **in `bridge-coordinator::detached`**, next to `encode_workflow_spec` / `WorkflowSpecEnvelope` — because `WorkflowSpecEnvelope.graph` is private to that module (verified; the placement is load-bearing).
- **Binding:** `TaskStore::get`, `node_checkpoint_nodes` (validation/DTO), `node_checkpoint_output(&task,&node,max_bytes)` (bounded fetch).
- **Response:** `200`, `Content-Type: text/plain; charset=utf-8`, `X-Content-Type-Options: nosniff`. Body = the checkpoint `output` string.
- **Storage fact (security basis):** artifacts are **DB values, not files** — SQLite `task_node_checkpoints.output TEXT NOT NULL`; the in-memory store holds a `String`. There is no filesystem path in the artifact path, so traversal is **structurally impossible**. If a future store moves artifacts to files, the route must still call a store-owned keyed method that opens the handle internally (never a separate path check).
- **Limit:** `NodeCheckpointOutput::TooLarge` → `413` (`application/json` body); no partial artifact.

| State | Status |
|---|---:|
| `[traces].enabled = false` | 404 |
| Missing / rejected bearer | 401 |
| Invalid/empty task id, invalid node syntax | 404 |
| Unknown task; node not in persisted set; known node unfinished; artifact purged; forbidden | 404 |
| Artifact over byte limit | 413 |
| Store failure | 500 |

## DTO changes

### `TraceRefs` (in `crates/bridge-coordinator/src/coordinator.rs`, by the status DTOs)

```rust
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TraceRefs {
    #[serde(skip_serializing_if = "Option::is_none")] pub turn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub turns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub artifacts: Option<std::collections::BTreeMap<String, String>>,
}
```

All values are **relative** URLs: `turn` = `/turns/{turn_id}`; `turns` = `[/turns/{turn_id}, …]`
(capped at `max_task_turns`); `journal` = `/tasks/{id}/journal.jsonl`; `artifacts` =
`{ "node-a": "/tasks/{id}/artifacts/node-a" }`. Each `{id}`/`{turn_id}`/`{node}` segment is
**percent-encoded** when built into a ref URL (re-review SMELL): today's server-minted detached
task/turn ids are path-safe by construction, but `TaskId`/`TurnId::parse` reject only the empty string
(not `/`,`?`,`#`; `NodeId` is strict `[a-z0-9_-]+`), so encoding is belt-and-suspenders should id
minting ever change. Refs are **best-effort**: a ref may resolve to `404` after retention purge,
before the async turn-log writer flushes, or if the caller races a mid-write task.

### `TaskStatusDto`

```rust
#[derive(serde::Serialize)]
pub struct TaskStatusDto {
    pub id: TaskId, pub workflow: String, pub status: &'static str,
    pub result: Option<String>, pub error: Option<String>, pub updated_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")] pub usage: Option<UsageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")] pub trace: Option<TraceRefs>,
}
```

**Population + the flag seam (fold S4).** Two independently-gated concerns:

- **`usage`** is *data*, reconciled to `turn_log`. Populated whenever `turn_log` rows exist for the
  task (i.e. whenever the turn-log writer is installed) — it does **not** depend on `[traces].enabled`.
  Source = `turn_log_usage_for_task(&id)`, the **UNBOUNDED** SQL rollup — NOT the capped
  `turn_log_rows_for_task` (re-review W: reusing the capped rows would undercount cost on a task with
  more turns than `max_task_turns`). Build `UsageSnapshot.terminal`: `input_tokens = agg.input_tokens`,
  `output_tokens = agg.output_tokens`, **`total_tokens = agg.input_tokens + agg.output_tokens`**
  (`turn_log` has no per-turn `total_tokens` column, so the task total is the reconstructed
  input+output sum; `thought_tokens`/`cached_*` carry their own summed columns, `None` when all-null),
  `cost = agg.cost` (already single-currency-or-`None`), `at_ms = agg.at_ms` (fallback
  `TaskRecord.updated_ms`), `used`/`size` = `None` (absent from `turn_log`). **Never** use
  `node_checkpoints(...).usage` or journal `Usage` events for displayed cost — that stays resume metadata.
- **`trace`** refs are *URLs into the drill-down routes*, so they are populated only when
  `[traces].enabled` (a ref to a disabled route would 404). `trace.turns` from the same rows;
  `trace.journal` for the durable task row; `trace.artifacts` from `node_checkpoint_nodes` (completed
  nodes only). All-absent → `trace` omitted.

**Seam:** the flag reaches the DTO builder via a `Coordinator` construction field
`trace_refs_enabled: bool` (from `[traces].enabled`). Ref strings are built **in the coordinator DTO
layer, deliberately** — the coordinator already owns `StatusDto` and the route shapes are a stable
public contract; this avoids a second decoration pass in the inbound adapter. (Documented as a chosen
layering; the alternative — build refs in `bridge-a2a-inbound` — is viable but adds a mapping seam
for no gain here.) **Impl note:** because population now issues `turn_log`/checkpoint store reads, the
sync `From<&TaskRecord> for TaskStatusDto` (coordinator.rs:94) is replaced by an **async status
builder** on `Coordinator`; the bare `From` may remain for the no-trace/no-usage path or be dropped.

### `SessionStatusDto` — warm-turn discoverability

```rust
// ... existing fields ...
#[serde(skip_serializing_if = "Option::is_none")] pub trace: Option<TraceRefs>,
```

When `[traces].enabled`, call `latest_turn_log_row_for_session(&context_id)`; if present set
`trace.turn = /turns/{turn_id}`. This closes the loop for **random** warm turn ids: after a warm turn
completes, callers learn its drill-down URL from `session/status`. Detached callers learn node-turn
URLs from `TaskStatusDto.trace.turns`.

> **Known limit (fable S3):** `latest_turn_log_row_for_session` is latest-only and the writer is
> async — if turn A completes and turn B completes (or the writer lags) before A is observed, A's ref
> is never surfaced (historical warm-turn listing is a Slice-2 non-goal). Acceptance criterion #5 is
> worded accordingly ("the most recently flushed warm turn"). A future last-N ref list closes it.

## `[traces]` config (`bin/a2a-bridge/src/config.rs`)

```toml
[traces]
enabled            = false
journal_max_bytes  = 16777216   # 16 MiB
journal_max_events = 100000
artifact_max_bytes = 4194304    # 4 MiB
max_task_turns     = 512        # cap on trace.turns / turn_log_rows_for_task (fold S5)
```

`TracesToml` (serde, all limits `#[serde(default = …)]`) → validated `TracesConfig`. Validation:
every limit `> 0`. **Independent of `[metrics]`:**

- `/metrics` controlled only by `[metrics]`; drill-down routes only by `[traces]`.
- The turn-log writer is installed when **either** `[traces].enabled` **or** (`[metrics].enabled && [metrics].turn_log`) — drill-down needs durable rows even with Prometheus off.
- A deployment may run drill-down without Prometheus, or Prometheus without drill-down.

## Security guards

1. **Bearer before any store read** — `bearer_token()` + `authorize`; missing/rejected → 401. *(unauth disclosure)*
2. **Separate trace gate** — `[traces].enabled` checked first; disabled → 404. *(accidental exposure via metrics-only intent)*
3. **Route params are store keys, not paths** — `{id}`/`{turn_id}` parsed to newtypes, used only as lookup params, never concatenated into a path. *(traversal / key-path confusion)*
4. **Node id strict + task-scoped** — `NodeId` `[a-z0-9_-]+` and must be in the task's persisted snapshot or completed-checkpoint set; else 404. *(arbitrary checkpoint fetch)*
5. **Artifacts are DB rows** — no filesystem path to canonicalize/open → traversal structurally impossible; any future file store must open via a store-owned keyed handle. *(traversal / canonicalize-then-open TOCTOU)*
6. **No SSRF** — routes do no outbound HTTP; `TraceRefs` are relative strings from fixed prefixes + parsed ids. *(attacker-controlled fetch)*
7. **Bounded journal** — `journal_jsonl_bounded` enforces `journal_max_events`/`journal_max_bytes` under one guard; over → 413; body assembled with `Content-Length`, never chunk-streamed. *(unbounded memory/bandwidth; mid-purge truncation)*
8. **Bounded artifact** — single size-checked statement; over `artifact_max_bytes` → 413 without loading output. *(unbounded output; measure-then-load race)*
9. **Explicit content types + nosniff** — `application/json` / `application/x-ndjson` / `text/plain; charset=utf-8`, each with `X-Content-Type-Options: nosniff` (bodies carry model-generated text). *(content sniffing / browser interpretation)*
10. **Audit per fetch** — one structured `tracing` line: `caller` (`auth.caller_id()` or `"unauthenticated"`), `route`, key fields, `status`, response `bytes`. Never log tokens or bodies. *(untraceable reads)*
11. **No existence leak via status** — unknown/forbidden/missing/purged all → 404; no 403 for object denial. *(enumeration)*
12. **Structured fields, not free-text** — ids go in `tracing` fields, never formatted into the log line. *(log injection)*

## Slice-1 & Slice-3 cohesion

**Slice 1:** reuse the shipped `turn_log` table + `TurnLogObserver`; add **no** second observer or
cost sink. `/turns/{turn_id}` reads `turn_log_row`, not in-memory observer state.
`TaskStatusDto.usage` reads `turn_log` rows only. `traceparent` (already persisted) is echoed by
`/turns/{turn_id}`. If `[traces].enabled && !metrics`, still install `TurnLogObserver`.

**Slice 3 (retention):** these routes serve exactly what retention purges (`turn_log`,
`task_journal`, `task_node_checkpoints.output`). Trace refs are best-effort; a previously-emitted ref
may later 404. Slice 2 returns 404 for purged data (no tombstone yet); Slice 3 may add tombstones and
upgrade *known-purged* to 410 — no Slice-2 code assumes immortality. Retention never deletes resumable
`TaskRecord`s by default, so a task's status can outlive its journal/artifacts/turn rows (exactly the
terminal-empty-journal → 404 branch above).

## Reconciliation invariant

`turn_log` is the single source for both the cost metric and displayed per-turn cost. Proof:

- Live metrics and turn-log writes share the existing `DedupObserver` gate keyed by `turn_id`.
- Prometheus restart rebuild already reads `turn_log_rows()`.
- `/turns/{turn_id}` shows `cost_amount`/`cost_currency` straight from the row.
- `TaskStatusDto.usage` aggregates from the **unbounded** `turn_log_usage_for_task()` (SQL SUM over ALL rows; `max_task_turns` caps only the `trace.turns` ref list, never the accounting total) — no second path.
- `node_checkpoints(...).usage` and journal `Usage` events are **not** used for displayed cost.
- A missing/purged row → neither `/turns` nor task status fabricates cost from another source.

## Testing

**Unit** (`bridge-core` / `bridge-store` / `bridge-coordinator` / config):
`traces_config_defaults_disabled`; `traces_config_rejects_zero_limits`; `trace_refs_skip_absent_fields`;
`task_status_dto_omits_usage_trace_when_none`; `task_usage_aggregates_from_turn_log_single_currency`;
`task_usage_omits_cost_for_mixed_currencies`; `workflow_spec_node_ids_reads_persisted_snapshot`;
`workflow_spec_node_ids_rejects_bad_snapshot`; `memory_turn_log_row_lookup` / `sqlite_turn_log_row_lookup`;
`sqlite_turn_log_rows_for_task_orders_and_limits` (**LIMIT respected** — fold S5);
`sqlite_turn_log_usage_for_task_sums_all_rows` (task with `max_task_turns + 1` costed USD rows → the
rollup includes **every** row — accounting is NOT capped, re-review W);
`sqlite_turn_log_usage_for_task_cost_none_on_mixed_currency` (two currencies → `cost = None`, tokens still summed);
`task_usage_terminal_total_tokens_is_input_plus_output` (derivation formula, re-review SMELL);
`trace_ref_segments_are_percent_encoded` (an id containing `/` round-trips as an encoded segment, re-review SMELL);
`sqlite_latest_turn_log_row_for_session_returns_latest`;
`sqlite_journal_jsonl_bounded_body_and_counts` (full body, correct events/bytes);
`sqlite_journal_jsonl_bounded_too_large_over_events` / `…_over_bytes` (**no body assembled** — fold W3);
`sqlite_node_checkpoint_nodes_metadata_only`;
`sqlite_node_checkpoint_output_too_large_single_statement` (output NULL when over limit — fold S1);
`node_id_invalid_maps_to_404`.

**Integration** (`bridge-a2a-inbound`):
`trace_routes_404_when_disabled_even_without_bearer`; `trace_routes_require_bearer_when_enabled`;
`trace_routes_reject_bad_bearer`; `turn_route_returns_json_turn_log_row` (+ `nosniff` header);
`turn_route_returns_warm_turn_row`; `session_status_includes_latest_warm_turn_trace_ref`;
`task_status_includes_usage_and_trace_refs`; `task_status_usage_present_when_traces_disabled`
(**usage decoupled from `[traces]`** — fold S4); `journal_route_returns_ndjson_with_content_length`;
`journal_route_empty_working_task_200` **and** `journal_route_terminal_empty_journal_404`
(**W2 branch**); `journal_route_413_over_byte_limit` / `…_over_event_limit`;
`artifact_route_returns_plain_text_nosniff`; `artifact_route_validates_node_against_snapshot`;
`artifact_route_404_for_known_unfinished_node`; `artifact_route_413_when_output_too_large`;
`trace_routes_audit_success_and_failure`; `metrics_and_traces_independent`;
`trace_ref_after_purge_returns_404`.

Before "done": `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`cargo test --workspace -j 1`; `cargo deny check`.

## Acceptance criteria

1. `[traces]` parses with documented defaults, validates positive limits, independent of `[metrics]`.
2. `router()` exposes the three routes with the status matrices above.
3. All three require bearer when enabled; return 404 when disabled.
4. `/turns/{turn_id}` reads one `turn_log` row via `turn_log_row`, JSON with `traceparent` as W3C text + `nosniff`.
5. The **most recently flushed** warm inline turn id is discoverable via `SessionStatusDto.trace.turn` and resolves through `/turns/{turn_id}` (fold S3).
6. `/tasks/{id}/journal.jsonl` returns a **bounded, `Content-Length`-delimited** NDJSON body assembled under one store guard; over-limit → 413 before any body; terminal-empty → 404, working-empty → 200 (folds W2/W3).
7. `/tasks/{id}/artifacts/{node}` validates node membership and returns the output as `text/plain; charset=utf-8` + `nosniff`; size-checked in a single statement (fold S1).
8. Artifact serving never touches a filesystem path (DB-backed).
9. Every fetch emits a structured audit line (caller, route, key, status, bytes).
10. `TaskStatusDto` serializes optional `usage`/`trace` only when present; `usage` populates whenever `turn_log` rows exist (independent of `[traces]`); `trace` refs only when `[traces].enabled` (fold S4).
11. `TaskStatusDto.usage` and `/turns/{turn_id}` cost read only from `turn_log`; task usage is the **unbounded** `turn_log_usage_for_task` rollup, never truncated by `max_task_turns`; `terminal.total_tokens = Σinput + Σoutput` (re-review W + SMELL).
12. `max_task_turns` caps the `trace.turns` **ref list only** — not the cost/token accounting total (fold S5 + re-review W).
13. Unknown/forbidden/mid-write-missing/purged → 404; refs are relative + best-effort after retention.
14. fmt, clippy, full workspace test, and `cargo deny` pass.

## Non-goals

OTLP exporter; metrics/drill-down UI; list/search routes for turns/journals/artifacts;
transcript/content redaction; multi-user ACLs/quotas/tenancy; filesystem-backed artifacts; retention
or tombstone schema (Slice 3); HTTP range / partial responses; a separate bind port for trace routes.

## Review provenance

- **Architect:** gpt-5.5 (xhigh), read-only against the live branch; every load-bearing anchor
  independently re-verified by the host (all real: `turn_log`/`task_journal`/`task_node_checkpoints`
  tables + columns, `encode_workflow_spec`/`WorkflowSpecEnvelope`, `TraceParent::to_header_value`,
  `UsageSnapshot` fields, `InboundRequest::with_token`→`authorize`).
- **Review:** fable (high) — REVISE, 3 WRONG + 6 SMELL. Folded: **W2** (terminal-empty→404 branch),
  **W3** (single-guard bounded journal read, no streaming), **S1** (atomic size-check), **S3**
  (acceptance #5 wording + known-limit note), **S4** (named the flag seam + usage/trace gating split),
  **S5** (`max_task_turns` cap + LIMIT), **S6** (`nosniff`). Accepted trade-off: **S2** (flag-probe
  leak, matches `/metrics`).
- **Rejected on evidence — W1** ("`node_checkpoint_nodes` ORDER BY `seq` → statement-prepare failure
  → 500"): the `seq` column **exists** on `task_node_checkpoints`, added by `migrate_tasks_columns`
  (sqlite.rs:252–254, run at every open via sqlite.rs:197) and written by
  `put_node_checkpoint_sequenced`. fable read the CREATE TABLE (sqlite.rs:140–149, which omits `seq`)
  and missed the migration. The query prepares and runs; `COALESCE(seq,0)` handles the NULLs left by
  the non-sequenced insert path. No change.
- **Re-review:** a fresh gpt-5.5 (xhigh) session, cold context, instructed to treat this provenance
  as claims-to-verify. It **independently AGREED** with the W1 rejection (verified the migration at
  sqlite.rs:197/252, the sequenced writer at sqlite.rs:1174, and the migration test at sqlite.rs:2050)
  and confirmed all six folds buildable. It found **one new WRONG the host introduced while folding
  S5**: `TaskStatusDto.usage` sourced from the *capped* `turn_log_rows_for_task(&id, max_task_turns)`
  would **undercount cost** on a task with more turns than the cap. Fixed here: usage now reads the
  unbounded `turn_log_usage_for_task` SQL rollup; the cap applies to `trace.turns` refs only. Also
  folded two re-review SMELLs: percent-encode ref segments, and specify
  `terminal.total_tokens = Σinput + Σoutput` (turn_log has no per-turn total column). Verdict after
  fixes: the sole must-fix and both nits are resolved.
