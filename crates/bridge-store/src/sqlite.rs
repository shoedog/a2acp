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
}

impl SqliteStore {
    /// Open an in-memory database (suitable for tests).
    pub fn open_in_memory() -> Result<Self, BridgeError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
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
                cancel_requested INTEGER NOT NULL DEFAULT 0
            );",
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{PeerTaskId, PendingKind, PendingRequest};
    use bridge_core::ids::{SessionId, TaskId};
    use bridge_core::ports::SessionStore;

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
}
