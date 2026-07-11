# M4 Slice 3 — Retention under [storage] (design, rev2)

## Changes from prior draft

| Review finding | Resolution in rev2 |
|---|---|
| Review A WRONG: `turn_log.task_id IS NULL` was treated as disposable warm-inline data, but detached and batch workflow turns also currently persist `NULL`. | Retention no longer treats `NULL task_id` as an orphan signal. New detached/batch/resume callers must set `WorkflowRunContext.task_id = Some(task.clone())`; `WorkflowRunContext` already carries `task_id` at `crates/bridge-workflow/src/executor.rs:25-32`, defaults it to `None` at `crates/bridge-workflow/src/executor.rs:34-43`, and copies it into `TurnContext` at `crates/bridge-workflow/src/executor.rs:231-244`. The shipped detached/batch callers currently omit it at `crates/bridge-coordinator/src/coordinator.rs:709-714`, `crates/bridge-coordinator/src/batch.rs:868-872`, `crates/bridge-coordinator/src/detached.rs:1340-1342`, `crates/bridge-coordinator/src/detached.rs:1629-1655`, and `crates/bridge-coordinator/src/batch.rs:513-539`; rev2 fixes those call sites. SQLite persists the nullable value at `crates/bridge-store/src/sqlite.rs:764-790`, so rev2 also adds a bounded backfill for legacy workflow turns. |
| Review A WRONG: terminal-task purge deleted `turn_log WHERE task_id IN victims`, missing detached/batch legacy rows with `NULL task_id`, leaving `/turns/:id` as 200. | Terminal task purge is feature-gated and, when enabled, first backfills confirmed legacy workflow rows, then hard-deletes both linked rows and confirmed legacy `NULL` rows whose `session_id` matches the victim task’s fresh or resume run id. `/turns/:turn_id` returns 200 solely from `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`, so the purge must remove the turn row before deleting the task. |
| Review A WRONG: `completed_ms` was used as a done marker even though `TurnFinished` is queued before `UsageFinalized`. | Add a durable finalization barrier: `turn_log.usage_finalized_ms` plus `turn_log.usage_finalization_kind`. `TurnFinished` still writes `completed_ms` from `crates/bridge-observ/src/lib.rs:271-287`; `UsageFinalized` remains a later command from `crates/bridge-observ/src/lib.rs:288-296` and is processed after `Finished` at `crates/bridge-observ/src/lib.rs:221-233`. Retention candidates require `usage_finalized_ms IS NOT NULL`, preventing deletion in the window where `update_turn_usage()` would otherwise fail because the row disappeared (`crates/bridge-store/src/sqlite.rs:812-844`). |
| Review A SMELL: default-safety proof was over-broad because serve boot can update tasks during resume. | The proof is retention-scoped. Default retention can delete only artifact rows and can write retention metadata (`turn_log.task_id`, turn finalization reconciliation, `tasks.artifacts_purged_at`). It cannot delete `tasks` rows or change task lifecycle fields. Serve boot resume remains separate and may update `tasks` through `coordinator.resume()` at `bin/a2a-bridge/src/main.rs:6294-6301` and `claim_resume_attempt()` at `crates/bridge-coordinator/src/detached.rs:1594-1598`; those updates are explicitly outside the retention safety claim. |
| Review A SMELL: original anchors were real but incomplete. | Rev2 grounds ownership, finalization, cascades, working-task resume, and route behavior in actual code: status model at `crates/bridge-core/src/task_store.rs:14-46`, `TaskRecord` at `crates/bridge-core/src/task_store.rs:128-152`, SQLite cascade tables at `crates/bridge-store/src/sqlite.rs:140-164`, non-cascading `turn_log` at `crates/bridge-store/src/sqlite.rs:165-194`, and `working_tasks()` at `crates/bridge-store/src/sqlite.rs:738-746`. |
| Review B WRONG: open-world terminal delete predicate is unsafe as a TOML knob. | Decision: gate hard terminal-task deletion behind a compile-time Cargo feature named `unsafe-terminal-task-purge`, default off. Without that feature, `purge_terminal_tasks_days > 0` is a config error. This keeps irreversible task-row deletion unreachable from ordinary TOML or debug flags while avoiding a tombstone/Coordinator redesign in this slice. |
| Review B SMELL: eviction policy lived in the `TaskStore` port. | Extract retention policy to `RetentionService`. `TaskStore` gets bounded storage primitives: candidate listing, logical byte accounting, guarded artifact-set delete, guarded turn-row delete, backfill, reconciliation, and feature-gated hard task delete. Sorting, cap iteration, and sweep ordering live only in `RetentionService`. |
| Review B SMELL: half-deleted task state was illegible. | Add `TaskRecord.artifacts_purged_at: Option<i64>` and SQLite `tasks.artifacts_purged_at`. `delete_task_artifact_set()` sets it when journal/checkpoint/task-linked turn artifacts are purged, so Slice-2 routes can distinguish “known task, artifacts purged” from “task never had artifacts.” |
| Review B SMELL: boot sweep was unbounded. | Every retention primitive takes `limit: u32`; boot runs one bounded non-hard-delete pass with `STORAGE_RETENTION_BATCH_LIMIT = 10_000`. Periodic sweeps continue draining later. |
| Review B SMELL: retention bypasses the Coordinator lifecycle seam. | Hard task deletion is not a normal runtime feature; it is compile-time gated. Default and size-based retention delete only artifact rows for terminal tasks and finalized warm/stale turn rows. A future Coordinator-owned archival/tombstone flow is a non-goal for this slice. |

## Goal

Implement storage retention for M4 Slice 1–2 artifacts with deletion safety as the overriding constraint.

Slice 3 adds:

- `[storage]` config with default artifact TTL retention.
- Optional size-based artifact eviction.
- Feature-gated terminal task hard deletion.
- Durable turn-log ownership repair for detached/batch turns.
- Durable turn-log finalization barrier before any turn row can be deleted.
- Legible purged-artifact state through `TaskRecord.artifacts_purged_at`.
- Bounded boot and periodic retention sweeps.
- No dependency on `[metrics]` or `[traces]`.

Grounding in shipped code:

- `TaskRecord` currently has `status` and `updated_ms` but no `artifacts_purged_at` at `crates/bridge-core/src/task_store.rs:128-152`.
- `TaskRecordStatus::is_terminal()` treats every non-`Working` state as terminal at `crates/bridge-core/src/task_store.rs:44-46`.
- SQLite `task_node_checkpoints`, `task_node_starts`, and `task_journal` cascade from `tasks`, while `turn_log` has no FK/cascade at `crates/bridge-store/src/sqlite.rs:140-194`.
- `turn_log_usage_for_task()` and task trace refs already key on `turn_log.task_id` at `crates/bridge-store/src/sqlite.rs:895-935` and `crates/bridge-coordinator/src/coordinator.rs:771-812`.
- Prometheus rebuild reads `turn_log_rows()` at `bin/a2a-bridge/src/main.rs:6119-6124`, then `PrometheusObserver::rebuild_from_turn_log()` at `crates/bridge-observ/src/lib.rs:430`.
- Trace routes are mounted at `crates/bridge-a2a-inbound/src/server.rs:298-300`; `/turns/:id` returns 200 if `turn_log_row()` returns a row at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.

