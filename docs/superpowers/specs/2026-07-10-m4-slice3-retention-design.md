# M4 Slice 3 — Retention under [storage] (design)

## Goal

Implement storage retention for the artifacts created by M4 Slices 1–2, with deletion safety as the primary constraint.

Slice 3 adds:

- Default artifact retention under `[storage].artifact_retention_days = 14`.
- Optional size-based artifact eviction under `[storage].artifact_retention_max_bytes`.
- Separate explicit terminal-task deletion under `[storage].purge_terminal_tasks_days`.
- Boot and periodic best-effort sweeps.
- No dependency on `[metrics]` or `[traces]`.

Grounding in shipped code:

- `TaskRecord` has `status` and `updated_ms`, but no `completed_ms`, at `crates/bridge-core/src/task_store.rs:130`.
- `TaskRecordStatus::is_terminal()` treats every non-`Working` status as terminal at `crates/bridge-core/src/task_store.rs:44`.
- Terminal writes set `updated_ms`, so `updated_ms` is the completion timestamp for terminal tasks: non-sequenced SQLite path at `crates/bridge-store/src/sqlite.rs:559`, sequenced path at `crates/bridge-store/src/sqlite.rs:1353`.
- `task_node_checkpoints`, `task_node_starts`, and `task_journal` cascade from `tasks`, but `turn_log` has no FK/cascade: DDL at `crates/bridge-store/src/sqlite.rs:140`, `crates/bridge-store/src/sqlite.rs:150`, `crates/bridge-store/src/sqlite.rs:158`, and `crates/bridge-store/src/sqlite.rs:165`.
- Slice 1 rebuilds Prometheus counters from `turn_log_rows()` during serve boot at `bin/a2a-bridge/src/main.rs:6119`, via `PrometheusObserver::rebuild_from_turn_log` at `crates/bridge-observ/src/lib.rs:430`.
- Slice 2 routes already read from `turn_log`, journal, and checkpoints through `TaskStore`: routes mounted at `crates/bridge-a2a-inbound/src/server.rs:294`.

## Global Constraints

- Toolchain stays Rust `1.94.0`: `rust-toolchain.toml:2`; workspace `rust-version = "1.94"` at `Cargo.toml:9`.
- Keep CI/local gates: `cargo fmt`, clippy with warnings denied, and full `cargo test --workspace`; local triad should run single-job where needed, `-j 1`.
- Metrics/trace surfaces remain opt-in and default off; retention is not a metrics/trace surface.
- No new unauthenticated HTTP route.
- `prometheus` types stay out of `bridge-core`; current workspace dependency is confined by design, and `bridge-core` only owns ports/DTOs.
- High-cardinality IDs remain out of Prometheus labels; retention only deletes durable rows.

## `[storage]` Config

Add a new top-level `[storage]` table. Do not reuse `[store]`; `[store]` already owns the durable DB path and resume cap at `bin/a2a-bridge/src/config.rs:64`.

```toml
[storage]
artifact_retention_days      = 14
artifact_retention_max_bytes = 0
purge_terminal_tasks_days    = 0
```

| Field | Type | Default | Validation | Meaning |
|---|---:|---:|---|---|
| `artifact_retention_days` | `u64` | `14` | `>= 0`; TOML negatives fail deserialize | `0` disables TTL artifact purge; otherwise purge eligible artifacts older than this many days. |
| `artifact_retention_max_bytes` | `u64` | `0` | `>= 0`; TOML negatives fail deserialize | `0` disables size eviction; otherwise evict oldest eligible artifacts until logical artifact bytes are under cap, unless protected live bytes alone exceed cap. |
| `purge_terminal_tasks_days` | `u64` | `0` | `>= 0`; TOML negatives fail deserialize | `0` never deletes `TaskRecord`s; otherwise opt-in terminal-only task purge older than this many days. |

Implementation shape in `bin/a2a-bridge/src/config.rs`:

```rust
fn default_artifact_retention_days() -> u64 {
    14
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct StorageToml {
    #[serde(default = "default_artifact_retention_days")]
    pub artifact_retention_days: u64,
    #[serde(default)]
    pub artifact_retention_max_bytes: u64,
    #[serde(default)]
    pub purge_terminal_tasks_days: u64,
}

impl Default for StorageToml {
    fn default() -> Self {
        Self {
            artifact_retention_days: default_artifact_retention_days(),
            artifact_retention_max_bytes: 0,
            purge_terminal_tasks_days: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageConfig {
    pub artifact_retention_days: u64,
    pub artifact_retention_max_bytes: u64,
    pub purge_terminal_tasks_days: u64,
}

impl RegistryConfig {
    pub fn storage_config(&self) -> Result<StorageConfig, ConfigError> {
        Ok(StorageConfig {
            artifact_retention_days: self.storage.artifact_retention_days,
            artifact_retention_max_bytes: self.storage.artifact_retention_max_bytes,
            purge_terminal_tasks_days: self.storage.purge_terminal_tasks_days,
        })
    }
}
```

