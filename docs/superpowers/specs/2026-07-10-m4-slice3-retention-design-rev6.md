# M4 Slice 3 — Retention under [storage] (design, rev6)

## Changes from rev5

| Rev5 sign-off finding | Resolution in rev6 |
|---|---|
| **WRONG — NULL recency must fail closed.** Pre-rev6 tasks migrate with `last_artifact_ms = NULL`, while current late rich-event and legacy-checkpoint writers do not update `updated_ms` at `crates/bridge-store/src/sqlite.rs:1423-1448` and `crates/bridge-store/src/sqlite.rs:646-660`. Falling back to an old `updated_ms` can purge a recently written artifact during the first boot sweep. | A terminal task that has any purgeable artifact is ineligible while `last_artifact_ms IS NULL`. The SQLite eligibility view maps NULL, invalid, and sentinel recency to `RETENTION_NEVER_ELIGIBLE_MS`; the memory helper returns no task candidate for the same state. Migration remains DDL-only and performs no recency backfill. A later bounded repair may populate recency, and any post-rev6 artifact write sets a real storage-authored barrier. |
| **WRONG — artifact recency must be storage-authored.** Production callers read event times before entering storage at `crates/bridge-coordinator/src/detached.rs:296-298` and `crates/bridge-coordinator/src/detached.rs:410-413`, while SQLite acquires its connection and transaction later, for example at `crates/bridge-store/src/sqlite.rs:1232-1235`. A stalled writer can therefore commit a fresh artifact with stale recency. | All seven task-linked writers compute `artifact_ms = durable_retention_ms((self.now_ms)())` only after SQLite has begun its immediate transaction or memory has acquired `journal_fold_guard`. Caller-provided event/checkpoint `ts`, `completed_ms`, and terminal `updated_ms` retain their existing meanings; they never author `last_artifact_ms`. The rule applies to the writers at `crates/bridge-store/src/sqlite.rs:646-660`, `:756-846`, and `:1225-1450`, with memory equivalents at `crates/bridge-core/src/task_store.rs:715-735`, `:777-835`, and `:1160-1347`. |
| **WRONG — the purge marker is history, not current emptiness.** `artifacts_purged_at` is set once, but post-purge terminal writes remain permitted. Suppressing all references based on the marker would hide rows written later; current trace-reference assembly is at `crates/bridge-coordinator/src/coordinator.rs:795-823`. | Keep `artifacts_purged_at` set once as purge history. Trace references are derived from current `task_journal`, `task_node_checkpoints`, and `turn_log` rows without consulting the marker. Add a bounded `task_journal_exists()` store read for the journal reference; existing turn/checkpoint reads already inspect current rows. Routes at `crates/bridge-a2a-inbound/src/server.rs:1113-1166` and `:1264-1372` return 200 whenever the requested row exists, 410 only when it is absent and purge history exists, and 404 for unknown or never-known data. |
| **SMELL — specify the memory persistence clock.** `MemoryTaskStore` and its constructors currently live at `crates/bridge-core/src/task_store.rs:597-635`, while SQLite’s store fields and constructors are at `crates/bridge-store/src/sqlite.rs:15-94`. | Define a shared injected persistence-clock callback. `MemoryTaskStore::new()` and `Default` use system wall time; `MemoryTaskStore::with_clock()` accepts a manual clock for tests. SQLite uses the same callback contract. Memory reads it after acquiring `journal_fold_guard`; SQLite reads it after beginning the immediate transaction. Invalid readings map through `durable_retention_ms()`. |

All settled rev5 design remains unchanged: the recency model is still `max(updated_ms, last_artifact_ms, linked-turn completion/finalization)` plus TTL and the 24-hour floor once a known recency barrier exists; writers remain free of task-status guards; the seven-writer inventory remains complete; `journal_fold_guard` atomicity and select-then-delete re-check remain required; `i64::MAX` remains the fail-closed sentinel; Slice 3a ownership and persistence-time usage finalization remain unchanged; A3, `TurnFinal` dedupe, every rev3 subtraction, and the 3a/3b split remain unchanged.

## Goal

Implement the smallest safe retention mechanism for M4 Slice 1–2 artifacts:

- Fix detached, batch, and resume turn ownership on the write path.
- Persist an explicit durable finalization barrier for both usage and genuine no-usage turns.
- Track storage-authored per-task artifact recency and use it to prevent deletion under late or stalled progress writers.
- Fail closed for migrated tasks whose artifact recency is unknown.
- Purge old eligible `task_journal`, `task_node_checkpoints`, and `turn_log` rows by TTL.
- Retain `tasks` rows and record purge history with `artifacts_purged_at`.
- Return 410 for artifacts removed by retention, return 200 for rows written after a purge, and preserve 404 for artifacts never known.
- Run bounded boot and hourly sweeps through a dedicated `RetentionService`.

Current grounding:

- `WorkflowRunContext.task_id` exists and defaults to `None` at `crates/bridge-workflow/src/executor.rs:21-44`, then flows into each `TurnContext` at `crates/bridge-workflow/src/executor.rs:221-250`.
- `TaskRecordStatus::is_terminal()` treats every non-`Working` status as terminal at `crates/bridge-core/src/task_store.rs:14-46`.
- `TaskRecord` has no purge marker or artifact-recency timestamp today at `crates/bridge-core/src/task_store.rs:128-152`.
- `MemoryTaskStore` stores tasks, starts, checkpoints, turn rows, and journals in split maps at `crates/bridge-core/src/task_store.rs:597-619`.
- Journal and checkpoint tables cascade from `tasks`, while `turn_log` has no task foreign key at `crates/bridge-store/src/sqlite.rs:130-194`.
- `/turns/:turn_id` is backed directly by `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.
- Journal and artifact routes are mounted at `crates/bridge-a2a-inbound/src/server.rs:294-301`.
- Coordinator task trace references are assembled from store reads at `crates/bridge-coordinator/src/coordinator.rs:795-823`.

## Global Constraints

- Data safety wins over byte reclamation.
- Rust remains `1.94.0`, as declared at `Cargo.toml:5-9`.
- Retention never deletes a `tasks` row.
- Retention never deletes artifacts belonging to a `Working` task.
- Artifact writers are not guarded by task status. Cancel-before-start, cancel-time `NodeFinished`, terminal transition, post-purge late writes, and resume progress writes must not be rejected by retention design.
- Task-linked artifact writers atomically bump `tasks.last_artifact_ms` in the same SQLite transaction or memory mutation guard that writes the artifact row.
- Storage authors every `last_artifact_ms` value after acquiring the SQLite immediate transaction or memory `journal_fold_guard`.
- Caller-provided event, checkpoint, completion, and terminal timestamps never author `last_artifact_ms`.
- `record_event_sequenced()` is a first-class artifact writer and participates in the same recency protocol.
- A task with purgeable artifacts and `last_artifact_ms = NULL` is never retention-eligible.
- `task_id IS NULL` is never an orphan predicate.
- A legacy row with `task_id IS NULL` and either `workflow` or `node` present is retained indefinitely.
- No session-ID string parsing, GLOB matching, ownership backfill, recency backfill, or time-based finalization reconciliation exists.
- Every turn-row deletion requires:
  - `completed_ms IS NOT NULL`;
  - `usage_finalized_ms IS NOT NULL`;
  - `usage_finalization_kind IN ('usage', 'no_usage')`;
  - both timestamps are retention-valid, not `0` or the never-eligible sentinel; and
  - an eligibility timestamp older than both the configured TTL and the 24-hour minimum-age floor.
- A lost or dropped finalization command leaves a row `pending` and therefore undeletable.
- Storage, not the producer or observer, authors `usage_finalized_ms` at persistence time.
- A clock read of `0` or another invalid retention timestamp never produces a deletable timestamp. Stores persist the never-eligible sentinel for durable recency/finalization/terminal barriers, or leave a barrier pending where explicitly stated.
- A retention sweep with invalid `now_ms` is a no-op.
- Retention age uses wall-clock timestamps.
- `artifacts_purged_at` records purge history and is never treated as proof that the current artifact set is empty.
- Present artifact rows are readable and discoverable even when purge history exists.
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
pub const RETENTION_NEVER_ELIGIBLE_MS: i64 = i64::MAX;

pub fn valid_retention_wall_ms(ms: i64) -> bool {
    ms > 0 && ms < RETENTION_NEVER_ELIGIBLE_MS
}

pub fn durable_retention_ms(ms: i64) -> i64 {
    if valid_retention_wall_ms(ms) {
        ms
    } else {
        RETENTION_NEVER_ELIGIBLE_MS
    }
}

pub type PersistenceClock =
    Arc<dyn Fn() -> i64 + Send + Sync + 'static>;

pub fn system_wall_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| {
            i64::try_from(duration.as_millis()).ok()
        })
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub enum TurnUsageFinalization {
    Usage(crate::orch::UsageSnapshot),
    NoUsage,
}

#[derive(Clone, Debug)]
pub struct TurnLogFinalized {
    pub ctx: crate::ports::TurnContext,
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
    /// max(task terminal/update time, known task artifact-recency time,
    /// linked turn completion/finalization time).
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
    pub last_artifact_ms: Option<i64>,
    pub input: String,
    pub workflow_spec_json: Option<String>,
    pub resume_attempts: u32,
    pub session_cwd: Option<String>,
    pub batch_id: Option<crate::ids::BatchId>,
    pub item_id: Option<String>,
    pub artifacts_purged_at: Option<i64>,
}
```