## Global Constraints

- Data safety wins over reclaiming bytes. Ambiguous legacy `NULL task_id` workflow rows are retained, not guessed.
- Rust remains `1.94.0`.
- Keep local/CI gates: `cargo fmt`, clippy with warnings denied, and full `cargo test --workspace`.
- No new unauthenticated HTTP route.
- `bridge-core` remains free of Prometheus types.
- High-cardinality IDs remain out of Prometheus labels.
- Retention is independent of `[metrics]` and `[traces]`.
- No retention operation may delete a `Working` task or artifacts of a `Working` task.
- No turn row is deletion-eligible until `usage_finalized_ms IS NOT NULL`.
- Hard `tasks` deletion is unreachable unless compiled with `--features unsafe-terminal-task-purge`.

## `[storage]` config

Add a new top-level `[storage]` table. Do not reuse `[store]`; `[store]` currently owns the durable DB path and resume cap at `bin/a2a-bridge/src/config.rs:64-68`.

```toml
[storage]
artifact_retention_days      = 14
artifact_retention_max_bytes = 0
purge_terminal_tasks_days    = 0
```

| Field | Type | Default | Validation | Meaning |
|---|---:|---:|---|---|
| `artifact_retention_days` | `u64` | `14` | `0` disables TTL; TOML negatives fail deserialize | Purge eligible task artifact sets and finalized warm/stale turn rows older than this many days. |
| `artifact_retention_max_bytes` | `u64` | `0` | `0` disables size eviction | Evict oldest eligible finalized artifact candidates until under cap or the bounded pass exhausts candidates. |
| `purge_terminal_tasks_days` | `u64` | `0` | `0` disables; `>0` requires Cargo feature `unsafe-terminal-task-purge` | Irreversibly hard-deletes terminal task rows older than this many days. Default builds reject non-zero values. |

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
        #[cfg(not(feature = "unsafe-terminal-task-purge"))]
        if self.storage.purge_terminal_tasks_days > 0 {
            return Err(ConfigError::Registry(
                "[storage].purge_terminal_tasks_days requires the unsafe-terminal-task-purge build feature".into(),
            ));
        }

        Ok(StorageConfig {
            artifact_retention_days: self.storage.artifact_retention_days,
            artifact_retention_max_bytes: self.storage.artifact_retention_max_bytes,
            purge_terminal_tasks_days: self.storage.purge_terminal_tasks_days,
        })
    }
}
```

Add to `RegistryConfig` beside the existing independent `[metrics]` and `[traces]` fields at `bin/a2a-bridge/src/config.rs:246-249`:

```rust
#[serde(default)]
pub storage: StorageToml,
```

Decision: `[storage]` is captured at serve boot like other non-registry runtime fields; hot reload continues to apply registry snapshots only.

## Store methods

### Core DTOs and trait signatures

Add to `crates/bridge-core/src/task_store.rs`.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionArtifactKind {
    Task,
    WarmTurn,
    StaleLinkedTurn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionArtifactCandidate {
    pub kind: RetentionArtifactKind,
    pub task_id: Option<TaskId>,
    pub turn_id: Option<TurnId>,
    pub completed_ms: i64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactDeleteStats {
    pub task_artifact_sets: u64,
    pub warm_turn_log_rows: u64,
    pub stale_linked_turn_log_rows: u64,
    pub journal_rows: u64,
    pub node_checkpoints: u64,
    pub turn_log_rows: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalTaskPurgeStats {
    pub task_records: u64,
    pub turn_log_rows: u64,
    pub session_rows: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnLogRepairStats {
    pub task_links_backfilled: u64,
    pub finalized_reconciled: u64,
}

#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    async fn backfill_turn_log_task_ids(&self, _limit: u32) -> Result<u64, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn reconcile_unfinalized_turns_completed_before(
        &self,
        _completed_before_ms: i64,
        _now_ms: i64,
        _limit: u32,
    ) -> Result<u64, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn logical_artifact_bytes(&self) -> Result<u64, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn list_terminal_artifact_candidates(
        &self,
        _cutoff_ms: Option<i64>,
        _limit: u32,
    ) -> Result<Vec<RetentionArtifactCandidate>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn list_reclaimable_turn_log_candidates(
        &self,
        _cutoff_ms: Option<i64>,
        _limit: u32,
    ) -> Result<Vec<RetentionArtifactCandidate>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn delete_task_artifact_set(
        &self,
        _task: &TaskId,
        _purged_at_ms: i64,
    ) -> Result<ArtifactDeleteStats, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn delete_reclaimable_turn_log_row(
        &self,
        _turn: &TurnId,
    ) -> Result<ArtifactDeleteStats, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    #[cfg(feature = "unsafe-terminal-task-purge")]
    async fn hard_delete_terminal_tasks_completed_before(
        &self,
        _cutoff_ms: i64,
        _limit: u32,
    ) -> Result<TerminalTaskPurgeStats, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
}
```

Add `artifacts_purged_at` to `TaskRecord`:

```rust
pub struct TaskRecord {
    pub id: TaskId,
    pub workflow: String,
    pub status: TaskRecordStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub input: String,
    pub workflow_spec_json: Option<String>,
    pub resume_attempts: u32,
    pub session_cwd: Option<String>,
    pub batch_id: Option<crate::ids::BatchId>,
    pub item_id: Option<String>,
    pub artifacts_purged_at: Option<i64>,
}
```

### SQLite schema and migrations

Existing schema anchors:

- `tasks` base table is created at `crates/bridge-store/src/sqlite.rs:130-138`.
- Additive task columns are migrated at `crates/bridge-store/src/sqlite.rs:204-259`.
- `turn_log` is created at `crates/bridge-store/src/sqlite.rs:165-194`.

Add to the task-column migration:

```rust
("artifacts_purged_at", "INTEGER"),
```

Update every task SELECT/INSERT/row mapping:

- `create()` INSERT at `crates/bridge-store/src/sqlite.rs:532-555` inserts `NULL` for `artifacts_purged_at`.
- `get()` and `list()` SELECTs at `crates/bridge-store/src/sqlite.rs:580-612` include `artifacts_purged_at`.
- `working_tasks()` SELECT at `crates/bridge-store/src/sqlite.rs:738-746` includes `artifacts_purged_at`.
- `row_to_task()` at `crates/bridge-store/src/sqlite.rs:1773-1805` reads it.

Add a `migrate_turn_log_columns(conn)` called after `migrate_tasks_columns(&conn)`:

```sql
ALTER TABLE turn_log ADD COLUMN usage_finalized_ms INTEGER;
ALTER TABLE turn_log ADD COLUMN usage_finalization_kind TEXT NOT NULL DEFAULT 'pending';

CREATE INDEX IF NOT EXISTS idx_turn_log_finalized
    ON turn_log(usage_finalized_ms, completed_ms);

CREATE INDEX IF NOT EXISTS idx_turn_log_session_workflow
    ON turn_log(session_id, workflow, node);
```

Each `ALTER TABLE` is conditional via `PRAGMA table_info(turn_log)`.

Legacy finalization backfill during migration:

```sql
UPDATE turn_log
SET usage_finalized_ms = completed_ms,
    usage_finalization_kind = 'legacy_usage_present'
WHERE usage_finalized_ms IS NULL
  AND completed_ms IS NOT NULL
  AND (
      input_tokens IS NOT NULL
      OR output_tokens IS NOT NULL
      OR thought_tokens IS NOT NULL
      OR cached_read_tokens IS NOT NULL
      OR cached_write_tokens IS NOT NULL
      OR cost_amount IS NOT NULL
      OR cost_currency IS NOT NULL
  );
```

Do not mark legacy rows with no usage columns during migration. They are reconciled only by the bounded runtime reconciliation pass.

### Turn-log ownership write fix

The shipped code already has the data path:

- `WorkflowRunContext.task_id` exists at `crates/bridge-workflow/src/executor.rs:25-32`.
- `WorkflowRunContext::default()` sets it to `None` at `crates/bridge-workflow/src/executor.rs:34-43`.
- `node_turn_context()` copies it to `TurnContext.task_id` at `crates/bridge-workflow/src/executor.rs:231-244`.
- SQLite stores `row.ctx.task_id` at `crates/bridge-store/src/sqlite.rs:764-790`.

Rev2 must set `task_id: Some(task.clone())` in all detached/batch workflow contexts:

```rust
WorkflowRunContext {
    session_cwd,
    make_rich_sink: None,
    observer: self.observer.clone(),
    task_id: Some(task.clone()),
    ..WorkflowRunContext::default()
}
```

Apply the same shape to:

- Fresh detached submit: `crates/bridge-coordinator/src/coordinator.rs:709-714`.
- Batch child spawn: `crates/bridge-coordinator/src/batch.rs:868-872`.
- Detached helper/test spawn: `crates/bridge-coordinator/src/detached.rs:1340-1342` and `crates/bridge-coordinator/src/detached.rs:1380-1382`.
- Detached boot resume: `crates/bridge-coordinator/src/detached.rs:1629-1655`.
- Batch boot resume: `crates/bridge-coordinator/src/batch.rs:513-539`.

Warm direct turns already set `task_id` when routed through a task at `crates/bridge-a2a-inbound/src/server.rs:595-615`; pure warm context rows may remain `NULL`.

### Turn-log finalization write fix

Change `TurnLogUsage`:

```rust
pub struct TurnLogUsage {
    pub ctx: crate::ports::TurnContext,
    pub usage: crate::orch::UsageSnapshot,
    pub usage_finalized_ms: i64,
}
```

`TurnLogObserver` sets it when receiving `ObsEvent::UsageFinalized`:

```rust
ObsEvent::UsageFinalized { ctx, usage, fin } => {
    if *fin != UsageFinalization::TurnFinal {
        return;
    }
    self.try_send(TurnLogCommand::Usage(TurnLogUsage {
        ctx: (*ctx).clone(),
        usage: (*usage).clone(),
        usage_finalized_ms: (self.now_ms)(),
    }));
}
```

SQLite `update_turn_usage()` updates the barrier in the same statement as tokens/cost:

```sql
UPDATE turn_log SET
    input_tokens = COALESCE(?2, input_tokens),
    output_tokens = COALESCE(?3, output_tokens),
    thought_tokens = COALESCE(?4, thought_tokens),
    cached_read_tokens = COALESCE(?5, cached_read_tokens),
    cached_write_tokens = COALESCE(?6, cached_write_tokens),
    cost_amount = COALESCE(?7, cost_amount),
    cost_currency = COALESCE(?8, cost_currency),
    usage_finalized_ms = ?9,
    usage_finalization_kind = 'usage'
WHERE turn_id = ?1;
```

`upsert_turn_finished()` inserts `usage_finalization_kind = 'pending'` and leaves `usage_finalized_ms` unset. On conflict it must not clear an already-finalized barrier.

### SQLite SQL primitives

All SQLite retention primitives run under `SqliteStore.conn: Arc<Mutex<rusqlite::Connection>>` and use `BEGIN IMMEDIATE` for mutation. Current single-connection locking is at `crates/bridge-store/src/sqlite.rs:15`; file-backed store locking and WAL setup are already in `SqliteStore::open`.

Logical turn byte expression includes the new finalization columns:

```sql
COALESCE(length(CAST(turn_id AS BLOB)), 0) +
COALESCE(length(CAST(session_id AS BLOB)), 0) +
COALESCE(length(CAST(task_id AS BLOB)), 0) +
COALESCE(length(CAST(workflow AS BLOB)), 0) +
COALESCE(length(CAST(node AS BLOB)), 0) +
8 +
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
COALESCE(length(CAST(traceparent AS BLOB)), 0) +
CASE WHEN usage_finalized_ms IS NULL THEN 0 ELSE 8 END +
COALESCE(length(CAST(usage_finalization_kind AS BLOB)), 0)
```

Task-link backfill:

```sql
WITH candidates AS (
    SELECT
        tl.turn_id,
        (
            SELECT t.id
            FROM tasks t
            WHERE t.workflow = tl.workflow
              AND (
                  tl.session_id = t.id
                  OR (
                      substr(tl.session_id, 1, length(t.id) + 8) = t.id || '-resume-'
                      AND length(substr(tl.session_id, length(t.id) + 9)) > 0
                      AND substr(tl.session_id, length(t.id) + 9) NOT GLOB '*[^0-9]*'
                  )
              )
            ORDER BY length(t.id) DESC
            LIMIT 1
        ) AS owner_task_id
    FROM turn_log tl
    WHERE tl.task_id IS NULL
      AND tl.workflow IS NOT NULL
      AND tl.node IS NOT NULL
      AND EXISTS (
          SELECT 1
          FROM tasks t
          WHERE t.workflow = tl.workflow
            AND (
                tl.session_id = t.id
                OR (
                    substr(tl.session_id, 1, length(t.id) + 8) = t.id || '-resume-'
                    AND length(substr(tl.session_id, length(t.id) + 9)) > 0
                    AND substr(tl.session_id, length(t.id) + 9) NOT GLOB '*[^0-9]*'
                )
            )
      )
    ORDER BY tl.completed_ms ASC, tl.turn_id ASC
    LIMIT :limit
)
UPDATE turn_log
SET task_id = (
    SELECT owner_task_id
    FROM candidates c
    WHERE c.turn_id = turn_log.turn_id
)
WHERE turn_id IN (
    SELECT turn_id FROM candidates WHERE owner_task_id IS NOT NULL
);
```