Add to `RegistryConfig` beside current independent `[metrics]` and `[traces]` fields at `bin/a2a-bridge/src/config.rs:246`:

```rust
#[serde(default)]
pub storage: StorageToml,
```

Decision: `[storage]` is captured at serve boot like other non-registry runtime fields; existing hot reload applies registry snapshots only, through `reg.apply(snap)` at `bin/a2a-bridge/src/main.rs:6025`.

## Store Methods

Add stats types and three retention methods to `TaskStore` in `crates/bridge-core/src/task_store.rs`, after the existing turn-log/read methods around `crates/bridge-core/src/task_store.rs:316`.

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactPurgeStats {
    pub task_artifact_sets: u64,
    pub orphan_turn_log_rows: u64,
    pub journal_rows: u64,
    pub node_checkpoints: u64,
    pub turn_log_rows: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactEvictionStats {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub evicted_task_artifact_sets: u64,
    pub evicted_orphan_turn_log_rows: u64,
    pub journal_rows: u64,
    pub node_checkpoints: u64,
    pub turn_log_rows: u64,
    pub cap_reached: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalTaskPurgeStats {
    pub task_records: u64,
    pub turn_log_rows: u64,
    pub session_rows: u64,
}

async fn purge_artifacts_completed_before(
    &self,
    cutoff_ms: i64,
) -> Result<ArtifactPurgeStats, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn evict_artifacts_to_max_bytes(
    &self,
    max_bytes: u64,
) -> Result<ArtifactEvictionStats, BridgeError> {
    Err(BridgeError::StoreFailure)
}

async fn purge_terminal_tasks_completed_before(
    &self,
    cutoff_ms: i64,
) -> Result<TerminalTaskPurgeStats, BridgeError> {
    Err(BridgeError::StoreFailure)
}
```

The default `Err(StoreFailure)` keeps wrapper/fake stores source-compatible; `MemoryTaskStore` and `SqliteStore` must override all three.

Logical byte accounting for caps:

- `task_journal`: `length(CAST(event_json AS BLOB)) + 1`, matching existing bounded journal byte accounting at `crates/bridge-store/src/sqlite.rs:1563`.
- `task_node_checkpoints`: `length(CAST(output AS BLOB)) + COALESCE(length(CAST(usage_json AS BLOB)), 0)`.
- `turn_log`: sum of UTF-8 byte lengths for text columns plus `8` bytes for each non-null integer/real column.
- This is a deterministic logical cap, not SQLite file-size accounting; physical `VACUUM` is a non-goal.

Shared SQL byte expression for `turn_log`:

```sql
COALESCE(length(CAST(turn_id AS BLOB)), 0) +
COALESCE(length(CAST(session_id AS BLOB)), 0) +
COALESCE(length(CAST(task_id AS BLOB)), 0) +
COALESCE(length(CAST(workflow AS BLOB)), 0) +
COALESCE(length(CAST(node AS BLOB)), 0) +
8 + -- attempt, NOT NULL
COALESCE(length(CAST(agent AS BLOB)), 0) +
COALESCE(length(CAST(model AS BLOB)), 0) +
COALESCE(length(CAST(effort AS BLOB)), 0) +
COALESCE(length(CAST(mode AS BLOB)), 0) +
COALESCE(length(CAST(prompt_id AS BLOB)), 0) +
CASE WHEN started_ms IS NULL THEN 0 ELSE 8 END +
CASE WHEN completed_ms IS NULL THEN 0 ELSE 8 END +
CASE WHEN latency_ms IS NULL THEN 0 ELSE 8 END +
CASE WHEN ttft_ms IS NULL THEN 0 ELSE 8 END +
COALESCE(length(CAST(outcome AS BLOB)), 0) +
COALESCE(length(CAST(failure_class AS BLOB)), 0) +
CASE WHEN input_tokens IS NULL THEN 0 ELSE 8 END +
CASE WHEN output_tokens IS NULL THEN 0 ELSE 8 END +
CASE WHEN thought_tokens IS NULL THEN 0 ELSE 8 END +
CASE WHEN cached_read_tokens IS NULL THEN 0 ELSE 8 END +
CASE WHEN cached_write_tokens IS NULL THEN 0 ELSE 8 END +
CASE WHEN cost_amount IS NULL THEN 0 ELSE 8 END +
COALESCE(length(CAST(cost_currency AS BLOB)), 0) +
COALESCE(length(CAST(traceparent AS BLOB)), 0)
```

### SQLite Implementation Shape

Use `SqliteStore.conn: Arc<Mutex<rusqlite::Connection>>` at `crates/bridge-store/src/sqlite.rs:15`, so each retention method runs under the same process-local single-connection mutex as live writes. File-backed stores already enforce single serve per DB through the lock at `crates/bridge-store/src/sqlite.rs:39` and `crates/bridge-store/src/sqlite.rs:64`; WAL/busy timeout are already configured at `crates/bridge-store/src/sqlite.rs:67` and `crates/bridge-store/src/sqlite.rs:88`.

Use `BEGIN IMMEDIATE` for each sweep transaction. Do not use `VACUUM` inside the sweep.

### In-Memory Implementation Shape

Use `MemoryTaskStore` maps at `crates/bridge-core/src/task_store.rs:600`:

- Hold `journal_fold_guard` for multi-map consistency; existing sequenced writers use this guard around journal/checkpoint transitions, e.g. `record_node_started` at `crates/bridge-core/src/task_store.rs:1167`.
- Collect victim IDs from `inner`.
- Mutate `journals`, `checkpoints`, `turn_log`, `starts`, and `inner` in short scoped locks.
- Never hold a map lock while parsing IDs or serializing large JSON if avoidable.

## Purge Algorithms

### Artifact TTL Purge

Guard predicate:

```sql
-- Eligible task-owned artifacts.
SELECT t.id
FROM tasks t
WHERE t.status IN ('completed', 'failed', 'canceled', 'interrupted')
  AND t.updated_ms < :cutoff_ms;
```

Rules:

- Deletes `task_journal`, `task_node_checkpoints`, and `turn_log` rows.
- Keeps every `tasks` row.
- Does not delete artifacts for `status = 'working'`.
- Also deletes orphan warm inline `turn_log` rows where `task_id IS NULL` and `completed_ms < cutoff`.
- Also deletes stale non-null orphan `turn_log` rows whose `task_id` no longer has a `tasks` row, using the row’s own `completed_ms`; this handles historical/manual deletes because `turn_log` is not cascade-linked.

SQLite SQL:

```sql
BEGIN IMMEDIATE;

-- Stats before.
SELECT /* logical_artifact_bytes */;

WITH eligible_tasks AS (
    SELECT id FROM tasks
    WHERE status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND updated_ms < :cutoff_ms
)
DELETE FROM task_journal
WHERE task_id IN (SELECT id FROM eligible_tasks);

WITH eligible_tasks AS (
    SELECT id FROM tasks
    WHERE status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND updated_ms < :cutoff_ms
)
DELETE FROM task_node_checkpoints
WHERE task_id IN (SELECT id FROM eligible_tasks);

WITH eligible_tasks AS (
    SELECT id FROM tasks
    WHERE status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND updated_ms < :cutoff_ms
)
DELETE FROM turn_log
WHERE task_id IN (SELECT id FROM eligible_tasks)
   OR (
        completed_ms IS NOT NULL
        AND completed_ms < :cutoff_ms
        AND (
            task_id IS NULL
            OR NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = turn_log.task_id)
        )
   );

-- Stats after.
SELECT /* logical_artifact_bytes */;

COMMIT;
```

Proof that default artifact retention never touches `TaskRecord`s:

- The SQL contains no `DELETE FROM tasks` and no `UPDATE tasks`.
- It only deletes from `task_journal`, `task_node_checkpoints`, and `turn_log`.
- Keeping `tasks` is required because `TaskRecord` backs `tasks/get` at `crates/bridge-a2a-inbound/src/server.rs:3892`, crash resume via `working_tasks()` at `crates/bridge-core/src/task_store.rs:305`, and coordinator resume at `crates/bridge-coordinator/src/detached.rs:1695`.

Memory shape:

- Build `eligible_task_ids` from `inner.values()` where `status.is_terminal()` and `updated_ms < cutoff_ms`.
- Remove `journals[id]`.
- Retain only checkpoints whose `task_id` is not in `eligible_task_ids`.
- Retain only turn-log rows that are not linked to eligible task IDs and are not orphan rows with `completed_ms < cutoff_ms`.
- Never mutate `inner`.

### Size Eviction

Guard predicate for evictable task-owned artifact sets:

```sql
SELECT t.id, t.updated_ms
FROM tasks t
WHERE t.status IN ('completed', 'failed', 'canceled', 'interrupted');
```

Guard predicate for evictable orphan turn rows:

```sql
SELECT tl.turn_id, tl.completed_ms
FROM turn_log tl
WHERE tl.completed_ms IS NOT NULL
  AND (
      tl.task_id IS NULL
      OR NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = tl.task_id)
  );