`last_artifact_ms` is the durable per-task artifact-recency barrier. It is storage-authored when a task-linked artifact row is written, not when a candidate is listed and not from the artifact’s caller timestamp. `None` means no trustworthy post-rev6 artifact-recency bump is known. A task that has purgeable artifacts while this field is `None` is ineligible; there is no fallback to `updated_ms`. Once a post-rev6 writer sets a valid barrier, task eligibility remains the settled maximum of `updated_ms`, `last_artifact_ms`, and linked-turn completion/finalization times.

`artifacts_purged_at` is set-once purge history. It does not assert that journal, checkpoint, or linked-turn rows are currently absent.

Extend `TurnLogRow`:

```rust
pub struct TurnLogRow {
    // existing fields unchanged
    pub usage_finalized_ms: Option<i64>,
    pub usage_finalization_kind: String,
}
```

Replace `update_turn_usage()` with explicit finalization, add three retention primitives, and add one bounded current-row existence read for trace-reference assembly:

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

    async fn task_journal_exists(
        &self,
        _task: &TaskId,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
}
```

SQLite implements `task_journal_exists()` as `SELECT EXISTS(...)`; memory checks whether the task’s journal vector is present and non-empty. It never loads or serializes the journal body.

There is no backfill, reconciliation, byte-accounting, size-eviction, or hard-task-delete method.

### SQLite schema and migration

The schema is created under the store’s single connection at `crates/bridge-store/src/sqlite.rs:15-20` and migrated through the additive-column path at `crates/bridge-store/src/sqlite.rs:197-259`.

Add `tasks.last_artifact_ms`, `tasks.artifacts_purged_at`, and two turn-log finalization columns:

```sql
ALTER TABLE tasks ADD COLUMN last_artifact_ms INTEGER;
ALTER TABLE tasks ADD COLUMN artifacts_purged_at INTEGER;

ALTER TABLE turn_log ADD COLUMN usage_finalized_ms INTEGER;
ALTER TABLE turn_log
    ADD COLUMN usage_finalization_kind TEXT NOT NULL DEFAULT 'pending';

CREATE INDEX IF NOT EXISTS idx_tasks_artifact_retention
    ON tasks(status, updated_ms, last_artifact_ms);

CREATE INDEX IF NOT EXISTS idx_turn_log_retention
    ON turn_log(usage_finalized_ms, completed_ms);
```

Each `ALTER TABLE` is conditional on `PRAGMA table_info`. Fresh schema creation includes the columns directly in the `tasks` table currently defined at `crates/bridge-store/src/sqlite.rs:130-138` and the `turn_log` table currently defined at `crates/bridge-store/src/sqlite.rs:165-191`.

Migration performs no `UPDATE`. Every pre-rev6 turn row therefore remains `usage_finalization_kind = 'pending'` with a NULL barrier, regardless of its token columns. Every pre-rev6 task row has `last_artifact_ms = NULL` and `artifacts_purged_at = NULL`. A migrated task with journal, checkpoint, or linked-turn artifacts is ineligible until a post-rev6 artifact writer sets `last_artifact_ms` or a future bounded repair explicitly supplies trustworthy recency. No ownership, finalization, or recency backfill is attempted.

`SqliteStore` gains a `PersistenceClock` beside the existing connection fields at `crates/bridge-store/src/sqlite.rs:15-20`. Production constructors use `system_wall_now_ms`; test constructors may inject a manual callback. Every artifact-recency and usage-finalization mutation reads this callback only after `TransactionBehavior::Immediate` has successfully begun.

Recreate one shared eligibility view after the columns exist. The numeric literal `9223372036854775807` is `RETENTION_NEVER_ELIGIBLE_MS`.

```sql
DROP VIEW IF EXISTS retention_artifact_eligibility;

CREATE VIEW retention_artifact_eligibility AS
WITH linked_turn_state AS (
    SELECT
        tl.task_id,
        SUM(
            CASE
                WHEN tl.completed_ms IS NULL
                  OR tl.usage_finalized_ms IS NULL
                  OR tl.usage_finalization_kind NOT IN ('usage', 'no_usage')
                  OR tl.completed_ms <= 0
                  OR tl.usage_finalized_ms <= 0
                  OR tl.completed_ms >= 9223372036854775807
                  OR tl.usage_finalized_ms >= 9223372036854775807
                THEN 1
                ELSE 0
            END
        ) AS blocked_turns,
        MAX(
            CASE
                WHEN tl.completed_ms > 0
                  AND tl.usage_finalized_ms > 0
                  AND tl.completed_ms < 9223372036854775807
                  AND tl.usage_finalized_ms < 9223372036854775807
                  AND tl.usage_finalization_kind IN ('usage', 'no_usage')
                THEN max(tl.completed_ms, tl.usage_finalized_ms)
                ELSE 9223372036854775807
            END
        ) AS linked_turn_recency_ms
    FROM turn_log tl
    WHERE tl.task_id IS NOT NULL
    GROUP BY tl.task_id
),
task_recency AS (
    SELECT
        t.id AS task_id,
        CASE
            WHEN t.updated_ms > 0
              AND t.updated_ms < 9223372036854775807
            THEN t.updated_ms
            ELSE 9223372036854775807
        END AS safe_updated_ms,
        CASE
            WHEN t.last_artifact_ms > 0
              AND t.last_artifact_ms < 9223372036854775807
            THEN t.last_artifact_ms
            ELSE 9223372036854775807
        END AS safe_last_artifact_ms
    FROM tasks t
)

SELECT
    'task' AS kind,
    t.id AS task_id,
    CAST(NULL AS TEXT) AS turn_id,
    max(
        r.safe_updated_ms,
        r.safe_last_artifact_ms,
        COALESCE(l.linked_turn_recency_ms, r.safe_updated_ms)
    ) AS eligible_ms
FROM tasks t
JOIN task_recency r ON r.task_id = t.id
LEFT JOIN linked_turn_state l ON l.task_id = t.id
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
  AND COALESCE(l.blocked_turns, 0) = 0

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
  AND tl.completed_ms > 0
  AND tl.usage_finalized_ms > 0
  AND tl.completed_ms < 9223372036854775807
  AND tl.usage_finalized_ms < 9223372036854775807
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