Finalization reconciliation for completed rows that produced no usage:

```sql
WITH victims AS (
    SELECT turn_id
    FROM turn_log
    WHERE completed_ms IS NOT NULL
      AND completed_ms < :completed_before_ms
      AND usage_finalized_ms IS NULL
      AND input_tokens IS NULL
      AND output_tokens IS NULL
      AND thought_tokens IS NULL
      AND cached_read_tokens IS NULL
      AND cached_write_tokens IS NULL
      AND cost_amount IS NULL
      AND cost_currency IS NULL
    ORDER BY completed_ms ASC, turn_id ASC
    LIMIT :limit
)
UPDATE turn_log
SET usage_finalized_ms = :now_ms,
    usage_finalization_kind = 'reconciled_no_usage'
WHERE turn_id IN (SELECT turn_id FROM victims);
```

Terminal task artifact candidates:

```sql
WITH terminal_tasks AS (
    SELECT t.id, t.updated_ms
    FROM tasks t
    WHERE t.status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND (:cutoff_ms IS NULL OR t.updated_ms < :cutoff_ms)
      AND NOT EXISTS (
          SELECT 1
          FROM turn_log tl
          WHERE tl.task_id = t.id
            AND (tl.completed_ms IS NULL OR tl.usage_finalized_ms IS NULL)
      )
    ORDER BY t.updated_ms ASC, t.id ASC
    LIMIT :limit
),
sized AS (
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
            SELECT SUM(/* turn byte expression */)
            FROM turn_log tl
            WHERE tl.task_id = t.id
        ), 0) AS bytes
    FROM terminal_tasks t
)
SELECT kind, task_id, turn_id, completed_ms, bytes
FROM sized
WHERE bytes > 0
ORDER BY completed_ms ASC, task_id ASC;
```

Warm/stale turn candidates:

```sql
WITH candidates AS (
    SELECT
        CASE
            WHEN tl.task_id IS NULL THEN 'warm_turn'
            ELSE 'stale_linked_turn'
        END AS kind,
        tl.task_id,
        tl.turn_id,
        tl.completed_ms,
        /* turn byte expression */ AS bytes
    FROM turn_log tl
    WHERE tl.completed_ms IS NOT NULL
      AND tl.usage_finalized_ms IS NOT NULL
      AND (:cutoff_ms IS NULL OR tl.completed_ms < :cutoff_ms)
      AND (
          (
              tl.task_id IS NULL
              AND tl.workflow IS NULL
              AND tl.node IS NULL
          )
          OR (
              tl.task_id IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = tl.task_id)
          )
      )
    ORDER BY tl.completed_ms ASC, tl.turn_id ASC
    LIMIT :limit
)
SELECT kind, task_id, turn_id, completed_ms, bytes
FROM candidates
WHERE bytes > 0;
```

Guarded task artifact delete:

```sql
BEGIN IMMEDIATE;

WITH guard AS (
    SELECT t.id
    FROM tasks t
    WHERE t.id = :task_id
      AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND NOT EXISTS (
          SELECT 1
          FROM turn_log tl
          WHERE tl.task_id = t.id
            AND (tl.completed_ms IS NULL OR tl.usage_finalized_ms IS NULL)
      )
)
DELETE FROM task_journal
WHERE task_id IN (SELECT id FROM guard);

WITH guard AS (
    SELECT t.id
    FROM tasks t
    WHERE t.id = :task_id
      AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND NOT EXISTS (
          SELECT 1
          FROM turn_log tl
          WHERE tl.task_id = t.id
            AND (tl.completed_ms IS NULL OR tl.usage_finalized_ms IS NULL)
      )
)
DELETE FROM task_node_checkpoints
WHERE task_id IN (SELECT id FROM guard);

WITH guard AS (
    SELECT t.id
    FROM tasks t
    WHERE t.id = :task_id
      AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
      AND NOT EXISTS (
          SELECT 1
          FROM turn_log tl
          WHERE tl.task_id = t.id
            AND (tl.completed_ms IS NULL OR tl.usage_finalized_ms IS NULL)
      )
)
DELETE FROM turn_log
WHERE task_id IN (SELECT id FROM guard);

WITH guard AS (
    SELECT t.id
    FROM tasks t
    WHERE t.id = :task_id
      AND t.status IN ('completed', 'failed', 'canceled', 'interrupted')
)
UPDATE tasks
SET artifacts_purged_at = COALESCE(artifacts_purged_at, :purged_at_ms)
WHERE id IN (SELECT id FROM guard);

COMMIT;
```

Guarded standalone turn delete:

```sql
BEGIN IMMEDIATE;

DELETE FROM turn_log
WHERE turn_id = :turn_id
  AND completed_ms IS NOT NULL
  AND usage_finalized_ms IS NOT NULL
  AND (
      (
          task_id IS NULL
          AND workflow IS NULL
          AND node IS NULL
      )
      OR (
          task_id IS NOT NULL
          AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id = turn_log.task_id)
      )
  );

COMMIT;
```

Feature-gated terminal task hard delete:

```sql
BEGIN IMMEDIATE;

CREATE TEMP TABLE retention_task_victims(
    id TEXT PRIMARY KEY,
    workflow TEXT NOT NULL
) WITHOUT ROWID;

INSERT INTO retention_task_victims(id, workflow)
SELECT t.id, t.workflow
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
  )
  AND NOT EXISTS (
      SELECT 1
      FROM turn_log tl
      WHERE (
          tl.task_id = t.id
          OR (
              tl.task_id IS NULL
              AND tl.workflow IS NOT NULL
              AND tl.node IS NOT NULL
              AND tl.workflow = t.workflow
              AND (
                  tl.session_id = t.id
                  OR (
                      substr(tl.session_id, 1, length(t.id) + 8) = t.id || '-resume-'
                      AND length(substr(tl.session_id, length(t.id) + 9)) > 0
                      AND substr(tl.session_id, length(t.id) + 9) NOT GLOB '*[^0-9]*'
                  )
              )
          )
      )
      AND (tl.completed_ms IS NULL OR tl.usage_finalized_ms IS NULL)
  )
ORDER BY t.updated_ms ASC, t.id ASC
LIMIT :limit;

DELETE FROM turn_log
WHERE task_id IN (SELECT id FROM retention_task_victims)
   OR (
       task_id IS NULL
       AND workflow IS NOT NULL
       AND node IS NOT NULL
       AND EXISTS (
           SELECT 1
           FROM retention_task_victims v
           WHERE v.workflow = turn_log.workflow
             AND (
                 turn_log.session_id = v.id
                 OR (
                     substr(turn_log.session_id, 1, length(v.id) + 8) = v.id || '-resume-'
                     AND length(substr(turn_log.session_id, length(v.id) + 9)) > 0
                     AND substr(turn_log.session_id, length(v.id) + 9) NOT GLOB '*[^0-9]*'
                 )
             )
       )
   );

DELETE FROM sessions
WHERE task_id IN (SELECT id FROM retention_task_victims);

DELETE FROM tasks
WHERE id IN (SELECT id FROM retention_task_victims);

DROP TABLE retention_task_victims;

COMMIT;
```

