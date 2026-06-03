// sqlite.rs — SQLite-backed SessionStore (spec §7, Task 9).

use bridge_core::{
    domain::{PeerTaskId, PendingKind, PendingRequest},
    error::BridgeError,
    ids::{SessionId, TaskId},
    ports::SessionStore,
};
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
            .truncate(false)
            .open(&lock_path)
            .map_err(|_| BridgeError::StoreFailure)?;
        lock.try_lock_exclusive()
            .map_err(|_| BridgeError::StoreFailure)?;
        let conn = rusqlite::Connection::open(path).map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            _lock: Some(lock),
        };
        store.create_schema()?;
        Ok(store)
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
            CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_ms);",
        )
        .map_err(|_| BridgeError::StoreFailure)
    }
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

fn row_to_task(row: &rusqlite::Row) -> Result<bridge_core::task_store::TaskRecord, BridgeError> {
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
}