```

Rules:

- Size cap is independent from TTL.
- It may evict artifacts newer than the TTL if the cap requires it.
- It never deletes `TaskRecord`s.
- It never evicts data for `status = 'working'`.
- It evicts in oldest-by-completion-time order:
  - task-owned artifact set completion time = terminal task `updated_ms`;
  - orphan turn completion time = `turn_log.completed_ms`.
- Candidate granularity:
  - one terminal task’s artifact set: journal rows, checkpoints, and task-linked turn-log rows;
  - one orphan turn-log row.
- The loop is finite: candidates are materialized once, sorted once, and each candidate is deleted at most once.
- If protected non-terminal artifacts alone exceed the cap, the method stops with `cap_reached = false` and logs; it must not evict non-terminal data to satisfy a cap.

Candidate SQL:

```sql
WITH task_candidates AS (
    SELECT
        'task' AS kind,
        t.id AS task_id,
        NULL AS turn_id,
        t.updated_ms AS completed_ms,
        COALESCE((
            SELECT SUM(length(CAST(j.event_json AS BLOB)) + 1)
            FROM task_journal j
            WHERE j.task_id = t.id
        ), 0)
        +
        COALESCE((
            SELECT SUM(
                length(CAST(c.output AS BLOB)) +
                COALESCE(length(CAST(c.usage_json AS BLOB)), 0)
            )
            FROM task_node_checkpoints c
            WHERE c.task_id = t.id
        ), 0)
        +
        COALESCE((
            SELECT SUM(/* turn_log byte expression */)
            FROM turn_log tl
            WHERE tl.task_id = t.id
        ), 0) AS bytes
    FROM tasks t
    WHERE t.status IN ('completed', 'failed', 'canceled', 'interrupted')
),
orphan_turn_candidates AS (
    SELECT
        'turn' AS kind,
        NULL AS task_id,
        tl.turn_id AS turn_id,
        tl.completed_ms AS completed_ms,
        /* turn_log byte expression */ AS bytes
    FROM turn_log tl
    WHERE tl.completed_ms IS NOT NULL
      AND (
          tl.task_id IS NULL
          OR NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = tl.task_id)
      )
)
SELECT kind, task_id, turn_id, completed_ms, bytes
FROM (
    SELECT * FROM task_candidates
    UNION ALL
    SELECT * FROM orphan_turn_candidates
)
WHERE bytes > 0
ORDER BY completed_ms ASC, kind ASC, COALESCE(task_id, turn_id) ASC;
```

Deletion SQL per candidate inside the same transaction:

```sql
-- kind = 'task'
DELETE FROM task_journal WHERE task_id = :task_id;
DELETE FROM task_node_checkpoints WHERE task_id = :task_id;
DELETE FROM turn_log WHERE task_id = :task_id;

