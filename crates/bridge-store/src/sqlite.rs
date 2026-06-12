// sqlite.rs — SQLite-backed SessionStore (spec §7, Task 9).

use bridge_core::{
    domain::{PeerTaskId, PendingKind, PendingRequest},
    error::BridgeError,
    ids::{NodeId, SessionId, TaskId},
    ports::SessionStore,
};
use rusqlite::OptionalExtension;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// SQLite-backed [`SessionStore`] that persists `task_id ↔ session_id` mappings
/// and a per-task pending-request that is cleared atomically on first read.
pub struct SqliteStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    // Held for the store's lifetime: an exclusive advisory lock on `<path>.lock`
    // so only one `serve` owns a DB file (makes the boot sweep safe). `None` for
    // in-memory stores.
    _lock: Option<std::fs::File>,
}

impl SqliteStore {
    /// Open an in-memory database (suitable for tests).
    pub fn open_in_memory() -> Result<Self, BridgeError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|_| BridgeError::StoreFailure)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| BridgeError::StoreFailure)?;
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
        // The store path may name a not-yet-created subdir (e.g. a `[store] path` of
        // `<config-dir>/.a2a-bridge/tasks.sqlite`); neither the lock-file open nor sqlite will
        // create it, so `mkdir -p` the parent first or `open` fails with StoreFailure.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|_| BridgeError::StoreFailure)?;
            }
        }
        let lock_path = {
            let mut p = path.as_os_str().to_os_string();
            p.push(".lock");
            std::path::PathBuf::from(p)
        };
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|_| BridgeError::StoreFailure)?;
        lock.try_lock_exclusive()
            .map_err(|_| BridgeError::StoreFailure)?;
        let conn = rusqlite::Connection::open(path).map_err(|_| BridgeError::StoreFailure)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            _lock: Some(lock),
        };
        store.create_schema()?;
        Ok(store)
    }

    /// Test helper: check if PRAGMA foreign_keys is enabled on this connection.
    #[cfg(test)]
    fn foreign_keys_on(&self) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let flag: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(flag != 0)
    }

    /// Test helper: delete a task row directly (used to verify ON DELETE CASCADE).
    #[cfg(test)]
    fn delete_for_test(&self, task: &TaskId) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM tasks WHERE id=?1",
            rusqlite::params![task.as_str()],
        )?;
        Ok(())
    }

    fn create_schema(&self) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
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
            CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_ms);
            CREATE TABLE IF NOT EXISTS task_node_checkpoints (
                task_id   TEXT NOT NULL,
                node_id   TEXT NOT NULL,
                output    TEXT NOT NULL,
                ok        INTEGER NOT NULL,
                ts        INTEGER NOT NULL,
                PRIMARY KEY (task_id, node_id),
                FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS task_node_starts (
                task_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                seq     INTEGER NOT NULL,
                ts      INTEGER NOT NULL,
                PRIMARY KEY (task_id, node_id),
                FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );",
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        migrate_tasks_columns(&conn).map_err(|_| BridgeError::StoreFailure)
    }
}

/// Idempotently add the W3b additive columns to the `tasks` table.
/// Reads existing columns via `PRAGMA table_info`, then issues `ALTER TABLE ADD COLUMN`
/// only for columns that are missing. Safe to call on both fresh and old databases.
fn migrate_tasks_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    // Collect existing column names for `tasks`.
    let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
    let existing: HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;

    let additive = [
        ("input", "TEXT NOT NULL DEFAULT ''"),
        ("workflow_spec_json", "TEXT"),
        ("resume_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("last_resume_ms", "INTEGER"),
        ("session_cwd", "TEXT"),
        ("last_event_seq", "INTEGER NOT NULL DEFAULT 0"),
        ("terminal_seq", "INTEGER"),
    ];
    for (col, def) in additive {
        if !existing.contains(col) {
            conn.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {col} {def};"))?;
        }
    }

    // Collect existing column names for `task_node_checkpoints`.
    let mut stmt2 = conn.prepare("PRAGMA table_info(task_node_checkpoints)")?;
    let cp_existing: HashSet<String> = stmt2
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !cp_existing.contains("seq") {
        conn.execute_batch("ALTER TABLE task_node_checkpoints ADD COLUMN seq INTEGER;")?;
    }

    Ok(())
}

