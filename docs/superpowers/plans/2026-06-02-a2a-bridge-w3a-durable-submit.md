# W3a — Durable Detached Submit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run a workflow detached — submit it, get a task id immediately, retrieve its persisted status+result later (surviving the client's exit and, for terminal tasks, a `serve` restart).

**Architecture:** A new `TaskStore` port (in `bridge-core`, with an in-memory default impl + a file-backed SQLite impl in `bridge-store`). `serve`'s `message/send` on a workflow skill becomes non-blocking: it mints a unique task id, persists `Working`, spawns a finalizer-guarded background runner that drains the `WorkflowExecutor` stream over a shared `WorkflowSink` and writes the terminal row, and returns a canonical `a2a::Task{working}` immediately. `tasks/get`/`tasks/cancel`/`tasks/list` read the store; a thin `reqwest` CLI (`submit`/`task get|list|cancel`) is a client of `serve`.

**Tech Stack:** Rust, tokio, axum, rusqlite (bundled), fs2 (advisory file lock), uuid (v4), `a2a-lf 0.3.0` SDK (`a2a::Task`/`TaskState`/`methods`), tokio_util `CancellationToken`.

**Spec:** `docs/superpowers/specs/2026-06-02-a2a-bridge-w3a-durable-detached-submit-design.md` (rev2, dual-reviewed).

**Branch:** `feat/w3a-durable-submit` off `main`.

**Spec deviations discovered during planning (grounded against the code):**
- `a2a::new_task_id()` does not exist → mint with `uuid::Uuid::new_v4()`.
- The SDK has no list-tasks method → `tasks/list` is a bridge-defined `const LIST_TASKS: &str = "tasks/list"`.
- `task_id_from_params` returns a fixed `"task-1"` for no-id sends (confirmed) → detached path must NOT use it for id minting.

---

## File Structure

- **Create** `crates/bridge-core/src/task_store.rs` — `TaskRecord`, `TaskRecordStatus`, `TaskStore` trait, `MemoryTaskStore`. One focused module; the durable-task control-plane port + its in-memory impl.
- **Modify** `crates/bridge-core/src/lib.rs` — `pub mod task_store;`
- **Modify** `crates/bridge-store/src/sqlite.rs` — `open(path)` + single-serve lock; `impl TaskStore`; `tasks` table.
- **Modify** `crates/bridge-store/Cargo.toml` — add `fs2`.
- **Create** `crates/bridge-a2a-inbound/src/workflow_sink.rs` — `WorkflowSink` trait + `drain_workflow` + `SseSink` + `TaskStoreSink` + `Finalizer` guard + `now_ms()`.
- **Modify** `crates/bridge-a2a-inbound/src/lib.rs` — `mod workflow_sink;`
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — `task_store` field + `.with_task_store`; refactor `spawn_workflow_producer` to use `drain_workflow`+`SseSink`; add `spawn_detached_workflow`; `message/send` workflow → detached; `get_task` canonical+store-aware; `cancel_task` store-aware; `tasks/list` dispatch+handler; `new_detached_task_id`.
- **Modify** `crates/bridge-a2a-inbound/Cargo.toml` — add `uuid` (v4) and `bridge-store` is NOT added (must stay absent).
- **Modify** `crates/bridge-a2a-inbound/tests/workflow_producer.rs` — rewrite the unary-reject test; add detached tests.
- **Modify** `bin/a2a-bridge/src/config.rs` — `StoreConfig { path }` + `RegistryConfig.store`.
- **Modify** `bin/a2a-bridge/src/main.rs` — store wiring (open(path) vs memory) + `.with_task_store` + boot sweep; CLI `submit`/`task` subcommands (reqwest client).
- **Create** `docs/adr/0010-durable-detached-submit.md` — record W3a + dual-review corrections.

---

## Phase A — `bridge-core` TaskStore port (additive, immune to ripple)

### Task 1: `TaskRecord` + `TaskRecordStatus` + `TaskStore` trait

**Files:**
- Create: `crates/bridge-core/src/task_store.rs`
- Modify: `crates/bridge-core/src/lib.rs`

- [ ] **Step 1: Create the module with types + trait (no impl yet)**

Create `crates/bridge-core/src/task_store.rs`:

```rust
//! Durable task control-plane port: persists a detached workflow run's status
//! and final result. Separate from `SessionStore` (ephemeral routing state) by
//! responsibility. Timestamps are passed IN — the core forbids `Date::now`.

use crate::error::BridgeError;
use crate::ids::TaskId;

/// Durable status of a detached task. `Interrupted` is distinct from `Failed`
/// (a crash mid-run, swept on the next boot) so triage can tell them apart; the
/// A2A wire collapses it to `failed` (see the inbound server).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskRecordStatus {
    Working,
    Completed,
    Failed,
    Canceled,
    Interrupted,
}

impl TaskRecordStatus {
    /// Lowercase wire/storage token.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskRecordStatus::Working => "working",
            TaskRecordStatus::Completed => "completed",
            TaskRecordStatus::Failed => "failed",
            TaskRecordStatus::Canceled => "canceled",
            TaskRecordStatus::Interrupted => "interrupted",
        }
    }
    /// Parse a stored token; unknown → None.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(TaskRecordStatus::Working),
            "completed" => Some(TaskRecordStatus::Completed),
            "failed" => Some(TaskRecordStatus::Failed),
            "canceled" => Some(TaskRecordStatus::Canceled),
            "interrupted" => Some(TaskRecordStatus::Interrupted),
            _ => None,
        }
    }
    pub fn is_terminal(&self) -> bool {
        !matches!(self, TaskRecordStatus::Working)
    }
}

/// One durable task row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: TaskId,
    pub workflow: String,
    pub status: TaskRecordStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    /// Non-clobbering INSERT. A duplicate id MUST return an error (NOT upsert),
    /// so a resubmit/colliding id can never overwrite a terminal result.
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError>;
    /// Set the terminal status + result/error on an existing row.
    async fn set_terminal(
        &self,
        id: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError>;
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError>;
    /// Newest-first, capped at `limit`.
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError>;
    /// Flip every `Working` row to `Interrupted`; returns the count flipped.
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError>;
}
```

Add to `crates/bridge-core/src/lib.rs` (after the existing `pub mod` lines):

```rust
pub mod task_store;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p bridge-core`
Expected: builds clean (types + trait only).

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-core/src/task_store.rs crates/bridge-core/src/lib.rs
git commit -m "feat(core): TaskStore port + TaskRecord/TaskRecordStatus"
```

### Task 2: `MemoryTaskStore` (the in-memory default)

**Files:**
- Modify: `crates/bridge-core/src/task_store.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-core/src/task_store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
        }
    }

    #[tokio::test]
    async fn create_get_roundtrip_then_set_terminal() {
        let s = MemoryTaskStore::new();
        let id = TaskId::parse("t-a").unwrap();
        s.create(&rec("t-a", 10)).await.unwrap();
        assert_eq!(s.get(&id).await.unwrap().unwrap().status, TaskRecordStatus::Working);
        s.set_terminal(&id, TaskRecordStatus::Completed, Some("OUT"), None, 20)
            .await
            .unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("OUT"));
        assert_eq!(got.updated_ms, 20);
    }

    #[tokio::test]
    async fn create_is_non_clobbering() {
        let s = MemoryTaskStore::new();
        s.create(&rec("dup", 1)).await.unwrap();
        // second create with the same id MUST error (not overwrite)
        assert!(s.create(&rec("dup", 2)).await.is_err());
    }

    #[tokio::test]
    async fn list_is_newest_first_and_limited() {
        let s = MemoryTaskStore::new();
        s.create(&rec("old", 1)).await.unwrap();
        s.create(&rec("new", 5)).await.unwrap();
        let got = s.list(1).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.as_str(), "new");
    }

    #[tokio::test]
    async fn sweep_flips_only_working() {
        let s = MemoryTaskStore::new();
        s.create(&rec("w", 1)).await.unwrap();
        let done = TaskId::parse("done").unwrap();
        s.create(&rec("done", 1)).await.unwrap();
        s.set_terminal(&done, TaskRecordStatus::Completed, Some("x"), None, 2)
            .await
            .unwrap();
        let n = s.sweep_interrupted(99).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            s.get(&TaskId::parse("w").unwrap()).await.unwrap().unwrap().status,
            TaskRecordStatus::Interrupted
        );
        // the already-completed row is untouched
        assert_eq!(s.get(&done).await.unwrap().unwrap().status, TaskRecordStatus::Completed);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core task_store::tests`
Expected: FAIL — `MemoryTaskStore` not found.

- [ ] **Step 3: Implement `MemoryTaskStore`**

Insert into `crates/bridge-core/src/task_store.rs` (before the `#[cfg(test)]` module):