-- kind = 'turn'
DELETE FROM turn_log WHERE turn_id = :turn_id;
```

Pseudo-code:

```rust
let before = logical_artifact_bytes(&tx)?;
if before <= max_bytes {
    return stats(before, before, true);
}

let candidates = load_ordered_candidates(&tx)?;
let mut current = before;

for candidate in candidates {
    if current <= max_bytes {
        break;
    }

    delete_candidate(&tx, &candidate)?;
    current = current.saturating_sub(candidate.bytes);
}

let after = logical_artifact_bytes(&tx)?;
let cap_reached = after <= max_bytes;
```

Memory shape:

- Compute total logical bytes from all `journals`, `checkpoints`, and `turn_log`.
- Build task candidates only for terminal tasks.
- Build orphan turn candidates for `task_id == None` or task ID absent from `inner`, with `completed_ms.is_some()`.
- Sort by `(completion_ms, kind, id)`.
- Delete candidates until `current <= max_bytes` or candidates are exhausted.
- Return `cap_reached = after_bytes <= max_bytes`.

### Terminal-Task Opt-In Purge

This is the dangerous knob. It is off by default and must be unreachable from debug/verbose flags.

Guard predicate:

```sql
SELECT t.id
FROM tasks t
LEFT JOIN batch b ON b.id = t.batch_id
WHERE t.updated_ms < :cutoff_ms
  AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
  AND NOT EXISTS (
      SELECT 1
      FROM task_node_starts ns
      WHERE ns.task_id = t.id
  )
  AND (
      t.batch_id IS NULL
      OR b.status IN ('completed', 'failed', 'canceled')
  )
  AND NOT EXISTS (
      SELECT 1
      FROM sessions s
      WHERE s.task_id = t.id
        AND (
            s.pending_request_id IS NOT NULL
            OR s.pending_kind IS NOT NULL
        )
  );