#[async_trait::async_trait]
impl SessionStore for SqliteStore {
    async fn put(&self, task: &TaskId, session: &SessionId) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(task_id, session_id) VALUES(?1, ?2)
             ON CONFLICT(task_id) DO UPDATE SET session_id = excluded.session_id",
            rusqlite::params![task.as_str(), session.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn session_for(&self, task: &TaskId) -> Result<Option<SessionId>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT session_id FROM sessions WHERE task_id = ?1")
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(None),
            Some(row) => {
                let sid: Option<String> = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                match sid {
                    None => Ok(None),
                    Some(s) => Ok(Some(
                        SessionId::parse(s).map_err(|_| BridgeError::StoreFailure)?,
                    )),
                }
            }
        }
    }

    async fn put_pending(&self, task: &TaskId, req: &PendingRequest) -> Result<(), BridgeError> {
        let kind_str = match req.kind {
            PendingKind::Permission => "permission",
            PendingKind::Auth => "auth",
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(task_id, pending_request_id, pending_kind)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(task_id) DO UPDATE SET
               pending_request_id = excluded.pending_request_id,
               pending_kind = excluded.pending_kind",
            rusqlite::params![task.as_str(), req.request_id.as_str(), kind_str],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn take_pending(&self, task: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT pending_request_id, pending_kind
                 FROM sessions WHERE task_id = ?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        let row = rows.next().map_err(|_| BridgeError::StoreFailure)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let request_id: Option<String> = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
        let kind_str: Option<String> = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
        match (request_id, kind_str) {
            (Some(rid), Some(k)) => {
                let kind = match k.as_str() {
                    "auth" => PendingKind::Auth,
                    _ => PendingKind::Permission,
                };
                // Clear the pending columns atomically.
                conn.execute(
                    "UPDATE sessions SET pending_request_id = NULL, pending_kind = NULL
                     WHERE task_id = ?1",
                    rusqlite::params![task.as_str()],
                )
                .map_err(|_| BridgeError::StoreFailure)?;
                Ok(Some(PendingRequest {
                    request_id: rid,
                    kind,
                }))
            }
            _ => Ok(None),
        }
    }

    async fn set_peer_task(&self, task: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(task_id, peer_task_id) VALUES(?1, ?2)
             ON CONFLICT(task_id) DO UPDATE SET peer_task_id = excluded.peer_task_id",
            rusqlite::params![task.as_str(), peer.0.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn peer_task_for(&self, task: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT peer_task_id FROM sessions WHERE task_id = ?1")
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(None),
            Some(row) => {
                let pid: Option<String> = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                Ok(pid.map(PeerTaskId))
            }
        }
    }

    async fn request_cancel(&self, task: &TaskId) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(task_id, cancel_requested) VALUES(?1, 1)
             ON CONFLICT(task_id) DO UPDATE SET cancel_requested = 1",
            rusqlite::params![task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn cancel_requested(&self, task: &TaskId) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT cancel_requested FROM sessions WHERE task_id = ?1")
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(false),
            Some(row) => {
                let flag: i64 = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                Ok(flag != 0)
            }
        }
    }

    async fn set_fanout(&self, task: &TaskId) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions(task_id, fanout) VALUES(?1, 1)
             ON CONFLICT(task_id) DO UPDATE SET fanout = 1",
            rusqlite::params![task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn is_fanout(&self, task: &TaskId) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT fanout FROM sessions WHERE task_id = ?1")
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(false),
            Some(row) => {
                let flag: i64 = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                Ok(flag != 0)
            }
        }
    }
}