```rust
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory `TaskStore` (the default when no DB path is configured). Production
/// use, not just a test fake — lives in `bridge-core` so `bridge-a2a-inbound`
/// can default to it WITHOUT depending on `bridge-store`.
#[derive(Default)]
pub struct MemoryTaskStore {
    inner: Mutex<HashMap<String, TaskRecord>>,
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl TaskStore for MemoryTaskStore {
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
        let mut g = self.inner.lock().unwrap();
        if g.contains_key(rec.id.as_str()) {
            return Err(BridgeError::StoreFailure);
        }
        g.insert(rec.id.as_str().to_string(), rec.clone());
        Ok(())
    }
    async fn set_terminal(
        &self,
        id: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError> {
        let mut g = self.inner.lock().unwrap();
        let row = g.get_mut(id.as_str()).ok_or(BridgeError::StoreFailure)?;
        row.status = status;
        row.result = result.map(|s| s.to_string());
        row.error = error.map(|s| s.to_string());
        row.updated_ms = updated_ms;
        Ok(())
    }
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
        Ok(self.inner.lock().unwrap().get(id.as_str()).cloned())
    }
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<TaskRecord> = g.values().cloned().collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        v.truncate(limit);
        Ok(v)
    }
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
        let mut g = self.inner.lock().unwrap();
        let mut n = 0;
        for row in g.values_mut() {
            if row.status == TaskRecordStatus::Working {
                row.status = TaskRecordStatus::Interrupted;
                row.error = Some("interrupted (serve restarted)".into());
                row.updated_ms = updated_ms;
                n += 1;
            }
        }
        Ok(n)
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p bridge-core task_store::tests`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/task_store.rs
git commit -m "feat(core): MemoryTaskStore (non-clobbering create, newest-first list, sweep)"
```

---

## Phase B — `bridge-store` SQLite impl (additive)

### Task 3: `SqliteStore::open(path)` + single-serve advisory lock

**Files:**
- Modify: `crates/bridge-store/Cargo.toml`
- Modify: `crates/bridge-store/src/sqlite.rs`

- [ ] **Step 1: Add the `fs2` dep**

In `crates/bridge-store/Cargo.toml` `[dependencies]`, add:

```toml
fs2 = "0.4"
```

- [ ] **Step 2: Write the failing test**

Append to the `#[cfg(test)] mod tests` in `crates/bridge-store/src/sqlite.rs` (it already exists; add these):

```rust
    #[test]
    fn second_open_same_path_fails_lock() {
        let dir = std::env::temp_dir().join(format!("a2a-w3a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lock-test.db");
        let _first = SqliteStore::open(&path).expect("first open succeeds");
        let second = SqliteStore::open(&path);
        assert!(second.is_err(), "second open of a locked db must fail");
        drop(_first);
        // after the first is dropped, a fresh open succeeds again
        assert!(SqliteStore::open(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p bridge-store second_open_same_path_fails_lock`
Expected: FAIL — `SqliteStore::open` not found.

- [ ] **Step 4: Implement `open(path)` + the lock field**

In `crates/bridge-store/src/sqlite.rs`, change the struct to hold the lock file, and add `open`:

Change the struct (lines ~13-15):

```rust
pub struct SqliteStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    // Held for the store's lifetime: an exclusive advisory lock on `<path>.lock`
    // so only one `serve` owns a DB file (makes the boot sweep safe). `None` for
    // in-memory stores.
    _lock: Option<std::fs::File>,
}
```

Update `open_in_memory` to set `_lock: None`:

```rust
    pub fn open_in_memory() -> Result<Self, BridgeError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            _lock: None,
        };
        store.create_schema()?;
        Ok(store)
    }

    /// Open a file-backed DB, acquiring an exclusive advisory lock on `<path>.lock`.
    /// A second `open` of the same path while the first is held returns an error —
    /// this single-serve-per-DB guarantee is what makes the boot `sweep_interrupted`
    /// safe (it can never flip a live serve's `Working` rows).
    pub fn open(path: &std::path::Path) -> Result<Self, BridgeError> {
        use fs2::FileExt;
        let lock_path = {
            let mut p = path.as_os_str().to_os_string();
            p.push(".lock");
            std::path::PathBuf::from(p)
        };
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
            .map_err(|_| BridgeError::StoreFailure)?;
        lock.try_lock_exclusive().map_err(|_| BridgeError::StoreFailure)?;
        let conn = rusqlite::Connection::open(path).map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            _lock: Some(lock),
        };
        store.create_schema()?;
        Ok(store)
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p bridge-store second_open_same_path_fails_lock`
Expected: PASS. Also run `cargo test -p bridge-store` — existing session tests still green (the new `_lock: None` field doesn't affect them).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-store/Cargo.toml crates/bridge-store/src/sqlite.rs
git commit -m "feat(store): SqliteStore::open(path) file-backed + single-serve advisory lock"
```

### Task 4: `tasks` table + `impl TaskStore for SqliteStore`

**Files:**
- Modify: `crates/bridge-store/src/sqlite.rs`

- [ ] **Step 1: Write the failing tests**

Append to `#[cfg(test)] mod tests` in `crates/bridge-store/src/sqlite.rs`:

```rust
    use bridge_core::task_store::{TaskRecord, TaskRecordStatus, TaskStore};

    fn trec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
        }
    }

    #[tokio::test]
    async fn task_create_get_set_terminal_inmemory() {
        let s = SqliteStore::open_in_memory().unwrap();
        let id = TaskId::parse("t1").unwrap();
        s.create(&trec("t1", 1)).await.unwrap();
        assert_eq!(s.get(&id).await.unwrap().unwrap().status, TaskRecordStatus::Working);
        s.set_terminal(&id, TaskRecordStatus::Completed, Some("SYNTH"), None, 9).await.unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("SYNTH"));
        // non-clobbering create
        assert!(s.create(&trec("t1", 2)).await.is_err());
    }

    #[tokio::test]
    async fn task_durable_across_reopen() {
        let dir = std::env::temp_dir().join(format!("a2a-w3a-dur-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dur.db");
        {
            let s = SqliteStore::open(&path).unwrap();
            let id = TaskId::parse("keep").unwrap();
            s.create(&trec("keep", 1)).await.unwrap();
            s.set_terminal(&id, TaskRecordStatus::Completed, Some("R"), None, 2).await.unwrap();
        } // drop releases the lock
        let s2 = SqliteStore::open(&path).unwrap();
        let got = s2.get(&TaskId::parse("keep").unwrap()).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("R"));
        drop(s2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn task_sweep_and_list_inmemory() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.create(&trec("a", 1)).await.unwrap();
        s.create(&trec("b", 3)).await.unwrap();
        assert_eq!(s.list(10).await.unwrap()[0].id.as_str(), "b"); // newest-first
        let n = s.sweep_interrupted(99).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(s.get(&TaskId::parse("a").unwrap()).await.unwrap().unwrap().status, TaskRecordStatus::Interrupted);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-store task_create_get_set_terminal_inmemory`
Expected: FAIL — `TaskStore` not implemented for `SqliteStore`.

- [ ] **Step 3: Add the `tasks` table to `create_schema`**

In `create_schema` (the `execute_batch` string), append a second statement after the `sessions` table:

```rust
            "CREATE TABLE IF NOT EXISTS sessions (
                task_id TEXT PRIMARY KEY,
                session_id TEXT,
                pending_request_id TEXT,
                pending_kind TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                peer_task_id TEXT,
                cancel_requested INTEGER NOT NULL DEFAULT 0,
                fanout INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id         TEXT PRIMARY KEY,
                workflow   TEXT NOT NULL,
                status     TEXT NOT NULL,
                result     TEXT,
                error      TEXT,
                created_ms INTEGER NOT NULL,
                updated_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_ms);",
```

- [ ] **Step 4: Implement `TaskStore` for `SqliteStore`**

Add a new `impl` block in `crates/bridge-store/src/sqlite.rs` (after the `impl SessionStore` block):

```rust
#[async_trait::async_trait]
impl bridge_core::task_store::TaskStore for SqliteStore {
    async fn create(
        &self,
        rec: &bridge_core::task_store::TaskRecord,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        // Plain INSERT (no ON CONFLICT): a duplicate id errors — non-clobbering.
        conn.execute(
            "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                rec.status.as_str(),
                rec.result,
                rec.error,
                rec.created_ms,
                rec.updated_ms
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn set_terminal(
        &self,
        id: &TaskId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE tasks SET status=?2, result=?3, error=?4, updated_ms=?5 WHERE id=?1",
                rusqlite::params![id.as_str(), status.as_str(), result, error, updated_ms],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if n == 0 {
            return Err(BridgeError::StoreFailure); // no such row
        }
        Ok(())
    }

    async fn get(
        &self,
        id: &TaskId,
    ) -> Result<Option<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms
                 FROM tasks WHERE id=?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![id.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_task(row)?)),
        }
    }

    async fn list(
        &self,
        limit: usize,
    ) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms
                 FROM tasks ORDER BY updated_ms DESC LIMIT ?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![limit as i64])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            out.push(row_to_task(row)?);
        }
        Ok(out)
    }

    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE tasks SET status='interrupted', error='interrupted (serve restarted)', updated_ms=?1
                 WHERE status='working'",
                rusqlite::params![updated_ms],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n as u64)
    }
}

fn row_to_task(
    row: &rusqlite::Row,
) -> Result<bridge_core::task_store::TaskRecord, BridgeError> {
    use bridge_core::task_store::{TaskRecord, TaskRecordStatus};
    let id: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
    let workflow: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
    let status_s: String = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
    let result: Option<String> = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
    let error: Option<String> = row.get(4).map_err(|_| BridgeError::StoreFailure)?;
    let created_ms: i64 = row.get(5).map_err(|_| BridgeError::StoreFailure)?;
    let updated_ms: i64 = row.get(6).map_err(|_| BridgeError::StoreFailure)?;
    Ok(TaskRecord {
        id: TaskId::parse(id).map_err(|_| BridgeError::StoreFailure)?,
        workflow,
        status: TaskRecordStatus::parse(&status_s).ok_or(BridgeError::StoreFailure)?,
        result,
        error,
        created_ms,
        updated_ms,
    })
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p bridge-store`
Expected: all green (session + the 3 new task tests).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-store/src/sqlite.rs
git commit -m "feat(store): tasks table + TaskStore impl (non-clobbering create, durable reopen, sweep)"
```

---

## Phase C — Workflow sink + unified drain (refactor; keep streaming green)

### Task 5: `WorkflowSink` + `drain_workflow` + `SseSink`; refactor `spawn_workflow_producer`

**Files:**
- Create: `crates/bridge-a2a-inbound/src/workflow_sink.rs`
- Modify: `crates/bridge-a2a-inbound/src/lib.rs`
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`spawn_workflow_producer` only)

**Context:** `spawn_workflow_producer` (server.rs:968-1041) drains the executor stream and forwards to an SSE `tx`. We extract the drain + outcome mapping + token/no-terminal handling into a reusable `drain_workflow` over a `WorkflowSink`, and make the streaming producer one sink. The existing `tests/workflow_producer.rs` streaming tests are the regression guard — behavior must not change.

- [ ] **Step 1: Create `workflow_sink.rs`**

Create `crates/bridge-a2a-inbound/src/workflow_sink.rs`:

```rust
//! One drain over the workflow stream, parameterized by a sink. The streaming
//! producer (SSE) and the detached runner (TaskStore) share the drain, the
//! WorkflowOutcome mapping, and the no-terminal guard — they differ only in sink.

use bridge_core::error::BridgeError;
use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome, WorkflowStream};
use futures::StreamExt;

/// A sink consumes the workflow's events. Intermediate node events are optional
/// (the detached sink ignores them in W3a); `terminal` is the meaningful one.
#[async_trait::async_trait]
pub(crate) trait WorkflowSink: Send {
    async fn node_started(&mut self, _node: &str) {}
    async fn node_finished(&mut self, _node: &str, _ok: bool) {}
    async fn terminal(&mut self, outcome: WorkflowOutcome, output: String);
    async fn error(&mut self, _err: BridgeError) {}
}

/// Drive the stream into the sink. Returns `true` if a terminal event was seen
/// (the caller handles the no-terminal case per its own sink semantics).
pub(crate) async fn drain_workflow<S: WorkflowSink>(
    mut stream: WorkflowStream,
    sink: &mut S,
) -> bool {
    let mut terminal_seen = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => sink.node_started(node.as_str()).await,
            Ok(WorkflowEvent::NodeFinished { node, ok }) => {
                sink.node_finished(node.as_str(), ok).await
            }
            Ok(WorkflowEvent::Terminal { outcome, output }) => {
                sink.terminal(outcome, output).await;
                terminal_seen = true;
            }
            Err(e) => sink.error(e).await,
        }
    }
    terminal_seen
}

/// Unix-ms timestamp (server-side; `bridge-core` forbids `Date::now`, the server does not).
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

Add to `crates/bridge-a2a-inbound/src/lib.rs`:

```rust
mod workflow_sink;
```

- [ ] **Step 2: Refactor `spawn_workflow_producer` to use the drain + an SSE sink**

In `crates/bridge-a2a-inbound/src/server.rs`, replace the body of `spawn_workflow_producer` (lines 968-1041) with a version that uses an inline SSE sink. Add this sink type near the function and rewrite the drive:

```rust
struct SseSink {
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
}

#[async_trait::async_trait]
impl crate::workflow_sink::WorkflowSink for SseSink {
    async fn node_started(&mut self, node: &str) {
        let _ = self.tx.send(Ok(Event::status(format!("node {node} started")))).await;
    }
    async fn node_finished(&mut self, node: &str, ok: bool) {
        let _ = self
            .tx
            .send(Ok(Event::status(format!(
                "node {node} {}",
                if ok { "ok" } else { "failed" }
            ))))
            .await;
    }
    async fn terminal(
        &mut self,
        outcome: bridge_workflow::executor::WorkflowOutcome,
        output: String,
    ) {
        use bridge_workflow::executor::WorkflowOutcome;
        let _ = self.tx.send(Ok(Event::artifact(output))).await;
        let to = match outcome {
            WorkflowOutcome::Completed => TaskOutcome::Completed,
            WorkflowOutcome::Failed => TaskOutcome::Failed,
            WorkflowOutcome::Canceled => TaskOutcome::Canceled,
        };
        let _ = self.tx.send(Ok(Event::terminal(to))).await;
    }
    async fn error(&mut self, err: BridgeError) {
        let _ = self.tx.send(Err(err)).await;
    }
}

fn spawn_workflow_producer(
    srv: &Arc<InboundServer>,
    routed: RoutedCall,
    wf_id: bridge_core::ids::WorkflowId,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
) {
    let srv = srv.clone();
    let task = routed.task;
    let parts = routed.parts.clone();
    tokio::spawn(async move {
        let (executor, graph) = match (&srv.executor, srv.workflows.get(&wf_id)) {
            (Some(e), Some(g)) => (e.clone(), g.clone()),
            _ => {
                let _ = tx.send(Ok(Event::terminal(TaskOutcome::Failed))).await;
                return;
            }
        };
        let input: String = parts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let token = tokio_util::sync::CancellationToken::new();
        srv.workflow_cancels
            .lock()
            .await
            .insert(task.clone(), token.clone());
        let stream = executor.run(graph, input, task.as_str().to_string(), token);
        let mut sink = SseSink { tx: tx.clone() };
        let terminal_seen = crate::workflow_sink::drain_workflow(stream, &mut sink).await;
        if !terminal_seen {
            let _ = tx.send(Ok(Event::terminal(TaskOutcome::Failed))).await;
        }
        srv.workflow_cancels.lock().await.remove(&task);
    });
}
```

