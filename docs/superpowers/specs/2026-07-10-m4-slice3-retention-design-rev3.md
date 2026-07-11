# M4 Slice 3 — Retention under [storage] (design, rev3)

## Changes from rev2

| Re-review finding | Resolution in rev3 |
|---|---|
| Codex A1 WRONG: terminal purge can delete another live task’s legacy `NULL task_id` turn because `task-a` matches `task-a-resume-1`. | **Cut terminal task deletion entirely.** Slice 3 has no hard-delete query, `purge_terminal_tasks_days`, Cargo feature, or `TaskStore` task-delete method. The collision is constructible because task/context IDs accept any non-empty string at `crates/bridge-core/src/ids.rs:5-37`, while resume IDs use `"{task}-resume-{attempt}"` at `crates/bridge-coordinator/src/detached.rs:1672`. Retention never parses that convention and never deletes `tasks`. |
| Codex A2 WRONG: NULL usage columns can mean a lost `UsageFinalized`, not genuine no usage. | `ObsEvent::UsageFinalized` carries `Option<&UsageSnapshot>`. Producers emit `Some(usage)` or an explicit `None`; only that producer-authored finalization command sets the durable barrier. A dropped command or crash leaves the row `pending` forever. There is no timer-based no-usage reconciliation. Current finish/usage commands are independently queued at `crates/bridge-observ/src/lib.rs:193-239` and can be dropped by `try_send` at `crates/bridge-observ/src/lib.rs:257-264`. |
| Codex A3 SMELL: `spawn_detached_workflow` remains an ownership bypass. | In addition to fixing callers, `spawn_detached_workflow` overwrites `ctx.task_id = Some(task.clone())` immediately before calling `run_from_with_context`; the central boundary is at `crates/bridge-coordinator/src/detached.rs:1193-1203` and `crates/bridge-coordinator/src/detached.rs:1252-1258`. |
| Codex A4 SMELL: migration performs an unbounded row update. | Migration is DDL-only: add columns and recreate the eligibility view/indexes. No migration or runtime sweep updates legacy rows. |
| Codex A5 WRONG: the Cargo feature is not coherently forwarded across crates. | **Cut the feature and all feature-gated code.** No feature forwarding or feature-matrix CI is required. |
| Fable B1 WRONG: the riskiest hard-delete SQL would ship outside the specified test matrix. | **Cut the SQL and feature.** Aged terminal-task-row deletion is deferred to a future Coordinator-owned tombstone/archival slice. `TaskRecord` is small metadata; Slice 3 reclaims journal, checkpoint, and turn-log payloads. |
| Fable B2 SMELL: `artifact_retention_max_bytes` measures an unobservable synthetic quantity and has no age floor. | **Defer size eviction.** `[storage]` contains TTL only. This removes the logical-byte expression, cap loop, and false ceiling promise. Every TTL candidate must also be at least 24 hours past both completion and usage finalization. |
| Fable B3 SMELL: runtime backfill makes the resume-session naming convention permanent SQL. | **Drop all task-ID backfill and session-ID matching.** New writes carry `task_id`; legacy `NULL task_id` workflow/node rows remain protected indefinitely. |
| Fable B4 SMELL: correctness prerequisites and deletion should not land together. | **Adopt an ordered split:** Slice 3a lands ownership plus explicit finalization with no deletion; Slice 3b adds `[storage]`, `RetentionService`, TTL deletion, and 410 routes only after 3a passes the full workspace suite. There is no heuristic reconciler. |
| Fable B5 SMELL: the boundary was misstated and eligibility was duplicated. | `TaskStore` owns eligibility mechanics, deterministic ordering, and atomic deletion guards; `RetentionService` owns sequencing, the batch cap, TTL, and the 24-hour floor. SQLite centralizes eligibility in one view used by both listing and delete-time guards. |
| Fable B6 WRONG: `ArtifactDeleteStats +=` corrupts pass-level byte snapshots. | Remove rev2’s byte-bearing `ArtifactDeleteStats`. Only `ArtifactDeleteCounts` is summable; `RetentionPassStats` is set once around a pass and is not additive. Because size eviction is deferred, rev3 carries no byte snapshot. |

## Goal

Implement the smallest safe retention mechanism for M4 Slice 1–2 artifacts:

- Fix detached, batch, and resume turn ownership on the write path.
- Persist an explicit durable finalization barrier for both usage and genuine no-usage turns.
- Purge old eligible `task_journal`, `task_node_checkpoints`, and `turn_log` rows by TTL.
- Retain `tasks` rows and mark purged artifact sets with `artifacts_purged_at`.
- Return 410 for artifacts removed by retention and preserve 404 for artifacts never known.
- Run bounded boot and hourly sweeps through a dedicated `RetentionService`.

Current grounding:

- `WorkflowRunContext.task_id` exists and defaults to `None` at `crates/bridge-workflow/src/executor.rs:25-43`, then flows into each `TurnContext` at `crates/bridge-workflow/src/executor.rs:222-245`.
- `TaskRecordStatus::is_terminal()` treats every non-`Working` status as terminal at `crates/bridge-core/src/task_store.rs:14-46`.
- `TaskRecord` has no purge marker today at `crates/bridge-core/src/task_store.rs:128-152`.
- Journal and checkpoint tables cascade from `tasks`, while `turn_log` has no task foreign key at `crates/bridge-store/src/sqlite.rs:130-194`.
- `/turns/:turn_id` is backed directly by `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.
- Journal and artifact routes are mounted at `crates/bridge-a2a-inbound/src/server.rs:294-301`.

## Global Constraints

- Data safety wins over byte reclamation.
- Rust remains `1.94.0`, as declared at `Cargo.toml:5-9`.
- Retention never deletes a `tasks` row.
- Retention never deletes artifacts belonging to a `Working` task.
- `task_id IS NULL` is never an orphan predicate.
- A legacy row with `task_id IS NULL` and either `workflow` or `node` present is retained indefinitely.
- No session-ID string parsing, GLOB matching, ownership backfill, or time-based finalization reconciliation exists.
- Every turn-row deletion requires:
  - `completed_ms IS NOT NULL`;
  - `usage_finalized_ms IS NOT NULL`;
  - `usage_finalization_kind IN ('usage', 'no_usage')`; and
  - an eligibility timestamp older than both the configured TTL and the 24-hour minimum-age floor.
- A lost or dropped finalization command leaves a row `pending` and therefore undeletable.
- All candidate lists and sweeps are bounded.
- SQLite mutations use one `BEGIN IMMEDIATE` transaction and re-check eligibility at deletion time.
- No new unauthenticated HTTP route.
- Retention is independent of `[metrics]` and `[traces]`.
- `bridge-core` remains free of Prometheus types and high-cardinality IDs remain absent from Prometheus labels.
- Implementation completion requires `cargo fmt --all -- --check`, clippy with warnings denied, and the full `cargo test --workspace`; totals and any unexercised behavior must be reported.

## `[storage]` config

`[store]` continues to own the durable database path and resume cap at `bin/a2a-bridge/src/config.rs:64-68`. Add an independent top-level table beside `[metrics]` and `[traces]`, currently located at `bin/a2a-bridge/src/config.rs:246-249`.

```toml
[storage]
artifact_retention_days = 14
```

| Field | Type | Default | Validation | Meaning |
|---|---:|---:|---|---|
| `artifact_retention_days` | `u64` | `14` | `0` disables retention; negative TOML values fail deserialization | Purge eligible artifacts older than this many days, subject to the additional 24-hour floor. |

`artifact_retention_max_bytes` and `purge_terminal_tasks_days` do not exist. `StorageToml` rejects unknown fields so removed rev2 knobs cannot be silently ignored.

```rust
fn default_artifact_retention_days() -> u64 {
    14
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageToml {
    #[serde(default = "default_artifact_retention_days")]
    pub artifact_retention_days: u64,
}

impl Default for StorageToml {
    fn default() -> Self {
        Self {
            artifact_retention_days: default_artifact_retention_days(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageConfig {
    pub artifact_retention_days: u64,
}
```

Add to `RegistryConfig`:

```rust
#[serde(default)]
pub storage: StorageToml,
```

`StorageConfig` is captured once at serve boot. As with other non-registry runtime state, changing `[storage]` requires restart.

## Store methods

### Core DTOs and trait signatures

Extend the existing turn-log DTOs and `TaskStore` surface at `crates/bridge-core/src/task_store.rs:154-197` and `crates/bridge-core/src/task_store.rs:252-343`.

```rust
#[derive(Clone, Debug)]
pub enum TurnUsageFinalization {
    Usage(crate::orch::UsageSnapshot),
    NoUsage,
}

#[derive(Clone, Debug)]
pub struct TurnLogFinalized {
    pub ctx: crate::ports::TurnContext,
    pub finalized_ms: i64,
    pub finalization: TurnUsageFinalization,
}

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
    /// max(completion/terminal time, usage-finalization persistence time).
    pub eligible_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactDeleteCounts {
    pub task_artifact_sets: u64,
    pub journal_rows: u64,
    pub node_checkpoint_rows: u64,
    pub task_linked_turn_rows: u64,
    pub standalone_turn_rows: u64,
}

impl std::ops::AddAssign for ArtifactDeleteCounts {
    fn add_assign(&mut self, rhs: Self) {
        self.task_artifact_sets += rhs.task_artifact_sets;
        self.journal_rows += rhs.journal_rows;
        self.node_checkpoint_rows += rhs.node_checkpoint_rows;
        self.task_linked_turn_rows += rhs.task_linked_turn_rows;
        self.standalone_turn_rows += rhs.standalone_turn_rows;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionPassStats {
    pub candidates_seen: u64,
    pub deleted: ArtifactDeleteCounts,
}
```

`RetentionPassStats` has no `AddAssign`. A future size-retention slice may add a separate set-once byte snapshot to this pass type; byte fields must never appear in `ArtifactDeleteCounts`.

Extend `TaskRecord`:

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

Extend `TurnLogRow`:

```rust
pub struct TurnLogRow {
    // existing fields unchanged
    pub usage_finalized_ms: Option<i64>,
    pub usage_finalization_kind: String,
}
```

Replace `update_turn_usage()` with explicit finalization and add three retention primitives:

```rust
#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    async fn finalize_turn_usage(
        &self,
        _row: &TurnLogFinalized,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn list_artifact_retention_candidates(
        &self,
        _eligible_before_ms: i64,
        _limit: u32,
    ) -> Result<Vec<RetentionArtifactCandidate>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn delete_task_artifact_set(
        &self,
        _task: &TaskId,
        _eligible_before_ms: i64,
        _purged_at_ms: i64,
    ) -> Result<ArtifactDeleteCounts, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn delete_reclaimable_turn_log_row(
        &self,
        _turn: &TurnId,
        _eligible_before_ms: i64,
    ) -> Result<ArtifactDeleteCounts, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
}
```

There is no backfill, reconciliation, byte-accounting, size-eviction, or hard-task-delete method.

### SQLite schema and migration

The schema is created under the store’s single connection at `crates/bridge-store/src/sqlite.rs:15-20` and migrated through the additive-column path at `crates/bridge-store/src/sqlite.rs:197-259`.

Add `tasks.artifacts_purged_at` and two turn-log finalization columns:

```sql
ALTER TABLE tasks ADD COLUMN artifacts_purged_at INTEGER;

ALTER TABLE turn_log ADD COLUMN usage_finalized_ms INTEGER;
ALTER TABLE turn_log
    ADD COLUMN usage_finalization_kind TEXT NOT NULL DEFAULT 'pending';

CREATE INDEX IF NOT EXISTS idx_turn_log_retention
    ON turn_log(usage_finalized_ms, completed_ms);
```

Each `ALTER TABLE` is conditional on `PRAGMA table_info`. Fresh schema creation includes the columns directly.

Migration performs no `UPDATE`. Every pre-rev3 row therefore remains `usage_finalization_kind = 'pending'` with a NULL barrier, regardless of its token columns. Such rows are ambiguous and manual-only.

Recreate one shared eligibility view after the columns exist:

```sql
DROP VIEW IF EXISTS retention_artifact_eligibility;

CREATE VIEW retention_artifact_eligibility AS
SELECT
    'task' AS kind,
    t.id AS task_id,
    CAST(NULL AS TEXT) AS turn_id,
    max(
        t.updated_ms,
        COALESCE(
            (
                SELECT MAX(max(tl.completed_ms, tl.usage_finalized_ms))
                FROM turn_log tl
                WHERE tl.task_id = t.id
            ),
            t.updated_ms
        )
    ) AS eligible_ms
FROM tasks t
WHERE t.status IN ('completed', 'failed', 'canceled', 'interrupted')
  AND (
      EXISTS (SELECT 1 FROM task_journal j WHERE j.task_id = t.id)
      OR EXISTS (
          SELECT 1
          FROM task_node_checkpoints c
          WHERE c.task_id = t.id
      )
      OR EXISTS (SELECT 1 FROM turn_log tl WHERE tl.task_id = t.id)
  )
  AND NOT EXISTS (
      SELECT 1
      FROM turn_log tl
      WHERE tl.task_id = t.id
        AND (
            tl.completed_ms IS NULL
            OR tl.usage_finalized_ms IS NULL
            OR tl.usage_finalization_kind NOT IN ('usage', 'no_usage')
        )
  )

UNION ALL

SELECT
    CASE
        WHEN tl.task_id IS NULL THEN 'warm_turn'
        ELSE 'stale_linked_turn'
    END AS kind,
    tl.task_id,
    tl.turn_id,
    max(tl.completed_ms, tl.usage_finalized_ms) AS eligible_ms
FROM turn_log tl
WHERE tl.completed_ms IS NOT NULL
  AND tl.usage_finalized_ms IS NOT NULL
  AND tl.usage_finalization_kind IN ('usage', 'no_usage')
  AND (
      (
          tl.task_id IS NULL
          AND tl.workflow IS NULL
          AND tl.node IS NULL
      )
      OR (
          tl.task_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1
              FROM tasks t
              WHERE t.id = tl.task_id
          )
      )
  );
```

This view is the single SQLite definition of base eligibility. Candidate listing and delete-time guards both query it; TTL and the 24-hour floor remain service policy passed in as `eligible_before_ms`.

Update:

- Task INSERT/SELECT paths at `crates/bridge-store/src/sqlite.rs:532-623`.
- `working_tasks()` at `crates/bridge-store/src/sqlite.rs:738-753`.
- `row_to_task()` at `crates/bridge-store/src/sqlite.rs:1773-1805`.
- `TURN_LOG_SELECT` and `row_to_turn_log_row()` at `crates/bridge-store/src/sqlite.rs:271-314`.
- All `TaskRecord` constructors in production and tests.

### Turn-log ownership write fix

All detached, batch, and resume contexts set `task_id: Some(task.clone())`:

- Fresh detached submit: `crates/bridge-coordinator/src/coordinator.rs:701-716`.
- Batch child spawn: `crates/bridge-coordinator/src/batch.rs:860-875`.
- Detached test helpers: `crates/bridge-coordinator/src/detached.rs:1312-1345` and `crates/bridge-coordinator/src/detached.rs:1352-1385`.
- Detached boot resume: `crates/bridge-coordinator/src/detached.rs:1622-1686`.
- Batch boot resume: `crates/bridge-coordinator/src/batch.rs:513-565`.

The central runner also enforces ownership:

```rust
pub fn spawn_detached_workflow(
    deps: &DetachedDeps,
    task: TaskId,
    // existing arguments
    mut ctx: WorkflowRunContext,
    hub: Arc<TaskProgressHub>,
) -> tokio::task::JoinHandle<()> {
    let deps = deps.clone();
    tokio::spawn(async move {
        // Existing setup remains.
        ctx.task_id = Some(task.clone());
        ctx.make_rich_sink = Some(Arc::new(DetachedRichSinkFactory {
            store: deps.task_store.clone(),
            task: task.clone(),
            op,
            hub: hub.clone(),
        }));
        let stream =
            executor.run_from_with_context(graph, input, run_id, token, seed, ctx);
        // Existing drain/finalization remains.
    })
}
```

The assignment occurs immediately before `run_from_with_context`, after all early validation and before any workflow turn can be emitted.

Warm task-backed turns already populate `task_id` in their `TurnContext`; pure warm contexts may continue to use `None`.

No legacy ownership row is mutated. In particular, there is no SQL or memory matcher for fresh or `-resume-<digits>` session IDs.

### Explicit usage/no-usage finalization

Change the existing event at `crates/bridge-core/src/ports.rs:290-331`:

```rust
UsageFinalized {
    ctx: &'a TurnContext,
    usage: Option<&'a UsageSnapshot>,
    fin: UsageFinalization,
},
```

`None` is valid only for `UsageFinalization::TurnFinal` and means the producer observed no usage update before the turn ended. `Partial` and `TaskFinal` continue to require `Some`.

Every producer emits exactly one turn-final event immediately after `TurnFinished`, including success, failure, cancellation, disconnect, prompt-open failure, and drop-guard exits. Replace conditional `if let Some(usage)` emissions with:

```rust
observer.record(&ObsEvent::TurnFinished {
    ctx,
    latency,
    ttft,
    outcome,
});
observer.record(&ObsEvent::UsageFinalized {
    ctx,
    usage: final_usage.as_ref(),
    fin: UsageFinalization::TurnFinal,
});
```

Current producer anchors requiring this change are:

- Workflow prompt-open/early exits and normal completion at `crates/bridge-workflow/src/executor.rs:322-415`.
- Workflow retry outcomes at `crates/bridge-workflow/src/executor.rs:726-803`.
- Coordinator local turns at `crates/bridge-coordinator/src/coordinator.rs:568-641`.
- Coordinator cancellation drop guard at `crates/bridge-coordinator/src/coordinator.rs:906-930`.
- Inbound streaming cancellation, disconnect, send failure, and completion at `crates/bridge-a2a-inbound/src/server.rs:2164-2290`.

`TurnLogObserver` maps the event to one separate finalization command:

```rust
ObsEvent::UsageFinalized { ctx, usage, fin } => {
    if *fin != UsageFinalization::TurnFinal {
        return;
    }

    self.try_send(TurnLogCommand::Finalized(TurnLogFinalized {
        ctx: (*ctx).clone(),
        finalized_ms: (self.now_ms)(),
        finalization: match usage {
            Some(usage) => TurnUsageFinalization::Usage((*usage).clone()),
            None => TurnUsageFinalization::NoUsage,
        },
    }));
}
```

The command remains independently droppable. That is safe: a dropped command leaves the barrier pending and retention cannot delete the row.

`upsert_turn_finished()` at `crates/bridge-store/src/sqlite.rs:756-809` inserts `usage_finalization_kind = 'pending'`, leaves `usage_finalized_ms` NULL, and never clears an already-finalized barrier during conflict update.

For `TurnUsageFinalization::Usage`, persist usage and the barrier atomically:

```sql
UPDATE turn_log
SET input_tokens = COALESCE(:input_tokens, input_tokens),
    output_tokens = COALESCE(:output_tokens, output_tokens),
    thought_tokens = COALESCE(:thought_tokens, thought_tokens),
    cached_read_tokens = COALESCE(:cached_read_tokens, cached_read_tokens),
    cached_write_tokens = COALESCE(:cached_write_tokens, cached_write_tokens),
    cost_amount = COALESCE(:cost_amount, cost_amount),
    cost_currency = COALESCE(:cost_currency, cost_currency),
    usage_finalized_ms = :finalized_ms,
    usage_finalization_kind = 'usage'
WHERE turn_id = :turn_id
  AND completed_ms IS NOT NULL
  AND usage_finalized_ms IS NULL;
```

For `TurnUsageFinalization::NoUsage`, persist only an explicit no-usage barrier and reject contradictory existing usage:

```sql
UPDATE turn_log
SET usage_finalized_ms = :finalized_ms,
    usage_finalization_kind = 'no_usage'
WHERE turn_id = :turn_id
  AND completed_ms IS NOT NULL
  AND usage_finalized_ms IS NULL
  AND input_tokens IS NULL
  AND output_tokens IS NULL
  AND thought_tokens IS NULL
  AND cached_read_tokens IS NULL
  AND cached_write_tokens IS NULL
  AND cost_amount IS NULL
  AND cost_currency IS NULL;
```

Zero affected rows are an error unless the row is already finalized with the same kind, in which case replay is idempotent. An unknown turn, contradictory no-usage marker, or conflicting finalization kind returns `StoreFailure`.

### Candidate listing

```sql
SELECT kind, task_id, turn_id, eligible_ms
FROM retention_artifact_eligibility
WHERE eligible_ms < :eligible_before_ms
ORDER BY eligible_ms ASC,
         kind ASC,
         COALESCE(task_id, turn_id) ASC
LIMIT :limit;
```

The SQL provides deterministic oldest-first ordering. `RetentionService` does not reimplement eligibility or ordering.

### Atomic task artifact deletion

Run with `rusqlite::TransactionBehavior::Immediate` under the existing connection mutex.

```sql
UPDATE tasks
SET artifacts_purged_at = COALESCE(artifacts_purged_at, :purged_at_ms)
WHERE id = :task_id
  AND EXISTS (
      SELECT 1
      FROM retention_artifact_eligibility e
      WHERE e.kind = 'task'
        AND e.task_id = tasks.id
        AND e.eligible_ms < :eligible_before_ms
  )
RETURNING id;
```

If this returns no row, commit a no-op and return zero counts. If it returns the task ID, the immediate transaction has acquired the guard and executes:

```sql
DELETE FROM task_journal
WHERE task_id = :task_id;

DELETE FROM task_node_checkpoints
WHERE task_id = :task_id;

DELETE FROM turn_log
WHERE task_id = :task_id;
```

Capture each statement’s affected-row count in `ArtifactDeleteCounts`, set `task_artifact_sets = 1`, and commit. Any failure rolls back the marker and all deletes together.

The transaction never deletes `task_node_starts` or `tasks`.

### Atomic standalone turn deletion

```sql
DELETE FROM turn_log
WHERE turn_id = :turn_id
  AND EXISTS (
      SELECT 1
      FROM retention_artifact_eligibility e
      WHERE e.kind IN ('warm_turn', 'stale_linked_turn')
        AND e.turn_id = turn_log.turn_id
        AND e.eligible_ms < :eligible_before_ms
  );
```

Run in an immediate transaction and return `standalone_turn_rows = affected_rows`. The shared view re-check ensures a row relinked to a real task or otherwise made ineligible after listing is retained.

### Memory shape

`MemoryTaskStore` currently owns task, checkpoint, turn-log, and journal maps at `crates/bridge-core/src/task_store.rs:597-619`.

Required changes:

- Add `artifacts_purged_at` to all stored `TaskRecord`s.
- Add finalization time/kind to `TurnLogRow`.
- Replace `update_turn_usage()` with the same explicit `Usage`/`NoUsage` state transition as SQLite.
- Keep `journal_fold_guard` for cross-map candidate snapshots and mutations.
- Implement one shared memory eligibility helper mirroring the SQLite view:
  - terminal task with at least one artifact;
  - every linked turn completed and explicitly finalized;
  - pure warm row only when task/workflow/node are all absent;
  - stale linked row only when `task_id` is non-NULL and absent from `inner`;
  - ambiguous legacy `NULL task_id` workflow/node rows never eligible;
  - `eligible_ms` includes finalization persistence time.
- Candidate listing sorts by `(eligible_ms, kind, id)` and truncates to `limit`.
- Task artifact deletion re-runs the helper while holding `journal_fold_guard`, removes journal/checkpoint/linked-turn rows, and sets the marker without removing `inner`.
- Standalone turn deletion re-runs the helper before removing the row.
- No task-ID backfill or session-ID parsing exists in memory.

## Purge algorithms

```rust
pub const STORAGE_RETENTION_SWEEP_INTERVAL: Duration =
    Duration::from_secs(60 * 60);
pub const STORAGE_RETENTION_BATCH_LIMIT: u32 = 10_000;
pub const MIN_ARTIFACT_AGE: Duration =
    Duration::from_secs(24 * 60 * 60);
pub const MS_PER_DAY: u64 = 86_400_000;

pub struct RetentionService {
    store: Arc<dyn TaskStore>,
    cfg: StorageConfig,
    limit: u32,
}
```

### Effective cutoff

The service combines TTL and the mandatory floor:

```rust
fn retention_cutoff_ms(now_ms: i64, days: u64) -> Option<i64> {
    if days == 0 {
        return None;
    }

    let ttl_ms = days
        .checked_mul(MS_PER_DAY)
        .and_then(|value| i64::try_from(value).ok());

    Some(match ttl_ms {
        Some(ttl_ms) => now_ms.saturating_sub(ttl_ms),
        None => i64::MIN,
    })
}

fn effective_artifact_cutoff_ms(now_ms: i64, days: u64) -> Option<i64> {
    let ttl_cutoff = retention_cutoff_ms(now_ms, days)?;
    let floor_ms =
        i64::try_from(MIN_ARTIFACT_AGE.as_millis()).unwrap_or(i64::MAX);
    let floor_cutoff = now_ms.saturating_sub(floor_ms);
    Some(ttl_cutoff.min(floor_cutoff))
}
```

Because candidates require `eligible_ms < cutoff`, overflow to `i64::MIN` deletes no normal row. The `min` means the effective required age is `max(configured TTL, 24 hours)`.

### Task artifact-set purge

Guard predicate:

- Task status is terminal.
- The task still has journal, checkpoint, or linked turn artifacts.
- Every linked turn is completed and explicitly finalized as `usage` or `no_usage`.
- The maximum of task terminal-update time and linked turn completion/finalization times is older than the effective cutoff.
- The task remains eligible when the immediate transaction begins.

Deletion:

- `task_journal` rows.
- `task_node_checkpoints` rows.
- `turn_log` rows with exact `task_id = task.id`.
- Set `tasks.artifacts_purged_at` in the same transaction.
- Retain the `tasks` row and all `task_node_starts`.

### Standalone turn purge

Guard predicate:

- The row is completed and explicitly finalized.
- Its completion and finalization times are both older than the effective cutoff.
- It is either:
  - a pure warm row with `task_id`, `workflow`, and `node` all NULL; or
  - a stale linked row with non-NULL `task_id` and no matching `tasks` row.
- It remains eligible when the immediate transaction begins.

A `NULL task_id` workflow/node row is never a standalone candidate.

### Bounded TTL pass

```rust
pub async fn run_pass(
    &self,
    now_ms: i64,
) -> Result<RetentionPassStats, BridgeError> {
    let Some(cutoff_ms) =
        effective_artifact_cutoff_ms(now_ms, self.cfg.artifact_retention_days)
    else {
        return Ok(RetentionPassStats::default());
    };

    let candidates = self
        .store
        .list_artifact_retention_candidates(cutoff_ms, self.limit)
        .await?;

    let mut stats = RetentionPassStats {
        candidates_seen: candidates.len() as u64,
        ..RetentionPassStats::default()
    };

    for candidate in candidates {
        let deleted = match candidate.kind {
            RetentionArtifactKind::Task => {
                let task = candidate
                    .task_id
                    .as_ref()
                    .ok_or(BridgeError::StoreFailure)?;
                self.store
                    .delete_task_artifact_set(task, cutoff_ms, now_ms)
                    .await?
            }
            RetentionArtifactKind::WarmTurn
            | RetentionArtifactKind::StaleLinkedTurn => {
                let turn = candidate
                    .turn_id
                    .as_ref()
                    .ok_or(BridgeError::StoreFailure)?;
                self.store
                    .delete_reclaimable_turn_log_row(turn, cutoff_ms)
                    .await?
            }
        };
        stats.deleted += deleted;
    }

    Ok(stats)
}
```

One pass lists and attempts at most `limit` candidates. No repair phase, size loop, terminal-task phase, or byte accounting exists.

## Sweep scheduling

Existing serve anchors:

- Task store opens at `bin/a2a-bridge/src/main.rs:6064-6095`.
- Prometheus rebuild reads `turn_log_rows()` at `bin/a2a-bridge/src/main.rs:6119-6124`.
- The warm-session reaper demonstrates the periodic-task pattern at `bin/a2a-bridge/src/main.rs:6198-6207`.
- Coordinator resume occurs before listener bind at `bin/a2a-bridge/src/main.rs:6294-6308`.

Boot order:

1. Parse and validate `[storage]`.
2. Open the task store and run DDL-only schema migration.
3. Construct `RetentionService`.
4. Run one bounded retention pass before Prometheus rebuild.
5. Rebuild Prometheus from surviving turn rows.
6. Build observer, server, and Coordinator.
7. Run `coordinator.resume().await`.
8. Spawn the hourly retention loop.
9. Bind the listener.

Boot and periodic execution errors log `warn` and do not abort serve. Configuration and store-open/migration errors still abort boot.

The hourly loop schedules its first tick one full interval after the boot pass:

```rust
tokio::spawn(async move {
    let start = tokio::time::Instant::now()
        + STORAGE_RETENTION_SWEEP_INTERVAL;
    let mut ticker = tokio::time::interval_at(
        start,
        STORAGE_RETENTION_SWEEP_INTERVAL,
    );

    loop {
        ticker.tick().await;
        if let Err(error) = retention.run_pass(now_ms()).await {
            tracing::warn!(
                error = ?error,
                "storage retention: bounded sweep failed"
            );
        }
    }
});
```

No `VACUUM` or unbounded draining loop runs.

## Data-Safety Guards

1. **Prevents deletion of a different live task’s turn row — closes Codex A1 and Fable B1:** terminal task hard-delete and all session-ID victim matching are absent. Retention deletes only exact non-NULL `turn_log.task_id` links or independently eligible standalone rows.

2. **Prevents deletion based on the `task-a`/`task-a-resume-1` namespace collision — closes Codex A1 and Fable B3:** there is no GLOB, prefix, longest-ID, or fresh/resume ownership inference in SQL or memory.

3. **Prevents new detached/batch turns from becoming ambiguous — closes Codex A3:** callers set `WorkflowRunContext.task_id`, and `spawn_detached_workflow` overwrites it from its authoritative `task` argument immediately before execution.

4. **Prevents deletion of real usage after a dropped usage command — closes Codex A2:** NULL token/cost columns never imply no usage. Only a persisted producer-authored `Usage(Some(...))` or explicit `NoUsage` sets the barrier.

5. **Prevents deletion when a crash occurs between finish and finalization — closes Codex A2:** a finished row whose separate finalization command was lost remains `pending` indefinitely and is absent from the eligibility view.

6. **Prevents deletion during the finalization window — closes Codex A2 and Fable B2:** `RetentionService` applies a 24-hour floor, and `eligible_ms` includes `usage_finalized_ms`, so no row is deleted until at least 24 hours after the barrier persisted.

7. **Prevents deletion of frozen legacy workflow data — closes Review A ownership WRONG and Fable B3:** any `NULL task_id` row with workflow or node metadata is permanently excluded; there is no runtime repair attempting to guess its owner.

8. **Prevents working-task artifact deletion — preserves the endorsed rev2 guard:** task-set eligibility requires an exact terminal status and is re-checked inside `BEGIN IMMEDIATE`.

9. **Prevents list/delete predicate drift — closes Fable B5:** candidate listing and both delete guards use the same `retention_artifact_eligibility` view.

10. **Prevents TOCTOU deletion after state changes — closes Fable B5:** the store re-checks the shared view and age cutoff after acquiring the immediate transaction; a task made `Working` or a turn relinked after listing produces a no-op.

11. **Prevents ambiguous finalization from becoming deletion-eligible — closes Codex A2:** legacy rows, contradictory explicit no-usage markers, and conflicting finalization kinds remain pending or return an error.

12. **Prevents an unbounded migration stall — closes Codex A4:** schema migration executes only additive DDL, indexes, and view creation; it performs no row update.

13. **Prevents the dangerous untested feature surface — closes Codex A5 and Fable B1:** no hard-delete feature, cross-crate feature forwarding, conditional trait method, or feature-only SQL exists.

14. **Prevents a misleading disk-ceiling promise — closes Fable B2:** `artifact_retention_max_bytes` and logical-byte accounting are deferred; rev3 promises TTL reclamation only.

15. **Prevents corrupt pass statistics — closes Fable B6:** only row counts are additive. Pass metadata is set once and contains no summable before/after byte fields.

16. **Prevents boot stalls and unlimited per-pass deletion:** one query returns at most `STORAGE_RETENTION_BATCH_LIMIT` candidates; boot runs one pass and future hourly passes drain any backlog.

17. **Prevents partial task-artifact state from being mistaken for “never existed”:** marker update and artifact deletes share one transaction, and routes use `artifacts_purged_at` to return 410.

18. **Prevents TTL arithmetic overflow from widening deletion:** overflow maps to `i64::MIN`, which no normal eligibility timestamp satisfies under the strict `<` comparison.

## Counter reconciliation

Current Prometheus rebuild reads persisted rows at `bin/a2a-bridge/src/main.rs:6119-6124` and delegates to `PrometheusObserver::rebuild_from_turn_log()` at `crates/bridge-observ/src/lib.rs:430-509`.

After Slice 3:

- Boot retention runs before rebuild, so boot counters reflect surviving turn rows.
- Periodic retention never decrements already-exported counters.
- Restart rebuild omits previously purged turns.
- `rebuild_from_turn_log()` seeds usage-finalization dedupe from `usage_finalized_ms` and `usage_finalization_kind`, not from the presence of token/cost columns. The current token-column heuristic is at `crates/bridge-observ/src/lib.rs:499-508`.
- An explicit `no_usage` row seeds finalization dedupe but contributes zero tokens and cost.
- A pending row does not seed finalization dedupe, so a replayed producer finalization may still complete it.
- No synthetic usage values are created.
- Historical task usage for frozen legacy `NULL task_id` workflow rows remains incomplete by design; safety takes precedence over retrospective aggregation. Current aggregation keys strictly on `task_id` at `crates/bridge-store/src/sqlite.rs:895-920`.

## Slice-1/2 cohesion

### Slice 1

- `TurnFinished` still precedes `UsageFinalized`; current ordering tests are anchored at `crates/bridge-workflow/src/executor.rs:4604-4692`.
- `UsageFinalized` now always describes the producer’s final knowledge: `Some(snapshot)` or explicit `None`.
- Turn-log usage fields and the finalization barrier are written atomically.
- A lost finalization command leaves the durable row available for diagnostics and replay.
- New detached, batch, and resume turns contribute to task usage because their exact task ID is written at creation.
- Legacy ambiguous ownership remains untouched.

### Slice 2

- `/turns/:turn_id` returns 404 after an eligible turn row is purged because the route directly reads `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.
- Journal and checkpoint routes first confirm the task row, as they currently do at `crates/bridge-a2a-inbound/src/server.rs:1113-1139` and `crates/bridge-a2a-inbound/src/server.rs:1264-1290`.
- If the requested body is absent and `artifacts_purged_at.is_some()`, return:

```http
HTTP/1.1 410 Gone
Content-Type: application/json
```

```json
{
  "error": "artifacts purged",
  "artifacts_purged_at": 1750000000000
}
```

- If the task is unknown, the node ID is invalid, or the body is absent while `artifacts_purged_at` is NULL, preserve 404.
- The journal route replaces the current terminal-empty 404 branch at `crates/bridge-a2a-inbound/src/server.rs:1150-1166` with the marker-aware distinction.
- The artifact route replaces the current unconditional missing-output 404 at `crates/bridge-a2a-inbound/src/server.rs:1361-1372`.
- `tasks/list` includes `artifacts_purged_at` beside the existing fields emitted at `crates/bridge-a2a-inbound/src/server.rs:3937-3952`.
- `tasks/get` includes the marker in task metadata when present; its durable-task path is at `crates/bridge-a2a-inbound/src/server.rs:3881-3910`.
- Coordinator trace refs omit journal, turn, and checkpoint references after a purge marker is present; the current reference assembly is at `crates/bridge-coordinator/src/coordinator.rs:795-823`.

No transcript table exists in the current SQLite schema; Slice 3 covers `task_journal`, `task_node_checkpoints`, and `turn_log`.

## Testing

### Slice 3a: ownership and finalization, no deletion

- `detached_runner_overwrites_missing_task_id`: pass `WorkflowRunContext { task_id: None, .. }` to `spawn_detached_workflow`; every emitted workflow turn has the runner’s task ID.
- `detached_runner_overwrites_conflicting_task_id`: pass a different task ID in the context; the authoritative runner task wins.
- `detached_fresh_turn_persists_task_id`: fresh submit writes `turn_log.task_id = task.id`.
- `batch_child_turn_persists_task_id`: a batch child writes its child task ID.
- `detached_resume_turn_persists_task_id`: a resumed task using `"{task}-resume-1"` still writes the original task ID.
- `batch_resume_turn_persists_task_id`: resumed batch child writes the original child task ID.
- `workflow_success_without_usage_emits_explicit_no_usage`: producer emits `TurnFinished` followed by `UsageFinalized { usage: None, TurnFinal }`.
- `workflow_failure_without_usage_emits_explicit_no_usage`: prompt-open/error path emits the same explicit marker.
- `workflow_cancel_without_usage_emits_explicit_no_usage`: cancellation emits the same explicit marker.
- `inbound_disconnect_without_usage_emits_explicit_no_usage`: disconnect/send-failure paths emit the marker.
- `turn_finish_drop_guard_without_usage_emits_explicit_no_usage`: drop guard emits an explicit no-usage finalization.
- `usage_finalized_some_updates_usage_and_barrier_atomically`: token/cost values, `usage_finalized_ms`, and kind `usage` appear together.
- `usage_finalized_none_sets_no_usage_barrier`: explicit `None` leaves usage columns NULL and sets kind `no_usage`.
- `no_usage_finalization_rejects_existing_usage_columns`: contradictory no-usage finalization affects no row and returns an error.
- `turn_finished_upsert_does_not_clear_finalization`: a replayed finish leaves an existing barrier and kind unchanged.
- `sqlite_legacy_migration_is_ddl_only`: open a pre-rev3 DB and assert every old row’s token/cost/task fields are unchanged and its barrier remains pending.
- `memory_finalization_matches_sqlite`: memory and SQLite agree for usage, no usage, duplicate, contradictory, and unknown-turn cases.
- `prometheus_rebuild_seeds_explicit_no_usage_finalization`: no-usage row seeds dedupe without adding token/cost counters.
- `prometheus_rebuild_keeps_pending_finalization_replayable`: pending row seeds finish dedupe but not finalization dedupe.

### Slice 3b: TTL retention and routes

- `storage_config_defaults_to_fourteen_days`: missing `[storage]` yields `artifact_retention_days = 14`.
- `storage_config_zero_disables_retention`: zero produces no candidate-list or delete calls.
- `storage_config_rejects_negative_days`: negative TOML fails deserialization.
- `storage_config_rejects_removed_max_bytes_knob`: `artifact_retention_max_bytes` is an unknown-field error.
- `storage_config_rejects_removed_terminal_purge_knob`: `purge_terminal_tasks_days` is an unknown-field error.
- `storage_config_independent_from_metrics_and_traces`: storage parses and runs with metrics/traces disabled.
- `retention_cutoff_overflow_deletes_nothing`: huge day count maps to `i64::MIN`; no normal candidate is returned.
- `retention_minimum_age_floor_blocks_recent_terminal_task`: a task terminal for one second remains intact even when its configured TTL would otherwise permit deletion.
- `retention_minimum_age_floor_blocks_recent_finalization`: an old completed row finalized one second ago remains intact.
- `retention_becomes_eligible_after_ttl_and_floor`: deletion occurs only after both thresholds are crossed.
- `sqlite_artifact_ttl_deletes_only_terminal_task_artifacts`: eligible terminal journal/checkpoint/linked turns are removed and the task remains; equivalent working-task artifacts remain.
- `sqlite_artifact_delete_rechecks_terminal_status`: list a task, change it to `Working`, then delete; the mutation returns zero and all artifacts remain.
- `sqlite_artifact_delete_rechecks_turn_finalization`: list a task, make a linked turn pending before deletion, then assert a no-op.
- `sqlite_artifacts_purged_marker_is_atomic`: injected failure rolls back marker and deletes; success commits both.
- `sqlite_artifacts_purged_at_is_set_once`: a later eligible cleanup preserves the original purge timestamp.
- `sqlite_ttl_deletes_finalized_pure_warm_turn`: an old pure warm row is removed.
- `sqlite_ttl_preserves_warm_turn_without_explicit_finalization`: pending pure warm row remains.
- `sqlite_ttl_deletes_finalized_stale_linked_turn`: finalized non-NULL link to a missing task is removed.
- `sqlite_ttl_preserves_ambiguous_null_workflow_turn`: old finalized `NULL task_id` workflow/node row remains permanently.
- `sqlite_codex_a1_prefix_collision_cannot_overdelete`: create old terminal `task-a`, live `task-a-resume-1`, and a legacy NULL workflow turn whose session is `task-a-resume-1`; run retention and assert the legacy turn and both task records remain. The hard-delete/session-matcher path is impossible by construction.
- `sqlite_codex_a2_lost_usage_row_is_retained`: persist `TurnFinished`, simulate loss of `UsageFinalized(Some(real_usage))`, advance beyond TTL and 24 hours, run retention, and assert the pending row remains.
- `sqlite_no_usage_row_requires_persisted_marker`: NULL usage columns plus elapsed time alone never make a row eligible.
- `sqlite_candidate_order_is_oldest_first`: mixed task/warm/stale candidates order by `(eligible_ms, kind, id)`.
- `sqlite_retention_batch_limit_is_total`: one pass lists and attempts no more than `STORAGE_RETENTION_BATCH_LIMIT` candidates across all kinds.
- `sqlite_shared_view_guards_candidate_and_delete`: every listed kind can be deleted while an equivalent row excluded by the view is rejected by the delete guard.
- `memory_retention_matches_sqlite`: memory matches SQLite for terminal, working, warm, stale, pending, ambiguous, ordering, age, and marker behavior.
- `retention_pass_counts_sum_only_row_counts`: multiple deletes produce exact row totals; no before/after byte fields are accumulated.
- `journal_route_returns_410_for_purged_task`: known marked task with missing journal returns the specified JSON 410.
- `artifact_route_returns_410_for_purged_task`: known marked task with missing checkpoint returns the specified JSON 410.
- `artifact_routes_preserve_404_for_never_created_data`: known unmarked task with missing data remains 404.
- `artifact_routes_preserve_404_for_unknown_task`: unknown task remains 404.
- `tasks_get_and_list_surface_artifacts_purged_at`: both durable task surfaces include the marker.
- `coordinator_omits_trace_refs_after_artifact_purge`: status DTO contains no dead journal, artifact, or turn references.
- `prometheus_boot_rebuild_uses_surviving_rows`: boot sweep runs first and rebuild counts only retained rows.
- `periodic_retention_does_not_decrement_live_counters`: counters remain monotonic until restart.
- `storage_boot_sweep_is_bounded`: fake store records exactly one boot list call with `STORAGE_RETENTION_BATCH_LIMIT`.
- `storage_periodic_first_tick_waits_one_hour`: paused Tokio time confirms no immediate second sweep.
- `storage_sweep_failure_does_not_abort_serve`: store failure logs a warning and server construction continues.
- `full_workspace_gates_pass`: run formatting, clippy with warnings denied, and the complete workspace test suite; report test totals and any behavior not exercised.

Every new deletion path has both an eligible-success test and an ineligible/TOCTOU negative test. Tests that require SQLite use the real `SqliteStore`, not SQL mocks.

## Acceptance criteria

1. `[storage]` contains only `artifact_retention_days`, defaulting to 14; zero disables retention.
2. `artifact_retention_max_bytes` and `purge_terminal_tasks_days` are absent and rejected as unknown storage fields.
3. No `unsafe-terminal-task-purge` feature exists in any crate.
4. No retention trait method or SQL statement deletes a `tasks` row.
5. Aged task-row deletion is explicitly deferred to a future Coordinator-owned tombstone/archival slice.
6. Detached, batch, test-helper, and resume workflow contexts set exact task ownership.
7. `spawn_detached_workflow` enforces `ctx.task_id = Some(task.clone())` before execution.
8. No runtime or migration-time ownership backfill exists.
9. No session-ID fresh/resume matcher exists in SQL or memory.
10. Ambiguous legacy `NULL task_id` workflow/node rows are retained indefinitely.
11. Producers emit explicit turn-final `Some(usage)` or `None` after every `TurnFinished` path.
12. Usage values and the `usage` finalization barrier are persisted atomically.
13. Explicit no usage, not elapsed time or NULL columns, is the only way to persist a `no_usage` barrier.
14. A lost/dropped finalization command leaves the row pending and undeletable.
15. Migration performs no data-row updates.
16. `RetentionService` applies a minimum age of 24 hours in addition to configured TTL.
17. Eligibility time includes both completion/terminal time and usage-finalization persistence time.
18. SQLite defines eligibility once in `retention_artifact_eligibility`.
19. `TaskStore` owns eligibility mechanics, atomic guards, and deterministic ordering SQL; `RetentionService` owns sequencing, the candidate cap, TTL, and the age floor.
20. Every delete re-checks the shared eligibility definition and cutoff under an immediate transaction.
21. Task artifact purge atomically deletes journal/checkpoint/exact-linked-turn rows and sets `artifacts_purged_at`.
22. `ArtifactDeleteCounts` is the only summable statistics type; pass metadata and future byte snapshots are never summed.
23. Known purged task artifacts return 410; unknown or never-created artifacts return 404.
24. Boot retention runs once, bounded, before Prometheus rebuild.
25. Periodic retention starts after Coordinator resume and waits one hour before its first tick.
26. Prometheus counters remain monotonic during a process lifetime and rebuild from surviving rows after restart.
27. SQLite and memory stores implement identical eligibility and guard behavior.
28. Slice 3a lands and passes the full workspace suite before Slice 3b enables deletion.
29. Formatting, clippy with warnings denied, and the full workspace test suite pass with reported totals.

## Non-goals

- No terminal `TaskRecord` deletion.
- No `purge_terminal_tasks_days`.
- No `unsafe-terminal-task-purge` Cargo feature.
- No terminal-task victim query or session-ID GLOB matcher.
- No runtime legacy task-ID backfill.
- No migration-time or timer-based finalization reconciliation.
- No automatic deletion of ambiguous legacy `NULL task_id` workflow turns.
- No size-based eviction or `artifact_retention_max_bytes`.
- No logical-byte accounting or hard database-size ceiling.
- No Coordinator tombstone/archival implementation in this slice; that is the required future home for aged terminal-task-row deletion.
- No retention HTTP API.
- No redaction.
- No `VACUUM` or physical SQLite compaction.
- No Prometheus counter decrement after periodic purge.
- No deletion of working or resumable task artifacts.
- No new merge-state persistence.
- No OTLP/exporter changes.
- No bearer-auth or trace-route authorization changes.