#[async_trait::async_trait]
impl bridge_core::task_store::TaskStore for SqliteStore {
    async fn create(&self, rec: &bridge_core::task_store::TaskRecord) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms,
                               input, workflow_spec_json, resume_attempts, session_cwd)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                rec.status.as_str(),
                rec.result,
                rec.error,
                rec.created_ms,
                rec.updated_ms,
                rec.input,
                rec.workflow_spec_json,
                rec.resume_attempts as i64,
                rec.session_cwd
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
            return Err(BridgeError::StoreFailure);
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
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        input, workflow_spec_json, resume_attempts, session_cwd
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
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        input, workflow_spec_json, resume_attempts, session_cwd
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
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE tasks SET status='canceled', updated_ms=?1 WHERE id=?2 AND status='working'",
                rusqlite::params![updated_ms, id.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n > 0)
    }
    async fn put_node_checkpoint(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO task_node_checkpoints(task_id, node_id, output, ok, ts)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![task.as_str(), node.as_str(), output, ok as i64, ts],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool)>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT node_id, output, ok FROM task_node_checkpoints WHERE task_id=?1")
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let node_s: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
            let output: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
            let ok_i: i64 = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
            let node = NodeId::parse(node_s).map_err(|_| BridgeError::StoreFailure)?;
            out.push((node, output, ok_i != 0));
        }
        Ok(out)
    }

    async fn claim_resume_attempt(
        &self,
        task: &TaskId,
        cap: u32,
        now_ms: i64,
    ) -> Result<bridge_core::task_store::ResumeClaim, BridgeError> {
        use bridge_core::task_store::ResumeClaim;
        let conn = self.conn.lock().unwrap();
        // unchecked_transaction takes &self — safe to use through the MutexGuard.
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| BridgeError::StoreFailure)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT resume_attempts FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        let current = current.ok_or(BridgeError::StoreFailure)?;
        if current >= cap as i64 {
            tx.commit().map_err(|_| BridgeError::StoreFailure)?;
            return Ok(ResumeClaim::Exhausted);
        }
        let new_val = current + 1;
        tx.execute(
            "UPDATE tasks SET resume_attempts=?1, last_resume_ms=?2 WHERE id=?3",
            rusqlite::params![new_val, now_ms, task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(ResumeClaim::Resumable {
            attempt: new_val as u32,
        })
    }

    async fn working_tasks(&self) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        input, workflow_spec_json, resume_attempts, session_cwd
                 FROM tasks WHERE status='working'",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt.query([]).map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            out.push(row_to_task(row)?);
        }
        Ok(out)
    }

    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| BridgeError::StoreFailure)?;
        // Allocate seq by bumping last_event_seq.
        let n = tx
            .execute(
                "UPDATE tasks SET last_event_seq = last_event_seq + 1 WHERE id=?1",
                rusqlite::params![task.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if n == 0 {
            return Err(BridgeError::StoreFailure);
        }
        let seq: i64 = tx
            .query_row(
                "SELECT last_event_seq FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        // Upsert start row — resume re-emits are allowed.
        tx.execute(
            "INSERT INTO task_node_starts(task_id, node_id, seq, ts)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(task_id, node_id) DO UPDATE SET seq=excluded.seq, ts=excluded.ts",
            rusqlite::params![task.as_str(), node.as_str(), seq, ts],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| BridgeError::StoreFailure)?;
        // Allocate seq.
        let n = tx
            .execute(
                "UPDATE tasks SET last_event_seq = last_event_seq + 1 WHERE id=?1",
                rusqlite::params![task.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if n == 0 {
            return Err(BridgeError::StoreFailure);
        }
        let seq: i64 = tx
            .query_row(
                "SELECT last_event_seq FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        // Plain INSERT (write-once per W3b; PK enforces uniqueness).
        tx.execute(
            "INSERT INTO task_node_checkpoints(task_id, node_id, output, ok, ts, seq)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![task.as_str(), node.as_str(), output, ok as i64, ts, seq],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        // Remove the start row — the node is no longer in-progress.
        tx.execute(
            "DELETE FROM task_node_starts WHERE task_id=?1 AND node_id=?2",
            rusqlite::params![task.as_str(), node.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| BridgeError::StoreFailure)?;
        // Allocate seq.
        let n = tx
            .execute(
                "UPDATE tasks SET last_event_seq = last_event_seq + 1 WHERE id=?1",
                rusqlite::params![task.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if n == 0 {
            return Err(BridgeError::StoreFailure);
        }
        let seq: i64 = tx
            .query_row(
                "SELECT last_event_seq FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        // Write the terminal status, result, error, and record terminal_seq.
        tx.execute(
            "UPDATE tasks SET status=?2, result=?3, error=?4, updated_ms=?5, terminal_seq=?6 WHERE id=?1",
            rusqlite::params![task.as_str(), status.as_str(), result, error, ts, seq],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        // Clear all start rows for this task.
        tx.execute(
            "DELETE FROM task_node_starts WHERE task_id=?1",
            rusqlite::params![task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn progress_snapshot(
        &self,
        task: &TaskId,
    ) -> Result<bridge_core::task_store::TaskProgressSnapshot, BridgeError> {
        use bridge_core::task_store::TaskProgressSnapshot;
        let conn = self.conn.lock().unwrap();
        // Use a transaction for a consistent read so cut_seq is exact.
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| BridgeError::StoreFailure)?;
        // Read task row: status, result, error, terminal_seq, last_event_seq.
        let (status_s, result, error, terminal_seq, cut_seq): (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
        ) = tx
            .query_row(
                "SELECT status, result, error, terminal_seq, last_event_seq FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let status = bridge_core::task_store::TaskRecordStatus::parse(&status_s)
            .ok_or(BridgeError::StoreFailure)?;
        // Read checkpoints ordered by seq (NULL seq → 0 via COALESCE).
        // Each stmt+rows pair is in its own scope so the borrow is released before the next prepare.
        let checkpoints: Vec<(NodeId, String, bool, i64)> = {
            let mut cp_stmt = tx
                .prepare(
                    "SELECT node_id, output, ok, COALESCE(seq, 0) FROM task_node_checkpoints
                     WHERE task_id=?1 ORDER BY COALESCE(seq, 0)",
                )
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut cp_rows = cp_stmt
                .query(rusqlite::params![task.as_str()])
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut out = Vec::new();
            while let Some(row) = cp_rows.next().map_err(|_| BridgeError::StoreFailure)? {
                let node_s: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                let output: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
                let ok_i: i64 = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
                let seq: i64 = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
                let node = NodeId::parse(node_s).map_err(|_| BridgeError::StoreFailure)?;
                out.push((node, output, ok_i != 0, seq));
            }
            out
        };
        // Read in-progress start rows ordered by seq.
        let starts: Vec<(NodeId, i64)> = {
            let mut st_stmt = tx
                .prepare("SELECT node_id, seq FROM task_node_starts WHERE task_id=?1 ORDER BY seq")
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut st_rows = st_stmt
                .query(rusqlite::params![task.as_str()])
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut out = Vec::new();
            while let Some(row) = st_rows.next().map_err(|_| BridgeError::StoreFailure)? {
                let node_s: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                let seq: i64 = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
                let node = NodeId::parse(node_s).map_err(|_| BridgeError::StoreFailure)?;
                out.push((node, seq));
            }
            out
        };
        // Read-only transaction: commit or just let it drop — either is fine.
        drop(tx);
        Ok(TaskProgressSnapshot {
            status,
            result,
            error,
            checkpoints,
            starts,
            terminal_seq,
            cut_seq,
        })
    }
}

fn row_to_task(row: &rusqlite::Row) -> Result<bridge_core::task_store::TaskRecord, BridgeError> {
    use bridge_core::task_store::{TaskRecord, TaskRecordStatus};
    let id: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
    let workflow: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
    let status_s: String = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
    let result: Option<String> = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
    let error: Option<String> = row.get(4).map_err(|_| BridgeError::StoreFailure)?;
    let created_ms: i64 = row.get(5).map_err(|_| BridgeError::StoreFailure)?;
    let updated_ms: i64 = row.get(6).map_err(|_| BridgeError::StoreFailure)?;
    let input: Option<String> = row.get(7).map_err(|_| BridgeError::StoreFailure)?;
    let workflow_spec_json: Option<String> = row.get(8).map_err(|_| BridgeError::StoreFailure)?;
    let resume_attempts: Option<i64> = row.get(9).map_err(|_| BridgeError::StoreFailure)?;
    let session_cwd: Option<String> = row.get(10).map_err(|_| BridgeError::StoreFailure)?;
    Ok(TaskRecord {
        id: TaskId::parse(id).map_err(|_| BridgeError::StoreFailure)?,
        workflow,
        status: TaskRecordStatus::parse(&status_s).ok_or(BridgeError::StoreFailure)?,
        result,
        error,
        created_ms,
        updated_ms,
        input: input.unwrap_or_default(),
        workflow_spec_json,
        resume_attempts: resume_attempts.unwrap_or(0) as u32,
        session_cwd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{PeerTaskId, PendingKind, PendingRequest};
    use bridge_core::ids::{SessionId, TaskId};
    use bridge_core::ports::SessionStore;
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
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        }
    }

    #[tokio::test]
    async fn task_create_get_set_terminal_inmemory() {
        let s = SqliteStore::open_in_memory().unwrap();
        let id = TaskId::parse("t1").unwrap();
        s.create(&trec("t1", 1)).await.unwrap();
        assert_eq!(
            s.get(&id).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );
        s.set_terminal(&id, TaskRecordStatus::Completed, Some("SYNTH"), None, 9)
            .await
            .unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("SYNTH"));
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
            s.set_terminal(&id, TaskRecordStatus::Completed, Some("R"), None, 2)
                .await
                .unwrap();
        }
        let s2 = SqliteStore::open(&path).unwrap();
        let got = s2
            .get(&TaskId::parse("keep").unwrap())
            .await
            .unwrap()
            .unwrap();
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
        assert_eq!(s.list(10).await.unwrap()[0].id.as_str(), "b");
        let n = s.sweep_interrupted(99).await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            s.get(&TaskId::parse("a").unwrap())
                .await
                .unwrap()
                .unwrap()
                .status,
            TaskRecordStatus::Interrupted
        );
    }

    #[tokio::test]
    async fn peer_task_roundtrips() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        assert!(s.peer_task_for(&t).await.unwrap().is_none());
        s.set_peer_task(&t, &PeerTaskId("p1".into())).await.unwrap();
        assert_eq!(
            s.peer_task_for(&t).await.unwrap().unwrap(),
            PeerTaskId("p1".into())
        );
    }

    #[tokio::test]
    async fn early_cancel_latches() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        assert!(!s.cancel_requested(&t).await.unwrap());
        s.request_cancel(&t).await.unwrap(); // before any peer id exists
        assert!(s.cancel_requested(&t).await.unwrap());
    }

    #[tokio::test]
    async fn put_then_session_for_roundtrips() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        let sid = SessionId::parse("sess").unwrap();
        s.put(&t, &sid).await.unwrap();
        assert_eq!(s.session_for(&t).await.unwrap().unwrap(), sid);
    }

    #[tokio::test]
    async fn session_for_missing_is_none() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s
            .session_for(&TaskId::parse("nope").unwrap())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn pending_persists_then_clears_on_take() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.put(&t, &SessionId::parse("sess").unwrap()).await.unwrap();
        s.put_pending(
            &t,
            &PendingRequest {
                request_id: "r1".into(),
                kind: PendingKind::Auth,
            },
        )
        .await
        .unwrap();
        let got = s.take_pending(&t).await.unwrap().unwrap();
        assert_eq!(got.request_id, "r1");
        assert!(matches!(got.kind, PendingKind::Auth));
        assert!(s.take_pending(&t).await.unwrap().is_none()); // cleared
    }

    #[tokio::test]
    async fn put_pending_without_session_row_still_works() {
        // put_pending should upsert so a pending request can be stored even before put()
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t2").unwrap();
        s.put_pending(
            &t,
            &PendingRequest {
                request_id: "r2".into(),
                kind: PendingKind::Permission,
            },
        )
        .await
        .unwrap();
        assert_eq!(s.take_pending(&t).await.unwrap().unwrap().request_id, "r2");
    }

    #[tokio::test]
    async fn task_mode_roundtrips() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        assert!(!s.is_fanout(&t).await.unwrap());
        s.set_fanout(&t).await.unwrap();
        assert!(s.is_fanout(&t).await.unwrap());
    }

    #[test]
    fn second_open_same_path_fails_lock() {
        let dir = std::env::temp_dir().join(format!("a2a-w3a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lock-test.db");
        let _first = SqliteStore::open(&path).expect("first open succeeds");
        let second = SqliteStore::open(&path);
        assert!(second.is_err(), "second open of a locked db must fail");
        drop(_first);
        assert!(SqliteStore::open(&path).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_creates_missing_parent_dir() {
        // A `[store] path` may name a not-yet-existing subdir (e.g. `<config-dir>/.a2a-bridge/...`).
        // `open` must `mkdir -p` the parent rather than fail with StoreFailure.
        let base = std::env::temp_dir().join(format!("a2a-store-mkdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("nested").join("tasks.sqlite");
        assert!(!base.exists(), "parent dir must not pre-exist");
        let store = SqliteStore::open(&path).expect("open creates the missing parent dir");
        assert!(path.exists(), "db file created under the freshly-made dir");
        drop(store);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn w3b_schema_and_checkpoints() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.create(&TaskRecord {
            id: t.clone(),
            workflow: "wf".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some("{\"v\":1}".into()),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
        use bridge_core::ids::NodeId;
        use bridge_core::task_store::ResumeClaim;
        s.put_node_checkpoint(&t, &NodeId::parse("codex").unwrap(), "OUT", true, 2)
            .await
            .unwrap();
        assert_eq!(s.node_checkpoints(&t).await.unwrap()[0].1, "OUT");
        assert!(matches!(
            s.claim_resume_attempt(&t, 1, 9).await.unwrap(),
            ResumeClaim::Resumable { attempt: 1 }
        ));
        assert!(matches!(
            s.claim_resume_attempt(&t, 1, 9).await.unwrap(),
            ResumeClaim::Exhausted
        ));
        assert_eq!(s.working_tasks().await.unwrap()[0].input, "DIFF");
    }

    #[tokio::test]
    async fn session_cwd_sqlite_roundtrip() {
        // A TaskRecord with session_cwd=Some("/req") must survive create→get via SQLite.
        let s = SqliteStore::open_in_memory().unwrap();
        let id = TaskId::parse("cwd-sq-1").unwrap();
        s.create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: Some("/req".to_string()),
        })
        .await
        .unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(
            got.session_cwd.as_deref(),
            Some("/req"),
            "session_cwd must survive SQLite create→get"
        );
    }

    #[tokio::test]
    async fn migration_on_old_schema_db_with_cascade_and_fk() {
        // Old DB with only the ORIGINAL tasks table; insert a row; reopen TWICE with new code →
        // columns added (idempotent), row intact, foreign_keys ON, ON DELETE CASCADE works.
        let dir = std::env::temp_dir().join(format!("a2a-w3b-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.db");
        {
            use rusqlite::Connection;
            let c = Connection::open(&path).unwrap();
            // Pre-create the legacy 5-column task_node_checkpoints table (no `seq` column) and
            // insert one row — exercises the ALTER-add-seq-on-a-populated-table path.
            c.execute_batch(
                "CREATE TABLE tasks(id TEXT PRIMARY KEY, workflow TEXT NOT NULL, \
                 status TEXT NOT NULL, result TEXT, error TEXT, \
                 created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);
                 CREATE TABLE task_node_checkpoints(
                     task_id TEXT NOT NULL, node_id TEXT NOT NULL,
                     output TEXT NOT NULL, ok INTEGER NOT NULL, ts INTEGER NOT NULL,
                     PRIMARY KEY(task_id, node_id),
                     FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE
                 );",
            )
            .unwrap();
            c.execute(
                "INSERT INTO tasks(id,workflow,status,created_ms,updated_ms) VALUES('old','wf','working',1,1)",
                [],
            )
            .unwrap();
            // Insert a legacy checkpoint row (no seq column).
            c.execute(
                "INSERT INTO task_node_checkpoints(task_id,node_id,output,ok,ts) VALUES('old','n','o',1,2)",
                [],
            )
            .unwrap();
        }
        // First reopen: migrates — adds tasks columns (last_event_seq, terminal_seq, etc.),
        // adds seq to task_node_checkpoints, creates task_node_starts.
        {
            let s = SqliteStore::open(&path).unwrap();
            let got = s
                .get(&TaskId::parse("old").unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got.status, TaskRecordStatus::Working);
            assert_eq!(got.input, ""); // default for migrated row
            assert_eq!(got.session_cwd, None); // NULL for migrated old row
            use bridge_core::ids::NodeId;
            let old = TaskId::parse("old").unwrap();
            // Verify the migration added the new columns by checking PRAGMA.
            {
                let conn = s.conn.lock().unwrap();
                let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
                let cols: HashSet<String> = stmt
                    .query_map([], |row| row.get::<_, String>(1))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                assert!(
                    cols.contains("last_event_seq"),
                    "tasks.last_event_seq must exist after migration"
                );
                assert!(
                    cols.contains("terminal_seq"),
                    "tasks.terminal_seq must exist after migration"
                );
                let mut stmt2 = conn
                    .prepare("PRAGMA table_info(task_node_checkpoints)")
                    .unwrap();
                let cp_cols: HashSet<String> = stmt2
                    .query_map([], |row| row.get::<_, String>(1))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                assert!(
                    cp_cols.contains("seq"),
                    "task_node_checkpoints.seq must exist after migration"
                );
                // task_node_starts must exist.
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_node_starts'",
                    [],
                    |row| row.get(0),
                ).unwrap();
                assert_eq!(count, 1, "task_node_starts table must be created");
            }
            // The pre-existing legacy checkpoint row should appear as seq=0 in the snapshot.
            let snap = s.progress_snapshot(&old).await.unwrap();
            let legacy_cp = snap.checkpoints.iter().find(|c| c.0.as_str() == "n");
            assert!(
                legacy_cp.is_some(),
                "legacy checkpoint must appear in snapshot"
            );
            assert_eq!(legacy_cp.unwrap().3, 0, "legacy NULL seq must map to 0");
            // A seq write on the freshly-migrated task works from the DEFAULT 0 baseline.
            let first = s
                .record_node_started(&old, &NodeId::parse("m").unwrap(), 10)
                .await
                .unwrap();
            assert_eq!(
                first, 1,
                "first seq on a migrated task (last_event_seq DEFAULT 0) must be 1"
            );
            // Adding a new checkpoint still works.
            s.node_checkpoints(&old).await.unwrap(); // already 1
                                                     // Verify we can't double-insert the legacy checkpoint (write-once).
            let res = s
                .put_node_checkpoint(&old, &NodeId::parse("n").unwrap(), "o2", true, 3)
                .await;
            assert!(
                res.is_err(),
                "write-once must be enforced for the legacy checkpoint key"
            );
        }
        // Second reopen: migration idempotent (no duplicate-column error), foreign_keys ON, cascade.
        {
            let s = SqliteStore::open(&path).unwrap();
            assert!(s.foreign_keys_on().unwrap()); // test helper
            let old = TaskId::parse("old").unwrap();
            // delete the parent task → checkpoint cascades away
            s.delete_for_test(&old).unwrap(); // test helper: DELETE FROM tasks WHERE id=?
            assert_eq!(s.node_checkpoints(&old).await.unwrap().len(), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sqlite_seq_and_snapshot() {
        let s = SqliteStore::open_in_memory().unwrap();
        use bridge_core::ids::NodeId;
        let t = TaskId::parse("t").unwrap();
        s.create(&trec("t", 1)).await.unwrap();
        let s1 = s
            .record_node_started(&t, &NodeId::parse("a").unwrap(), 1)
            .await
            .unwrap();
        let s2 = s
            .put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "OUT", true, 2)
            .await
            .unwrap();
        assert!(s2 > s1);
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.checkpoints[0].3, s2); // seq carried
        assert!(snap.starts.is_empty()); // start cleared on finish
                                         // record_node_started is an UPSERT (resume re-emit): no PK error
        let r1 = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), 3)
            .await
            .unwrap();
        let r2 = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), 4)
            .await
            .unwrap();
        assert!(r2 > r1);
        let term = s
            .set_terminal_sequenced(&t, TaskRecordStatus::Completed, Some("R"), None, 5)
            .await
            .unwrap();
        assert_eq!(
            s.progress_snapshot(&t).await.unwrap().terminal_seq,
            Some(term)
        );
    }

    #[tokio::test]
    async fn null_seq_legacy_checkpoint_is_seq_zero() {
        let s = SqliteStore::open_in_memory().unwrap();
        use bridge_core::ids::NodeId;
        let t = TaskId::parse("t").unwrap();
        s.create(&trec("t", 1)).await.unwrap();
        // Use the legacy (no-seq) put_node_checkpoint to insert without a seq.
        s.put_node_checkpoint(&t, &NodeId::parse("old").unwrap(), "O", true, 1)
            .await
            .unwrap();
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(
            snap.checkpoints
                .iter()
                .find(|c| c.0.as_str() == "old")
                .unwrap()
                .3,
            0
        );
    }

    #[tokio::test]
    async fn seq_continues_across_resume_seed() {
        let s = SqliteStore::open_in_memory().unwrap();
        use bridge_core::ids::NodeId;
        let t = TaskId::parse("t").unwrap();
        s.create(&trec("t", 1)).await.unwrap();
        let a = s
            .put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "A", true, 1)
            .await
            .unwrap();
        let b = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), 2)
            .await
            .unwrap();
        assert!(b > a, "seq continues across a resumed run, not reset");
    }
}