- [ ] **Step 3: Verify streaming behavior is unchanged**

Run: `cargo test -p a2a-bridge --test workflow_producer` (and `cargo build -p bridge-a2a-inbound`)
Expected: all existing streaming workflow tests still pass (the refactor is behavior-preserving). NOTE: the `unary_workflow_send_returns_invalid_request_error` test still passes here — it is rewritten in Task 8.

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-a2a-inbound/src/workflow_sink.rs crates/bridge-a2a-inbound/src/lib.rs crates/bridge-a2a-inbound/src/server.rs
git commit -m "refactor(inbound): drain workflow over a WorkflowSink; streaming producer = SSE sink"
```

---

## Phase D — server detached path (ATOMIC: server change + test rewrite together)

### Task 6: `task_store` field + `.with_task_store` builder (additive)

**Files:**
- Modify: `crates/bridge-a2a-inbound/Cargo.toml`
- Modify: `crates/bridge-a2a-inbound/src/server.rs`

- [ ] **Step 1: Add deps (`uuid`); confirm NO `bridge-store`**

In `crates/bridge-a2a-inbound/Cargo.toml` `[dependencies]`, add:

```toml
uuid = { version = "1", features = ["v4"] }
```

Confirm `bridge-store` is **not** listed (DoD-1). `bridge-core` is already a dep.

- [ ] **Step 2: Add the field + builder + default**

In the `InboundServer` struct (server.rs:118), add a field:

```rust
    task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
```

In `InboundServer::new`, initialize it to the in-memory default. Find the struct literal returned by `new` and add:

```rust
            task_store: std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new()),
```

Add the builder next to `with_workflows` (server.rs ~208):

```rust
    #[must_use]
    pub fn with_task_store(
        mut self,
        task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    ) -> Self {
        self.task_store = task_store;
        self
    }
```

- [ ] **Step 3: Verify build + existing tests green**

Run: `cargo build -p bridge-a2a-inbound && cargo test -p a2a-bridge --test workflow_producer`
Expected: green (field is additive; default is MemoryTaskStore; nothing else changed yet).

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-a2a-inbound/Cargo.toml crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(inbound): InboundServer.task_store (MemoryTaskStore default) + with_task_store"
```

### Task 7: detached runner (`spawn_detached_workflow`) + `Finalizer` + `TaskStoreSink`

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/workflow_sink.rs` (add `TaskStoreSink`, `Finalizer`)
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (add `spawn_detached_workflow`, `new_detached_task_id`)

This task adds the runner but does NOT yet wire `message/send` to it (that's Task 8, atomic with the test rewrite). It is exercised by a direct unit test here.

- [ ] **Step 1: Add `TaskStoreSink` + `Finalizer` to `workflow_sink.rs`**

Append to `crates/bridge-a2a-inbound/src/workflow_sink.rs`:

```rust
use bridge_core::ids::TaskId;
use bridge_core::task_store::{TaskRecordStatus, TaskStore};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Detached sink: ignores intermediate node events (W3a has no history) and
/// captures the terminal mapping for the runner to persist.
pub(crate) struct TaskStoreSink {
    terminal: Option<(TaskRecordStatus, Option<String>, Option<String>)>,
}

impl TaskStoreSink {
    pub(crate) fn new() -> Self {
        Self { terminal: None }
    }
    /// The captured terminal mapping (status, result, error), or None if no
    /// terminal arrived.
    pub(crate) fn take(self) -> Option<(TaskRecordStatus, Option<String>, Option<String>)> {
        self.terminal
    }
}

#[async_trait::async_trait]
impl WorkflowSink for TaskStoreSink {
    async fn terminal(&mut self, outcome: WorkflowOutcome, output: String) {
        self.terminal = Some(match outcome {
            WorkflowOutcome::Completed => (TaskRecordStatus::Completed, Some(output), None),
            WorkflowOutcome::Failed => (TaskRecordStatus::Failed, None, Some(output)),
            WorkflowOutcome::Canceled => (TaskRecordStatus::Canceled, None, None),
        });
    }
}

/// Drop guard: if the runner exits without finalizing (early return, error, or
/// **panic**), write `Failed` and remove the cancel token, so a `Working` row is
/// never permanently orphaned within a serve lifetime.
pub(crate) struct Finalizer {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) task: TaskId,
    pub(crate) cancels: Arc<Mutex<std::collections::HashMap<TaskId, CancellationToken>>>,
    pub(crate) done: bool,
}