### Memory shape

`MemoryTaskStore` maps are at `crates/bridge-core/src/task_store.rs:600-619`.

Required changes:

- Add `artifacts_purged_at` to every `TaskRecord` constructor in tests and memory helpers.
- Keep using `journal_fold_guard` for multi-map retention mutation.
- `backfill_turn_log_task_ids(limit)`:
  - Build a snapshot of `inner` task rows by `(workflow, id)`.
  - For each `turn_log` row with `task_id == None`, `workflow.is_some()`, and `node.is_some()`, match `session_id == task.id` or `session_id == "{task}-resume-{digits}"` with equal workflow.
  - Set `row.task_id = Some(task.id.clone())` for at most `limit`.
- `reconcile_unfinalized_turns_completed_before()`:
  - For rows with `completed_ms < completed_before_ms`, no usage/cost fields, and no barrier, set `usage_finalized_ms`/kind in the memory row. This requires adding the same finalization fields to `TurnLogRow`.
- `list_terminal_artifact_candidates()`:
  - Snapshot terminal task rows from `inner`.
  - Exclude tasks with any turn row for that task whose `completed_ms` is `None` or barrier is unset.
  - Sum journal/checkpoint/turn logical bytes.
- `list_reclaimable_turn_log_candidates()`:
  - Include only finalized pure warm rows (`task_id == None`, `workflow == None`, `node == None`) and finalized stale linked rows whose task ID is absent from `inner`.
  - Never include `task_id == None` workflow/node rows.
- `delete_task_artifact_set()`:
  - Re-check terminal status and finalization barrier.
  - Remove `journals[task]`, checkpoints keyed by task, and linked turn rows.
  - Set `inner[task].artifacts_purged_at = Some(existing.unwrap_or(purged_at_ms))`.
  - Never remove `inner`.
- Feature-gated hard delete:
  - Under `journal_fold_guard`, collect limited victims with the same terminal/start/batch/session-equivalent guards available in memory.
  - Remove victims from `inner`, `journals`, `checkpoints`, `starts`, `terminal_seqs`, `seq_counters`, `birth`, and linked/confirmed legacy turn rows.

## Purge algorithms

All policy lives in a new `RetentionService` outside `TaskStore`.

```rust
pub const STORAGE_RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const STORAGE_RETENTION_BATCH_LIMIT: u32 = 10_000;
pub const UNFINALIZED_TURN_RECONCILE_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
pub const MS_PER_DAY: u64 = 86_400_000;

pub struct RetentionService {
    store: Arc<dyn TaskStore>,
    cfg: StorageConfig,
    limit: u32,
}
```

Cutoff helper:

```rust
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

### Repair pass

Guard predicates:

- Backfill only `NULL task_id` rows with `workflow IS NOT NULL` and `node IS NOT NULL`.
- Backfill only when a real `tasks` row exists with the same workflow and a confirmed fresh/resume session ID.
- Reconcile no-usage finalization only after a 24-hour grace and only if no usage/cost columns were ever written.

Pseudo-code:

```rust
async fn repair_turn_log_metadata(&self, now_ms: i64) -> Result<TurnLogRepairStats, BridgeError> {
    let task_links_backfilled = self.store.backfill_turn_log_task_ids(self.limit).await?;

    let completed_before_ms = now_ms.saturating_sub(
        i64::try_from(UNFINALIZED_TURN_RECONCILE_GRACE.as_millis()).unwrap_or(i64::MAX),
    );

    let finalized_reconciled = self
        .store
        .reconcile_unfinalized_turns_completed_before(
            completed_before_ms,
            now_ms,
            self.limit,
        )
        .await?;

    Ok(TurnLogRepairStats {
        task_links_backfilled,
        finalized_reconciled,
    })
}
```

This pass performs metadata repair only. It does not delete user data.

### Artifact TTL purge

Guard predicate:

- Task artifact set: task is terminal, `tasks.updated_ms < cutoff`, all linked turns are finalized, and candidate has positive artifact bytes.
- Pure warm turn row: `task_id IS NULL`, `workflow IS NULL`, `node IS NULL`, `completed_ms < cutoff`, and `usage_finalized_ms IS NOT NULL`.
- Stale linked turn row: `task_id IS NOT NULL`, no matching `tasks` row exists, `completed_ms < cutoff`, and `usage_finalized_ms IS NOT NULL`.
- `task_id IS NULL` workflow/node rows are never TTL-deleted unless backfilled to a real task first.

Pseudo-code:

```rust
async fn purge_artifacts_completed_before(
    &self,
    cutoff_ms: i64,
    now_ms: i64,
) -> Result<ArtifactDeleteStats, BridgeError> {
    self.repair_turn_log_metadata(now_ms).await?;

    let before = self.store.logical_artifact_bytes().await?;
    let mut stats = ArtifactDeleteStats {
        bytes_before: before,
        ..ArtifactDeleteStats::default()
    };

    let mut candidates = self
        .store
        .list_terminal_artifact_candidates(Some(cutoff_ms), self.limit)
        .await?;
    candidates.extend(
        self.store
            .list_reclaimable_turn_log_candidates(Some(cutoff_ms), self.limit)
            .await?,
    );

    candidates.sort_by(|a, b| {
        (a.completed_ms, candidate_sort_key(a))
            .cmp(&(b.completed_ms, candidate_sort_key(b)))
    });

    for candidate in candidates.into_iter().take(self.limit as usize) {
        match candidate.kind {
            RetentionArtifactKind::Task => {
                let task = candidate.task_id.as_ref().ok_or(BridgeError::StoreFailure)?;
                stats += self.store.delete_task_artifact_set(task, now_ms).await?;
            }
            RetentionArtifactKind::WarmTurn | RetentionArtifactKind::StaleLinkedTurn => {
                let turn = candidate.turn_id.as_ref().ok_or(BridgeError::StoreFailure)?;
                stats += self.store.delete_reclaimable_turn_log_row(turn).await?;
            }
        }
    }

    stats.bytes_after = self.store.logical_artifact_bytes().await?;
    Ok(stats)
}
```

Deletion effect:

- Deletes `task_journal`, `task_node_checkpoints`, and linked `turn_log` rows for eligible terminal task artifact sets.
- Sets `tasks.artifacts_purged_at` for those retained task rows.
- Deletes finalized pure warm and finalized stale linked `turn_log` rows.
- Never deletes `tasks`.
- Never deletes ambiguous legacy `NULL task_id` workflow rows.
- Never deletes a row whose usage barrier is pending.

### Size eviction

Guard predicate:

- Same candidate eligibility as artifact TTL, but without age cutoff.
- Pending usage rows are protected.
- Working task artifacts are protected.
- Ambiguous legacy `NULL task_id` workflow rows are protected.
- Protected bytes may keep total bytes above cap.

Pseudo-code:

```rust
async fn evict_artifacts_to_max_bytes_pass(
    &self,
    max_bytes: u64,
    now_ms: i64,
) -> Result<(ArtifactDeleteStats, bool), BridgeError> {
    self.repair_turn_log_metadata(now_ms).await?;

    let before = self.store.logical_artifact_bytes().await?;
    let mut stats = ArtifactDeleteStats {
        bytes_before: before,
        ..ArtifactDeleteStats::default()
    };

    if before <= max_bytes {
        stats.bytes_after = before;
        return Ok((stats, true));
    }

    let mut candidates = self
        .store
        .list_terminal_artifact_candidates(None, self.limit)
        .await?;
    candidates.extend(
        self.store
            .list_reclaimable_turn_log_candidates(None, self.limit)
            .await?,
    );

    candidates.sort_by(|a, b| {
        (a.completed_ms, candidate_sort_key(a))
            .cmp(&(b.completed_ms, candidate_sort_key(b)))
    });

    let mut current = before;

    for candidate in candidates.into_iter().take(self.limit as usize) {
        if current <= max_bytes {
            break;
        }

        match candidate.kind {
            RetentionArtifactKind::Task => {
                let task = candidate.task_id.as_ref().ok_or(BridgeError::StoreFailure)?;
                stats += self.store.delete_task_artifact_set(task, now_ms).await?;
            }
            RetentionArtifactKind::WarmTurn | RetentionArtifactKind::StaleLinkedTurn => {
                let turn = candidate.turn_id.as_ref().ok_or(BridgeError::StoreFailure)?;
                stats += self.store.delete_reclaimable_turn_log_row(turn).await?;
            }
        }

        current = current.saturating_sub(candidate.bytes);
    }

    let after = self.store.logical_artifact_bytes().await?;
    stats.bytes_after = after;
    Ok((stats, after <= max_bytes))
}
```

`cap_reached = false` means one of:

- Protected live/pending/ambiguous bytes exceed the cap.
- The bounded pass deleted `limit` candidates and more eligible candidates may remain for a future sweep.

### Terminal-task opt-in purge

Decision: no tombstone in Slice 3. Hard task deletion is available only in builds compiled with `unsafe-terminal-task-purge`; default builds reject non-zero `purge_terminal_tasks_days`.

Guard predicate:

- `tasks.status IN ('completed','failed','canceled','interrupted')`.
- `tasks.updated_ms < cutoff`.
- No `task_node_starts` rows.
- Parent batch is absent or terminal (`completed`, `failed`, `canceled`); active batches are `working`/`canceling` at `crates/bridge-store/src/sqlite.rs:1065-1074`.
- No same-DB pending `sessions` row; pending columns exist at `crates/bridge-store/src/sqlite.rs:120-128` and are written at `crates/bridge-store/src/sqlite.rs:388-403`.
- No owned turn row has pending finalization.
- Victim count is bounded by `limit`.
- Confirmed legacy `NULL` workflow turns for the victim are deleted together with linked turns.

Pseudo-code:

```rust
#[cfg(feature = "unsafe-terminal-task-purge")]
async fn purge_terminal_tasks_completed_before(
    &self,
    cutoff_ms: i64,
    now_ms: i64,
) -> Result<TerminalTaskPurgeStats, BridgeError> {
    self.repair_turn_log_metadata(now_ms).await?;
    self.store
        .hard_delete_terminal_tasks_completed_before(cutoff_ms, self.limit)
        .await
}
```

Default builds do not compile this call path. `RetentionService` uses `#[cfg]` around the terminal purge phase, and `storage_config()` rejects non-zero values without the feature.