```

Do-not-delete conditions encoded by the predicate:

- Non-terminal task: rejected by `status IN (...)`.
- Crash-resumable task: rejected because crash resume scans only `Working` rows through `working_tasks()` at `crates/bridge-core/src/task_store.rs:305` and `crates/bridge-coordinator/src/detached.rs:1695`.
- In-progress node: rejected by `NOT EXISTS task_node_starts`; starts are the durable in-progress node table at `crates/bridge-store/src/sqlite.rs:150`.
- Child of unfinished batch: rejected unless the parent batch is terminal; active batch statuses are `working` and `canceling` at `crates/bridge-store/src/sqlite.rs:1065`.
- Pending permission/auth request in the same SQLite store: rejected by `sessions.pending_request_id` / `pending_kind`; pending request columns exist at `crates/bridge-store/src/sqlite.rs:120`, and writes happen at `crates/bridge-store/src/sqlite.rs:388`.
- Unfinished merge: no TaskStore-visible merge state exists today. ADR-0027 merge resolves an implement clone outside TaskStore at `bin/a2a-bridge/src/main.rs:2801` and calls `merge_clone` at `bin/a2a-bridge/src/main.rs:2803`. Assumption: Slice 3 cannot infer merge state from current durable task tables; if a future merge table references task IDs, add it to this same victim CTE before enabling purge for those rows.

SQLite SQL:

```sql
BEGIN IMMEDIATE;

CREATE TEMP TABLE retention_task_victims(id TEXT PRIMARY KEY) WITHOUT ROWID;

INSERT INTO retention_task_victims(id)
SELECT t.id
FROM tasks t
LEFT JOIN batch b ON b.id = t.batch_id
WHERE t.updated_ms < :cutoff_ms
  AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
  AND NOT EXISTS (
      SELECT 1 FROM task_node_starts ns WHERE ns.task_id = t.id
  )
  AND (
      t.batch_id IS NULL
      OR b.status IN ('completed', 'failed', 'canceled')
  )
  AND NOT EXISTS (
      SELECT 1
      FROM sessions s
      WHERE s.task_id = t.id
        AND (
            s.pending_request_id IS NOT NULL
            OR s.pending_kind IS NOT NULL
        )
  );

-- turn_log is not cascade-linked; delete explicitly before deleting tasks.
DELETE FROM turn_log
WHERE task_id IN (SELECT id FROM retention_task_victims);

-- Defensive cleanup for stores where sessions and tasks share this SqliteStore.
-- Current serve uses a separate in-memory SessionStore at bin/a2a-bridge/src/main.rs:6034
-- and bin/a2a-bridge/src/main.rs:6216, so this is effective for same-DB users and harmless otherwise.
DELETE FROM sessions
WHERE task_id IN (SELECT id FROM retention_task_victims);

-- Cascades task_journal, task_node_checkpoints, and task_node_starts via FK.
DELETE FROM tasks
WHERE id IN (SELECT id FROM retention_task_victims);

DROP TABLE retention_task_victims;

COMMIT;
```

Interaction with cascades:

- `task_journal`, `task_node_checkpoints`, and `task_node_starts` are removed by `ON DELETE CASCADE`.
- `turn_log` is not cascade-linked, so it is explicitly deleted.
- `sessions` is not a task artifact, but same-DB cleanup prevents stale task/session state when the same `SqliteStore` backs both ports.

Memory shape:

- Under `journal_fold_guard`, collect victim IDs from `inner` where:
  - status is terminal;
  - `updated_ms < cutoff_ms`;
  - no matching key exists in `starts`;
  - either no `batch_id` or the batch exists and has terminal status.
- Remove victim IDs from `turn_log`, `journals`, `checkpoints`, `starts`, `terminal_seqs`, `seq_counters`, `birth`, and `inner`.
- No in-memory `sessions` table exists in `MemoryTaskStore`; session cleanup is not part of the memory implementation.

## Sweep Scheduling

Add a storage-retention runner in `bin/a2a-bridge/src/main.rs`.

Existing patterns to reuse:

- Boot crash-orphan recovery is best-effort and idempotent in `recover_orphans` at `bin/a2a-bridge/src/main.rs:526`.
- Serve already uses a periodic `tokio::time::interval` task for warm idle reaping at `bin/a2a-bridge/src/main.rs:6198`.
- The prompt’s `main.rs:3560` interval anchor currently points to CLI batch polling, not serve maintenance: `bin/a2a-bridge/src/main.rs:3559`.

Add:

```rust
const STORAGE_RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MS_PER_DAY: u64 = 86_400_000;