impl Drop for Finalizer {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let store = self.store.clone();
        let task = self.task.clone();
        let cancels = self.cancels.clone();
        // Drop can't be async; spawn the finalize write. A runtime exists (the
        // runner runs inside tokio::spawn); this also fires during panic unwind.
        tokio::spawn(async move {
            let _ = store
                .set_terminal(
                    &task,
                    TaskRecordStatus::Failed,
                    None,
                    Some("runner ended without terminal"),
                    now_ms(),
                )
                .await;
            cancels.lock().await.remove(&task);
        });
    }
}
```

- [ ] **Step 2: Write the failing test (runner persists Completed)**

Append to `crates/bridge-a2a-inbound/tests/workflow_producer.rs` (reuse the existing `build_workflow_server` harness + `FakeRegistry` that returns `SYNTH_FINAL` for the synth node):

```rust
#[tokio::test]
async fn detached_runner_persists_completed_result() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let task = TaskId::parse("detached-1").unwrap();
    // Pre-create the Working row (the message/send handler does this in prod).
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .await
        .unwrap();

    // Drive the detached runner directly and await its completion handle.
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    );
    handle.await.unwrap();

    let got = store.get(&task).await.unwrap().unwrap();
    assert_eq!(got.status, TaskRecordStatus::Completed);
    assert_eq!(got.result.as_deref(), Some("SYNTH_FINAL"));
}
```

Also add a harness helper near `build_workflow_server` in the test file:

```rust
fn build_workflow_server_with_task_store(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> Arc<InboundServer> {
    // identical to build_workflow_server() but with .with_task_store(store)
    let registry = Arc::new(FakeRegistry {
        replies: [
            ("codex".to_string(), "CODEX_REVIEW".to_string()),
            ("claude".to_string(), "CLAUDE_REVIEW".to_string()),
            ("synth".to_string(), "SYNTH_FINAL".to_string()),
        ]
        .into(),
    });
    let executor = Arc::new(WorkflowExecutor::new(registry.clone() as Arc<dyn AgentRegistry>));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(WorkflowRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_workflows(executor, map)
        .with_task_store(store),
    )
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p a2a-bridge --test workflow_producer detached_runner_persists_completed_result`
Expected: FAIL — `spawn_detached_workflow_for_test` not found.

- [ ] **Step 4: Implement `spawn_detached_workflow` + a test seam + `new_detached_task_id`**

In `crates/bridge-a2a-inbound/src/server.rs`, add:

```rust
/// Mint a fresh unique task id for a detached submit. (NOT `task_id_from_params`,
/// which returns the fixed `"task-1"` stub — unusable as a unique PK.)
fn new_detached_task_id() -> TaskId {
    TaskId::parse(uuid::Uuid::new_v4().to_string()).expect("uuid is non-empty")
}

/// Spawn the finalizer-guarded background runner for a detached workflow. Returns
/// the JoinHandle so callers/tests can await completion deterministically. The
/// caller MUST have already `create`d the Working row and registered a token in
/// `workflow_cancels` (so cancel can't race the spawn).
fn spawn_detached_workflow(
    srv: &Arc<InboundServer>,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let srv = srv.clone();
    tokio::spawn(async move {
        let mut fin = crate::workflow_sink::Finalizer {
            store: srv.task_store.clone(),
            task: task.clone(),
            cancels: srv.workflow_cancels.clone(),
            done: false,
        };
        let (executor, graph) = match (&srv.executor, srv.workflows.get(&wf_id)) {
            (Some(e), Some(g)) => (e.clone(), g.clone()),
            _ => {
                let _ = srv
                    .task_store
                    .set_terminal(
                        &task,
                        bridge_core::task_store::TaskRecordStatus::Failed,
                        None,
                        Some("no executor / unknown workflow"),
                        crate::workflow_sink::now_ms(),
                    )
                    .await;
                fin.done = true;
                srv.workflow_cancels.lock().await.remove(&task);
                return;
            }
        };
        let input = text_parts.join("\n");
        let stream = executor.run(graph, input, task.as_str().to_string(), token);
        let mut sink = crate::workflow_sink::TaskStoreSink::new();
        let terminal_seen = crate::workflow_sink::drain_workflow(stream, &mut sink).await;
        let now = crate::workflow_sink::now_ms();
        if terminal_seen {
            if let Some((status, result, error)) = sink.take() {
                let _ = srv
                    .task_store
                    .set_terminal(&task, status, result.as_deref(), error.as_deref(), now)
                    .await;
            }
        } else {
            let _ = srv
                .task_store
                .set_terminal(
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("workflow ended without terminal"),
                    now,
                )
                .await;
        }
        fin.done = true;
        srv.workflow_cancels.lock().await.remove(&task);
    })
}

/// Test-only seam: spawn the runner with a fresh token. (The prod path in
/// `message/send` does create()+register-token+spawn() itself; these tests don't
/// exercise cancel, so no `workflow_cancels` registration is needed here.)
#[doc(hidden)]
pub fn spawn_detached_workflow_for_test(
    srv: &Arc<InboundServer>,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
) -> tokio::task::JoinHandle<()> {
    let token = tokio_util::sync::CancellationToken::new();
    spawn_detached_workflow(srv, task, text_parts, wf_id, token)
}
```

(If `InboundServer`'s module is not already `pub`, ensure `pub fn spawn_detached_workflow_for_test` is reachable as `bridge_a2a_inbound::server::spawn_detached_workflow_for_test` — the `server` module is already `pub` per existing test imports.)

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p a2a-bridge --test workflow_producer detached_runner_persists_completed_result`
Expected: PASS (`Completed` + `SYNTH_FINAL`).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/workflow_sink.rs crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(inbound): detached workflow runner (finalizer-guarded, TaskStore sink) + test seam"
```

### Task 8: ATOMIC — `message/send` workflow → detached + rewrite the reject test

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (the `RouteTarget::Workflow` arm in `unary_message`)
- Modify: `crates/bridge-a2a-inbound/tests/workflow_producer.rs` (rewrite the reject test)

**This is the atomic ripple:** changing the unary workflow arm from reject→detached breaks `unary_workflow_send_returns_invalid_request_error`. The server change and the test rewrite land in the SAME commit.

- [ ] **Step 1: Rewrite the existing reject test to assert detached working return**

In `crates/bridge-a2a-inbound/tests/workflow_producer.rs`, replace `unary_workflow_send_returns_invalid_request_error` (lines ~529-564) with:

```rust
/// **unary detached submit**: a UNARY `skill="code-review"` now returns a
/// canonical `a2a::Task` with state `working` IMMEDIATELY and persists a Working
/// row — it no longer rejects with InvalidRequest.
#[tokio::test]
async fn unary_workflow_send_returns_working_task() {
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).expect("valid JSON");
    // No JSON-RPC error.
    assert!(body.get("error").is_none(), "must not be an error: {body}");
    // A working task with a real (non-"task-1") id.
    let task = &body["result"]["task"];
    let id = task["id"].as_str().expect("task id present");
    assert_ne!(id, "task-1", "detached submit must mint a unique id");
    let state = task["status"]["state"].as_str().or_else(|| task["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_WORKING"), "state must be working: {body}");
    // The Working row was persisted under that id.
    let rec = store
        .get(&bridge_core::ids::TaskId::parse(id).unwrap())
        .await
        .unwrap()
        .expect("row created");
    assert_eq!(rec.status, bridge_core::task_store::TaskRecordStatus::Working);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --test workflow_producer unary_workflow_send_returns_working_task`
Expected: FAIL — currently returns InvalidRequest.

- [ ] **Step 3: Change the unary workflow arm to detached**

In `crates/bridge-a2a-inbound/src/server.rs`, replace the `RouteTarget::Workflow(_)` reject arm in `unary_message` (lines ~1254-1256) with the detached path. The arm has the `wf_id` available (it is `RouteTarget::Workflow(id)`). Replace with:

```rust
        RouteTarget::Workflow(ref wf_id) => {
            // Detached submit: mint a unique id, persist Working, register the
            // cancel token, spawn the runner, and return a working Task NOW.
            let task = new_detached_task_id();
            let now = crate::workflow_sink::now_ms();
            let rec = bridge_core::task_store::TaskRecord {
                id: task.clone(),
                workflow: wf_id.as_str().to_string(),
                status: bridge_core::task_store::TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: now,
                updated_ms: now,
            };
            if srv.task_store.create(&rec).await.is_err() {
                return bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure);
            }
            let token = tokio_util::sync::CancellationToken::new();
            srv.workflow_cancels.lock().await.insert(task.clone(), token.clone());
            // text parts from the request
            let text_parts: Vec<String> =
                routed.parts.iter().map(|p| p.text.clone()).collect();
            let _ = spawn_detached_workflow(&srv, task.clone(), text_parts, wf_id.clone(), token);
            let working = a2a::Task {
                id: task.as_str().to_owned(),
                context_id: task.as_str().to_owned(),
                status: a2a::TaskStatus {
                    state: a2a::TaskState::Working,
                    message: None,
                    timestamp: None,
                },
                artifacts: None,
                history: None,
                metadata: None,
            };
            return jsonrpc_ok(id, json!({ "task": working }));
        }
```

NOTE: confirm the surrounding `match` binds `routed` and `srv` and `id` in scope (it does — the unary arm runs after routing). If `routed.task` is referenced elsewhere in the arm, it is replaced by the minted `task` here. Confirm `a2a::TaskState::Working` exists (the `a2a` crate's TaskState includes Working/Submitted/Completed/Failed/Canceled); if the exact variant name differs, use the variant that serializes to `TASK_STATE_WORKING`.

- [ ] **Step 4: Run to verify pass (and the detached runner test, and streaming)**

Run: `cargo test -p a2a-bridge --test workflow_producer`
Expected: `unary_workflow_send_returns_working_task` passes; `detached_runner_persists_completed_result` passes; all streaming tests pass.

- [ ] **Step 5: Commit (atomic)**

```bash
git add crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(inbound): message/send workflow -> detached submit (atomic: rewrite the reject test)"
```

### Task 9: `tasks/get` — canonical `a2a::Task` from the store

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`get_task`)
- Test: `crates/bridge-a2a-inbound/tests/workflow_producer.rs`

- [ ] **Step 1: Write the failing test**

Append to `tests/workflow_producer.rs`:

```rust
#[tokio::test]
async fn tasks_get_returns_completed_with_artifact() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("g1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .await
        .unwrap();
    store
        .set_terminal(&id, TaskRecordStatus::Completed, Some("THE_RESULT"), None, 2)
        .await
        .unwrap();

    let resp = srv
        .router()
        .oneshot(post_request(methods::GET_TASK, json!({ "taskId": "g1" })))
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    let task = &body["result"]["task"];
    let state = task["status"]["state"].as_str().or_else(|| task["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_COMPLETED"), "{body}");
    // result surfaces as an artifact text
    assert!(
        body.to_string().contains("THE_RESULT"),
        "completed task must carry the result: {body}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --test workflow_producer tasks_get_returns_completed_with_artifact`
Expected: FAIL — current `get_task` returns only `{id, state}` from the session heuristic, no store consult, no artifact.

- [ ] **Step 3: Make `get_task` consult the store and build a canonical Task**

Replace `get_task` (server.rs:1531-1551) with:

```rust
async fn get_task(
    srv: Arc<InboundServer>,
    _headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let task = match task_id_from_params(&params) {
        Ok(t) => t,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    // Durable task row first (detached workflows).
    if let Ok(Some(rec)) = srv.task_store.get(&task).await {
        let (state, artifacts) = task_record_to_a2a(&rec);
        let t = a2a::Task {
            id: rec.id.as_str().to_owned(),
            context_id: rec.id.as_str().to_owned(),
            status: a2a::TaskStatus { state, message: rec.error.clone(), timestamp: None },
            artifacts,
            history: None,
            metadata: None,
        };
        return jsonrpc_ok(id, json!({ "task": t }));
    }
    // Fallback: session-mapping heuristic (non-workflow tasks; unchanged).
    let known = matches!(srv.store.session_for(&task).await, Ok(Some(_)));
    let state = if known { "TASK_STATE_WORKING" } else { "TASK_STATE_SUBMITTED" };
    jsonrpc_ok(id, json!({ "task": { "id": task.as_str(), "state": state } }))
}

/// Map a durable `TaskRecord` to (A2A state, artifacts). `Interrupted` collapses
/// to `failed` at the wire (reason in `status.message`).
fn task_record_to_a2a(
    rec: &bridge_core::task_store::TaskRecord,
) -> (a2a::TaskState, Option<Vec<a2a::Artifact>>) {
    use bridge_core::task_store::TaskRecordStatus;
    let state = match rec.status {
        TaskRecordStatus::Working => a2a::TaskState::Working,
        TaskRecordStatus::Completed => a2a::TaskState::Completed,
        TaskRecordStatus::Failed => a2a::TaskState::Failed,
        TaskRecordStatus::Canceled => a2a::TaskState::Canceled,
        TaskRecordStatus::Interrupted => a2a::TaskState::Failed,
    };
    let artifacts = rec.result.as_ref().map(|r| {
        vec![a2a::Artifact {
            artifact_id: a2a::new_artifact_id(),
            name: None,
            description: None,
            parts: vec![a2a::Part::text(r.clone())],
            metadata: None,
            extensions: None,
        }]
    });
    (state, artifacts)
}
```

NOTE: the `a2a::TaskStatus.message` field is typed in the SDK — if it is not `Option<String>` but `Option<a2a::Message>`, set `message: None` and instead append the error to the artifact. Confirm the field type against `a2a::TaskStatus` and adjust (the failing-task error must surface SOMEWHERE the CLI can print). The `a2a::Artifact`/`Part::text` shape is copied from the existing builder (server.rs:1366).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --test workflow_producer tasks_get_returns_completed_with_artifact`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(inbound): tasks/get reads TaskStore, returns canonical a2a::Task + artifact"
```

### Task 10: `tasks/cancel` — TaskStore-aware

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`cancel_task`)
- Test: `crates/bridge-a2a-inbound/tests/workflow_producer.rs`

- [ ] **Step 1: Write the failing test (cancel an already-terminal detached task returns its true state, default backend untouched)**

Append to `tests/workflow_producer.rs`:

```rust
#[tokio::test]
async fn cancel_terminal_detached_returns_true_state_not_recancel() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("c1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .await
        .unwrap();
    store
        .set_terminal(&id, TaskRecordStatus::Completed, Some("DONE"), None, 2)
        .await
        .unwrap();

    let resp = srv
        .router()
        .oneshot(post_request(methods::CANCEL_TASK, json!({ "taskId": "c1" })))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    // For an already-completed task, cancel reports the TRUE state (completed),
    // not CANCELED, and does not touch the default backend.
    let state = body["result"]["task"]["state"]
        .as_str()
        .or_else(|| body["result"]["task"]["status"]["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_COMPLETED"), "{body}");
    // store row is still Completed (not flipped to Canceled)
    assert_eq!(store.get(&id).await.unwrap().unwrap().status, TaskRecordStatus::Completed);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --test workflow_producer cancel_terminal_detached_returns_true_state_not_recancel`
Expected: FAIL — current `cancel_task` ignores the store and falls through to the local-backend cancel, returning `TASK_STATE_CANCELED`.

- [ ] **Step 3: Add a TaskStore-aware branch at the top of `cancel_task`**

In `cancel_task` (server.rs:1407), immediately AFTER the auth check and `task` extraction and the `request_cancel` latch, BEFORE the `workflow_cancels` block, insert:

```rust
    // Durable detached task? Consult the store first (it owns the truth).
    if let Ok(Some(rec)) = srv.task_store.get(&task).await {
        use bridge_core::task_store::TaskRecordStatus;
        if rec.status.is_terminal() {
            // Already finished — return its true state; do NOT re-cancel / touch a backend.
            let wire = match rec.status {
                TaskRecordStatus::Completed => "TASK_STATE_COMPLETED",
                TaskRecordStatus::Canceled => "TASK_STATE_CANCELED",
                _ => "TASK_STATE_FAILED", // Failed | Interrupted
            };
            return jsonrpc_ok(id, json!({ "task": { "id": task.as_str(), "state": wire } }));
        }
        // Working: fire the token if present (runner writes Canceled); else write Canceled directly.
        if let Some(tok) = srv.workflow_cancels.lock().await.get(&task) {
            tok.cancel();
        } else {
            let _ = srv
                .task_store
                .set_terminal(&task, TaskRecordStatus::Canceled, None, None, crate::workflow_sink::now_ms())
                .await;
        }
        return jsonrpc_ok(
            id,
            json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
        );
    }
```

(The existing `workflow_cancels`/fanout/delegate/local branches remain unchanged below for non-detached tasks.)

- [ ] **Step 4: Run to verify pass + existing cancel tests green**

Run: `cargo test -p a2a-bridge --test workflow_producer` and `cargo test -p a2a-bridge`
Expected: the new test passes; existing `cancel_task_*` tests (fanout/local/streaming-workflow cancel) still pass (they don't pre-create TaskStore rows, so they skip the new branch).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(inbound): TaskStore-aware tasks/cancel (terminal->true state; working->token/Canceled)"
```

### Task 11: `tasks/list` method + dispatch + deterministic gated-fake submit test

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (dispatch + `list_tasks` handler + `LIST_TASKS` const)
- Test: `crates/bridge-a2a-inbound/tests/workflow_producer.rs`

- [ ] **Step 1: Write the failing test (list returns recent tasks)**

Append to `tests/workflow_producer.rs`:

```rust
#[tokio::test]
async fn tasks_list_returns_recent_newest_first() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    for (id, ms) in [("l-old", 1i64), ("l-new", 5i64)] {
        store
            .create(&TaskRecord {
                id: TaskId::parse(id).unwrap(),
                workflow: "code-review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: ms,
                updated_ms: ms,
            })
            .await
            .unwrap();
    }
    let resp = srv
        .router()
        .oneshot(post_request("tasks/list", json!({ "limit": 10 })))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let tasks = body["result"]["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks[0]["id"].as_str(), Some("l-new"), "newest-first: {body}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --test workflow_producer tasks_list_returns_recent_newest_first`
Expected: FAIL — `tasks/list` → method not found.

- [ ] **Step 3: Add the `LIST_TASKS` const, dispatch arm, and handler**

In `server.rs`, near the top (after the `use a2a::{methods, ...}`), add:

```rust
/// Bridge-defined list method (the a2a-lf 0.3.0 SDK has no list-tasks method).
const LIST_TASKS: &str = "tasks/list";
```

In the dispatch match (server.rs:466-475), add an arm before the `""` arm:

```rust
    m if m == LIST_TASKS => list_tasks(srv, headers, id, params).await,
```

Add the handler:

```rust
async fn list_tasks(
    srv: Arc<InboundServer>,
    _headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    match srv.task_store.list(limit).await {
        Ok(recs) => {
            let tasks: Vec<Value> = recs
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id.as_str(),
                        "workflow": r.workflow,
                        "state": r.status.as_str(),
                        "updated_ms": r.updated_ms,
                    })
                })
                .collect();
            jsonrpc_ok(id, json!({ "tasks": tasks }))
        }
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --test workflow_producer tasks_list_returns_recent_newest_first`
Expected: PASS.

- [ ] **Step 5: Add the deterministic "returns before terminal" gated test (DoD-6)**

The default `FakeRegistry` completes eagerly, so to assert "send returns while still Working" deterministically we use a **gated** registry whose backend blocks until released. Append a gated fake + test to `tests/workflow_producer.rs`:

```rust
// A registry whose node backends block on a Notify until released — lets a test
// assert the submit returns Working BEFORE the workflow can finish.
// (Implement a GatedRegistry mirroring FakeRegistry but each prompt awaits a
// shared tokio::sync::Notify before yielding its reply. See FakeRegistry above
// for the trait surface to mirror.)
#[tokio::test]
async fn submit_returns_working_before_completion_then_completes() {
    use bridge_core::task_store::{MemoryTaskStore, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let srv = build_gated_workflow_server(store.clone(), gate.clone());

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": { "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" } } }),
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(),
    )
    .unwrap();
    let id = body["result"]["task"]["id"].as_str().unwrap().to_string();
    let tid = bridge_core::ids::TaskId::parse(&id).unwrap();
    // Still Working while gated.
    assert_eq!(store.get(&tid).await.unwrap().unwrap().status, TaskRecordStatus::Working);
    // Release the gate; await completion deterministically.
    gate.notify_waiters();
    // Poll the store until terminal (bounded).
    for _ in 0..200 {
        if store.get(&tid).await.unwrap().unwrap().status.is_terminal() { break; }
        tokio::task::yield_now().await;
    }
    assert_eq!(store.get(&tid).await.unwrap().unwrap().status, TaskRecordStatus::Completed);
}
```

Implement `build_gated_workflow_server(store, gate)` mirroring `build_workflow_server_with_task_store` but with a `GatedRegistry` whose backends `gate.notified().await` once before replying (model on the existing `FakeRegistry`/fake backend in this test file; reuse `review_graph()`).

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p a2a-bridge --test workflow_producer submit_returns_working_before_completion_then_completes`
Expected: PASS (Working while gated → Completed after release).

- [ ] **Step 7: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "feat(inbound): tasks/list method + deterministic gated submit-returns-Working test"
```

### Task 12: runner-panic finalizer test

**Files:**
- Test: `crates/bridge-a2a-inbound/tests/workflow_producer.rs`

- [ ] **Step 1: Write the failing/should-pass test**

A `GatedRegistry` variant whose node backend **panics** lets us assert the finalizer writes `Failed`. Append:

```rust
#[tokio::test]
async fn runner_panic_finalizes_failed_no_orphan() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    // A registry whose first node backend panics mid-prompt.
    let srv = build_panicking_workflow_server(store.clone());
    let task = TaskId::parse("panic-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    );
    let _ = handle.await; // join (may be Err on panic)
    // Give the finalizer's spawned write a moment.
    for _ in 0..200 {
        if store.get(&task).await.unwrap().unwrap().status.is_terminal() { break; }
        tokio::task::yield_now().await;
    }
    let rec = store.get(&task).await.unwrap().unwrap();
    assert!(rec.status.is_terminal(), "panic must finalize, not orphan Working");
}
```

Implement `build_panicking_workflow_server(store)` with a registry whose backend `panic!`s during the node turn. NOTE: if the executor catches node failures into a `Terminal{Failed}` rather than unwinding the runner, this still finalizes to a terminal — assert `is_terminal()` (not specifically the panic path). The point is no orphan `Working`.

- [ ] **Step 2: Run**

Run: `cargo test -p a2a-bridge --test workflow_producer runner_panic_finalizes_failed_no_orphan`
Expected: PASS (terminal, not orphaned).

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-a2a-inbound/tests/workflow_producer.rs
git commit -m "test(inbound): runner panic/abnormal exit finalizes terminal (no orphan Working)"
```

---

## Phase E — bin: config, serve wiring, CLI client

### Task 13: `[store] path` config

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test**

Append to `bin/a2a-bridge/src/config.rs` tests (or create a `#[cfg(test)] mod` if none):

```rust
#[cfg(test)]
mod store_cfg_tests {
    use super::*;

    #[test]
    fn store_path_parses_when_present() {
        let toml = r#"
default = "codex"
[server]
addr = "127.0.0.1:8080"
[store]
path = "/tmp/x.db"
"#;
        let cfg = RegistryConfig::parse(toml).unwrap();
        assert_eq!(cfg.store.unwrap().path, "/tmp/x.db");
    }

    #[test]
    fn store_absent_is_none() {
        let toml = "default = \"codex\"\n[server]\naddr = \"127.0.0.1:8080\"\n";
        let cfg = RegistryConfig::parse(toml).unwrap();
        assert!(cfg.store.is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib store_cfg_tests`
Expected: FAIL — no `store` field.

- [ ] **Step 3: Add `StoreConfig` + field**

In `config.rs`, add:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct StoreConfig {
    pub path: String,
}
```

Add to `RegistryConfig`:

```rust
    #[serde(default)]
    pub store: Option<StoreConfig>,
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p a2a-bridge --lib store_cfg_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): optional [store] path"
```

### Task 14: serve wires the store (file or memory) + boot sweep

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Wire the store at serve startup**

In `main.rs`, where `serve` constructs the store + `InboundServer` (around line 432), build the task store from config:

```rust
    // W3a: durable task store. File-backed when [store] path is set (acquires the
    // single-serve lock + runs the boot sweep); else in-memory (ephemeral).
    let task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore> =
        match cfg.store.as_ref().map(|s| s.path.clone()) {
            Some(path) => {
                let s = std::sync::Arc::new(
                    bridge_store::sqlite::SqliteStore::open(std::path::Path::new(&path))
                        .map_err(|e| format!("serve: cannot open task store {path:?}: {e:?}"))?,
                );
                // Sweep stale Working rows from a previous serve that died mid-run.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let _ = s.sweep_interrupted(now).await;
                s as std::sync::Arc<dyn bridge_core::task_store::TaskStore>
            }
            None => std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new()),
        };