## Sweep scheduling

Add a storage-retention runner in `bin/a2a-bridge/src/main.rs`.

Existing anchors:

- Store is opened at `bin/a2a-bridge/src/main.rs:6088-6095`.
- Prometheus rebuild currently happens at `bin/a2a-bridge/src/main.rs:6119-6124`.
- Warm idle reaping already uses a periodic task at `bin/a2a-bridge/src/main.rs:6198-6207`.
- Coordinator resume runs before binding at `bin/a2a-bridge/src/main.rs:6294-6301`.

Boot order:

1. Open/build `task_store`.
2. Parse `storage_config()`. Config errors abort boot, matching `[metrics]`/`[traces]` validation style at `bin/a2a-bridge/src/config.rs:1169-1205`.
3. Run one bounded boot retention pass before Prometheus rebuild:
   - turn-link backfill,
   - no-usage finalization reconciliation,
   - artifact TTL purge if enabled,
   - size eviction if enabled,
   - no hard terminal-task deletion in this pre-resume boot pass.
4. Rebuild Prometheus from surviving `turn_log` rows.
5. Build observer/server/coordinator.
6. Run `coordinator.resume().await`.
7. Spawn periodic retention task after resume. The first periodic tick may run immediately and may include feature-gated hard task deletion.

Boot pass:

```rust
async fn run_storage_retention_boot_once(
    retention: RetentionService,
    now_ms: i64,
) {
    if let Err(e) = retention.run_non_hard_delete_pass(now_ms).await {
        tracing::warn!(error = ?e, "storage retention: bounded boot sweep failed");
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
        if let Err(e) = retention.run_periodic_pass(now_ms).await {
            tracing::warn!(error = ?e, "storage retention: periodic sweep failed");
        }
    }
});
```

`run_non_hard_delete_pass()` order:

1. Repair turn-log metadata.
2. Artifact TTL purge if `artifact_retention_days > 0`.
3. Size eviction if `artifact_retention_max_bytes > 0`.

`run_periodic_pass()` order:

1. `run_non_hard_delete_pass()`.
2. If compiled with `unsafe-terminal-task-purge` and `purge_terminal_tasks_days > 0`, run bounded terminal hard delete.

Failure behavior:

- Config errors abort boot.
- Sweep execution errors never abort serve.
- Sweep execution errors log `warn`.
- Each primitive is bounded by `STORAGE_RETENTION_BATCH_LIMIT`.
- No `VACUUM` runs inside retention.

## Data-Safety Guards