The absence of a NULL branch for `safe_last_artifact_ms` is deliberate. NULL, `0`, negative, and sentinel recency all become the fail-closed sentinel. Because the task arm requires at least one purgeable artifact, a migrated artifact set with unknown recency cannot be listed.

This view is the single SQLite definition of base eligibility. Candidate listing and delete-time guards both query it; TTL and the 24-hour floor remain service policy passed in as `eligible_before_ms`.

Update:

- Task INSERT/SELECT paths at `crates/bridge-store/src/sqlite.rs:532-623`.
- `working_tasks()` at `crates/bridge-store/src/sqlite.rs:738-753`.
- `row_to_task()` at `crates/bridge-store/src/sqlite.rs:1773-1805`.
- `TURN_LOG_SELECT` and `row_to_turn_log_row()` at `crates/bridge-store/src/sqlite.rs:271-314`.
- All `TaskRecord` constructors in production and tests.
- `SqliteStore` constructors at `crates/bridge-store/src/sqlite.rs:23-94` to supply the default or injected persistence clock.

### Task artifact recency write protocol

Current artifact writers only require the task row to exist:

- Legacy checkpoint write at `crates/bridge-store/src/sqlite.rs:646-660`.
- Sequenced node start at `crates/bridge-store/src/sqlite.rs:1225-1274`.
- Sequenced checkpoint/journal write at `crates/bridge-store/src/sqlite.rs:1278-1349`.
- Terminal sequenced journal write at `crates/bridge-store/src/sqlite.rs:1353-1409`.
- Rich-event journal write at `crates/bridge-store/src/sqlite.rs:1412-1450`.
- Memory equivalents at `crates/bridge-core/src/task_store.rs:715-735`, `crates/bridge-core/src/task_store.rs:1160-1198`, `crates/bridge-core/src/task_store.rs:1200-1255`, `crates/bridge-core/src/task_store.rs:1257-1314`, and `crates/bridge-core/src/task_store.rs:1316-1347`.
- Task-linked turn row writes at `crates/bridge-store/src/sqlite.rs:756-810` and `crates/bridge-core/src/task_store.rs:777-815`.
- Usage finalization writes replacing `crates/bridge-store/src/sqlite.rs:812-846` and `crates/bridge-core/src/task_store.rs:818-835`.

Slice 3b does not add `status='working'` to any of these writers. Each task-linked artifact writer instead:

1. acquires its SQLite immediate transaction or memory `journal_fold_guard`;
2. reads the store’s persistence clock;
3. computes `artifact_ms = durable_retention_ms(clock_value)`;
4. bumps `TaskRecord.last_artifact_ms`; and
5. writes the artifact before committing or releasing the guard.

Caller-provided `ts`, `completed_ms`, and context timestamps remain stored in their existing event/checkpoint/turn fields. They are not reused as artifact recency.

Shared SQLite helper shape:

```sql
UPDATE tasks
SET last_artifact_ms =
    CASE
        WHEN last_artifact_ms IS NULL OR last_artifact_ms < :artifact_ms
        THEN :artifact_ms
        ELSE last_artifact_ms
    END
WHERE id = :task_id;
```

`:artifact_ms` is `durable_retention_ms((self.now_ms)())`, computed after `BEGIN IMMEDIATE` succeeds. If the storage clock returns `0`, the persisted value is `RETENTION_NEVER_ELIGIBLE_MS`, not `0`. Because the update is monotonic, a sentinel remains sticky and fail closed until a future explicit repair.

Writer requirements:

1. `put_node_checkpoint()` begins an immediate transaction, computes `artifact_ms`, bumps `last_artifact_ms`, inserts `task_node_checkpoints` with the caller’s unchanged `ts`, and commits. Unknown task or duplicate checkpoint remains `StoreFailure`; terminal status is irrelevant.

2. `record_node_started()` begins an immediate transaction and computes `artifact_ms`. The existing sequence allocation update becomes:

```sql
UPDATE tasks
SET last_event_seq = last_event_seq + 1,
    last_artifact_ms =
        CASE
            WHEN last_artifact_ms IS NULL OR last_artifact_ms < :artifact_ms
            THEN :artifact_ms
            ELSE last_artifact_ms
        END
WHERE id = :task_id;
```

The transaction then reads `last_event_seq`, upserts `task_node_starts` with the caller’s `ts`, appends the `NodeStarted` journal row with that same event time, and commits.

3. `put_node_checkpoint_sequenced()` begins an immediate transaction, computes `artifact_ms`, uses the same sequence-plus-recency update, inserts `task_node_checkpoints` with caller-authored `ts`, deletes the node start row, appends the `NodeFinished` journal row, and commits.

4. `set_terminal_sequenced()` begins an immediate transaction, computes `artifact_ms`, and uses the same sequence-plus-recency update. It writes terminal status/result/error with `updated_ms = durable_retention_ms(ts)`, where `ts` remains the caller-authored terminal event time; clears starts; appends the terminal journal row; and commits. A stale terminal event time cannot age the newly appended artifact because `last_artifact_ms` comes from the later storage clock.

5. `record_event_sequenced()` begins an immediate transaction, computes `artifact_ms`, uses the same sequence-plus-recency update, appends the rich event with caller-authored `ts`, and commits. This covers the production sink at `crates/bridge-coordinator/src/detached.rs:400-414`.

6. `upsert_turn_finished()` begins an immediate transaction and computes `artifact_ms`. It upserts the `turn_log` row without clearing any existing finalization barrier. If `row.ctx.task_id` is `Some(task)` and the task row exists, it bumps `tasks.last_artifact_ms` to `artifact_ms` in the same transaction. A missing task does not reject the turn row because stale linked turns are a supported retention class. Caller-authored `started_ms` and `completed_ms` remain unchanged.

7. `finalize_turn_usage()` begins an immediate transaction, computes one storage-authored `persistence_ms` after transaction acquisition, maps invalid time through `durable_retention_ms()`, writes usage plus finalization atomically, and, if the finalized row has a task ID whose task row exists, uses the same `persistence_ms` to bump `tasks.last_artifact_ms`. No caller timestamp participates.

8. Legacy `set_terminal()`, `cancel_if_working()`, and `sweep_interrupted()` sanitize their `updated_ms` input with `durable_retention_ms()` because those paths can create terminal task eligibility without appending a journal row. This covers the live cancel call at `crates/bridge-coordinator/src/coordinator.rs:846-853`.

No writer rejects a terminal task solely because it is terminal. The `Failed("checkpoint write failed")` normalization at `crates/bridge-coordinator/src/detached.rs:1289-1298` cannot be triggered by retention’s status guard because that guard does not exist. Real store faults, duplicate checkpoint primary keys, invalid IDs, and contradictory usage finalizations can still fail as before.

### Turn-log ownership write fix

All detached, batch, and resume contexts set `task_id: Some(task.clone())`:

- Fresh detached submit: `crates/bridge-coordinator/src/coordinator.rs:701-716`.
- Batch child spawn: `crates/bridge-coordinator/src/batch.rs:860-875`.
- Detached boot resume: `crates/bridge-coordinator/src/detached.rs:1622-1686`.
- Batch boot resume: `crates/bridge-coordinator/src/batch.rs:513-565`.