```

Then chain `.with_task_store(task_store)` onto the `InboundServer` builder:

```rust
        .with_workflows(executor, wf_map.clone())
        .with_task_store(task_store),
```

NOTE: this requires `bridge-store` and `bridge-core` as deps of `bin/a2a-bridge` (both already present — `bin` uses `SqliteStore` for the session store today). If the session store and task store should share ONE `SqliteStore` (one DB file, one lock), construct the `SqliteStore` once and share it as both `Arc<dyn SessionStore>` and `Arc<dyn TaskStore>` — but a single lock means the session store must ALSO move to `open(path)` only when a store path is set. For W3a keep them independent unless a path is set for both; document that `[store] path` powers the task store specifically. (Simplest correct MVP: the task store uses `[store] path`; the session store keeps its existing construction. If they share a path, the lock would conflict — so if both need the same file, construct ONE SqliteStore and pass clones. Decide during impl based on how the session store is currently built; default to a SEPARATE task DB path to avoid the self-lock.)

- [ ] **Step 2: Verify build**

Run: `cargo build -p a2a-bridge`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(bin): serve wires file/memory task store + boot sweep"
```

### Task 15: CLI `submit` + `task get|list|cancel` (reqwest A2A client)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`
- Modify: `bin/a2a-bridge/Cargo.toml` (ensure `reqwest` with json; likely present via bridge-api)