1. **Prevents live detached/batch turn deletion (closes Review A WRONG #1):** `NULL task_id` no longer means orphan. Retention deletes pure warm rows only when `workflow IS NULL AND node IS NULL`; workflow/node rows with `NULL task_id` are protected unless backfilled to a real task.

2. **Prevents new detached/batch `NULL task_id` rows (closes Review A WRONG #1):** all detached, batch, and resume `WorkflowRunContext` construction sites set `task_id: Some(task.clone())`.

3. **Prevents legacy detached/batch live-row deletion (closes Review A WRONG #1):** backfilled rows join to their real task, so `Working` tasks are excluded by terminal-task artifact candidate predicates.

4. **Prevents leaked `/turns/:id` after terminal hard purge (closes Review A WRONG #2):** feature-gated hard purge deletes both `turn_log.task_id IN victims` and confirmed legacy `NULL` workflow rows before deleting `tasks`.

5. **Prevents usage/cost loss between `TurnFinished` and `UsageFinalized` (closes Review A WRONG #3):** no retention delete predicate accepts a turn row unless `usage_finalized_ms IS NOT NULL`.

6. **Prevents deleting no-usage turns immediately after finish (closes Review A WRONG #3):** no-usage reconciliation requires a 24-hour grace and all usage/cost columns still null.

7. **Prevents default task-row deletion (closes Review A SMELL):** default config has `purge_terminal_tasks_days = 0`; default builds also reject non-zero hard-delete config unless compiled with `unsafe-terminal-task-purge`.

8. **Scopes default retention safety to retention only (closes Review A SMELL):** retention may delete artifact rows and write retention metadata; serve boot resume may independently update `tasks` through the Coordinator and is not part of this proof.

9. **Prevents debug/trace/metrics-triggered hard deletion (closes Review B WRONG):** hard task deletion depends only on compile-time feature plus `[storage].purge_terminal_tasks_days > 0`, never on debug, trace, metrics, or verbosity flags.

10. **Prevents open-world TOML foot-gun (closes Review B WRONG):** ordinary binaries cannot enable terminal hard delete from TOML; a developer must deliberately compile `unsafe-terminal-task-purge`.

11. **Prevents deletion of in-progress node state:** terminal hard purge rejects any task with `task_node_starts` rows.

12. **Prevents unfinished batch child deletion:** terminal hard purge rejects children whose parent batch is `working` or `canceling`.

13. **Prevents pending permission/auth loss:** terminal hard purge rejects same-DB `sessions` rows with `pending_request_id` or `pending_kind`.

14. **Prevents artifact deletion for live tasks:** artifact TTL and size eviction list only terminal task artifact candidates, finalized pure warm turns, and finalized stale linked turns.

15. **Prevents ambiguous legacy data loss:** `task_id IS NULL AND workflow IS NOT NULL AND node IS NOT NULL` rows with no confirmed task backfill are never TTL- or size-deleted.

16. **Prevents infinite size eviction:** size eviction operates on one materialized candidate batch of at most `limit`.

17. **Prevents boot stalls:** boot runs one bounded pass; large backlogs drain over future periodic sweeps.

18. **Prevents dangerous TTL overflow:** overflow in `days * MS_PER_DAY` maps to `cutoff = i64::MIN`, which matches no normal rows.

19. **Prevents negative config ambiguity:** unsigned config fields reject negative TOML before serve starts.

20. **Makes half-deleted state legible (closes Review B SMELL):** task artifact purges set `artifacts_purged_at`, allowing routes and operators to distinguish retention from never-created artifacts.

## Counter reconciliation

Retention never decrements live Prometheus counters.

Current shipped behavior:

- Serve reads `turn_log_rows()` at `bin/a2a-bridge/src/main.rs:6119-6124`.
- `PrometheusObserver::rebuild_from_turn_log()` rebuilds counters from those rows at `crates/bridge-observ/src/lib.rs:430`.

After Slice 3:

- Boot retention runs before rebuild, so rebuilt counters reflect surviving rows.
- Periodic retention after rebuild does not decrement counters.
- On restart, counters rebuild from surviving rows and omit purged turns.
- Turn-link backfill improves task usage aggregation because `turn_log_usage_for_task()` already filters by `task_id` at `crates/bridge-store/src/sqlite.rs:895-935`.
- Finalization reconciliation does not synthesize token/cost values; it only marks old no-usage rows as safe to delete.

## Slice-1/2 cohesion

Slice 1:

- `turn_log` remains the source for metric rebuild and usage aggregation.
- `TurnFinished` and `UsageFinalized` ordering remains unchanged; retention adds the durable barrier instead of changing event order.
- Purging finalized `turn_log` rows reduces future rebuilt totals.
- Linked detached/batch workflow turns now contribute to task usage because their `task_id` is populated.

Slice 2:

- `/turns/:turn_id` returns 404 after a turn row is purged because the route is backed directly by `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.
- `/tasks/:id/journal.jsonl` and `/tasks/:id/artifacts/:node` must inspect `TaskRecord.artifacts_purged_at` after `get()` succeeds:
  - if the artifact row/body is missing and `artifacts_purged_at.is_some()`, return `410 Gone` with JSON `{ "error": "artifacts purged", "artifacts_purged_at": <ms> }`;
  - if `artifacts_purged_at.is_none()`, preserve current 404 behavior.
- `tasks/get` includes `artifacts_purged_at` in its JSON/debug surface where it currently emits task fields around `crates/bridge-a2a-inbound/src/server.rs:3888-3947`.
- Coordinator trace refs omit journal/artifact refs once checkpoints/journal rows are gone; existing lookup points are `crates/bridge-coordinator/src/coordinator.rs:771-812`.

Assumption: no shipped transcript table exists in the SQLite schema; Slice 3 covers `task_journal`, `task_node_checkpoints`, and `turn_log`.

## Testing

- `storage_config_defaults_artifact_ttl_on_task_purge_off`: parsing without `[storage]` yields `artifact_retention_days = 14`, `artifact_retention_max_bytes = 0`, `purge_terminal_tasks_days = 0`.
- `storage_config_independent_from_metrics_and_traces`: storage parses with metrics/traces disabled.
- `storage_config_rejects_negative_values`: negative storage integers fail deserialize.
- `storage_config_rejects_terminal_purge_without_feature`: without `unsafe-terminal-task-purge`, `purge_terminal_tasks_days > 0` returns `ConfigError`.
- `retention_cutoff_zero_disables`: `days = 0` returns `None`.
- `retention_cutoff_huge_saturates_to_min`: overflow days returns `Some(i64::MIN)` and deletes no normal rows.
- `sqlite_backfills_detached_fresh_turn_task_id`: legacy row with `task_id NULL`, workflow/node set, and `session_id = task.id` gets linked to the task.
- `sqlite_backfills_detached_resume_turn_task_id`: legacy row with `session_id = "{task}-resume-1"` gets linked.
- `sqlite_backfills_batch_child_turn_task_id`: batch child workflow row gets linked to the child task.
- `sqlite_backfill_refuses_ambiguous_workflow_turn_without_task`: workflow/node row with no matching task remains `NULL`.
- `sqlite_artifact_ttl_preserves_live_detached_legacy_null_turn`: old completed turn row for a `Working` detached task is backfilled, then retained because the task is not terminal. Regression for Review A WRONG #1.
- `sqlite_size_eviction_preserves_live_detached_legacy_null_turn`: same setup with `artifact_retention_max_bytes = 1`; row remains. Regression for Review A WRONG #1.
- `sqlite_terminal_purge_deletes_legacy_null_detached_turn`: with feature enabled, old terminal task plus legacy `NULL` workflow turn is hard-deleted and `/turns/:id` returns 404. Regression for Review A WRONG #2.
- `sqlite_terminal_purge_deletes_legacy_null_batch_turn`: same for batch child.
- `sqlite_turn_finished_not_deletion_eligible_before_usage_finalized`: row with `completed_ms` but `usage_finalized_ms NULL` is not listed for TTL or size eviction. Regression for Review A WRONG #3.
- `sqlite_usage_finalized_sets_barrier_and_tokens_atomically`: `update_turn_usage()` writes token/cost fields and `usage_finalized_ms` in one update.
- `sqlite_size_eviction_cannot_delete_between_finished_and_usage`: cap below row size does not delete a finished-but-unfinalized row; after usage finalization it becomes eligible.
- `sqlite_reconcile_no_usage_requires_grace`: completed no-usage row younger than 24h remains pending.
- `sqlite_reconcile_no_usage_marks_old_row_finalized`: completed no-usage row older than 24h gets `usage_finalization_kind = reconciled_no_usage`.
- `sqlite_default_never_deletes_task_record`: default retention removes old terminal artifacts and sets `artifacts_purged_at`, but `TaskStore::get()` still returns the same task.
- `memory_default_never_deletes_task_record`: same assertion for `MemoryTaskStore`.
- `sqlite_artifact_ttl_deletes_only_terminal_task_artifacts`: old terminal artifacts are deleted; old working artifacts remain.
- `sqlite_artifact_ttl_deletes_pure_warm_turn_only`: `task_id NULL`, `workflow NULL`, `node NULL`, finalized old row is deleted; `completed_ms NULL` or unfinalized rows remain.
- `sqlite_artifact_ttl_preserves_null_workflow_node_rows`: `task_id NULL`, workflow/node set, no confirmed task link remains even if old.
- `sqlite_artifact_ttl_deletes_stale_nonnull_orphan_turn_log`: finalized row with non-null missing task ID is deleted.
- `sqlite_artifacts_purged_at_set_on_task_artifact_delete`: task remains and marker is set to purge time.
- `trace_routes_return_410_for_purged_known_task_artifacts`: known task with `artifacts_purged_at` and missing journal/checkpoint returns 410.
- `trace_routes_return_404_for_never_created_artifacts`: known task without marker and missing artifact returns current 404.
- `sqlite_size_eviction_orders_by_completion_time`: older eligible candidates delete before newer ones.
- `sqlite_size_eviction_never_evicts_working_task_data`: working task artifacts remain even when cap is below total bytes.
- `sqlite_size_eviction_protected_bytes_can_exceed_cap`: method exits with `cap_reached = false` when protected bytes remain above cap.
- `sqlite_size_eviction_honors_batch_limit`: one pass deletes at most `STORAGE_RETENTION_BATCH_LIMIT` candidates.
- `memory_size_eviction_matches_sqlite_candidate_rules`: memory candidate filtering matches SQLite for terminal, warm, stale, pending, and ambiguous rows.
- `sqlite_terminal_purge_refuses_working_task`: old `Working` task is retained.
- `sqlite_terminal_purge_refuses_task_with_node_start`: terminal task with `task_node_starts` row is retained.
- `sqlite_terminal_purge_refuses_active_batch_child`: child under `batch.status = working` or `canceling` is retained.
- `sqlite_terminal_purge_refuses_pending_session`: terminal task with pending permission/auth session is retained.
- `sqlite_terminal_purge_refuses_unfinalized_turn`: terminal task with owned pending turn finalization is retained.
- `sqlite_terminal_purge_cascades_child_tables`: hard-deleting a victim removes journal/checkpoints/starts through FK cascade.
- `sqlite_terminal_purge_cleans_same_db_sessions`: same-DB session rows for victims are deleted.
- `memory_terminal_purge_removes_all_task_maps`: feature-gated memory hard delete removes all task-owned maps and linked/confirmed legacy turns.
- `prometheus_rebuild_after_purge_uses_surviving_turn_log`: fresh observer rebuild includes only surviving rows.
- `storage_boot_sweep_is_bounded`: fake store records all boot calls with `limit = STORAGE_RETENTION_BATCH_LIMIT`.
- `storage_sweep_failure_does_not_abort_serve`: fake store returns `StoreFailure`; boot/periodic sweep logs warn and serve construction continues.
- `storage_sweep_not_triggered_by_debug_or_trace_flags`: debug/metrics/traces flags do not call terminal hard delete.
- `full_workspace_tests_pass`: `cargo fmt`, clippy with warnings denied, and `cargo test --workspace` pass.

## Acceptance criteria

1. `[storage]` exists with defaults `14`, `0`, `0`.
2. `purge_terminal_tasks_days > 0` is rejected unless built with `unsafe-terminal-task-purge`.
3. Detached, batch, and resume workflow turns set `TurnContext.task_id`.
4. Legacy detached/batch workflow turns are backfilled only when matched to a real task by workflow plus fresh/resume session ID.
5. `task_id NULL` is never used as an orphan predicate.
6. Ambiguous legacy `NULL task_id` workflow rows are retained.
7. Every turn-row delete requires `usage_finalized_ms IS NOT NULL`.
8. `UsageFinalized` writes token/cost fields and the finalization barrier atomically.
9. No-usage reconciliation is explicit, bounded, and grace-delayed.
10. Default retention never deletes a `tasks` row.
11. Default retention may delete only artifact rows and may write only retention metadata.
12. Artifact purge sets `tasks.artifacts_purged_at`.
13. Slice-2 artifact routes distinguish purged known-task artifacts from never-created artifacts.
14. Size eviction policy lives in `RetentionService`, not in `TaskStore`.
15. `TaskStore` exposes bounded storage primitives, not eviction loops.
16. Boot retention runs one bounded non-hard-delete pass before Prometheus rebuild.
17. Periodic retention runs hourly after Coordinator resume.
18. Hard terminal-task purge, when feature-enabled, deletes linked and confirmed legacy turn rows before deleting task rows.
19. Terminal hard purge rejects working, in-progress, pending-session, active-batch, and unfinalized-turn tasks.
20. Prometheus counters are not decremented by retention; restart rebuilds from surviving rows.
21. SQLite and memory stores implement the new primitives.
22. Full workspace fmt, clippy, and tests pass.

## Non-goals

- No retention HTTP API.
- No redaction.
- No physical SQLite compaction or `VACUUM`.
- No Prometheus counter decrement/backfill after periodic purge.
- No deletion of working task records.
- No deletion of live/resumable task artifacts to satisfy a size cap.
- No deletion of ambiguous legacy `NULL task_id` workflow turns.
- No tombstone/Coordinator archival redesign in Slice 3.
- No new merge-state persistence.
- No OTLP/exporter changes.
- No bearer-auth or trace-route authorization changes.