The central runner also enforces ownership. This is an insertion at the current boundary where code builds `make_rich_sink` and calls `run_from_with_context` at `crates/bridge-coordinator/src/detached.rs:1252-1258`; current code does not already set `ctx.task_id` there.

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
        ctx.make_rich_sink = Some(Arc::new(DetachedRichSinkFactory {
            store: deps.task_store.clone(),
            task: task.clone(),
            op,
            hub: hub.clone(),
        }));
        ctx.task_id = Some(task.clone());
        let stream =
            executor.run_from_with_context(graph, input, run_id, token, seed, ctx);
        // Existing drain/finalization remains.
    })
}
```

The assignment occurs immediately before `run_from_with_context`, after all early validation and after sink construction, before any workflow turn can be emitted. Caller-side ownership fixes remain required, but the central overwrite is the authoritative safety boundary.

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

- Workflow prompt-open/early exits and normal completion at `crates/bridge-workflow/src/executor.rs:322-416`.
- Workflow retry outcomes at `crates/bridge-workflow/src/executor.rs:726-803`.
- Coordinator local turns at `crates/bridge-coordinator/src/coordinator.rs:568-641`.
- Coordinator cancellation drop guard at `crates/bridge-coordinator/src/coordinator.rs:906-930`.
- Inbound streaming cancellation, disconnect, send failure, and completion at `crates/bridge-a2a-inbound/src/server.rs:2164-2290`.

`DedupObserver` currently marks usage for every `UsageFinalized` at `crates/bridge-observ/src/lib.rs:67-83`. Change it to mark only turn-final usage events:

```rust
ObsEvent::UsageFinalized { ctx, fin, .. }
    if *fin == UsageFinalization::TurnFinal =>
{
    if !self.dedupe.mark_usage(&ctx.turn_id) {
        return;
    }
}
ObsEvent::UsageFinalized { .. } => {}
```

`TurnLogObserver` maps only `TurnFinal` events to one separate finalization command. The command contains no timestamp.

```rust
ObsEvent::UsageFinalized { ctx, usage, fin } => {
    if *fin != UsageFinalization::TurnFinal {
        return;
    }

    self.try_send(TurnLogCommand::Finalized(TurnLogFinalized {
        ctx: (*ctx).clone(),
        finalization: match usage {
            Some(usage) => TurnUsageFinalization::Usage((*usage).clone()),
            None => TurnUsageFinalization::NoUsage,
        },
    }));
}
```

The command remains independently droppable through the async worker at `crates/bridge-observ/src/lib.rs:219-238` and `try_send` at `crates/bridge-observ/src/lib.rs:257-264`. That is safe: a dropped command leaves the barrier pending and retention cannot delete the row.

`upsert_turn_finished()` at `crates/bridge-store/src/sqlite.rs:756-810` inserts `usage_finalization_kind = 'pending'`, leaves `usage_finalized_ms` NULL, and never clears an already-finalized barrier during conflict update. If the row is task-linked, it also bumps `tasks.last_artifact_ms` using the storage clock in the same transaction.

For `TurnUsageFinalization::Usage`, SQLite starts an immediate transaction, computes `persistence_ms = durable_retention_ms((self.now_ms)())` inside that transaction immediately before the update, and persists usage plus the barrier atomically:

```sql
UPDATE turn_log
SET input_tokens = COALESCE(:input_tokens, input_tokens),
    output_tokens = COALESCE(:output_tokens, output_tokens),
    thought_tokens = COALESCE(:thought_tokens, thought_tokens),
    cached_read_tokens = COALESCE(:cached_read_tokens, cached_read_tokens),
    cached_write_tokens = COALESCE(:cached_write_tokens, cached_write_tokens),
    cost_amount = COALESCE(:cost_amount, cost_amount),
    cost_currency = COALESCE(:cost_currency, cost_currency),
    usage_finalized_ms = :persistence_ms,
    usage_finalization_kind = 'usage'
WHERE turn_id = :turn_id
  AND completed_ms IS NOT NULL
  AND usage_finalized_ms IS NULL;
```

For `TurnUsageFinalization::NoUsage`, SQLite computes `persistence_ms` inside the same transaction and persists only an explicit no-usage barrier, rejecting contradictory existing usage:

```sql
UPDATE turn_log
SET usage_finalized_ms = :persistence_ms,
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

If `persistence_ms` is `RETENTION_NEVER_ELIGIBLE_MS`, the row is finalized for dedupe and diagnostics but never deletion-eligible until a future explicit repair slice exists. A value of `0` is never persisted by this method.

Zero affected rows are an error unless the row is already finalized with the same kind, in which case replay is idempotent. An unknown turn, contradictory no-usage marker, or conflicting finalization kind returns `StoreFailure`.

Memory performs the same state transition while holding `journal_fold_guard`, computes `persistence_ms` from its injected/default store clock after acquiring that guard, and uses the same value for task-linked `last_artifact_ms`. Producer or observer time is never copied into either durable persistence barrier.

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

The SQL provides deterministic oldest-first ordering. `RetentionService` does not reimplement eligibility or ordering. Tasks with NULL artifact recency appear in the view with sentinel `eligible_ms` and therefore cannot satisfy a normal cutoff.

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

The transaction never deletes `task_node_starts` or `tasks`. `artifacts_purged_at` remains set once if a later writer creates a new artifact set; a later eligible purge may delete those rows while preserving the original historical timestamp.

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

Run in an immediate transaction and return `standalone_turn_rows = affected_rows`. The shared view re-check ensures a row relinked to a real task, finalized with an invalid timestamp, or otherwise made ineligible after listing is retained.

### Memory shape

`MemoryTaskStore` currently owns task, checkpoint, turn-log, and journal maps at `crates/bridge-core/src/task_store.rs:597-619`, and its current `new()` construction is at `crates/bridge-core/src/task_store.rs:621-635`.

Required clock shape:

```rust
pub struct MemoryTaskStore {
    now_ms: PersistenceClock,
    journal_fold_guard: Mutex<()>,
    // existing maps
}

impl Default for MemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(system_wall_now_ms))
    }

    pub fn with_clock(now_ms: PersistenceClock) -> Self {
        Self {
            now_ms,
            journal_fold_guard: Mutex::new(()),
            // existing map initialization unchanged
        }
    }
}
```

Removing `#[derive(Default)]` and implementing `Default` explicitly preserves all existing `MemoryTaskStore::default()` and `MemoryTaskStore::new()` construction sites. The default is system wall time. Tests inject a deterministic callback through `with_clock()`.

Required behavior:

- Add `last_artifact_ms` and `artifacts_purged_at` to all stored `TaskRecord`s.
- Add finalization time/kind to `TurnLogRow`.
- Replace `update_turn_usage()` with the same explicit `Usage`/`NoUsage` state transition as SQLite.
- Stamp both finalization persistence time and artifact recency from `self.now_ms` inside `journal_fold_guard`, never from producer, observer, event, or checkpoint timestamps.
- Map invalid persistence/recency/terminal times through `durable_retention_ms()`.
- Keep `journal_fold_guard` for cross-map candidate snapshots and mutations.
- Do not reject `record_node_started`, `put_node_checkpoint`, `put_node_checkpoint_sequenced`, `set_terminal_sequenced`, or `record_event_sequenced` because a task is terminal.
- Require task existence where current code already does. Unknown task remains `StoreFailure`.
- For every task-linked artifact writer, acquire `journal_fold_guard`, compute one storage-authored `artifact_ms`, and hold the guard across task lookup, `last_artifact_ms` bump, and row/map write:
  - `put_node_checkpoint()` currently must be changed from its split-lock shape at `crates/bridge-core/src/task_store.rs:715-735`.
  - `record_node_started()` at `crates/bridge-core/src/task_store.rs:1160-1198`.
  - `put_node_checkpoint_sequenced()` at `crates/bridge-core/src/task_store.rs:1200-1255`.
  - `set_terminal_sequenced()` at `crates/bridge-core/src/task_store.rs:1257-1314`.
  - `record_event_sequenced()` at `crates/bridge-core/src/task_store.rs:1316-1347`.
  - task-linked `upsert_turn_finished()` at `crates/bridge-core/src/task_store.rs:777-815`.
  - task-linked `finalize_turn_usage()` replacing `crates/bridge-core/src/task_store.rs:818-835`.
- Implement one shared memory eligibility helper mirroring the SQLite view:
  - terminal task with at least one purgeable artifact;
  - `last_artifact_ms = None` makes such a task ineligible;
  - invalid `0` or sentinel task timestamps make the task ineligible;
  - every linked turn completed and explicitly finalized;
  - pure warm row only when task/workflow/node are all absent;
  - stale linked row only when `task_id` is non-NULL and absent from `inner`;
  - ambiguous legacy `NULL task_id` workflow/node rows never eligible;
  - once recency is known, task `eligible_ms` is the maximum of `updated_ms`, `last_artifact_ms`, and linked-turn completion/finalization persistence time.