- [ ] **Step 1: Add the subcommand dispatch + parsing**

In `main()` (main.rs:271), after the `run-workflow` dispatch, add:

```rust
    if raw_args.get(1).map(|s| s.as_str()) == Some("submit") {
        return submit_cmd(&raw_args[2..]).await;
    }
    if raw_args.get(1).map(|s| s.as_str()) == Some("task") {
        return task_cmd(&raw_args[2..]).await;
    }
```

- [ ] **Step 2: Implement the client commands**

Add to `main.rs` (a small JSON-RPC-over-HTTP client; `--url` default `http://127.0.0.1:8080`):

```rust
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

async fn rpc_call(url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value, BoxError> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp = reqwest::Client::new()
        .post(url)
        .header("X-A2A-Version", "1.0") // SVC_PARAM_VERSION header name; confirm constant
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("bad response: {e}"))?;
    Ok(v)
}

async fn submit_cmd(args: &[String]) -> Result<(), BoxError> {
    let skill = args.first().cloned().ok_or("submit: missing <skill>")?;
    let input_path = flag(args, "--input").ok_or("submit: --input <file> required")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let text = std::fs::read_to_string(input_path)?;
    let params = serde_json::json!({ "message": { "text": text, "metadata": { "a2a-bridge.skill": skill } } });
    let v = rpc_call(url, "message/send", params).await?;
    if let Some(err) = v.get("error") {
        return Err(format!("submit failed: {err}").into());
    }
    let id = v["result"]["task"]["id"].as_str().ok_or("no task id in response")?;
    println!("{id}");
    Ok(())
}

async fn task_cmd(args: &[String]) -> Result<(), BoxError> {
    let sub = args.first().map(|s| s.as_str()).ok_or("task: missing subcommand (get|list|cancel)")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    match sub {
        "get" => {
            let id = args.get(1).cloned().ok_or("task get: missing <id>")?;
            let v = rpc_call(url, "tasks/get", serde_json::json!({ "taskId": id })).await?;
            println!("{}", serde_json::to_string_pretty(&v["result"]["task"])?);
        }
        "list" => {
            let limit: u64 = flag(args, "--limit").and_then(|s| s.parse().ok()).unwrap_or(50);
            let v = rpc_call(url, "tasks/list", serde_json::json!({ "limit": limit })).await?;
            for t in v["result"]["tasks"].as_array().cloned().unwrap_or_default() {
                println!("{}\t{}\t{}", t["id"].as_str().unwrap_or("?"), t["state"].as_str().unwrap_or("?"), t["workflow"].as_str().unwrap_or("?"));
            }
        }
        "cancel" => {
            let id = args.get(1).cloned().ok_or("task cancel: missing <id>")?;
            let v = rpc_call(url, "tasks/cancel", serde_json::json!({ "taskId": id })).await?;
            println!("{}", serde_json::to_string_pretty(&v["result"]["task"])?);
        }
        other => return Err(format!("task: unknown subcommand {other:?}").into()),
    }
    Ok(())
}
```