fn retention_cutoff_ms(now_ms: i64, days: u64) -> Option<i64> {
    if days == 0 {
        return None;
    }
    let Some(ttl_ms_u64) = days.checked_mul(MS_PER_DAY) else {
        return Some(i64::MIN);
    };
    let Ok(ttl_ms) = i64::try_from(ttl_ms_u64) else {
        return Some(i64::MIN);
    };
    Some(now_ms.saturating_sub(ttl_ms))
}
```

Sweep order:

1. Open/build the `task_store`.
2. Parse `storage_config()`.
3. Run one boot retention sweep best-effort before Prometheus counter rebuild, so rebuilt counters reflect surviving `turn_log` rows.
4. Build observers and rebuild metrics from `turn_log_rows()` as currently done at `bin/a2a-bridge/src/main.rs:6119`.
5. Spawn the periodic retention task.
6. Resume working tasks before binding, as currently done at `bin/a2a-bridge/src/main.rs:6294`.

Sweep function:

```rust
async fn run_storage_retention_once(
    task_store: Arc<dyn TaskStore>,
    cfg: StorageConfig,
    now_ms: i64,
) {
    if let Some(cutoff) = retention_cutoff_ms(now_ms, cfg.artifact_retention_days) {
        if let Err(e) = task_store.purge_artifacts_completed_before(cutoff).await {
            tracing::warn!(error = ?e, cutoff_ms = cutoff, "storage retention: artifact TTL sweep failed");
        }
    }

    if cfg.artifact_retention_max_bytes > 0 {
        if let Err(e) = task_store
            .evict_artifacts_to_max_bytes(cfg.artifact_retention_max_bytes)
            .await
        {
            tracing::warn!(
                error = ?e,
                max_bytes = cfg.artifact_retention_max_bytes,
                "storage retention: artifact size eviction failed"
            );
        }
    }

    if let Some(cutoff) = retention_cutoff_ms(now_ms, cfg.purge_terminal_tasks_days) {
        if let Err(e) = task_store.purge_terminal_tasks_completed_before(cutoff).await {
            tracing::warn!(error = ?e, cutoff_ms = cutoff, "storage retention: terminal task purge failed");
        }
    }
}
```

Periodic loop:

```rust
tokio::spawn(async move {
    let mut ticker = tokio::time::interval(STORAGE_RETENTION_SWEEP_INTERVAL);
    loop {
        ticker.tick().await;
        let now_ms = now_ms();
        run_storage_retention_once(task_store.clone(), storage_cfg, now_ms).await;
    }
});
```

Failure behavior:

- Config parse/validation errors still abort boot, like existing `[metrics]`/`[traces]` validation at `bin/a2a-bridge/src/config.rs:1169` and `bin/a2a-bridge/src/config.rs:1184`.
- Sweep execution errors never abort serve.
- Sweep execution errors only log `warn`.
- Sweeps are idempotent; deleting already-deleted rows is a no-op.

Locking/concurrency:

- SQLite sweeps serialize with live store writes through the same connection mutex.
- File-backed stores have one serve owner per DB path through the lock at `crates/bridge-store/src/sqlite.rs:39`.
- `turn_log` writes happen off-path through `TurnLogObserver`, but still call the same `TaskStore` methods at `crates/bridge-observ/src/lib.rs:225`; old-row predicates prevent deleting in-flight/new turns.
- Periodic sweeps should use one transaction per method and avoid `VACUUM`.

## Data-Safety Guards

1. **Prevents default `TaskRecord` deletion:** default artifact purge SQL never executes `DELETE FROM tasks` or `UPDATE tasks`; it only touches `task_journal`, `task_node_checkpoints`, and `turn_log`.

2. **Prevents debug/verbose-triggered deletion:** terminal-task deletion is only reachable when `[storage].purge_terminal_tasks_days > 0`; no debug, trace, metrics, or verbosity flag is consulted.

3. **Prevents deleting crash-resumable tasks:** terminal-task purge requires `status IN ('completed','failed','canceled','interrupted')`; crash resume scans `Working` rows through `working_tasks()` at `crates/bridge-core/src/task_store.rs:305`.

4. **Prevents deleting in-progress node state:** terminal-task purge requires no `task_node_starts` rows for the task.

5. **Prevents deleting unfinished batch children:** terminal-task purge rejects children whose parent batch is `working` or `canceling`; active batch query is shipped at `crates/bridge-store/src/sqlite.rs:1065`.

6. **Prevents deleting artifacts for live tasks:** artifact TTL and size eviction only target terminal task-owned artifact sets, plus orphan `turn_log` rows with `completed_ms IS NOT NULL`.

7. **Prevents deleting in-flight warm inline rows:** orphan warm rows are only purged when `task_id IS NULL`, `completed_ms IS NOT NULL`, and either older than cutoff or selected by size eviction. A row without `completed_ms` is retained.

8. **Prevents infinite size eviction:** size eviction materializes a finite candidate list and deletes each candidate at most once; if protected bytes exceed cap, it exits with `cap_reached = false`.

9. **Prevents `turn_log` leaks after task purge:** terminal-task purge explicitly deletes `turn_log` rows for victim task IDs because `turn_log` has no FK/cascade.

10. **Prevents dangerous huge TTL math:** `days * 86_400_000` overflow maps to `cutoff = i64::MIN`, which matches no normal rows and deletes nothing.

11. **Prevents negative config ambiguity:** config uses unsigned integer fields, so negative TOML values fail deserialization before any sweep runs.

12. **Clock skew behavior:** backward skew makes the cutoff older and deletes less; forward skew can make artifacts eligible according to wall-clock policy, but default deletion still cannot touch `TaskRecord`s and terminal-task deletion still requires explicit opt-in plus guarded predicates.

## Counter Reconciliation

Retention never rewrites Prometheus counters.

Current boot behavior:

- Serve reads `turn_log_rows()` at `bin/a2a-bridge/src/main.rs:6119`.
- `PrometheusObserver::rebuild_from_turn_log` rebuilds counters and seeds dedupe from those rows at `crates/bridge-observ/src/lib.rs:430`.

After retention:

- If boot retention runs before rebuild, purged turns are absent from rebuilt counters.
- If a periodic sweep purges rows after counters were incremented live, live counters are not decremented.
- On the next restart, counters rebuild from surviving `turn_log` rows and drop purged turns from totals.
- This is accepted and should be documented as Prometheus-style retention behavior, not a bug.

## Slice-1/2 Cohesion

Slice 1:

- `turn_log` remains the source for metric rebuild and usage aggregation.
- Purging `turn_log` rows reduces future rebuilt totals and task usage aggregates.
- `turn_log` rows are nullable by `task_id`, so warm inline turns are retained/purged by their own `completed_ms`.

Slice 2:

- `/turns/:turn_id` returns 404 when `turn_log_row()` returns none at `crates/bridge-a2a-inbound/src/server.rs:1034`.
- `/tasks/:id/journal.jsonl` checks `task_store.get()` first and returns 404 for a missing `TaskRecord` at `crates/bridge-a2a-inbound/src/server.rs:1113`.
- A terminal task whose journal has been artifact-purged returns 404 for the journal route at `crates/bridge-a2a-inbound/src/server.rs:1155`.
- `/tasks/:id/artifacts/:node` checks `task_store.get()` first at `crates/bridge-a2a-inbound/src/server.rs:1264`; missing/purged artifacts return 404 through `node_checkpoint_output()` at `crates/bridge-a2a-inbound/src/server.rs:1328`.
- `TaskStatusDto` usage and trace refs degrade naturally because usage reads from `turn_log_usage_for_task()` at `crates/bridge-coordinator/src/coordinator.rs:774`, turn refs from `turn_log_rows_for_task()` at `crates/bridge-coordinator/src/coordinator.rs:796`, and artifact refs from `node_checkpoint_nodes()` at `crates/bridge-coordinator/src/coordinator.rs:806`.

Assumption: no shipped transcript table exists in the SQLite schema; Slice 3 covers `task_journal`, `task_node_checkpoints`, and `turn_log`.

## Testing

- `storage_config_defaults_artifact_ttl_on_task_purge_off`: parsing a config without `[storage]` yields `artifact_retention_days = 14`, `artifact_retention_max_bytes = 0`, `purge_terminal_tasks_days = 0`.
- `storage_config_independent_from_metrics_and_traces`: retention config parses and sweeps with `[metrics].enabled = false` and `[traces].enabled = false`.
- `storage_config_rejects_negative_values`: TOML negative values for each storage integer fail parse before sweep execution.
- `retention_cutoff_zero_disables`: `days = 0` returns `None`.
- `retention_cutoff_huge_saturates_to_min`: overflow-sized days returns `Some(i64::MIN)` and a sweep deletes no normal rows.
- `sqlite_default_never_touches_a_task_record`: with default storage, an old terminal task loses journal/checkpoint/turn-log rows but `TaskStore::get()` still returns the same `TaskRecord`.
- `memory_default_never_touches_a_task_record`: same assertion for `MemoryTaskStore`.
- `sqlite_artifact_ttl_deletes_only_terminal_task_artifacts`: old terminal task artifacts are deleted; old `Working` task journal/checkpoints/turn-log rows remain.
- `sqlite_artifact_ttl_deletes_orphan_warm_turn_log`: `turn_log` row with `task_id NULL` and old `completed_ms` is deleted; a `task_id NULL` row with `completed_ms NULL` remains.
- `sqlite_artifact_ttl_deletes_stale_nonnull_orphan_turn_log`: `turn_log` row whose task row is absent and old `completed_ms` is deleted.
- `sqlite_terminal_purge_refuses_a_resumable_task`: an old `Working` task with `workflow_spec_json`, checkpoints, and `resume_attempts < cap` is not deleted.
- `sqlite_terminal_purge_refuses_task_with_node_start`: old terminal task with a stale `task_node_starts` row is not deleted.
- `sqlite_terminal_purge_refuses_active_batch_child`: old terminal child under `batch.status = working` or `canceling` is not deleted.
- `sqlite_terminal_purge_deletes_terminal_task_and_turn_log`: old guarded terminal task is deleted; linked `turn_log` rows are also deleted.
- `sqlite_terminal_purge_cascades_child_tables`: deleting a terminal task removes journal/checkpoints/starts via FK cascade.
- `sqlite_terminal_purge_cleans_same_db_sessions`: when `sessions` rows live in the same `SqliteStore`, victim task session rows are deleted.
- `memory_terminal_purge_removes_all_task_maps`: memory terminal purge removes inner record, journals, checkpoints, starts, terminal seqs, and task turn-log rows.
- `sqlite_size_eviction_stops_at_cap`: with only terminal/orphan artifacts, eviction deletes oldest candidates until `bytes_after <= max_bytes`.
- `sqlite_size_eviction_orders_by_completion_time`: older terminal task artifacts are deleted before newer terminal task artifacts; orphan warm turns order by `completed_ms`.
- `sqlite_size_eviction_never_evicts_non_terminal_data`: old `Working` task artifacts remain even when cap is below total bytes.
- `sqlite_size_eviction_protected_bytes_can_exceed_cap`: if protected bytes alone exceed cap, method exits with `cap_reached = false` and does not loop.
- `memory_size_eviction_stops_at_cap`: memory implementation mirrors SQLite cap behavior.
- `prometheus_rebuild_after_purge_uses_surviving_turn_log`: after purging one row and rebuilding a fresh observer, counters include only surviving rows.
- `trace_routes_404_after_artifact_purge`: terminal task retained but journal/checkpoint purged returns 404 for journal/artifact routes.
- `trace_routes_404_after_terminal_task_purge`: purged task record returns 404 for journal/artifact routes and linked turn route returns 404.
- `storage_sweep_failure_does_not_abort_serve`: fake store returns `StoreFailure`; boot/periodic sweep logs warn and serve construction continues.
- `storage_sweep_not_triggered_by_debug_or_trace_flags`: enabling tracing/metrics/debug-style flags without `[storage].purge_terminal_tasks_days > 0` never calls terminal-task purge.

## Acceptance Criteria

1. `[storage]` exists with defaults `14`, `0`, `0`; it is parsed independently of `[metrics]` and `[traces]`.
2. Artifact TTL purge runs by default and never deletes or updates a `tasks` row.
3. Size eviction deletes only terminal task-owned artifacts and orphan completed `turn_log` rows, oldest by completion time.
4. Size eviction terminates even when protected non-terminal bytes exceed the cap.
5. `purge_terminal_tasks_days = 0` never calls terminal-task purge.
6. Terminal-task purge with `days > 0` uses one guarded victim predicate that rejects non-terminal, in-progress, crash-resumable, and active-batch tasks.
7. Terminal-task purge explicitly deletes linked `turn_log` rows before deleting `tasks`.
8. Terminal-task purge relies on FK cascade for journal/checkpoint/start rows and has tests proving cascade behavior.
9. Warm inline `turn_log` rows with `task_id NULL` are purged by `completed_ms`, never when `completed_ms IS NULL`.
10. Boot sweep runs best-effort and cannot abort serve on store failure.
11. Periodic sweep runs hourly and logs failures without aborting serve.
12. Prometheus counters are never decremented or rewritten during retention; restart rebuilds from surviving `turn_log`.
13. Slice 2 drill-down routes return 404 or omit refs cleanly after artifact/task purge.
14. SQLite and memory stores both implement the three new `TaskStore` methods.
15. Full workspace fmt, clippy, and tests pass.

## Non-goals

- No retention HTTP API.
- No redaction.
- No metrics counter backfill/decrement after purge.
- No deletion of non-terminal `TaskRecord`s.
- No deletion of live/resumable task artifacts to satisfy a size cap.
- No new merge-state persistence.
- No physical SQLite compaction or `VACUUM`; deleted pages are reusable by SQLite.
- No OTLP/exporter changes.
- No change to bearer auth or trace-route authorization.