- Candidate listing sorts by `(eligible_ms, kind, id)` and truncates to `limit`.
- Task artifact deletion re-runs the helper while holding `journal_fold_guard`, removes journal/checkpoint/linked-turn rows, and sets the marker without removing `inner`.
- Standalone turn deletion re-runs the helper before removing the row.
- `task_journal_exists()` reads the current journal vector under its map lock and does not consult `artifacts_purged_at`.
- No task-ID backfill, recency backfill, or session-ID parsing exists in memory.

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

The service combines TTL and the mandatory floor. Invalid sweep time disables the pass.

```rust
fn retention_cutoff_ms(now_ms: i64, days: u64) -> Option<i64> {
    if days == 0 || !valid_retention_wall_ms(now_ms) {
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

Clock semantics are wall-clock based, but invalid clock reads fail closed:

- a sweep with `now_ms <= 0` or the sentinel performs no candidate listing or deletion;
- a persisted recency/finalization/terminal timestamp of `0` is never treated as old;
- a NULL task artifact-recency barrier is never treated as `updated_ms`;
- store-authored invalid timestamps are persisted as `RETENTION_NEVER_ELIGIBLE_MS`, not `0`;
- event/checkpoint caller time does not affect artifact-recency persistence time.

### Task artifact-set purge

Guard predicate:

- Task status is terminal.
- The task still has journal, checkpoint, or linked-turn artifacts.
- `last_artifact_ms` is present and retention-valid.
- Every linked turn is completed and explicitly finalized as `usage` or `no_usage`.
- No linked turn has an invalid or never-eligible completion/finalization timestamp.
- The maximum of task terminal-update time, task artifact-recency time, and linked-turn completion/finalization persistence times is older than the effective cutoff.
- The task remains eligible when the immediate transaction begins.

Deletion:

- `task_journal` rows.
- `task_node_checkpoints` rows.
- `turn_log` rows with exact `task_id = task.id`.
- Set `tasks.artifacts_purged_at` in the same transaction.
- Retain the `tasks` row and all `task_node_starts`.

A post-purge writer may create new journal, checkpoint, or linked-turn rows. The historical marker remains, the writer stamps new recency, the new rows are returned by current-row reads, and they are not eligible until their storage-authored recency passes both age thresholds.

### Standalone turn purge

Guard predicate:

- The row is completed and explicitly finalized.
- Completion and finalization persistence timestamps are valid retention wall times, not `0` and not the sentinel.
- Its completion and finalization persistence times are both older than the effective cutoff.
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
                    .delete_task_artifact_set(
                        task,
                        cutoff_ms,
                        durable_retention_ms(now_ms),
                    )
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
4. Run one bounded retention pass before Prometheus rebuild. If the clock is invalid, the pass is a no-op. Migrated artifact sets with NULL recency are not candidates.
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

3. **Prevents new detached/batch turns from becoming ambiguous — closes Codex A3 after implementation:** callers set `WorkflowRunContext.task_id`, and `spawn_detached_workflow` overwrites it from its authoritative `task` argument immediately before execution.

4. **Prevents deletion of real usage after a dropped usage command — closes Codex A2:** NULL token/cost columns never imply no usage. Only a persisted storage-authored finalization barrier for `Usage(Some(...))` or explicit `NoUsage` sets deletion eligibility.

5. **Prevents deletion when a crash occurs between finish and finalization — closes Codex A2:** a finished row whose separate finalization command was lost remains `pending` indefinitely and is absent from the eligibility view.

6. **Prevents deletion during the finalization window — closes Codex A2 and Fable B2:** `RetentionService` applies a 24-hour wall-clock floor, and `eligible_ms` includes storage-stamped `usage_finalized_ms`, so no row is deleted until at least 24 hours after the barrier persisted.

7. **Prevents stale producer/enqueue timestamps from defeating the finalization floor:** `usage_finalized_ms` is computed inside the store mutation, not carried by `ObsEvent`, `TurnLogObserver`, or `TurnLogCommand`.

8. **Prevents invalid clock reads from becoming old deletion barriers — closes rev4 sign-off #1:** stores never persist `0` as artifact recency, terminal update, or finalization time; eligibility treats existing `0` as never eligible; sweeps skip invalid `now_ms`.

9. **Prevents migrated late artifacts from being purged using old task state — closes rev5 sign-off #1:** a task with purgeable artifacts and NULL `last_artifact_ms` receives sentinel eligibility rather than falling back to `updated_ms`.

10. **Prevents stalled writers from persisting stale artifact recency — closes rev5 sign-off #2:** all seven writers read the persistence clock only after transaction/guard acquisition. Caller event time may be stale without making a just-committed artifact old.

11. **Prevents late terminal-task artifact writes from being purged immediately — keeps rev4 sign-off #2 closed:** all task-linked artifact writers bump `last_artifact_ms` atomically, and known task eligibility uses `max(updated_ms, last_artifact_ms, linked-turn completion/finalization)`.

12. **Prevents the live-cancel path from being converted to `Failed("checkpoint write failed")`:** writers are not rejected just because `cancel_task()` has already flipped the task to `Canceled`; cancel-before-start and cancel-time `NodeFinished` progress writes remain valid.

13. **Prevents rich-event journal writes from bypassing retention recency — keeps rev4 sign-off #3 closed:** `record_event_sequenced()` storage-stamps `last_artifact_ms` in both stores before appending its journal row.

14. **Prevents memory stale-age interleaves — keeps rev4 sign-off #4 closed:** memory clock read, recency bump, row append, eligibility re-check, and deletion share `journal_fold_guard`.

15. **Prevents SQLite/memory clock drift — closes rev5 sign-off #4:** both stores implement the same injected/default `PersistenceClock` contract and sanitize its result through `durable_retention_ms()`.

16. **Prevents post-purge rows from becoming permanently undiscoverable — closes rev5 sign-off #3:** `artifacts_purged_at` is history only; trace references come from current rows, and present route bodies return 200 regardless of the marker.

17. **Prevents a missing post-purge artifact from being mistaken for never-known data:** an absent body on a known marked task still returns 410, while an absent body on an unmarked task remains 404.

18. **Prevents non-`TurnFinal` usage events from suppressing the durable barrier:** `DedupObserver` marks usage only for `UsageFinalization::TurnFinal`.

19. **Prevents deletion of frozen legacy workflow data — closes Review A ownership WRONG and Fable B3:** any `NULL task_id` row with workflow or node metadata is permanently excluded; there is no runtime repair attempting to guess its owner.

20. **Prevents working-task artifact deletion — preserves the endorsed rev2 guard:** task-set eligibility requires an exact terminal status and is re-checked inside `BEGIN IMMEDIATE`.

21. **Prevents list/delete predicate drift — closes Fable B5:** candidate listing and both delete guards use the same `retention_artifact_eligibility` view.

22. **Prevents TOCTOU deletion after state changes — closes Fable B5:** the store re-checks the shared view and age cutoff after acquiring the immediate transaction; a task made too recent by an artifact write or a turn relinked after listing produces a no-op.

23. **Prevents ambiguous finalization from becoming deletion-eligible — closes Codex A2:** legacy rows, contradictory explicit no-usage markers, conflicting finalization kinds, and invalid timestamps remain pending or never eligible.

24. **Prevents an unbounded migration stall — closes Codex A4:** schema migration executes only additive DDL, indexes, and view creation; it performs no row update.

25. **Prevents the dangerous untested feature surface — closes Codex A5 and Fable B1:** no hard-delete feature, cross-crate feature forwarding, conditional trait method, or feature-only SQL exists.

26. **Prevents a misleading disk-ceiling promise — closes Fable B2:** `artifact_retention_max_bytes` and logical-byte accounting are deferred; rev6 promises TTL reclamation only.

27. **Prevents corrupt pass statistics — closes Fable B6:** only row counts are additive. Pass metadata is set once and contains no summable before/after byte fields.

28. **Prevents boot stalls and unlimited per-pass deletion:** one query returns at most `STORAGE_RETENTION_BATCH_LIMIT` candidates; boot runs one pass and future hourly passes drain any backlog.

29. **Prevents partial task-artifact state from being mistaken for “never existed”:** marker update and artifact deletes share one transaction, and routes consult the marker only after proving the requested current row is absent.

30. **Prevents TTL arithmetic overflow from widening deletion:** overflow maps to `i64::MIN`, which no normal eligibility timestamp satisfies under the strict `<` comparison.

## Counter reconciliation

Current Prometheus rebuild reads persisted rows at `bin/a2a-bridge/src/main.rs:6119-6124` and delegates to `PrometheusObserver::rebuild_from_turn_log()` at `crates/bridge-observ/src/lib.rs:430-509`.

After Slice 3:

- Boot retention runs before rebuild, so boot counters reflect surviving turn rows.
- Periodic retention never decrements already-exported counters.
- Restart rebuild omits previously purged turns.
- Post-purge late turn rows are included on the next rebuild if they exist at restart.
- `DedupObserver` seeds and marks usage only for explicit `TurnFinal` usage finalization.
- `rebuild_from_turn_log()` seeds usage-finalization dedupe from `usage_finalized_ms` and `usage_finalization_kind`, not from the presence of token/cost columns. The current token-column heuristic is at `crates/bridge-observ/src/lib.rs:499-508`.
- An explicit `no_usage` row seeds finalization dedupe but contributes zero tokens and cost.
- A pending row does not seed finalization dedupe, so a replayed producer finalization may still complete it.
- A sentinel `usage_finalized_ms` row seeds finalization dedupe but is never retention-eligible.
- No synthetic usage values are created.
- Historical task usage for frozen legacy `NULL task_id` workflow rows remains incomplete by design; safety takes precedence over retrospective aggregation. Current aggregation keys strictly on `task_id` at `crates/bridge-store/src/sqlite.rs:895-934`.

## Slice-1/2 cohesion

### Slice 1

- `TurnFinished` still precedes `UsageFinalized`; current ordering tests are anchored at `crates/bridge-workflow/src/executor.rs:4604-4692`.
- `UsageFinalized` now always describes the producer’s final knowledge: `Some(snapshot)` or explicit `None`.
- Producer/observer code authors finalization kind only; storage authors finalization persistence time.
- Turn-log usage fields and the finalization barrier are written atomically.
- A lost finalization command leaves the durable row available for diagnostics and replay.
- Non-`TurnFinal` usage events cannot mark or suppress the durable finalization barrier.
- New detached, batch, and resume turns contribute to task usage because their exact task ID is written at creation.
- Legacy ambiguous ownership remains untouched.

### Slice 2

- `/turns/:turn_id` returns 404 after an eligible turn row is purged because the route directly reads `turn_log_row()` at `crates/bridge-a2a-inbound/src/server.rs:1009-1052`.
- Journal and checkpoint routes first confirm the task row, as they currently do at `crates/bridge-a2a-inbound/src/server.rs:1113-1139` and `crates/bridge-a2a-inbound/src/server.rs:1264-1290`.
- Route resolution order is:
  1. unknown task or invalid node ID: 404;
  2. requested current row exists: 200, regardless of `artifacts_purged_at`;
  3. requested current row is absent and `artifacts_purged_at.is_some()`: 410;
  4. requested current row is absent and the marker is NULL: 404.
- The 410 response is:

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

- The journal route replaces the current terminal-empty 404 branch at `crates/bridge-a2a-inbound/src/server.rs:1150-1166` with the current-body-first, marker-second distinction.
- The artifact route replaces the current unconditional missing-output 404 at `crates/bridge-a2a-inbound/src/server.rs:1361-1372` with the same distinction.
- `tasks/list` includes `artifacts_purged_at` beside the existing fields emitted at `crates/bridge-a2a-inbound/src/server.rs:3937-3952`.
- `tasks/get` includes the marker in task metadata when present; its durable-task path is at `crates/bridge-a2a-inbound/src/server.rs:3881-3910`, and durable task mapping is at `crates/bridge-a2a-inbound/src/server.rs:4150-4176`.
- Coordinator trace references at `crates/bridge-coordinator/src/coordinator.rs:795-823` are built exclusively from current rows:
  - `turns` comes from non-empty `turn_log_rows_for_task()`;
  - `artifacts` comes from non-empty `node_checkpoint_nodes()`;
  - `journal` is present only when `task_journal_exists()` returns true;
  - `artifacts_purged_at` is not consulted when constructing any reference.
- Immediately after a purge, no dead references are emitted because no current rows exist.
- After a permitted late write, references for the newly existing rows reappear even though the set-once marker remains.

No transcript table exists in the current SQLite schema; Slice 3 covers `task_journal`, `task_node_checkpoints`, and `turn_log`.

## Testing

### Slice 3a: ownership and finalization, no deletion

- `spawn_detached_workflow_overwrites_ctx_task_id`: pass `WorkflowRunContext { task_id: None, .. }` and a conflicting `Some(other_task)` in separate cases; every emitted workflow turn has the runner’s authoritative task ID.
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
- `usage_finalized_some_updates_usage_and_barrier_atomically`: token/cost values, storage-stamped `usage_finalized_ms`, and kind `usage` appear together.
- `usage_finalized_none_sets_no_usage_barrier`: explicit `None` leaves usage columns NULL and sets kind `no_usage`.
- `usage_finalization_uses_persistence_time_not_old_event_time`: simulate a finalization command whose producer/observer event time is more than 24 hours old; after the store persists it, assert `usage_finalized_ms` reflects persistence time and the row is not immediately retention-eligible.
- `usage_finalization_invalid_clock_uses_never_eligible_timestamp`: inject store clock `0`, finalize usage, and assert `usage_finalized_ms == RETENTION_NEVER_ELIGIBLE_MS`, never `0`, and no candidate appears after normal clock recovery.
- `no_usage_finalization_rejects_existing_usage_columns`: contradictory no-usage finalization affects no row and returns an error.
- `turn_finished_upsert_does_not_clear_finalization`: a replayed finish leaves an existing barrier and kind unchanged.
- `turn_finished_task_linked_bumps_artifact_recency`: upserting a task-linked turn bumps `last_artifact_ms` from the store clock in the same mutation.
- `finalize_turn_usage_task_linked_bumps_artifact_recency`: finalizing a task-linked turn bumps `last_artifact_ms` from the finalization persistence clock in the same mutation.
- `sqlite_legacy_migration_is_ddl_only`: open a pre-rev6 DB and assert every old row’s token/cost/task fields are unchanged, its barrier remains pending, and task recency/purge marker columns are NULL.
- `memory_finalization_matches_sqlite`: memory and SQLite agree for usage, no usage, duplicate, contradictory, unknown-turn, invalid-clock, sentinel, and persistence-time stamping cases.
- `dedup_observer_non_turn_final_usage_does_not_suppress_turn_final_barrier`: record a non-`TurnFinal` `UsageFinalized`, then a `TurnFinal`; the durable finalization event is still delivered.
- `prometheus_rebuild_seeds_explicit_no_usage_finalization`: no-usage row seeds dedupe without adding token/cost counters.
- `prometheus_rebuild_keeps_pending_finalization_replayable`: pending row seeds finish dedupe but not finalization dedupe.
- `prometheus_rebuild_sentinel_finalization_seeds_dedupe_without_retention_eligibility`: sentinel-finalized row seeds usage finalization dedupe but remains absent from retention candidates.

### Slice 3b: TTL retention and routes

The four rev6 sign-off regressions are mandatory:

- `pre_rev6_null_recency_with_fresh_late_artifact_is_ineligible`: build a pre-rev6 database containing a terminal task whose `updated_ms` is 30 days old and whose late artifact is one hour old; cover both a direct `task_journal` row corresponding to the current `record_event_sequenced()` behavior at `crates/bridge-store/src/sqlite.rs:1423-1448` and a legacy checkpoint corresponding to `crates/bridge-store/src/sqlite.rs:654-659`. Migrate, verify `last_artifact_ms IS NULL`, run the default-age boot pass, and assert neither case is listed or deleted.
- `stale_caller_time_does_not_age_just_committed_artifact_for_every_writer`: use a shared conformance matrix for all seven writer families—legacy checkpoint, node start, sequenced checkpoint, terminal sequenced event, rich event, task-linked turn finish, and task-linked usage finalization. Give each caller/event timestamp a value older than the cutoff, hold the writer before persistence where applicable, set the injected store clock to a fresh value, commit, and assert `last_artifact_ms` equals the storage time and the artifact set is not eligible. Run the matrix against real SQLite and memory stores.
- `purge_then_late_write_is_discoverable_and_missing_artifacts_stay_410`: purge an eligible canceled task, then perform a permitted late `NodeFinished` write. Assert the new checkpoint and journal rows return 200 and reappear in Coordinator task trace references despite the unchanged marker. Request a still-absent valid artifact and assert 410 with the original `artifacts_purged_at`; an unknown task and an unmarked never-created artifact remain 404.
- `memory_sqlite_persistence_clock_parity`: inject identical valid, zero, and sentinel clocks into memory and SQLite. For every writer family and usage finalization, assert identical `last_artifact_ms`, eligibility, and fail-closed behavior. Also verify `MemoryTaskStore::new()`, `MemoryTaskStore::default()`, and production SQLite construction use a valid system-wall-time default bounded by test reads before and after construction.

The retained rev5 coverage remains required, with fresh-write tests using stale caller timestamps and injected fresh storage clocks:

- `storage_config_defaults_to_fourteen_days`: missing `[storage]` yields `artifact_retention_days = 14`.
- `storage_config_zero_disables_retention`: zero produces no candidate-list or delete calls.
- `storage_config_rejects_negative_days`: negative TOML fails deserialization.
- `storage_config_rejects_removed_max_bytes_knob`: `artifact_retention_max_bytes` is an unknown-field error.
- `storage_config_rejects_removed_terminal_purge_knob`: `purge_terminal_tasks_days` is an unknown-field error.
- `storage_config_independent_from_metrics_and_traces`: storage parses and runs with metrics/traces disabled.
- `retention_cutoff_overflow_deletes_nothing`: huge day count maps to `i64::MIN`; no normal candidate is returned.
- `retention_sweep_invalid_now_is_noop`: `run_pass(0)` and `run_pass(RETENTION_NEVER_ELIGIBLE_MS)` return default stats and never call candidate listing.
- `retention_zero_recency_then_clock_recovery_does_not_delete`: force a task artifact recency timestamp of `0`, advance the sweep clock to a normal recovered value beyond TTL, and assert the row/artifacts are not deleted before any valid fresh artifact write plus 24 hours.
- `retention_null_recency_is_never_updated_ms_fallback`: task artifacts with NULL recency remain absent from candidates even when `updated_ms` is old and all linked turns are otherwise eligible.
- `retention_zero_finalization_then_clock_recovery_does_not_delete`: force a completed/finalized turn with `usage_finalized_ms = 0`, recover the clock, and assert the row is not listed or deleted.
- `cancel_with_invalid_clock_sets_non_deletable_task_timestamp`: inject `clock.now_ms() == 0` through `cancel_task()` and assert the persisted terminal `updated_ms` is not `0` and the task is not retention-eligible after clock recovery.
- `retention_minimum_age_floor_blocks_recent_terminal_task`: a task terminal for one second remains intact even when its configured TTL would otherwise permit deletion.
- `retention_minimum_age_floor_blocks_recent_finalization`: an old completed row finalized one second ago remains intact.
- `retention_wall_clock_semantics_are_explicit`: effective cutoff is computed from supplied wall-clock `now_ms`; invalid `now_ms` disables the pass.
- `retention_becomes_eligible_after_ttl_and_floor`: deletion occurs only after both thresholds are crossed.
- `sqlite_artifact_ttl_deletes_only_terminal_task_artifacts`: eligible terminal journal/checkpoint/linked turns are removed and the task remains; equivalent working-task artifacts remain.
- `sqlite_artifact_delete_rechecks_terminal_status`: list a task, change it to `Working`, then delete; the mutation returns zero and all artifacts remain.
- `sqlite_artifact_delete_rechecks_turn_finalization`: list a task, make a linked turn pending before deletion, then assert a no-op.
- `sqlite_artifact_delete_rechecks_last_artifact_recency`: list an eligible task, perform a fresh artifact write that storage-stamps `last_artifact_ms`, then delete; the mutation returns zero and all artifacts remain.
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
- `sqlite_record_node_started_fresh_write_resets_age`: pass a stale event `ts`, inject a fresh store clock, call `record_node_started()`, and assert storage-stamped recency advances and deletion no-ops until TTL plus floor elapse.
- `sqlite_put_node_checkpoint_fresh_write_resets_age`: same for legacy `put_node_checkpoint()`.
- `sqlite_put_node_checkpoint_sequenced_fresh_write_resets_age`: same for `put_node_checkpoint_sequenced()`.
- `sqlite_set_terminal_sequenced_fresh_write_resets_age`: same for the terminal journal append; caller-authored `updated_ms` may remain old while `last_artifact_ms` is fresh.
- `sqlite_record_event_sequenced_fresh_write_resets_age`: same for `record_event_sequenced()`.
- `sqlite_upsert_turn_finished_fresh_write_resets_age`: same for a task-linked `turn_log` upsert with stale `completed_ms`.
- `sqlite_finalize_turn_usage_fresh_write_resets_age`: same for task-linked usage finalization.
- `memory_fresh_write_resets_age_for_every_writer`: memory matches SQLite for `record_node_started`, `put_node_checkpoint`, `put_node_checkpoint_sequenced`, `set_terminal_sequenced`, `record_event_sequenced`, task-linked `upsert_turn_finished`, and task-linked `finalize_turn_usage`, using stale caller times and a fresh injected store clock.
- `memory_retention_matches_sqlite`: memory matches SQLite for terminal, working, warm, stale, pending, ambiguous, ordering, age, marker history, NULL recency, storage-authored recency, invalid clock, and persistence-time finalization behavior.
- `memory_put_node_checkpoint_recency_and_insert_are_atomic`: controlled interleaving proves no retention delete can observe stale recency after a checkpoint insert or insert a checkpoint after a deletion using stale age.
- `memory_record_event_sequenced_recency_and_insert_are_atomic`: controlled interleaving proves the rich-event journal writer is protected by the same guard.
- `memory_clock_is_read_after_journal_fold_guard`: block a writer before guard acquisition, advance the injected clock, release the guard, and assert the later value is persisted.
- `sqlite_clock_is_read_after_immediate_transaction_begins`: block a writer before transaction acquisition, advance the injected clock, release the transaction blocker, and assert the later value is persisted.
- `cancel_before_start_progress_writes_succeed`: create a detached task, register the cancel token, cancel before first `NodeStarted`, then let the runner write `NodeStarted` and terminal; assert writes succeed, task remains `Canceled`, and no `Failed("checkpoint write failed")` result is persisted.
- `node_finished_during_cancel_progress_write_succeeds`: exercise the executor ordering at `crates/bridge-workflow/src/executor.rs:1083-1106`; after cancellation, `NodeFinished` checkpoint and journal write succeed before terminal, task remains `Canceled`, and no `Failed("checkpoint write failed")` result is persisted.
- `resume_progress_writes_are_not_rejected_after_cancel_or_terminal_recency`: resumed working tasks still write starts/checkpoints/terminal events normally; terminal status alone is not a writer rejection reason.
- `post_purge_terminal_write_is_not_status_rejected`: a marked terminal task accepts a new task-linked artifact write and storage-stamps fresh recency.
- `retention_pass_counts_sum_only_row_counts`: multiple deletes produce exact row totals; no before/after byte fields are accumulated.
- `journal_route_returns_410_for_purged_task`: known marked task with no current journal returns the specified JSON 410.
- `artifact_route_returns_410_for_purged_task`: known marked task with no current checkpoint returns the specified JSON 410.
- `journal_route_returns_200_for_post_purge_write`: a journal row written after purge is returned despite the historical marker.
- `artifact_route_returns_200_for_post_purge_write`: a checkpoint written after purge is returned despite the historical marker.
- `artifact_routes_preserve_404_for_never_created_data`: known unmarked task with missing data remains 404.
- `artifact_routes_preserve_404_for_unknown_task`: unknown task remains 404.
- `tasks_get_and_list_surface_artifacts_purged_at`: both durable task surfaces include the historical marker.
- `coordinator_trace_refs_follow_current_rows_not_purge_marker`: immediately after purge there are no dead journal, artifact, or turn references; after late writes, references for current rows reappear while missing-row references remain absent.
- `task_journal_exists_matches_current_rows`: SQLite and memory return false before a write, true after a journal write, false after purge, and true after a post-purge write, regardless of marker state.
- `prometheus_boot_rebuild_uses_surviving_rows`: boot sweep runs first and rebuild counts only retained rows.
- `periodic_retention_does_not_decrement_live_counters`: counters remain monotonic until restart.
- `storage_boot_sweep_is_bounded`: fake store records exactly one boot list call with `STORAGE_RETENTION_BATCH_LIMIT`.
- `storage_periodic_first_tick_waits_one_hour`: paused Tokio time confirms no immediate second sweep.
- `storage_sweep_failure_does_not_abort_serve`: store failure logs a warning and server construction continues.
- `full_workspace_gates_pass`: run formatting, clippy with warnings denied, and the complete workspace test suite; report test totals and any behavior not exercised.

Every new deletion path has both an eligible-success test and an ineligible/TOCTOU negative test. Every artifact writer family has a stale-caller-time regression. Tests that require SQLite use the real `SqliteStore`, not SQL mocks.

## Acceptance criteria

1. `[storage]` contains only `artifact_retention_days`, defaulting to 14; zero disables retention.
2. `artifact_retention_max_bytes` and `purge_terminal_tasks_days` are absent and rejected as unknown storage fields.
3. No `unsafe-terminal-task-purge` feature exists in any crate.
4. No retention trait method or SQL statement deletes a `tasks` row.
5. Aged task-row deletion is explicitly deferred to a future Coordinator-owned tombstone/archival slice.
6. Detached, batch, test-helper, and resume workflow contexts set exact task ownership.
7. `spawn_detached_workflow` enforces `ctx.task_id = Some(task.clone())` immediately before execution, regardless of caller-supplied `None` or conflicting IDs.
8. No runtime or migration-time ownership backfill exists.
9. No session-ID fresh/resume matcher exists in SQL or memory.
10. Ambiguous legacy `NULL task_id` workflow/node rows are retained indefinitely.
11. Producers emit explicit turn-final `Some(usage)` or `None` after every `TurnFinished` path.
12. Producer/observer code authors finalization kind only; storage authors `usage_finalized_ms`.
13. Usage values and the `usage` finalization barrier are persisted atomically.
14. Explicit no usage, not elapsed time or NULL columns, is the only way to persist a `no_usage` barrier.
15. A lost/dropped finalization command leaves the row pending and undeletable.
16. A stale producer/observer event time cannot make a newly persisted finalization immediately eligible.
17. `DedupObserver` dedupes `UsageFinalized` only for `UsageFinalization::TurnFinal`.
18. Migration performs no data-row updates.
19. A task with purgeable artifacts and `last_artifact_ms = NULL` is ineligible; eligibility never falls back from NULL recency to `updated_ms`.
20. Artifact writers are not status-guarded; terminal or historically purged status alone never rejects a start, checkpoint, journal, terminal, rich-event, or task-linked turn write.
21. Every task-linked artifact writer bumps `tasks.last_artifact_ms` in the same SQLite transaction or memory mutation guard as the artifact row write.
22. Storage authors every artifact-recency bump after immediate-transaction or `journal_fold_guard` acquisition.
23. Caller-provided event/checkpoint/completion timestamps never author `last_artifact_ms`.
24. `record_event_sequenced()` is included in the recency protocol in both stores.
25. Invalid clock values are never persisted as deletable recency, terminal-update, or finalization timestamps.
26. A retention sweep with invalid `now_ms` is a no-op.
27. `RetentionService` applies a minimum wall-clock age of 24 hours in addition to configured TTL.
28. Once task artifact recency is known, task eligibility time includes task terminal/update time, storage-authored artifact-recency time, and linked-turn completion/finalization persistence time.
29. SQLite defines eligibility once in `retention_artifact_eligibility`.
30. `TaskStore` owns eligibility mechanics, atomic guards, deterministic ordering SQL, persistence-time finalization stamping, persistence-time artifact-recency stamping, and current-journal existence reads; `RetentionService` owns sequencing, the candidate cap, TTL, and the age floor.
31. Every delete re-checks the shared eligibility definition and cutoff under an immediate transaction.
32. Task artifact purge atomically deletes journal/checkpoint/exact-linked-turn rows and sets `artifacts_purged_at`.
33. `artifacts_purged_at` is set-once purge history, not a current-emptiness flag.
34. Current post-purge rows return 200 and appear in task trace references; an absent artifact on a marked task returns 410; unknown or never-created data returns 404.
35. Coordinator trace references are derived from current journal, checkpoint, and linked-turn rows without consulting the marker.
36. `ArtifactDeleteCounts` is the only summable statistics type; pass metadata and future byte snapshots are never summed.
37. Boot retention runs once, bounded, before Prometheus rebuild.
38. Periodic retention starts after Coordinator resume and waits one hour before its first tick.
39. Prometheus counters remain monotonic during a process lifetime and rebuild from surviving rows after restart.
40. SQLite and memory stores implement identical eligibility, recency, invalid-clock, persistence-clock, marker-history, and atomic-guard behavior.
41. `MemoryTaskStore::new()` and `Default` use system wall time; tests can inject a deterministic persistence clock.
42. Cancel-before-start and cancel-time `NodeFinished` progress writes succeed and cannot be normalized to `Failed("checkpoint write failed")` by retention design.
43. Slice 3a lands and passes the full workspace suite before Slice 3b enables deletion.
44. Formatting, clippy with warnings denied, and the full workspace test suite pass with reported totals.

## Non-goals

- No terminal `TaskRecord` deletion.
- No `purge_terminal_tasks_days`.
- No `unsafe-terminal-task-purge` Cargo feature.
- No terminal-task victim query or session-ID GLOB matcher.
- No runtime legacy task-ID backfill.
- No migration-time recency backfill.
- No migration-time or timer-based finalization reconciliation.
- No producer-authored durable finalization timestamp.
- No caller-authored durable artifact-recency timestamp.
- No automatic deletion of task artifacts whose `last_artifact_ms` remains NULL.
- No automatic deletion of ambiguous legacy `NULL task_id` workflow turns.
- No status-based artifact writer guard.
- No clearing or rewriting `artifacts_purged_at` after late writes.
- No use of `artifacts_purged_at` as proof that current rows are absent.
- No monotonic-clock retention guard; Slice 3 uses explicit wall-clock semantics with invalid-clock fail-closed handling.
- No repair of rows made never-eligible by NULL or invalid recency; a future explicit bounded repair/admin slice may address them.
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