NOTE: confirm the version header NAME/value the server requires (`SVC_PARAM_VERSION` from the `a2a` crate — see `post_request` in tests which sets `SVC_PARAM_VERSION` to `"1.0"`). Use the same header key. Ensure `reqwest` is a dep of `bin/a2a-bridge` (add `reqwest = { version = "0.12", features = ["json"] }` if missing).

- [ ] **Step 3: Verify build + a unit test for `flag`/arg parsing**

Add a small test for `flag` parsing; Run: `cargo build -p a2a-bridge`.
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/Cargo.toml
git commit -m "feat(bin): submit + task get/list/cancel CLI (reqwest A2A client; serve-down UX)"
```

---

## Phase F — verification, live gate, ADR

### Task 16: full sweep + clippy/fmt + coverage

**Files:** none (verification)

- [ ] **Step 1: Workspace green**

Run: `cargo fmt --all && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 2: Coverage floors**

Run:
```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov --package bridge-core --fail-under-lines 90
cargo llvm-cov --package bridge-workflow --fail-under-lines 90
```
Expected: all pass. If `bridge-core` dips below 90 from the new `task_store.rs`, add targeted unit tests (the Task 1/2 tests should keep it above).

- [ ] **Step 3: Commit any coverage-driven test additions**

```bash
git add -A && git commit -m "test(w3a): coverage top-ups for TaskStore"
```

### Task 17: gated live DoD (real agents) — manual, recorded

**Files:** none (a recorded manual run)

- [ ] **Step 1: Build + start serve with a durable store**

Create `/tmp/w3a-serve.toml` = the example workflows config plus:
```toml
[store]
path = "/tmp/w3a-tasks.db"
```
Run (background): `./target/debug/a2a-bridge serve --config /tmp/w3a-serve.toml`

- [ ] **Step 2: Submit a real code-review detached, poll, retrieve**

```bash
./target/debug/a2a-bridge submit code-review --input /tmp/smoke-review-input.md
# prints <task-id>
./target/debug/a2a-bridge task get <task-id>   # repeat until state=completed
```
Expected: state transitions working → completed; `task get` prints the synth artifact. Record the outcome in the ADR.

- [ ] **Step 3: Restart durability check**

Kill + restart `serve` with the same `[store] path`; `task get <task-id>` still returns the completed result (durable across restart). Submit a new one, kill serve mid-run, restart, `task get` → `failed` (interrupted).

### Task 18: ADR-0010

**Files:**
- Create: `docs/adr/0010-durable-detached-submit.md`

- [ ] **Step 1: Write ADR-0010**

Record: the decision (durable detached submit, result-durable slice); the components (TaskStore port + MemoryTaskStore in core; SqliteStore file-backed + single-serve lock; detached runner over a shared WorkflowSink; canonical a2a::Task; bridge-defined tasks/list); the dual-review corrections (unique uuid id-gen since `a2a::new_task_id` doesn't exist; non-clobbering create; MemoryTaskStore crate-boundary; TaskStore-aware cancel; finalizer guard; no SDK ListTasks so a bridge `tasks/list`); the live-gate result from Task 17; and the W3b follow-ons (history needs an additive `NodeFinished{output}`; resume). End the commit with the controller trailer.

- [ ] **Step 2: Commit**

```bash
git add docs/adr/0010-durable-detached-submit.md
git commit -m "docs(adr): 0010 — durable detached submit (W3a)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Coverage of spec §10 DoD → tasks

| DoD | Task |
|-----|------|
| 1 (TaskStore + MemoryTaskStore in core; no bridge-store dep in inbound) | 1, 2, 6 (Step 1 confirms no dep) |
| 2 (open(path) + idempotent table + lock; second open fails) | 3, 4 |
| 3 (TaskStore impl; non-clobbering create; unit tests) | 4 |
| 4 (file-backed reopen durability) | 4 |
| 5 (with_task_store; new untouched; build green) | 6 |
| 6 (send returns working without blocking — deterministic) | 8, 11 (gated test) |
| 7 (terminal records Completed/Failed/Canceled; panic→Failed) | 7, 12 |
| 8 (tasks/get canonical a2a::Task + artifact; non-workflow unchanged) | 9 |
| 9 (cancel TaskStore-aware) | 10 |
| 10 (boot sweep → Interrupted → get reports failed) | 4 (store), 14 (boot), 9 (wire) |
| 11 (CLI submit/get/list/cancel; serve-down UX) | 15 |
| 12 (store path: durable across restart; unset → memory) | 13, 14, 17 |
| 13 (rewrite the InvalidRequest test; nothing left red) | 8 |
| 14 (gated live) | 17 |
| 15 (fmt/clippy/coverage) | 16 |

---

## Notes for the implementer
- **Atomicity:** Task 8 changes server behavior AND rewrites the test in one commit — never split (leaves the tree red otherwise).
- **Firewall:** `~/code/a2a-local-bridge` is black-box only; design from the bridge's ports + A2A semantics. `~/code/agent-knowledge` is readable.
- **`a2a::TaskState::Working` / `a2a::TaskStatus.message` types:** confirm the exact SDK variant/field types when you reach Tasks 8-9; adjust the construction to whatever serializes to `TASK_STATE_WORKING` and lets the error text reach the wire. These are the two spots most likely to need a small shape tweak.
- **Session-store vs task-store DB:** keep the task store on its own `[store] path` for W3a to avoid a single-serve self-lock conflict with the existing session store construction (Task 14 note).
- Controller doc commits (this plan, ADR-0010) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; subagent task commits do NOT.
