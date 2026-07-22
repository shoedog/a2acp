// sqlite.rs — SQLite-backed SessionStore (spec §7, Task 9).

use bridge_core::{
    domain::{PeerTaskId, PendingKind, PendingRequest},
    error::BridgeError,
    ids::{NodeId, OperationId, SessionId, TaskId},
    ports::SessionStore,
    task_store::{durable_retention_ms, system_wall_now_ms, PersistenceClock},
};
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
enum MigrationValidationError {
    MalformedLocator,
    ConflictingAuthority,
}

#[derive(Debug)]
enum SchemaMigrationError {
    Sqlite(rusqlite::Error),
    Validation(MigrationValidationError),
}

impl From<rusqlite::Error> for SchemaMigrationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// SQLite-backed [`SessionStore`] that persists `task_id ↔ session_id` mappings
/// and a per-task pending-request that is cleared atomically on first read.
pub struct SqliteStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    now_ms: PersistenceClock,
    // Held for the store's lifetime: an exclusive advisory lock on `<path>.lock`
    // so only one `serve` owns a DB file (makes the boot sweep safe). `None` for
    // in-memory stores.
    _lock: Option<std::fs::File>,
    /// Present for every file-backed workflow-history allocation so physical
    /// admission accounts for the database and live sidecars.
    history_path: Option<std::path::PathBuf>,
    /// Per-attempt process leases are used only by the concurrent platform ledger.
    /// Configured primary stores retain their lifetime-exclusive database lock and
    /// in-memory stores need no cross-process ownership signal.
    history_attempt_lock_dir: Option<std::path::PathBuf>,
    history_attempt_locks: Mutex<HashMap<String, std::fs::File>>,
}

impl SqliteStore {
    /// Open an in-memory database (suitable for tests).
    pub fn open_in_memory() -> Result<Self, BridgeError> {
        Self::open_in_memory_with_clock(Arc::new(system_wall_now_ms))
    }

    pub fn open_in_memory_with_clock(now_ms: PersistenceClock) -> Result<Self, BridgeError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|_| BridgeError::StoreFailure)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| BridgeError::StoreFailure)?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")
            .map_err(|_| BridgeError::StoreFailure)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            now_ms,
            _lock: None,
            history_path: None,
            history_attempt_lock_dir: None,
            history_attempt_locks: Mutex::new(HashMap::new()),
        };
        store.create_schema()?;
        Ok(store)
    }

    /// Open a file-backed DB, acquiring an exclusive advisory lock on `<path>.lock`.
    /// A second `open` of the same path while the first is held returns a typed
    /// ledger error. SQLite failures retain both primary and extended codes.
    pub fn open(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        Self::open_lifetime_typed(path, false)
    }

    fn open_lifetime_typed(
        path: &std::path::Path,
        bounded_history: bool,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;
        use fs2::FileExt;

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                history_io_error_with_permission(&error, R::Open, R::ReadOnlyParent)
            })?;
        }
        let lock_path = history_schema_lock_path(path);
        let lock_permission = if lock_path.exists() {
            R::ReadOnlyLock
        } else {
            R::ReadOnlyParent
        };
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| history_io_error_with_permission(&error, R::Open, lock_permission))?;
        lock.try_lock_exclusive()
            .map_err(|error| history_lock_error(&error))?;
        // Resolve database-file permissions before SQLite can collapse an OS
        // EACCES into CANTOPEN. A file that existed before this open owns its
        // permissions; failure to create a missing file belongs to its parent.
        let database_permission = if path.exists() {
            R::ReadOnlyDatabase
        } else {
            R::ReadOnlyParent
        };
        std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|error| {
                history_io_error_with_permission(&error, R::Open, database_permission)
            })?;
        let conn = rusqlite::Connection::open(path).map_err(|error| history_error(&error))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|error| history_error(&error))?;
        if !bounded_history {
            let mode = conn
                .query_row("PRAGMA journal_mode = WAL", [], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| history_error(&error))?;
            if mode != "wal" {
                tracing::warn!(
                    mode = %mode,
                    path = %path.display(),
                    "PRAGMA journal_mode=WAL not honored; continuing without WAL"
                );
            }
        }
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")
            .map_err(|error| history_error(&error))?;
        #[cfg(unix)]
        {
            set_history_permissions(path, 0o600, R::ReadOnlyDatabase)?;
            set_history_permissions(&lock_path, 0o600, R::ReadOnlyLock)?;
            for suffix in ["-wal", "-shm", "-journal"] {
                let mut sidecar = path.as_os_str().to_os_string();
                sidecar.push(suffix);
                let sidecar = std::path::PathBuf::from(sidecar);
                if sidecar.exists() {
                    set_history_permissions(&sidecar, 0o600, R::ReadOnlyDatabase)?;
                }
            }
        }
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            now_ms: Arc::new(system_wall_now_ms),
            _lock: Some(lock),
            history_path: bounded_history.then(|| path.to_path_buf()),
            history_attempt_lock_dir: None,
            history_attempt_locks: Mutex::new(HashMap::new()),
        };
        let _admission_lease = if bounded_history {
            store.acquire_history_admission_lease()?
        } else {
            None
        };
        if bounded_history {
            // Bound the allocation before any migration can grow the database.
            store.configure_history_physical_limit()?;
        }
        store
            .create_schema_sqlite()
            .map_err(|error| schema_migration_error(&error))?;
        if bounded_history {
            store.checkpoint_and_verify_history_size()?;
        }
        Ok(store)
    }

    /// Open the workflow-history allocation inside an explicitly configured shared store.
    /// The same 128 MiB physical database-plus-sidecar ceiling applies to shared
    /// and platform allocations.
    pub fn open_shared_history(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        Self::open_lifetime_typed(path, true)
    }

    /// Open the owner-private platform ledger used when no shared store is
    /// configured. Its process-lifetime lock gives one durable owner, while
    /// whole-file accounting enforces the 128 MiB database-plus-sidecar cap.
    /// Active attempts are intentionally left for coordinator checkpoint-first
    /// reconciliation instead of being interrupted during store construction.
    pub fn open_platform_history(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        Self::open_lifetime_typed(path, true)
    }

    /// Open a selected workflow-history ledger and collapse raw filesystem/SQLite text
    /// into the closed admission vocabulary.
    pub fn open_history(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        Self::open_history_with_schema_lock_timeout(path, std::time::Duration::from_secs(5))
    }

    fn open_history_with_schema_lock_timeout(
        path: &std::path::Path,
        schema_lock_timeout: std::time::Duration,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
        use fs2::FileExt;

        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        if let Some(parent) = parent {
            std::fs::create_dir_all(parent).map_err(|error| {
                history_io_error_with_permission(&error, R::Open, R::ReadOnlyParent)
            })?;
        }

        let lock_path = history_schema_lock_path(path);
        let attempt_lock_dir = history_attempt_lock_dir(path);
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut file_options = std::fs::OpenOptions::new();
        file_options
            .create(true)
            .read(true)
            .write(true)
            .truncate(false);
        #[cfg(unix)]
        file_options.mode(0o600);
        let lock_permission = if lock_path.exists() {
            R::ReadOnlyLock
        } else {
            R::ReadOnlyParent
        };
        let schema_lock = file_options
            .open(&lock_path)
            .map_err(|error| history_io_error_with_permission(&error, R::Open, lock_permission))?;

        let started = std::time::Instant::now();
        loop {
            match schema_lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && started.elapsed() < schema_lock_timeout =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(LedgerError::new(R::Locked));
                }
                Err(error) => return Err(history_lock_error(&error)),
            }
        }

        std::fs::create_dir_all(&attempt_lock_dir).map_err(|error| {
            history_io_error_with_permission(&error, R::Open, R::ReadOnlyParent)
        })?;
        let database_permission = if path.exists() {
            R::ReadOnlyDatabase
        } else {
            R::ReadOnlyParent
        };
        file_options.open(path).map_err(|error| {
            history_io_error_with_permission(&error, R::Open, database_permission)
        })?;
        #[cfg(unix)]
        {
            if let Some(parent) = parent {
                set_history_permissions(parent, 0o700, R::ReadOnlyParent)?;
            }
            set_history_permissions(&lock_path, 0o600, R::ReadOnlyLock)?;
            set_history_permissions(path, 0o600, R::ReadOnlyDatabase)?;
            set_history_permissions(&attempt_lock_dir, 0o700, R::ReadOnlyParent)?;
        }

        let conn = rusqlite::Connection::open(path).map_err(|error| history_error(&error))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|error| history_error(&error))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")
            .map_err(|error| history_error(&error))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            now_ms: Arc::new(system_wall_now_ms),
            _lock: None,
            history_path: Some(path.to_path_buf()),
            history_attempt_lock_dir: Some(attempt_lock_dir),
            history_attempt_locks: Mutex::new(HashMap::new()),
        };
        let admission_lease = store.acquire_history_admission_lease()?;
        // The schema lock serializes migration, while the physical limit must
        // already be active before migration writes their first page.
        store.configure_history_physical_limit()?;
        store
            .create_schema_sqlite()
            .map_err(|error| schema_migration_error(&error))?;
        store.checkpoint_and_verify_history_size()?;
        drop(schema_lock);
        drop(admission_lease);

        #[cfg(unix)]
        set_history_sidecar_permissions(path)?;
        store.interrupt_active_sync(system_wall_now_ms(), &[])?;
        Ok(store)
    }

    /// Open an existing history allocation without migration, lock-file creation, or writes.
    pub fn open_history_read_only(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| history_error(&error))?;
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|error| history_error(&error))?;
        conn.prepare(
            "SELECT attempt_id, reservation_json, terminal_json, prompt_acceptance, pinned,
                    task_id, ordinal, started_ms, status, completed_ms
             FROM workflow_attempt_summaries LIMIT 0",
        )
        .map_err(|error| history_migration_error(&error))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            now_ms: Arc::new(system_wall_now_ms),
            _lock: None,
            history_path: Some(path.to_path_buf()),
            history_attempt_lock_dir: None,
            history_attempt_locks: Mutex::new(HashMap::new()),
        })
    }
    /// Open an existing workflow-history allocation for bounded operator mutations.
    /// This does not migrate, reconcile active rows, or take the configured store's
    /// process-lifetime lock; SQLite's immediate transaction is the pin/unpin
    /// linearization point against concurrent retention.
    pub fn open_history_admin(
        path: &std::path::Path,
    ) -> Result<Self, bridge_core::workflow_history::LedgerError> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| history_error(&error))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|error| history_error(&error))?;
        conn.prepare(
            "SELECT attempt_id, pinned, reservation_json
             FROM workflow_attempt_summaries LIMIT 0",
        )
        .map_err(|error| history_migration_error(&error))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            now_ms: Arc::new(system_wall_now_ms),
            _lock: None,
            history_path: Some(path.to_path_buf()),
            history_attempt_lock_dir: None,
            history_attempt_locks: Mutex::new(HashMap::new()),
        };
        let _admission_lease = store.acquire_history_admission_lease()?;
        store.configure_history_physical_limit()?;
        Ok(store)
    }

    fn open_history_attempt_lock(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<std::fs::File, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let directory = self
            .history_attempt_lock_dir
            .as_ref()
            .ok_or_else(|| bridge_core::workflow_history::LedgerError::new(R::Schema))?;
        let path = directory.join(format!("{}.lock", id.as_str()));
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let permission = if path.exists() {
            R::ReadOnlyLock
        } else {
            R::ReadOnlyParent
        };
        let file = options
            .open(&path)
            .map_err(|error| history_io_error_with_permission(&error, R::Io, permission))?;
        #[cfg(unix)]
        set_history_permissions(&path, 0o600, R::ReadOnlyLock)?;
        Ok(file)
    }

    /// Serialize physical admission across primary, platform, and live-admin
    /// connections. This dedicated lease is distinct from the configured
    /// primary's lifetime schema lock, so bounded admin pinning remains usable.
    fn acquire_history_admission_lease(
        &self,
    ) -> Result<Option<std::fs::File>, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
        use fs2::FileExt;

        let Some(path) = self.history_path.as_ref() else {
            return Ok(None);
        };
        let lock_path = history_admission_lock_path(path);
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let permission = if lock_path.exists() {
            R::ReadOnlyLock
        } else {
            R::ReadOnlyParent
        };
        let file = options.open(&lock_path).map_err(|error| {
            history_io_error_with_permission(&error, R::AdvisoryLockIo, permission)
        })?;
        #[cfg(unix)]
        set_history_permissions(&lock_path, 0o600, R::ReadOnlyLock)?;
        let started = std::time::Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Some(file)),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && started.elapsed() < std::time::Duration::from_secs(5) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(LedgerError::new(R::Locked));
                }
                Err(error) => return Err(history_lock_error(&error)),
            }
        }
    }

    fn remove_history_attempt_lock_file(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let Some(directory) = self.history_attempt_lock_dir.as_ref() else {
            return Ok(());
        };
        let path = directory.join(format!("{}.lock", id.as_str()));
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(history_io_error_with_permission(
                &error,
                R::Io,
                R::ReadOnlyParent,
            )),
        }
    }

    /// Acquire a process-lifetime lease before exposing an active platform row.
    /// The configured primary store returns false because its database-wide lease
    /// already proves that every active row belongs to this process.
    fn acquire_history_attempt_lease(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<bool, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
        use fs2::FileExt;

        if self.history_attempt_lock_dir.is_none() {
            return Ok(false);
        }
        let mut leases = self
            .history_attempt_locks
            .lock()
            .map_err(|_| LedgerError::new(R::Io))?;
        if leases.contains_key(id.as_str()) {
            return Err(LedgerError::new(R::Collision));
        }
        let file = self.open_history_attempt_lock(id)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                // The schema lease uses locked; contention for this exact
                // high-entropy attempt identity is an identity collision.
                LedgerError::new(R::Collision)
            } else {
                history_lock_error(&error)
            }
        })?;
        leases.insert(id.as_str().to_owned(), file);
        Ok(true)
    }

    fn release_history_attempt_lease(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};

        let lease = self
            .history_attempt_locks
            .lock()
            .map_err(|_| LedgerError::new(R::Io))?
            .remove(id.as_str());
        drop(lease);
        self.remove_history_attempt_lock_file(id)
    }

    /// Probe whether an active platform row's owning process is gone. A held
    /// advisory lock is positive live-owner evidence, so reconciliation skips it.
    fn acquire_reconciliation_lease(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<Option<std::fs::File>, bridge_core::workflow_history::LedgerError> {
        use fs2::FileExt;

        if self.history_attempt_lock_dir.is_none() {
            return Ok(None);
        }
        if self
            .history_attempt_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(id.as_str())
        {
            return Ok(None);
        }
        let file = self.open_history_attempt_lock(id)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(history_lock_error(&error)),
        }
    }

    fn interrupt_active_sync(
        &self,
        completed_ms: i64,
        excluded: &[bridge_core::ids::AttemptId],
    ) -> Result<u64, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            AttemptTerminal, LedgerError, LedgerUnavailableReason as R, NodeCounts,
        };
        if completed_ms <= 0 {
            return Err(LedgerError::new(R::Schema));
        }
        let _admission_lease = self.acquire_history_admission_lease()?;
        self.checkpoint_and_verify_history_size()?;
        let excluded = excluded
            .iter()
            .map(bridge_core::ids::AttemptId::as_str)
            .collect::<HashSet<_>>();
        let mut conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| history_error(&error))?;
        let active = {
            let mut statement = tx
                .prepare(
                    "SELECT attempt_id, prompt_acceptance, length(terminal_reserve)
                     FROM workflow_attempt_summaries \
                     WHERE status='active' ORDER BY attempt_id",
                )
                .map_err(|error| history_error(&error))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                })
                .map_err(|error| history_error(&error))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| history_error(&error))?
        };
        let concurrent_platform = self.history_attempt_lock_dir.is_some();
        let mut candidates = Vec::with_capacity(active.len());
        for (attempt_id, prompt_acceptance, terminal_reserve) in active {
            let parsed = bridge_core::ids::AttemptId::parse(attempt_id)
                .map_err(|_| LedgerError::new(R::Corruption))?;
            if excluded.contains(parsed.as_str()) {
                continue;
            }
            if !concurrent_platform {
                candidates.push((parsed, prompt_acceptance, terminal_reserve, None));
                continue;
            }
            if let Some(lease) = self.acquire_reconciliation_lease(&parsed)? {
                candidates.push((parsed, prompt_acceptance, terminal_reserve, Some(lease)));
            }
        }
        for (attempt_id, prompt_acceptance, terminal_reserve, _lease) in &candidates {
            let terminal = AttemptTerminal {
                completed_ms,
                work_ms: 0,
                end_to_end_ms: 0,
                queue_ms: 0,
                cancellation_ms: 0,
                cleanup_ms: 0,
                finalization_ms: 0,
                outcome: "interrupted".into(),
                terminal_reason: "process_restart".into(),
                producer_terminal: "unknown".into(),
                final_message: "unknown".into(),
                process_liveness: "exited".into(),
                terminal_evidence_capability: "unsupported".into(),
                terminal_evidence_version: "none".into(),
                terminal_evidence_source: "none".into(),
                terminal_evidence_complete: false,
                degraded: true,
                prompt_acceptance: prompt_acceptance.clone(),
                cleanup_disposition: "unknown".into(),
                node_counts: NodeCounts::default(),
                phase_durations: Vec::new(),
                telemetry_complete: false,
                monotonic_clock: false,
            };
            let json = serde_json::to_string(&terminal).map_err(|_| LedgerError::new(R::Schema))?;
            let terminal_reserve = terminal_reserve
                .map(u64::try_from)
                .transpose()
                .map_err(|_| LedgerError::new(R::Corruption))?
                .unwrap_or(0);
            let growth = u64::try_from(json.len())
                .unwrap_or(u64::MAX)
                .saturating_sub(terminal_reserve);
            self.ensure_terminal_rewrite_headroom(&tx)?;
            if !self.history_growth_fits(&tx, growth)? {
                return Err(LedgerError::new(R::CapacityProtected));
            }
            let changed = tx
                .execute(
                    "UPDATE workflow_attempt_summaries SET status='terminal', completed_ms=?2,
                     outcome='interrupted', degraded=1, producer_terminal='unknown',
                     final_message='unknown', process_liveness='exited',
                     terminal_evidence_capability='unsupported',
                     terminal_evidence_version='none', terminal_evidence_source='none',
                     terminal_evidence_complete=0, telemetry_complete=0,
                     terminal_json=?3, terminal_reserve=NULL
                     WHERE attempt_id=?1 AND status='active'",
                    rusqlite::params![attempt_id.as_str(), completed_ms, json],
                )
                .map_err(|error| history_error(&error))?;
            if changed != 1 {
                return Err(LedgerError::new(R::Io));
            }
            self.ensure_terminal_rewrite_headroom(&tx)?;
        }
        tx.commit().map_err(|error| history_error(&error))?;
        drop(conn);
        let reconciled = candidates.len() as u64;
        let ids = candidates
            .iter()
            .map(|(attempt_id, _, _, _)| attempt_id.clone())
            .collect::<Vec<_>>();
        drop(candidates);
        for attempt_id in ids {
            if let Err(error) = self.remove_history_attempt_lock_file(&attempt_id) {
                tracing::warn!(
                    attempt = attempt_id.as_str(),
                    reason = error.reason.as_str(),
                    "active-attempt reconciliation committed; stale lease-file cleanup deferred"
                );
            }
        }
        self.checkpoint_after_committed_history_mutation("interrupt_active");
        Ok(reconciled)
    }

    fn checked_history_file_bytes(
        &self,
    ) -> Result<u64, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let Some(path) = self.history_path.as_ref() else {
            return Ok(0);
        };
        let mut total = std::fs::metadata(path)
            .map(|meta| meta.len())
            .map_err(|error| history_io_error_with_permission(&error, R::Io, R::Permission))?;
        for suffix in ["-wal", "-journal", "-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            match std::fs::metadata(std::path::PathBuf::from(sidecar)) {
                Ok(meta) => total = total.saturating_add(meta.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(history_io_error_with_permission(
                        &error,
                        R::Io,
                        R::Permission,
                    ));
                }
            }
        }
        Ok(total)
    }

    fn history_growth_fits(
        &self,
        conn: &rusqlite::Connection,
        requested_bytes: u64,
    ) -> Result<bool, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, MAX_CHARGED_BYTES,
        };

        if self.history_path.is_none() {
            return Ok(true);
        }
        let physical_bytes = self.checked_history_file_bytes()?;
        if physical_bytes > MAX_CHARGED_BYTES {
            return Ok(false);
        }
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let freelist_count: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let page_size = u64::try_from(page_size).map_err(|_| LedgerError::new(R::Corruption))?;
        let freelist_count =
            u64::try_from(freelist_count).map_err(|_| LedgerError::new(R::Corruption))?;
        if page_size == 0 {
            return Err(LedgerError::new(R::Corruption));
        }
        let reusable_bytes = page_size.saturating_mul(freelist_count);
        // Rollback-journal transactions may write page images even when the
        // main file reuses existing pages. MEMORY/OFF modes have no disk
        // journal; every disk-backed mode retains the worst-case transaction
        // reserve established by configure_history_physical_limit.
        let disk_journal_headroom = match journal_mode.to_ascii_lowercase().as_str() {
            "memory" | "off" => 0,
            _ => HISTORY_DISK_TRANSACTION_HEADROOM_BYTES,
        };
        let physical_expansion = requested_bytes.saturating_sub(reusable_bytes);
        Ok(physical_bytes
            .saturating_add(physical_expansion)
            .saturating_add(disk_journal_headroom)
            <= MAX_CHARGED_BYTES)
    }

    /// Keep a small proven-reusable page pool so replacing an active row's
    /// fixed terminal reservation cannot grow the main file by a B-tree split
    /// after live sidecars have consumed the remaining aggregate headroom.
    ///
    /// The empty scratch table is allocation-only: the insert and delete occur
    /// in the caller's transaction, and the postcondition is expressed solely
    /// as SQLite's durable freelist count.
    fn ensure_terminal_rewrite_headroom(
        &self,
        conn: &rusqlite::Connection,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, MAX_TERMINAL_JSON_BYTES,
        };

        if self.history_path.is_none() {
            return Ok(());
        }
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let page_size = u64::try_from(page_size).map_err(|_| LedgerError::new(R::Corruption))?;
        if page_size == 0 {
            return Err(LedgerError::new(R::Corruption));
        }
        let required_bytes = u64::try_from(MAX_TERMINAL_JSON_BYTES)
            .map_err(|_| LedgerError::new(R::Schema))?
            .saturating_add(page_size.saturating_mul(2));
        let required_pages = required_bytes.div_ceil(page_size);
        let freelist_pages: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let freelist_pages =
            u64::try_from(freelist_pages).map_err(|_| LedgerError::new(R::Corruption))?;
        if freelist_pages >= required_pages {
            return Ok(());
        }

        let occupied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workflow_history_rewrite_reserve",
                [],
                |row| row.get(0),
            )
            .map_err(|error| history_error(&error))?;
        if occupied != 0 {
            return Err(LedgerError::new(R::Corruption));
        }
        // Two extra pages cover the scratch row's local payload and B-tree
        // balancing while the deleted overflow pages form the reusable pool.
        let provision_bytes = required_pages.saturating_add(2).saturating_mul(page_size);
        if !self.history_growth_fits(conn, provision_bytes)? {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        let provision_bytes =
            i64::try_from(provision_bytes).map_err(|_| LedgerError::new(R::Schema))?;
        let inserted = conn
            .execute(
                "INSERT INTO workflow_history_rewrite_reserve(singleton, reserve)
                 VALUES(1, zeroblob(?1))",
                rusqlite::params![provision_bytes],
            )
            .map_err(|error| history_error(&error))?;
        let deleted = conn
            .execute(
                "DELETE FROM workflow_history_rewrite_reserve WHERE singleton=1",
                [],
            )
            .map_err(|error| history_error(&error))?;
        if inserted != 1 || deleted != 1 {
            return Err(LedgerError::new(R::Corruption));
        }
        let freelist_pages: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        if u64::try_from(freelist_pages).map_err(|_| LedgerError::new(R::Corruption))?
            < required_pages
        {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        Ok(())
    }

    #[cfg(test)]
    fn live_history_file_bytes(&self) -> u64 {
        self.checked_history_file_bytes()
            .expect("test history size remains readable")
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
        self.create_schema_sqlite()
            .map_err(|_| BridgeError::StoreFailure)
    }

    fn create_schema_sqlite(&self) -> Result<(), SchemaMigrationError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(SchemaMigrationError::Sqlite)?;
        tx.execute_batch(
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
                updated_ms INTEGER NOT NULL,
                last_artifact_ms INTEGER,
                artifacts_purged_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_ms);
            CREATE TABLE IF NOT EXISTS task_attempt_locators (
                task_id TEXT PRIMARY KEY,
                locator_json TEXT NOT NULL,
                FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS attempt_identities (
                attempt_id TEXT PRIMARY KEY,
                execution_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                task_id TEXT,
                owner_surface TEXT NOT NULL,
                summary_attached INTEGER NOT NULL DEFAULT 0
                    CHECK(summary_attached IN (0, 1)),
                UNIQUE(execution_id, ordinal)
            );
            CREATE INDEX IF NOT EXISTS idx_attempt_identities_task
                ON attempt_identities(task_id, ordinal);
            CREATE TABLE IF NOT EXISTS task_node_checkpoints (
                task_id   TEXT NOT NULL,
                node_id   TEXT NOT NULL,
                output    TEXT NOT NULL,
                ok        INTEGER NOT NULL,
                ts        INTEGER NOT NULL,
                usage_json TEXT,
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
            );
            CREATE TABLE IF NOT EXISTS task_journal (
                task_id    TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                event_json TEXT NOT NULL,
                PRIMARY KEY (task_id, seq),
                FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS turn_log (
                turn_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                task_id TEXT,
                workflow TEXT,
                node TEXT,
                attempt INTEGER NOT NULL,
                agent TEXT NOT NULL,
                model TEXT,
                effort TEXT,
                mode TEXT,
                prompt_id TEXT,
                started_ms INTEGER,
                completed_ms INTEGER,
                latency_ms INTEGER,
                ttft_ms INTEGER,
                outcome TEXT,
                failure_class TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                thought_tokens INTEGER,
                cached_read_tokens INTEGER,
                cached_write_tokens INTEGER,
                cost_amount REAL,
                cost_currency TEXT,
                traceparent TEXT,
                usage_finalized_ms INTEGER,
                usage_finalization_kind TEXT NOT NULL DEFAULT 'pending'
            );
            CREATE INDEX IF NOT EXISTS idx_turn_log_completed ON turn_log(completed_ms);
            CREATE INDEX IF NOT EXISTS idx_turn_log_task ON turn_log(task_id, node);
            CREATE INDEX IF NOT EXISTS idx_turn_log_eval ON turn_log(prompt_id, model, effort);
            CREATE TABLE IF NOT EXISTS workflow_attempt_summaries (
                attempt_id TEXT PRIMARY KEY,
                execution_id TEXT NOT NULL,
                parent_attempt_id TEXT,
                ordinal INTEGER NOT NULL,
                task_id TEXT,
                workflow TEXT NOT NULL,
                task_class TEXT NOT NULL,
                surface TEXT NOT NULL,
                policy TEXT NOT NULL,
                workload_fingerprint TEXT NOT NULL,
                workload_fingerprint_complete INTEGER NOT NULL DEFAULT 0,
                started_ms INTEGER NOT NULL,
                completed_ms INTEGER,
                status TEXT NOT NULL,
                prompt_acceptance TEXT NOT NULL DEFAULT 'not_dispatched',
                producer_terminal TEXT NOT NULL DEFAULT 'unknown',
                final_message TEXT NOT NULL DEFAULT 'unknown',
                process_liveness TEXT NOT NULL DEFAULT 'unknown',
                terminal_evidence_capability TEXT NOT NULL DEFAULT 'unsupported',
                terminal_evidence_version TEXT NOT NULL DEFAULT 'none',
                terminal_evidence_source TEXT NOT NULL DEFAULT 'none',
                terminal_evidence_complete INTEGER NOT NULL DEFAULT 0,
                telemetry_complete INTEGER NOT NULL DEFAULT 0,
                outcome TEXT,
                degraded INTEGER NOT NULL DEFAULT 0,
                pinned INTEGER NOT NULL DEFAULT 0,
                charged_bytes INTEGER NOT NULL,
                reservation_json TEXT NOT NULL,
                terminal_json TEXT,
                terminal_reserve BLOB
            );
            CREATE TABLE IF NOT EXISTS workflow_history_rewrite_reserve (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                reserve BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_workflow_attempt_terminal
                ON workflow_attempt_summaries(status, completed_ms, pinned);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_attempt_execution_ordinal
                ON workflow_attempt_summaries(execution_id, ordinal);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_attempt_parent
                ON workflow_attempt_summaries(parent_attempt_id) WHERE parent_attempt_id IS NOT NULL;
            ",
        )?;
        migrate_tasks_columns(&tx)?;
        migrate_workflow_history_columns(&tx)?;
        migrate_attempt_identity_authority(&tx)?;
        tx.commit()?;
        Ok(())
    }
}

fn migrate_attempt_identity_authority(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), SchemaMigrationError> {
    let legacy_exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='table' AND name='task_attempt_identities'",
        [],
        |row| row.get(0),
    )?;
    if legacy_exists != 0 {
        tx.execute_batch(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             SELECT legacy.attempt_id, legacy.execution_id, legacy.ordinal, legacy.task_id,
                    'served_task',
                    CASE WHEN EXISTS(
                        SELECT 1 FROM workflow_attempt_summaries summary
                        WHERE summary.attempt_id=legacy.attempt_id
                          AND summary.execution_id=legacy.execution_id
                          AND summary.ordinal=legacy.ordinal
                          AND summary.task_id=legacy.task_id
                          AND summary.surface='served_task'
                    ) THEN 1 ELSE 0 END
             FROM task_attempt_identities legacy
             WHERE 1
             ON CONFLICT(attempt_id) DO NOTHING;",
        )?;
        let invalid: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_attempt_identities legacy
             LEFT JOIN attempt_identities admitted
               ON admitted.attempt_id=legacy.attempt_id
             WHERE admitted.attempt_id IS NULL
                OR admitted.execution_id<>legacy.execution_id
                OR admitted.ordinal<>legacy.ordinal
                OR admitted.task_id<>legacy.task_id
                OR admitted.owner_surface<>'served_task'",
            [],
            |row| row.get(0),
        )?;
        if invalid != 0 {
            return Err(SchemaMigrationError::Validation(
                MigrationValidationError::ConflictingAuthority,
            ));
        }
    }

    let locators = {
        let mut statement =
            tx.prepare("SELECT task_id, locator_json FROM task_attempt_locators ORDER BY task_id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (task_id, json) in locators {
        let locator: bridge_core::task_store::TaskAttemptLocator = serde_json::from_str(&json)
            .map_err(|_| {
                SchemaMigrationError::Validation(MigrationValidationError::MalformedLocator)
            })?;
        if locator.identity.execution_id.as_str() != task_id {
            return Err(SchemaMigrationError::Validation(
                MigrationValidationError::MalformedLocator,
            ));
        }
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(
                 ?1, ?2, ?3, ?4, 'served_task',
                 CASE WHEN EXISTS(
                     SELECT 1 FROM workflow_attempt_summaries
                     WHERE attempt_id=?1 AND execution_id=?2 AND ordinal=?3
                       AND task_id=?4 AND surface='served_task'
                 ) THEN 1 ELSE 0 END)
             ON CONFLICT(attempt_id) DO NOTHING",
            rusqlite::params![
                locator.identity.attempt_id.as_str(),
                locator.identity.execution_id.as_str(),
                i64::from(locator.identity.ordinal),
                task_id
            ],
        )?;
        let admitted: Option<(String, i64, Option<String>, String)> = tx
            .query_row(
                "SELECT execution_id, ordinal, task_id, owner_surface
                 FROM attempt_identities WHERE attempt_id=?1",
                rusqlite::params![locator.identity.attempt_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if admitted
            != Some((
                locator.identity.execution_id.as_str().to_owned(),
                i64::from(locator.identity.ordinal),
                Some(task_id),
                "served_task".to_owned(),
            ))
        {
            return Err(SchemaMigrationError::Validation(
                MigrationValidationError::ConflictingAuthority,
            ));
        }
    }

    let summaries = {
        let mut statement = tx.prepare(
            "SELECT attempt_id, execution_id, ordinal, task_id, surface
             FROM workflow_attempt_summaries ORDER BY attempt_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (attempt_id, execution_id, ordinal, task_id, surface) in summaries {
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(attempt_id) DO NOTHING",
            rusqlite::params![attempt_id, execution_id, ordinal, task_id, surface],
        )?;
        let admitted: Option<(String, i64, Option<String>, String)> = tx
            .query_row(
                "SELECT execution_id, ordinal, task_id, owner_surface
                 FROM attempt_identities WHERE attempt_id=?1",
                rusqlite::params![attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if admitted.as_ref().map(|(execution, ord, task, owner)| {
            (execution.as_str(), *ord, task.as_deref(), owner.as_str())
        }) != Some((
            execution_id.as_str(),
            ordinal,
            task_id.as_deref(),
            surface.as_str(),
        )) {
            return Err(SchemaMigrationError::Validation(
                MigrationValidationError::ConflictingAuthority,
            ));
        }
        let changed = tx.execute(
            "UPDATE attempt_identities SET summary_attached=1 WHERE attempt_id=?1",
            rusqlite::params![attempt_id],
        )?;
        if changed != 1 {
            return Err(SchemaMigrationError::Validation(
                MigrationValidationError::ConflictingAuthority,
            ));
        }
    }
    if legacy_exists != 0 {
        tx.execute_batch("DROP TABLE task_attempt_identities;")?;
    }
    Ok(())
}

fn migrate_workflow_history_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(workflow_attempt_summaries)")?;
    let mut columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    drop(stmt);
    for (name, declaration) in [
        (
            "workload_fingerprint_complete",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "prompt_acceptance",
            "TEXT NOT NULL DEFAULT 'not_dispatched'",
        ),
        ("producer_terminal", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("final_message", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("process_liveness", "TEXT NOT NULL DEFAULT 'unknown'"),
        (
            "terminal_evidence_capability",
            "TEXT NOT NULL DEFAULT 'unsupported'",
        ),
        ("terminal_evidence_version", "TEXT NOT NULL DEFAULT 'none'"),
        ("terminal_evidence_source", "TEXT NOT NULL DEFAULT 'none'"),
        ("terminal_evidence_complete", "INTEGER NOT NULL DEFAULT 0"),
        ("telemetry_complete", "INTEGER NOT NULL DEFAULT 0"),
        ("terminal_reserve", "BLOB"),
    ] {
        if columns.insert(name.to_owned()) {
            conn.execute(
                &format!("ALTER TABLE workflow_attempt_summaries ADD COLUMN {name} {declaration}"),
                [],
            )?;
        }
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_attempt_execution_ordinal
             ON workflow_attempt_summaries(execution_id, ordinal);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_attempt_parent
             ON workflow_attempt_summaries(parent_attempt_id)
             WHERE parent_attempt_id IS NOT NULL;",
    )?;
    Ok(())
}

/// Idempotently add additive task/batch schema.
/// Reads existing columns via `PRAGMA table_info`, then issues `ALTER TABLE ADD COLUMN`
/// only for columns that are missing. Safe to call on both fresh and old databases.
fn migrate_tasks_columns(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS batch (
            id TEXT PRIMARY KEY,
            workflow TEXT NOT NULL,
            concurrency INTEGER NOT NULL,
            total INTEGER NOT NULL,
            status TEXT NOT NULL,
            items_json TEXT NOT NULL,
            error TEXT,
            created_ms INTEGER NOT NULL,
            updated_ms INTEGER NOT NULL
        );",
    )?;

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
        ("journal_complete_from_birth", "INTEGER NOT NULL DEFAULT 0"),
        ("batch_id", "TEXT"),
        ("item_id", "TEXT"),
        ("last_artifact_ms", "INTEGER"),
        ("artifacts_purged_at", "INTEGER"),
        (
            "terminal_projection_ready",
            "INTEGER NOT NULL DEFAULT 1 CHECK(terminal_projection_ready IN (0, 1))",
        ),
        ("terminal_projection_attempt_id", "TEXT"),
        ("terminal_projection_json", "TEXT"),
    ];
    for (col, def) in additive {
        if !existing.contains(col) {
            conn.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {col} {def};"))?;
        }
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_batch_item
            ON tasks(batch_id, item_id) WHERE batch_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_tasks_terminal_projection
            ON tasks(terminal_projection_ready, id);",
    )?;

    // Collect existing column names for `task_node_checkpoints`.
    let mut stmt2 = conn.prepare("PRAGMA table_info(task_node_checkpoints)")?;
    let cp_existing: HashSet<String> = stmt2
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !cp_existing.contains("seq") {
        conn.execute_batch("ALTER TABLE task_node_checkpoints ADD COLUMN seq INTEGER;")?;
    }
    if !cp_existing.contains("usage_json") {
        conn.execute_batch("ALTER TABLE task_node_checkpoints ADD COLUMN usage_json TEXT;")?;
    }

    let mut stmt3 = conn.prepare("PRAGMA table_info(turn_log)")?;
    let turn_existing: HashSet<String> = stmt3
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !turn_existing.contains("usage_finalized_ms") {
        conn.execute_batch("ALTER TABLE turn_log ADD COLUMN usage_finalized_ms INTEGER;")?;
    }
    if !turn_existing.contains("usage_finalization_kind") {
        conn.execute_batch(
            "ALTER TABLE turn_log ADD COLUMN usage_finalization_kind TEXT NOT NULL DEFAULT 'pending';",
        )?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tasks_artifact_retention
            ON tasks(status, updated_ms, last_artifact_ms);
         CREATE INDEX IF NOT EXISTS idx_turn_log_retention
            ON turn_log(usage_finalized_ms, completed_ms);",
    )?;

    Ok(())
}

fn traceparent_to_string(tp: &Option<bridge_core::ports::TraceParent>) -> Option<String> {
    tp.as_ref().map(|t| t.to_header_value())
}

fn traceparent_from_string(raw: Option<String>) -> Option<bridge_core::ports::TraceParent> {
    raw.as_deref()
        .and_then(bridge_core::ports::TraceParent::parse_header_value)
}

fn row_to_turn_log_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<bridge_core::task_store::TurnLogRow> {
    Ok(bridge_core::task_store::TurnLogRow {
        turn_id: bridge_core::ids::TurnId::parse(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        session_id: bridge_core::ids::ContextId::parse(row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        task_id: row
            .get::<_, Option<String>>(2)?
            .map(bridge_core::ids::TaskId::parse)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        workflow: row.get(3)?,
        node: row.get(4)?,
        attempt: row.get::<_, i64>(5)? as u32,
        agent: row.get(6)?,
        model: row.get(7)?,
        effort: row.get(8)?,
        mode: row.get(9)?,
        prompt_id: row.get(10)?,
        started_ms: row.get(11)?,
        completed_ms: row.get(12)?,
        latency_ms: row.get::<_, Option<i64>>(13)?.map(|v| v as u64),
        ttft_ms: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
        outcome: row.get(15)?,
        failure_class: row.get(16)?,
        input_tokens: row.get::<_, Option<i64>>(17)?.map(|v| v as u64),
        output_tokens: row.get::<_, Option<i64>>(18)?.map(|v| v as u64),
        thought_tokens: row.get::<_, Option<i64>>(19)?.map(|v| v as u64),
        cached_read_tokens: row.get::<_, Option<i64>>(20)?.map(|v| v as u64),
        cached_write_tokens: row.get::<_, Option<i64>>(21)?.map(|v| v as u64),
        cost_amount: row.get(22)?,
        cost_currency: row.get(23)?,
        traceparent: traceparent_from_string(row.get(24)?),
        usage_finalized_ms: row.get(25)?,
        usage_finalization_kind: row.get(26)?,
    })
}

const TURN_LOG_SELECT: &str =
    "SELECT turn_id, session_id, task_id, workflow, node, attempt, agent, model, effort, mode,
        prompt_id, started_ms, completed_ms, latency_ms, ttft_ms, outcome, failure_class,
        input_tokens, output_tokens, thought_tokens, cached_read_tokens, cached_write_tokens,
        cost_amount, cost_currency, traceparent, usage_finalized_ms, usage_finalization_kind
 FROM turn_log";

fn insert_journal_event(
    tx: &rusqlite::Transaction<'_>,
    task: &TaskId,
    event: &bridge_core::orch::OrchEvent,
) -> Result<(), BridgeError> {
    let event_json = serde_json::to_string(event).map_err(|_| BridgeError::StoreFailure)?;
    tx.execute(
        "INSERT INTO task_journal(task_id, seq, event_json) VALUES(?1, ?2, ?3)",
        rusqlite::params![task.as_str(), event.seq, event_json],
    )
    .map_err(|_| BridgeError::StoreFailure)?;
    Ok(())
}

fn bump_last_artifact_sql(
    tx: &rusqlite::Transaction<'_>,
    task: &TaskId,
    artifact_ms: i64,
) -> Result<usize, BridgeError> {
    tx.execute(
        "UPDATE tasks
         SET last_artifact_ms = CASE
             WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
             ELSE last_artifact_ms
         END
         WHERE id=?1",
        rusqlite::params![task.as_str(), artifact_ms],
    )
    .map_err(|_| BridgeError::StoreFailure)
}

fn immediate_transaction(
    conn: &rusqlite::Connection,
) -> Result<rusqlite::Transaction<'_>, BridgeError> {
    rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| BridgeError::StoreFailure)
}

fn batch_status_as_str(status: bridge_core::task_store::BatchStatus) -> &'static str {
    use bridge_core::task_store::BatchStatus;
    match status {
        BatchStatus::Working => "working",
        BatchStatus::Completed => "completed",
        BatchStatus::Canceling => "canceling",
        BatchStatus::Canceled => "canceled",
        BatchStatus::Failed => "failed",
    }
}

fn parse_batch_status(s: &str) -> Option<bridge_core::task_store::BatchStatus> {
    use bridge_core::task_store::BatchStatus;
    match s {
        "working" => Some(BatchStatus::Working),
        "completed" => Some(BatchStatus::Completed),
        "canceling" => Some(BatchStatus::Canceling),
        "canceled" => Some(BatchStatus::Canceled),
        "failed" => Some(BatchStatus::Failed),
        _ => None,
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
            "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms,
                               last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                               journal_complete_from_birth, batch_id, item_id, artifacts_purged_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, ?15)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                rec.status.as_str(),
                rec.result,
                rec.error,
                rec.created_ms,
                rec.updated_ms,
                rec.last_artifact_ms,
                rec.input,
                rec.workflow_spec_json,
                rec.resume_attempts as i64,
                rec.session_cwd,
                rec.batch_id.as_ref().map(|b| b.as_str()),
                rec.item_id,
                rec.artifacts_purged_at
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn create_with_attempt_locator(
        &self,
        rec: &bridge_core::task_store::TaskRecord,
        locator: &bridge_core::task_store::TaskAttemptLocator,
    ) -> Result<(), BridgeError> {
        if !locator.belongs_to(&rec.id) {
            return Err(BridgeError::StoreFailure);
        }
        let json = serde_json::to_string(locator).map_err(|_| BridgeError::StoreFailure)?;
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        tx.execute(
            "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms,
                               last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                               journal_complete_from_birth, batch_id, item_id, artifacts_purged_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, ?15)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                rec.status.as_str(),
                rec.result,
                rec.error,
                rec.created_ms,
                rec.updated_ms,
                rec.last_artifact_ms,
                rec.input,
                rec.workflow_spec_json,
                i64::from(rec.resume_attempts),
                rec.session_cwd,
                rec.batch_id.as_ref().map(|b| b.as_str()),
                rec.item_id,
                rec.artifacts_purged_at
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(?1, ?2, ?3, ?4, 'served_task', 0)",
            rusqlite::params![
                locator.identity.attempt_id.as_str(),
                locator.identity.execution_id.as_str(),
                i64::from(locator.identity.ordinal),
                rec.id.as_str()
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "INSERT INTO task_attempt_locators(task_id, locator_json) VALUES(?1, ?2)",
            rusqlite::params![rec.id.as_str(), json],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)
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
                "UPDATE tasks SET status=?2, result=?3, error=?4, updated_ms=?5,
                    terminal_projection_ready=1, terminal_projection_attempt_id=NULL,
                    terminal_projection_json=NULL WHERE id=?1 AND terminal_projection_ready=1",
                rusqlite::params![
                    id.as_str(),
                    status.as_str(),
                    result,
                    error,
                    durable_retention_ms(updated_ms)
                ],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if n == 0 {
            return Err(BridgeError::StoreFailure);
        }
        Ok(())
    }

    async fn put_attempt_locator(
        &self,
        task: &TaskId,
        locator: &bridge_core::task_store::TaskAttemptLocator,
    ) -> Result<(), BridgeError> {
        if !locator.belongs_to(task) {
            return Err(BridgeError::StoreFailure);
        }
        let json = serde_json::to_string(locator).map_err(|_| BridgeError::StoreFailure)?;
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(?1, ?2, ?3, ?4, 'served_task', 0)",
            rusqlite::params![
                locator.identity.attempt_id.as_str(),
                locator.identity.execution_id.as_str(),
                i64::from(locator.identity.ordinal),
                task.as_str()
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        let changed = tx
            .execute(
                "INSERT INTO task_attempt_locators(task_id, locator_json) VALUES(?1, ?2)",
                rusqlite::params![task.as_str(), json],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if changed != 1 {
            return Err(BridgeError::StoreFailure);
        }
        tx.commit().map_err(|_| BridgeError::StoreFailure)
    }

    async fn mark_attempt_telemetry_unavailable(
        &self,
        task: &TaskId,
        attempt: &bridge_core::ids::AttemptId,
        reason: bridge_core::workflow_history::LedgerUnavailableReason,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT locator_json FROM task_attempt_locators WHERE task_id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut locator: bridge_core::task_store::TaskAttemptLocator =
            serde_json::from_str(&prior.ok_or(BridgeError::StoreFailure)?)
                .map_err(|_| BridgeError::StoreFailure)?;
        if &locator.identity.attempt_id != attempt {
            return Err(BridgeError::StoreFailure);
        }
        locator.telemetry_unavailable = Some(reason);
        let json = serde_json::to_string(&locator).map_err(|_| BridgeError::StoreFailure)?;
        let changed = tx
            .execute(
                "UPDATE task_attempt_locators SET locator_json=?2 WHERE task_id=?1",
                rusqlite::params![task.as_str(), json],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if changed != 1 {
            return Err(BridgeError::StoreFailure);
        }
        tx.commit().map_err(|_| BridgeError::StoreFailure)
    }

    async fn get_attempt_locator(
        &self,
        task: &TaskId,
    ) -> Result<Option<bridge_core::task_store::TaskAttemptLocator>, BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let json: Option<String> = conn
            .query_row(
                "SELECT locator_json FROM task_attempt_locators WHERE task_id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        json.map(|value| serde_json::from_str(&value).map_err(|_| BridgeError::StoreFailure))
            .transpose()
    }

    async fn terminal_attempts_with_telemetry_markers(
        &self,
    ) -> Result<Vec<bridge_core::ids::AttemptId>, BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let mut statement = conn
            .prepare(
                "SELECT t.id, l.locator_json
                 FROM tasks t
                 JOIN task_attempt_locators l ON l.task_id=t.id
                 WHERE t.status IN ('completed','failed','canceled','interrupted')
                   AND t.terminal_projection_ready=1
                 ORDER BY t.id",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut attempts = Vec::new();
        for row in rows {
            let (task_id, locator_json) = row.map_err(|_| BridgeError::StoreFailure)?;
            let task = TaskId::parse(task_id).map_err(|_| BridgeError::StoreFailure)?;
            let locator: bridge_core::task_store::TaskAttemptLocator =
                serde_json::from_str(&locator_json).map_err(|_| BridgeError::StoreFailure)?;
            if !locator.belongs_to(&task) {
                return Err(BridgeError::StoreFailure);
            }
            if locator.telemetry_unavailable.is_some() {
                attempts.push(locator.identity.attempt_id);
            }
        }
        Ok(attempts)
    }

    async fn get(
        &self,
        id: &TaskId,
    ) -> Result<Option<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                        batch_id, item_id, artifacts_purged_at, terminal_projection_ready
                 FROM tasks WHERE id=?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![id.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(None),
            Some(row) => {
                let task = row_to_task(row)?;
                Ok(Some(project_task_record(
                    task,
                    row.get(15).map_err(|_| BridgeError::StoreFailure)?,
                )?))
            }
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
                        last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                        batch_id, item_id, artifacts_purged_at, terminal_projection_ready
                 FROM tasks ORDER BY updated_ms DESC LIMIT ?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![limit as i64])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let task = row_to_task(row)?;
            out.push(project_task_record(
                task,
                row.get(15).map_err(|_| BridgeError::StoreFailure)?,
            )?);
        }
        Ok(out)
    }

    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE tasks SET status='interrupted', error='interrupted (serve restarted)', updated_ms=?1
                 WHERE status='working'",
                rusqlite::params![durable_retention_ms(updated_ms)],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n as u64)
    }
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE tasks SET status='canceled', updated_ms=?1 WHERE id=?2 AND status='working'",
                rusqlite::params![durable_retention_ms(updated_ms), id.as_str()],
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
        let tx = immediate_transaction(&conn)?;
        let artifact_ms = durable_retention_ms((self.now_ms)());
        if bump_last_artifact_sql(&tx, task, artifact_ms)? == 0 {
            return Err(BridgeError::StoreFailure);
        }
        tx.execute(
            "INSERT INTO task_node_checkpoints(task_id, node_id, output, ok, ts)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![task.as_str(), node.as_str(), output, ok as i64, ts],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<
        Vec<(
            NodeId,
            String,
            bool,
            Option<bridge_core::orch::UsageSnapshot>,
        )>,
        BridgeError,
    > {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id, output, ok, usage_json FROM task_node_checkpoints WHERE task_id=?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let node_s: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
            let output: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
            let ok_i: i64 = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
            let usage_s: Option<String> = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
            let usage = usage_s
                .map(|s| serde_json::from_str::<bridge_core::orch::UsageSnapshot>(&s))
                .transpose()
                .map_err(|_| BridgeError::StoreFailure)?;
            let node = NodeId::parse(node_s).map_err(|_| BridgeError::StoreFailure)?;
            out.push((node, output, ok_i != 0, usage));
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
        // `new_unchecked` takes `&Connection`; the enclosing mutex serializes transactions.
        let tx = immediate_transaction(&conn)?;
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

    async fn claim_resume_attempt_with_locator(
        &self,
        task: &TaskId,
        cap: u32,
        now_ms: i64,
        expected: &bridge_core::task_store::TaskAttemptLocator,
        next: &bridge_core::task_store::TaskAttemptLocator,
    ) -> Result<bridge_core::task_store::ResumeClaim, BridgeError> {
        use bridge_core::task_store::ResumeClaim;
        if !expected.belongs_to(task) || !next.is_direct_successor_of(expected) {
            return Err(BridgeError::StoreFailure);
        }
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        let current: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT t.resume_attempts, t.status, l.locator_json
                 FROM tasks t
                 JOIN task_attempt_locators l ON l.task_id=t.id
                 WHERE t.id=?1",
                rusqlite::params![task.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        let (attempts, status, locator_json) = current.ok_or(BridgeError::StoreFailure)?;
        let current_locator: bridge_core::task_store::TaskAttemptLocator =
            serde_json::from_str(&locator_json).map_err(|_| BridgeError::StoreFailure)?;
        if status != "working" || &current_locator != expected {
            return Err(BridgeError::StoreFailure);
        }
        if attempts >= i64::from(cap) {
            tx.commit().map_err(|_| BridgeError::StoreFailure)?;
            return Ok(ResumeClaim::Exhausted);
        }
        let next_attempt = attempts.checked_add(1).ok_or(BridgeError::StoreFailure)?;
        let next_json = serde_json::to_string(next).map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(?1, ?2, ?3, ?4, 'served_task', 0)",
            rusqlite::params![
                next.identity.attempt_id.as_str(),
                next.identity.execution_id.as_str(),
                i64::from(next.identity.ordinal),
                task.as_str()
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        let task_changed = tx
            .execute(
                "UPDATE tasks
                 SET resume_attempts=?2, last_resume_ms=?3, updated_ms=?4
                 WHERE id=?1 AND status='working' AND resume_attempts=?5",
                rusqlite::params![
                    task.as_str(),
                    next_attempt,
                    now_ms,
                    durable_retention_ms(now_ms),
                    attempts
                ],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let locator_changed = tx
            .execute(
                "UPDATE task_attempt_locators SET locator_json=?2
                 WHERE task_id=?1 AND locator_json=?3",
                rusqlite::params![task.as_str(), next_json, locator_json],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if task_changed != 1 || locator_changed != 1 {
            return Err(BridgeError::StoreFailure);
        }
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(ResumeClaim::Resumable {
            attempt: u32::try_from(next_attempt).map_err(|_| BridgeError::StoreFailure)?,
        })
    }

    async fn working_tasks(&self) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                        batch_id, item_id, artifacts_purged_at
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

    async fn upsert_turn_finished(
        &self,
        row: &bridge_core::task_store::TurnLogFinished,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        let artifact_ms = durable_retention_ms((self.now_ms)());
        if let Some(task) = row.ctx.task_id.as_ref() {
            let _ = bump_last_artifact_sql(&tx, task, artifact_ms)?;
        }
        let (outcome, failure_class) =
            bridge_core::task_store::turn_log_outcome_strings(&row.outcome);
        tx.execute(
            "INSERT INTO turn_log(
                turn_id, session_id, task_id, workflow, node, attempt, agent, model, effort, mode,
                prompt_id, started_ms, completed_ms, latency_ms, ttft_ms, outcome, failure_class,
                traceparent, usage_finalization_kind
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 'pending')
             ON CONFLICT(turn_id) DO UPDATE SET
                session_id=excluded.session_id,
                task_id=excluded.task_id,
                workflow=excluded.workflow,
                node=excluded.node,
                attempt=excluded.attempt,
                agent=excluded.agent,
                model=excluded.model,
                effort=excluded.effort,
                mode=excluded.mode,
                prompt_id=excluded.prompt_id,
                started_ms=excluded.started_ms,
                completed_ms=excluded.completed_ms,
                latency_ms=excluded.latency_ms,
                ttft_ms=excluded.ttft_ms,
                outcome=excluded.outcome,
                failure_class=excluded.failure_class,
                traceparent=excluded.traceparent",
            rusqlite::params![
                row.ctx.turn_id.as_str(),
                row.ctx.session_id.as_str(),
                row.ctx.task_id.as_ref().map(|t| t.as_str()),
                row.ctx.workflow,
                row.ctx.node,
                row.ctx.attempt as i64,
                row.ctx.agent,
                row.ctx.model,
                row.ctx.effort,
                row.ctx.mode,
                row.ctx.prompt_id,
                row.started_ms,
                row.completed_ms,
                row.latency.as_millis() as i64,
                row.ttft.map(|d| d.as_millis() as i64),
                outcome,
                failure_class,
                traceparent_to_string(&row.ctx.traceparent),
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn finalize_turn_usage(
        &self,
        row: &bridge_core::task_store::TurnLogFinalized,
    ) -> Result<(), BridgeError> {
        use bridge_core::task_store::TurnUsageFinalization;

        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        let persistence_ms = durable_retention_ms((self.now_ms)());
        let (expected_kind, n) = match &row.finalization {
            TurnUsageFinalization::Usage(usage) => {
                let term = usage.terminal.as_ref();
                let cost = usage.cost.as_ref();
                let n = tx
                    .execute(
                        "UPDATE turn_log SET
                            input_tokens=COALESCE(?2, input_tokens),
                            output_tokens=COALESCE(?3, output_tokens),
                            thought_tokens=COALESCE(?4, thought_tokens),
                            cached_read_tokens=COALESCE(?5, cached_read_tokens),
                            cached_write_tokens=COALESCE(?6, cached_write_tokens),
                            cost_amount=COALESCE(?7, cost_amount),
                            cost_currency=COALESCE(?8, cost_currency),
                            usage_finalized_ms=?9,
                            usage_finalization_kind='usage'
                         WHERE turn_id=?1
                           AND completed_ms IS NOT NULL
                           AND usage_finalized_ms IS NULL",
                        rusqlite::params![
                            row.ctx.turn_id.as_str(),
                            term.map(|t| t.input_tokens as i64),
                            term.map(|t| t.output_tokens as i64),
                            term.and_then(|t| t.thought_tokens).map(|v| v as i64),
                            term.and_then(|t| t.cached_read_tokens).map(|v| v as i64),
                            term.and_then(|t| t.cached_write_tokens).map(|v| v as i64),
                            cost.map(|c| c.amount),
                            cost.map(|c| c.currency.as_str()),
                            persistence_ms,
                        ],
                    )
                    .map_err(|_| BridgeError::StoreFailure)?;
                ("usage", n)
            }
            TurnUsageFinalization::NoUsage => {
                let n = tx
                    .execute(
                        "UPDATE turn_log SET
                            usage_finalized_ms=?2,
                            usage_finalization_kind='no_usage'
                         WHERE turn_id=?1
                           AND completed_ms IS NOT NULL
                           AND usage_finalized_ms IS NULL
                           AND input_tokens IS NULL
                           AND output_tokens IS NULL
                           AND thought_tokens IS NULL
                           AND cached_read_tokens IS NULL
                           AND cached_write_tokens IS NULL
                           AND cost_amount IS NULL
                           AND cost_currency IS NULL",
                        rusqlite::params![row.ctx.turn_id.as_str(), persistence_ms],
                    )
                    .map_err(|_| BridgeError::StoreFailure)?;
                ("no_usage", n)
            }
        };

        if n == 0 {
            let current: Option<String> = tx
                .query_row(
                    "SELECT usage_finalization_kind FROM turn_log
                     WHERE turn_id=?1 AND usage_finalized_ms IS NOT NULL",
                    rusqlite::params![row.ctx.turn_id.as_str()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| BridgeError::StoreFailure)?;
            if current.as_deref() == Some(expected_kind) {
                tx.commit().map_err(|_| BridgeError::StoreFailure)?;
                return Ok(());
            }
            return Err(BridgeError::StoreFailure);
        }

        let task_id: Option<String> = tx
            .query_row(
                "SELECT task_id FROM turn_log WHERE turn_id=?1",
                rusqlite::params![row.ctx.turn_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?
            .flatten();
        if let Some(task_id) = task_id {
            let task = TaskId::parse(task_id).map_err(|_| BridgeError::StoreFailure)?;
            let _ = bump_last_artifact_sql(&tx, &task, persistence_ms)?;
        }
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn turn_log_rows(&self) -> Result<Vec<bridge_core::task_store::TurnLogRow>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("{TURN_LOG_SELECT} ORDER BY turn_id");
        let mut stmt = conn.prepare(&sql).map_err(|_| BridgeError::StoreFailure)?;
        let rows = stmt
            .query_map([], row_to_turn_log_row)
            .map_err(|_| BridgeError::StoreFailure)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(rows)
    }

    async fn turn_log_row(
        &self,
        turn_id: &bridge_core::ids::TurnId,
    ) -> Result<Option<bridge_core::task_store::TurnLogRow>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("{TURN_LOG_SELECT} WHERE turn_id=?1");
        conn.query_row(
            &sql,
            rusqlite::params![turn_id.as_str()],
            row_to_turn_log_row,
        )
        .optional()
        .map_err(|_| BridgeError::StoreFailure)
    }

    async fn turn_log_rows_for_task(
        &self,
        task: &TaskId,
        limit: usize,
    ) -> Result<Vec<bridge_core::task_store::TurnLogRow>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let sql =
            format!("{TURN_LOG_SELECT} WHERE task_id=?1 ORDER BY completed_ms, turn_id LIMIT ?2");
        let mut stmt = conn.prepare(&sql).map_err(|_| BridgeError::StoreFailure)?;
        let rows = stmt
            .query_map(
                rusqlite::params![task.as_str(), limit as i64],
                row_to_turn_log_row,
            )
            .map_err(|_| BridgeError::StoreFailure)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(rows)
    }

    async fn turn_log_usage_for_task(
        &self,
        task: &TaskId,
    ) -> Result<Option<bridge_core::task_store::TaskUsageAgg>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let (
            rows,
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
            sum_cost_amount,
            distinct_currency_count,
            min_cost_currency,
            at_ms,
        ): (
            i64,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            i64,
            Option<String>,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    SUM(thought_tokens),
                    SUM(cached_read_tokens),
                    SUM(cached_write_tokens),
                    SUM(cost_amount),
                    COUNT(DISTINCT cost_currency),
                    MIN(cost_currency),
                    MAX(completed_ms)
             FROM turn_log WHERE task_id=?1",
                rusqlite::params![task.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .map_err(|_| BridgeError::StoreFailure)?;

        if rows == 0 {
            return Ok(None);
        }

        let cost = if distinct_currency_count == 1 {
            match (sum_cost_amount, min_cost_currency) {
                (Some(amount), Some(currency)) => {
                    Some(bridge_core::orch::UsageCost { amount, currency })
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Some(bridge_core::task_store::TaskUsageAgg {
            rows: rows as u64,
            input_tokens: input_tokens as u64,
            output_tokens: output_tokens as u64,
            thought_tokens: thought_tokens.map(|v| v as u64),
            cached_read_tokens: cached_read_tokens.map(|v| v as u64),
            cached_write_tokens: cached_write_tokens.map(|v| v as u64),
            cost,
            at_ms: at_ms.unwrap_or(0),
        }))
    }

    async fn latest_turn_log_row_for_session(
        &self,
        session: &bridge_core::ids::ContextId,
    ) -> Result<Option<bridge_core::task_store::TurnLogRow>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "{TURN_LOG_SELECT} WHERE session_id=?1 ORDER BY completed_ms DESC, turn_id DESC LIMIT 1"
        );
        conn.query_row(
            &sql,
            rusqlite::params![session.as_str()],
            row_to_turn_log_row,
        )
        .optional()
        .map_err(|_| BridgeError::StoreFailure)
    }

    async fn create_batch(
        &self,
        rec: &bridge_core::task_store::BatchRecord,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO batch(id, workflow, concurrency, total, status, items_json, error,
                               created_ms, updated_ms)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                rec.concurrency as i64,
                rec.total as i64,
                batch_status_as_str(rec.status),
                rec.items_json,
                rec.error,
                rec.created_ms,
                rec.updated_ms
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        Ok(())
    }

    async fn get_batch(
        &self,
        id: &bridge_core::ids::BatchId,
    ) -> Result<Option<bridge_core::task_store::BatchRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, concurrency, total, status, items_json, error,
                        created_ms, updated_ms
                 FROM batch WHERE id=?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![id.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        match rows.next().map_err(|_| BridgeError::StoreFailure)? {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_batch(row)?)),
        }
    }

    async fn list_batches(
        &self,
        limit: usize,
    ) -> Result<Vec<bridge_core::task_store::BatchRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, concurrency, total, status, items_json, error,
                        created_ms, updated_ms
                 FROM batch ORDER BY updated_ms DESC LIMIT ?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![limit as i64])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            out.push(row_to_batch(row)?);
        }
        Ok(out)
    }

    async fn active_batches(
        &self,
    ) -> Result<Vec<bridge_core::task_store::BatchRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, concurrency, total, status, items_json, error,
                        created_ms, updated_ms
                 FROM batch WHERE status IN ('working','canceling')",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt.query([]).map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            out.push(row_to_batch(row)?);
        }
        Ok(out)
    }

    async fn batch_children(
        &self,
        id: &bridge_core::ids::BatchId,
    ) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                        batch_id, item_id, artifacts_purged_at, terminal_projection_ready
                 FROM tasks WHERE batch_id=?1",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![id.as_str()])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let task = row_to_task(row)?;
            out.push(project_task_record(
                task,
                row.get(15).map_err(|_| BridgeError::StoreFailure)?,
            )?);
        }
        Ok(out)
    }

    async fn claim_batch_child(
        &self,
        batch: &bridge_core::ids::BatchId,
        item: &str,
        rec: &bridge_core::task_store::TaskRecord,
    ) -> Result<bridge_core::task_store::ChildClaim, BridgeError> {
        use bridge_core::task_store::{ChildClaim, TaskRecordStatus};

        let conn = self.conn.lock().unwrap();
        conn.execute_batch("BEGIN IMMEDIATE;")
            .map_err(|_| BridgeError::StoreFailure)?;
        let result = (|| -> rusqlite::Result<ChildClaim> {
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, status FROM tasks WHERE batch_id=?1 AND item_id=?2",
                    rusqlite::params![batch.as_str(), item],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((_id, status)) = existing {
                conn.execute_batch("COMMIT;")?;
                return Ok(if status == TaskRecordStatus::Working.as_str() {
                    ChildClaim::ExistingWorking
                } else {
                    ChildClaim::ExistingTerminal
                });
            }

            conn.execute(
                "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms,
                                   last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                                   journal_complete_from_birth, batch_id, item_id, artifacts_purged_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, ?15)",
                rusqlite::params![
                    rec.id.as_str(),
                    rec.workflow,
                    TaskRecordStatus::Working.as_str(),
                    rec.result,
                    rec.error,
                    rec.created_ms,
                    rec.updated_ms,
                    rec.last_artifact_ms,
                    rec.input,
                    rec.workflow_spec_json,
                    rec.resume_attempts as i64,
                    rec.session_cwd,
                    batch.as_str(),
                    item,
                    rec.artifacts_purged_at
                ],
            )?;
            conn.execute_batch("COMMIT;")?;
            Ok(ChildClaim::Created)
        })();

        match result {
            Ok(claim) => Ok(claim),
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(BridgeError::StoreFailure)
            }
        }
    }

    async fn claim_batch_child_with_locator(
        &self,
        batch: &bridge_core::ids::BatchId,
        item: &str,
        rec: &bridge_core::task_store::TaskRecord,
        locator: &bridge_core::task_store::TaskAttemptLocator,
    ) -> Result<bridge_core::task_store::ChildClaim, BridgeError> {
        use bridge_core::task_store::{ChildClaim, TaskRecordStatus};
        if !locator.belongs_to(&rec.id) {
            return Err(BridgeError::StoreFailure);
        }
        let locator_json = serde_json::to_string(locator).map_err(|_| BridgeError::StoreFailure)?;
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT status FROM tasks WHERE batch_id=?1 AND item_id=?2",
                rusqlite::params![batch.as_str(), item],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        if let Some(status) = existing {
            tx.commit().map_err(|_| BridgeError::StoreFailure)?;
            return Ok(if status == TaskRecordStatus::Working.as_str() {
                ChildClaim::ExistingWorking
            } else {
                ChildClaim::ExistingTerminal
            });
        }
        tx.execute(
            "INSERT INTO tasks(id, workflow, status, result, error, created_ms, updated_ms,
                               last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                               journal_complete_from_birth, batch_id, item_id, artifacts_purged_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, ?15)",
            rusqlite::params![
                rec.id.as_str(),
                rec.workflow,
                TaskRecordStatus::Working.as_str(),
                rec.result,
                rec.error,
                rec.created_ms,
                rec.updated_ms,
                rec.last_artifact_ms,
                rec.input,
                rec.workflow_spec_json,
                i64::from(rec.resume_attempts),
                rec.session_cwd,
                batch.as_str(),
                item,
                rec.artifacts_purged_at
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "INSERT INTO attempt_identities(
                 attempt_id, execution_id, ordinal, task_id, owner_surface, summary_attached)
             VALUES(?1, ?2, ?3, ?4, 'served_task', 0)",
            rusqlite::params![
                locator.identity.attempt_id.as_str(),
                locator.identity.execution_id.as_str(),
                i64::from(locator.identity.ordinal),
                rec.id.as_str()
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "INSERT INTO task_attempt_locators(task_id, locator_json) VALUES(?1, ?2)",
            rusqlite::params![rec.id.as_str(), locator_json],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(ChildClaim::Created)
    }

    async fn cancel_batch_if_working(
        &self,
        id: &bridge_core::ids::BatchId,
        ts: i64,
    ) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE batch SET status='canceling', updated_ms=?1
                 WHERE id=?2 AND status='working'",
                rusqlite::params![durable_retention_ms(ts), id.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n > 0)
    }

    async fn settle_batch_if_status(
        &self,
        id: &bridge_core::ids::BatchId,
        expect: bridge_core::task_store::BatchStatus,
        new: bridge_core::task_store::BatchStatus,
        ts: i64,
    ) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE batch SET status=?1, updated_ms=?2 WHERE id=?3 AND status=?4",
                rusqlite::params![
                    batch_status_as_str(new),
                    durable_retention_ms(ts),
                    id.as_str(),
                    batch_status_as_str(expect)
                ],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n > 0)
    }

    async fn fail_batch_if_status(
        &self,
        id: &bridge_core::ids::BatchId,
        expect: bridge_core::task_store::BatchStatus,
        error: &str,
        ts: i64,
    ) -> Result<bool, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute(
                "UPDATE batch SET status='failed', error=?1, updated_ms=?2
                 WHERE id=?3 AND status=?4",
                rusqlite::params![
                    error,
                    durable_retention_ms(ts),
                    id.as_str(),
                    batch_status_as_str(expect)
                ],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        Ok(n > 0)
    }

    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        // Allocate seq by bumping last_event_seq.
        let n = tx
            .execute(
                "UPDATE tasks SET
                    last_event_seq = last_event_seq + 1,
                    last_artifact_ms = CASE
                        WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
                        ELSE last_artifact_ms
                    END
                 WHERE id=?1",
                rusqlite::params![task.as_str(), durable_retention_ms((self.now_ms)())],
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
        let event = bridge_core::orch::OrchEvent {
            v: bridge_core::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: bridge_core::orch::OrchEventKind::NodeStarted {
                node: node.as_str().to_string(),
            },
        };
        insert_journal_event(&tx, task, &event)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        output: &str,
        ok: bool,
        ts: i64,
        usage: Option<&bridge_core::orch::UsageSnapshot>,
    ) -> Result<i64, BridgeError> {
        let usage_json = usage
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| BridgeError::StoreFailure)?;
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        // Allocate seq.
        let n = tx
            .execute(
                "UPDATE tasks SET
                    last_event_seq = last_event_seq + 1,
                    last_artifact_ms = CASE
                        WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
                        ELSE last_artifact_ms
                    END
                 WHERE id=?1",
                rusqlite::params![task.as_str(), durable_retention_ms((self.now_ms)())],
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
            "INSERT INTO task_node_checkpoints(task_id, node_id, output, ok, ts, seq, usage_json)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                task.as_str(),
                node.as_str(),
                output,
                ok as i64,
                ts,
                seq,
                usage_json
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        // Remove the start row — the node is no longer in-progress.
        tx.execute(
            "DELETE FROM task_node_starts WHERE task_id=?1 AND node_id=?2",
            rusqlite::params![task.as_str(), node.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        let event = bridge_core::orch::OrchEvent {
            v: bridge_core::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: bridge_core::orch::OrchEventKind::NodeFinished {
                node: node.as_str().to_string(),
                ok,
                output: output.to_string(),
                usage: usage.cloned(),
            },
        };
        insert_journal_event(&tx, task, &event)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        operation_id: &OperationId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        // Allocate seq.
        let n = tx
            .execute(
                "UPDATE tasks SET
                    last_event_seq = last_event_seq + 1,
                    last_artifact_ms = CASE
                        WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
                        ELSE last_artifact_ms
                    END
                 WHERE id=?1 AND terminal_projection_ready=1",
                rusqlite::params![task.as_str(), durable_retention_ms((self.now_ms)())],
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
            "UPDATE tasks SET status=?2, result=?3, error=?4, updated_ms=?5, terminal_seq=?6,
                terminal_projection_ready=1, terminal_projection_attempt_id=NULL,
                terminal_projection_json=NULL WHERE id=?1",
            rusqlite::params![
                task.as_str(),
                status.as_str(),
                result,
                error,
                durable_retention_ms(ts),
                seq
            ],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        // Clear all start rows for this task.
        tx.execute(
            "DELETE FROM task_node_starts WHERE task_id=?1",
            rusqlite::params![task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        let event = bridge_core::orch::OrchEvent {
            v: bridge_core::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: bridge_core::orch::OrchEventKind::Terminal {
                status: bridge_core::task_store::terminal_status_from_record(&status),
                output: result.or(error).unwrap_or("").to_string(),
            },
        };
        insert_journal_event(&tx, task, &event)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    #[allow(clippy::too_many_arguments)]
    async fn set_terminal_sequenced_pending(
        &self,
        task: &TaskId,
        operation_id: &OperationId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
        attempt_id: &bridge_core::ids::AttemptId,
        terminal: &bridge_core::workflow_history::AttemptTerminal,
    ) -> Result<i64, BridgeError> {
        if !status.is_terminal() || terminal.validate().is_err() {
            return Err(BridgeError::StoreFailure);
        }
        let terminal_json =
            serde_json::to_string(terminal).map_err(|_| BridgeError::StoreFailure)?;
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let tx = immediate_transaction(&conn)?;
        let current: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            Option<String>,
            Option<String>,
            String,
        )> = tx
            .query_row(
                "SELECT t.status, t.result, t.error, t.terminal_seq,
                        t.terminal_projection_ready, t.terminal_projection_attempt_id,
                        t.terminal_projection_json, l.locator_json
                 FROM tasks t
                 JOIN task_attempt_locators l ON l.task_id=t.id
                 WHERE t.id=?1",
                rusqlite::params![task.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;
        let (
            current_status,
            current_result,
            current_error,
            current_terminal_seq,
            ready,
            pending_attempt,
            pending_json,
            locator_json,
        ) = current.ok_or(BridgeError::StoreFailure)?;
        if !matches!(ready, 0 | 1) {
            return Err(BridgeError::StoreFailure);
        }
        let locator: bridge_core::task_store::TaskAttemptLocator =
            serde_json::from_str(&locator_json).map_err(|_| BridgeError::StoreFailure)?;
        if !locator.belongs_to(task) || locator.identity.attempt_id != *attempt_id {
            return Err(BridgeError::StoreFailure);
        }
        if ready == 0 {
            let prior: bridge_core::workflow_history::AttemptTerminal =
                serde_json::from_str(pending_json.as_deref().ok_or(BridgeError::StoreFailure)?)
                    .map_err(|_| BridgeError::StoreFailure)?;
            let compatible = current_status == status.as_str()
                && current_result.as_deref() == result
                && current_error.as_deref() == error
                && pending_attempt.as_deref() == Some(attempt_id.as_str())
                && prior == *terminal;
            tx.commit().map_err(|_| BridgeError::StoreFailure)?;
            return compatible
                .then_some(current_terminal_seq.ok_or(BridgeError::StoreFailure)?)
                .ok_or(BridgeError::StoreFailure);
        }
        if current_status != "working"
            || current_terminal_seq.is_some()
            || pending_attempt.is_some()
            || pending_json.is_some()
        {
            return Err(BridgeError::StoreFailure);
        }

        let changed = tx
            .execute(
                "UPDATE tasks SET
                    last_event_seq=last_event_seq+2,
                    last_artifact_ms=CASE
                        WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
                        ELSE last_artifact_ms
                    END,
                    status=?3, result=?4, error=?5, updated_ms=?6,
                    terminal_seq=last_event_seq+2,
                    terminal_projection_ready=0,
                    terminal_projection_attempt_id=?7,
                    terminal_projection_json=?8
                 WHERE id=?1 AND status='working' AND terminal_projection_ready=1",
                rusqlite::params![
                    task.as_str(),
                    durable_retention_ms((self.now_ms)()),
                    status.as_str(),
                    result,
                    error,
                    durable_retention_ms(ts),
                    attempt_id.as_str(),
                    terminal_json,
                ],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if changed != 1 {
            return Err(BridgeError::StoreFailure);
        }
        let seq: i64 = tx
            .query_row(
                "SELECT terminal_seq FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        tx.execute(
            "DELETE FROM task_node_starts WHERE task_id=?1",
            rusqlite::params![task.as_str()],
        )
        .map_err(|_| BridgeError::StoreFailure)?;
        let event = bridge_core::orch::OrchEvent {
            v: bridge_core::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: bridge_core::orch::OrchEventKind::Terminal {
                status: bridge_core::task_store::terminal_status_from_record(&status),
                output: result.or(error).unwrap_or("").to_owned(),
            },
        };
        insert_journal_event(&tx, task, &event)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn pending_terminal_projection(
        &self,
        task: &TaskId,
    ) -> Result<Option<bridge_core::task_store::PendingTerminalProjection>, BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        conn.query_row(
            "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                    last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                    batch_id, item_id, artifacts_purged_at, terminal_seq,
                    terminal_projection_attempt_id, terminal_projection_json
             FROM tasks WHERE id=?1 AND terminal_projection_ready=0",
            rusqlite::params![task.as_str()],
            row_to_pending_terminal_projection,
        )
        .optional()
        .map_err(|_| BridgeError::StoreFailure)?
        .transpose()
    }

    async fn pending_terminal_projections(
        &self,
    ) -> Result<Vec<bridge_core::task_store::PendingTerminalProjection>, BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let mut statement = conn
            .prepare(
                "SELECT id, workflow, status, result, error, created_ms, updated_ms,
                        last_artifact_ms, input, workflow_spec_json, resume_attempts, session_cwd,
                        batch_id, item_id, artifacts_purged_at, terminal_seq,
                        terminal_projection_attempt_id, terminal_projection_json
                 FROM tasks WHERE terminal_projection_ready=0 ORDER BY id",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let rows = statement
            .query_map([], row_to_pending_terminal_projection)
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut pending = Vec::new();
        for row in rows {
            pending.push(row.map_err(|_| BridgeError::StoreFailure)??);
        }
        Ok(pending)
    }

    async fn mark_terminal_projection_ready(
        &self,
        task: &TaskId,
        attempt_id: &bridge_core::ids::AttemptId,
    ) -> Result<(), BridgeError> {
        let conn = self.conn.lock().map_err(|_| BridgeError::StoreFailure)?;
        let changed = conn
            .execute(
                "UPDATE tasks SET terminal_projection_ready=1,
                    terminal_projection_attempt_id=NULL, terminal_projection_json=NULL
                 WHERE id=?1 AND terminal_projection_ready=0
                   AND terminal_projection_attempt_id=?2",
                rusqlite::params![task.as_str(), attempt_id.as_str()],
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        if changed != 1 {
            return Err(BridgeError::StoreFailure);
        }
        Ok(())
    }

    async fn record_event_sequenced(
        &self,
        task: &TaskId,
        op: &OperationId,
        ts: i64,
        kind: bridge_core::orch::OrchEventKind,
    ) -> Result<i64, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        let n = tx
            .execute(
                "UPDATE tasks SET
                    last_event_seq = last_event_seq + 1,
                    last_artifact_ms = CASE
                        WHEN last_artifact_ms IS NULL OR last_artifact_ms < ?2 THEN ?2
                        ELSE last_artifact_ms
                    END
                 WHERE id=?1",
                rusqlite::params![task.as_str(), durable_retention_ms((self.now_ms)())],
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
        let event = bridge_core::orch::OrchEvent {
            v: bridge_core::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: op.clone(),
            session: None,
            source: None,
            kind,
        };
        insert_journal_event(&tx, task, &event)?;
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(seq)
    }

    async fn journal_from(
        &self,
        task: &TaskId,
        after_seq: i64,
    ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let pending_terminal_seq = terminal_projection_boundary(&conn, task)?;
        let mut stmt = conn
            .prepare(
                "SELECT seq, event_json FROM task_journal
                 WHERE task_id=?1 AND seq>?2 ORDER BY seq",
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut rows = stmt
            .query(rusqlite::params![task.as_str(), after_seq])
            .map_err(|_| BridgeError::StoreFailure)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
            let seq: i64 = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
            let event_json: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
            if pending_terminal_seq.is_some_and(|terminal| seq >= terminal) {
                continue;
            }
            let mut event: bridge_core::orch::OrchEvent =
                serde_json::from_str(&event_json).map_err(|_| BridgeError::StoreFailure)?;
            event.seq = seq;
            out.push(event);
        }
        Ok(out)
    }

    async fn journal_fold_inputs(
        &self,
        task: &TaskId,
    ) -> Result<bridge_core::task_store::JournalFoldInputs, BridgeError> {
        use bridge_core::task_store::{JournalFoldInputs, JournalScalars, TaskRecordStatus};
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        let (
            mut status_s,
            mut result,
            mut error,
            mut terminal_seq,
            mut cut_seq,
            complete_from_birth,
            terminal_projection_ready,
        ): (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            i64,
            i64,
        ) = tx
            .query_row(
                "SELECT status, result, error, terminal_seq, last_event_seq,
                        journal_complete_from_birth, terminal_projection_ready
                 FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        let pending_terminal_seq = match terminal_projection_ready {
            1 => None,
            0 => {
                let seq = terminal_seq.ok_or(BridgeError::StoreFailure)?;
                status_s = TaskRecordStatus::Working.as_str().to_owned();
                result = None;
                error = None;
                terminal_seq = None;
                cut_seq = seq.saturating_sub(1);
                Some(seq)
            }
            _ => return Err(BridgeError::StoreFailure),
        };

        let scalars = JournalScalars {
            status: TaskRecordStatus::parse(&status_s).ok_or(BridgeError::StoreFailure)?,
            result,
            error,
            terminal_seq,
            cut_seq,
        };
        let events = {
            let mut stmt = tx
                .prepare(
                    "SELECT seq, event_json FROM task_journal
                     WHERE task_id=?1 ORDER BY seq",
                )
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut rows = stmt
                .query(rusqlite::params![task.as_str()])
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
                let seq: i64 = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
                let event_json: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
                if pending_terminal_seq.is_some_and(|terminal| seq >= terminal) {
                    continue;
                }
                let mut event: bridge_core::orch::OrchEvent =
                    serde_json::from_str(&event_json).map_err(|_| BridgeError::StoreFailure)?;
                event.seq = seq;
                out.push(event);
            }
            out
        };
        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(JournalFoldInputs {
            complete_from_birth: complete_from_birth != 0,
            scalars,
            events,
        })
    }

    async fn journal_jsonl_bounded(
        &self,
        task: &TaskId,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<bridge_core::task_store::JournalRead, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let tx = immediate_transaction(&conn)?;
        let pending_terminal_seq = terminal_projection_boundary(&tx, task)?;

        let (events, bytes): (i64, i64) = tx
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(event_json AS BLOB))+1),0)
                 FROM task_journal WHERE task_id=?1 AND (?2 IS NULL OR seq < ?2)",
                rusqlite::params![task.as_str(), pending_terminal_seq],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| BridgeError::StoreFailure)?;

        if events as usize > max_events || bytes as usize > max_bytes {
            tx.commit().map_err(|_| BridgeError::StoreFailure)?;
            return Ok(bridge_core::task_store::JournalRead::TooLarge {
                events: events as u64,
                bytes: bytes as u64,
            });
        }

        let jsonl = {
            let mut stmt = tx
                .prepare(
                    "SELECT seq, event_json FROM task_journal
                 WHERE task_id=?1 AND (?2 IS NULL OR seq < ?2) ORDER BY seq",
                )
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut rows = stmt
                .query(rusqlite::params![task.as_str(), pending_terminal_seq])
                .map_err(|_| BridgeError::StoreFailure)?;
            let mut out = String::with_capacity(bytes as usize);
            while let Some(row) = rows.next().map_err(|_| BridgeError::StoreFailure)? {
                let event_json: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
                out.push_str(&event_json);
                out.push('\n');
            }
            out
        };

        tx.commit().map_err(|_| BridgeError::StoreFailure)?;
        Ok(bridge_core::task_store::JournalRead::Body {
            jsonl,
            events: events as u64,
            bytes: bytes as u64,
        })
    }

    async fn node_checkpoint_nodes(&self, task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id FROM task_node_checkpoints
                 WHERE task_id=?1 ORDER BY COALESCE(seq, 0), ts, node_id",
            )
            .map_err(|_| BridgeError::StoreFailure)?;

        let nodes = stmt
            .query_map(rusqlite::params![task.as_str()], |row| {
                let raw: String = row.get(0)?;
                NodeId::parse(raw).map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .map_err(|_| BridgeError::StoreFailure)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| BridgeError::StoreFailure)?;

        Ok(nodes)
    }

    async fn node_checkpoint_output(
        &self,
        task: &TaskId,
        node: &NodeId,
        max_bytes: usize,
    ) -> Result<Option<bridge_core::task_store::NodeCheckpointOutput>, BridgeError> {
        // Saturate to i64::MAX: SQLite binds i64, and `max_bytes as i64` would wrap a huge
        // configured cap (usize near/above i64::MAX) to a negative value, making the
        // `<= ?3` gate reject every artifact. `[traces].artifact_max_bytes` only rejects 0.
        let max_bytes_i64 = i64::try_from(max_bytes).unwrap_or(i64::MAX);
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT
                (CASE WHEN length(CAST(output AS BLOB)) <= ?3 THEN output END),
                ok,
                usage_json,
                length(CAST(output AS BLOB))
             FROM task_node_checkpoints
             WHERE task_id=?1 AND node_id=?2",
                rusqlite::params![task.as_str(), node.as_str(), max_bytes_i64],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| BridgeError::StoreFailure)?;

        let Some((output, ok, usage_json, bytes)) = row else {
            return Ok(None);
        };

        if output.is_none() {
            return Ok(Some(
                bridge_core::task_store::NodeCheckpointOutput::TooLarge {
                    bytes: bytes as u64,
                },
            ));
        }

        let usage = usage_json
            .as_deref()
            .map(serde_json::from_str::<bridge_core::orch::UsageSnapshot>)
            .transpose()
            .map_err(|_| BridgeError::StoreFailure)?;

        Ok(Some(bridge_core::task_store::NodeCheckpointOutput::Found {
            output: output.unwrap(),
            ok: ok != 0,
            usage,
            bytes: bytes as u64,
        }))
    }

    async fn progress_snapshot(
        &self,
        task: &TaskId,
    ) -> Result<bridge_core::task_store::TaskProgressSnapshot, BridgeError> {
        use bridge_core::task_store::TaskProgressSnapshot;
        let conn = self.conn.lock().unwrap();
        // Use a transaction for a consistent read so cut_seq is exact.
        let tx = immediate_transaction(&conn)?;
        // Read task row: status, result, error, terminal_seq, last_event_seq.
        let (
            mut status_s,
            mut result,
            mut error,
            mut terminal_seq,
            mut cut_seq,
            terminal_projection_ready,
        ): (
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            i64,
        ) = tx
            .query_row(
                "SELECT status, result, error, terminal_seq, last_event_seq,
                        terminal_projection_ready FROM tasks WHERE id=?1",
                rusqlite::params![task.as_str()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(|_| BridgeError::StoreFailure)?;
        match terminal_projection_ready {
            1 => {}
            0 => {
                let seq = terminal_seq.ok_or(BridgeError::StoreFailure)?;
                status_s = bridge_core::task_store::TaskRecordStatus::Working
                    .as_str()
                    .to_owned();
                result = None;
                error = None;
                terminal_seq = None;
                cut_seq = seq.saturating_sub(1);
            }
            _ => return Err(BridgeError::StoreFailure),
        }
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

fn terminal_projection_boundary(
    conn: &rusqlite::Connection,
    task: &TaskId,
) -> Result<Option<i64>, BridgeError> {
    let (ready, terminal_seq): (i64, Option<i64>) = conn
        .query_row(
            "SELECT terminal_projection_ready, terminal_seq FROM tasks WHERE id=?1",
            rusqlite::params![task.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| BridgeError::StoreFailure)?;
    match ready {
        1 => Ok(None),
        0 => Ok(Some(terminal_seq.ok_or(BridgeError::StoreFailure)?)),
        _ => Err(BridgeError::StoreFailure),
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
    let last_artifact_ms: Option<i64> = row.get(7).map_err(|_| BridgeError::StoreFailure)?;
    let input: Option<String> = row.get(8).map_err(|_| BridgeError::StoreFailure)?;
    let workflow_spec_json: Option<String> = row.get(9).map_err(|_| BridgeError::StoreFailure)?;
    let resume_attempts: Option<i64> = row.get(10).map_err(|_| BridgeError::StoreFailure)?;
    let session_cwd: Option<String> = row.get(11).map_err(|_| BridgeError::StoreFailure)?;
    let batch_id: Option<String> = row.get(12).map_err(|_| BridgeError::StoreFailure)?;
    let item_id: Option<String> = row.get(13).map_err(|_| BridgeError::StoreFailure)?;
    let artifacts_purged_at: Option<i64> = row.get(14).map_err(|_| BridgeError::StoreFailure)?;
    Ok(TaskRecord {
        id: TaskId::parse(id).map_err(|_| BridgeError::StoreFailure)?,
        workflow,
        status: TaskRecordStatus::parse(&status_s).ok_or(BridgeError::StoreFailure)?,
        result,
        error,
        created_ms,
        updated_ms,
        last_artifact_ms,
        input: input.unwrap_or_default(),
        workflow_spec_json,
        resume_attempts: resume_attempts.unwrap_or(0) as u32,
        session_cwd,
        batch_id: batch_id
            .map(bridge_core::ids::BatchId::parse)
            .transpose()
            .map_err(|_| BridgeError::StoreFailure)?,
        item_id,
        artifacts_purged_at,
    })
}

fn project_task_record(
    mut task: bridge_core::task_store::TaskRecord,
    terminal_projection_ready: i64,
) -> Result<bridge_core::task_store::TaskRecord, BridgeError> {
    match terminal_projection_ready {
        1 => Ok(task),
        0 => {
            task.status = bridge_core::task_store::TaskRecordStatus::Working;
            task.result = None;
            task.error = None;
            Ok(task)
        }
        _ => Err(BridgeError::StoreFailure),
    }
}

fn row_to_pending_terminal_projection(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<bridge_core::task_store::PendingTerminalProjection, BridgeError>> {
    Ok((|| {
        let task = row_to_task(row)?;
        if !task.status.is_terminal() {
            return Err(BridgeError::StoreFailure);
        }
        let terminal_seq: Option<i64> = row.get(15).map_err(|_| BridgeError::StoreFailure)?;
        let attempt_id: Option<String> = row.get(16).map_err(|_| BridgeError::StoreFailure)?;
        let terminal_json: Option<String> = row.get(17).map_err(|_| BridgeError::StoreFailure)?;
        let attempt_id =
            bridge_core::ids::AttemptId::parse(attempt_id.ok_or(BridgeError::StoreFailure)?)
                .map_err(|_| BridgeError::StoreFailure)?;
        let terminal: bridge_core::workflow_history::AttemptTerminal =
            serde_json::from_str(terminal_json.as_deref().ok_or(BridgeError::StoreFailure)?)
                .map_err(|_| BridgeError::StoreFailure)?;
        terminal.validate().map_err(|_| BridgeError::StoreFailure)?;
        Ok(bridge_core::task_store::PendingTerminalProjection {
            task,
            attempt_id,
            terminal_seq: terminal_seq.ok_or(BridgeError::StoreFailure)?,
            terminal,
        })
    })())
}

fn row_to_batch(row: &rusqlite::Row) -> Result<bridge_core::task_store::BatchRecord, BridgeError> {
    use bridge_core::task_store::BatchRecord;
    let id: String = row.get(0).map_err(|_| BridgeError::StoreFailure)?;
    let workflow: String = row.get(1).map_err(|_| BridgeError::StoreFailure)?;
    let concurrency: i64 = row.get(2).map_err(|_| BridgeError::StoreFailure)?;
    let total: i64 = row.get(3).map_err(|_| BridgeError::StoreFailure)?;
    let status_s: String = row.get(4).map_err(|_| BridgeError::StoreFailure)?;
    let items_json: String = row.get(5).map_err(|_| BridgeError::StoreFailure)?;
    let error: Option<String> = row.get(6).map_err(|_| BridgeError::StoreFailure)?;
    let created_ms: i64 = row.get(7).map_err(|_| BridgeError::StoreFailure)?;
    let updated_ms: i64 = row.get(8).map_err(|_| BridgeError::StoreFailure)?;
    Ok(BatchRecord {
        id: bridge_core::ids::BatchId::parse(id).map_err(|_| BridgeError::StoreFailure)?,
        workflow,
        concurrency: concurrency as u32,
        total: total as u32,
        status: parse_batch_status(&status_s).ok_or(BridgeError::StoreFailure)?,
        items_json,
        error,
        created_ms,
        updated_ms,
    })
}

fn history_schema_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    std::path::PathBuf::from(name)
}

fn history_admission_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".admission.lock");
    std::path::PathBuf::from(name)
}

fn history_attempt_lock_dir(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".attempt-locks");
    std::path::PathBuf::from(name)
}

fn history_io_error_with_permission(
    error: &std::io::Error,
    fallback: bridge_core::workflow_history::LedgerUnavailableReason,
    permission: bridge_core::workflow_history::LedgerUnavailableReason,
) -> bridge_core::workflow_history::LedgerError {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
    let reason = match error.kind() {
        std::io::ErrorKind::PermissionDenied => permission,
        std::io::ErrorKind::WouldBlock => R::Locked,
        _ => fallback,
    };
    LedgerError::new(reason)
}

fn history_lock_error(error: &std::io::Error) -> bridge_core::workflow_history::LedgerError {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
    let reason = match error.kind() {
        std::io::ErrorKind::WouldBlock => R::Locked,
        std::io::ErrorKind::Unsupported => R::AdvisoryLockUnsupported,
        std::io::ErrorKind::PermissionDenied => R::ReadOnlyLock,
        _ => R::AdvisoryLockIo,
    };
    LedgerError::new(reason)
}

#[cfg(unix)]
fn set_history_permissions(
    path: &std::path::Path,
    mode: u32,
    permission: bridge_core::workflow_history::LedgerUnavailableReason,
) -> Result<(), bridge_core::workflow_history::LedgerError> {
    use bridge_core::workflow_history::LedgerUnavailableReason as R;
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| history_io_error_with_permission(&error, R::Io, permission))
}

#[cfg(unix)]
fn set_history_sidecar_permissions(
    path: &std::path::Path,
) -> Result<(), bridge_core::workflow_history::LedgerError> {
    use bridge_core::workflow_history::LedgerUnavailableReason as R;

    set_history_permissions(path, 0o600, R::ReadOnlyDatabase)?;
    set_history_permissions(&history_schema_lock_path(path), 0o600, R::ReadOnlyLock)?;
    let admission_lock = history_admission_lock_path(path);
    if admission_lock.exists() {
        set_history_permissions(&admission_lock, 0o600, R::ReadOnlyLock)?;
    }
    set_history_permissions(&history_attempt_lock_dir(path), 0o700, R::ReadOnlyParent)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = std::path::PathBuf::from(sidecar);
        if sidecar.exists() {
            set_history_permissions(&sidecar, 0o600, R::ReadOnlyDatabase)?;
        }
    }
    Ok(())
}

fn history_error(error: &rusqlite::Error) -> bridge_core::workflow_history::LedgerError {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
    if let rusqlite::Error::SqliteFailure(sqlite, _) = error {
        let reason = match sqlite.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => R::Locked,
            rusqlite::ErrorCode::ReadOnly => R::ReadOnlyDatabase,
            rusqlite::ErrorCode::PermissionDenied
            | rusqlite::ErrorCode::AuthorizationForStatementDenied => R::Permission,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                R::Corruption
            }
            rusqlite::ErrorCode::CannotOpen => R::Open,
            rusqlite::ErrorCode::SchemaChanged => R::Migration,
            rusqlite::ErrorCode::FileLockingProtocolFailed
            | rusqlite::ErrorCode::NoLargeFileSupport => R::AdvisoryLockUnsupported,
            rusqlite::ErrorCode::SystemIoFailure => R::Io,
            rusqlite::ErrorCode::DiskFull => R::CapacityProtected,
            _ => R::Io,
        };
        let extended = sqlite.extended_code;
        return LedgerError::with_sqlite_codes(reason, extended & 0xff, extended);
    }
    let message = error.to_string().to_ascii_lowercase();
    let reason = if message.contains("locked") || message.contains("busy") {
        R::Locked
    } else if message.contains("permission") {
        R::Permission
    } else if message.contains("read-only") || message.contains("readonly") {
        R::ReadOnlyDatabase
    } else if message.contains("corrupt") || message.contains("not a database") {
        R::Corruption
    } else if message.contains("schema") || message.contains("no such table") {
        R::Migration
    } else {
        R::Io
    };
    LedgerError::new(reason)
}

fn history_migration_error(error: &rusqlite::Error) -> bridge_core::workflow_history::LedgerError {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};

    let mapped = history_error(error);
    // Only SQL/schema failures become migration failures. Preserve CANTOPEN,
    // IOERR (including extended variants), read-only, lock, capacity, and
    // corruption classifications together with their exact SQLite codes.
    if let rusqlite::Error::SqliteFailure(sqlite, _)
    | rusqlite::Error::SqlInputError { error: sqlite, .. } = error
    {
        if matches!(
            sqlite.code,
            rusqlite::ErrorCode::Unknown
                | rusqlite::ErrorCode::SchemaChanged
                | rusqlite::ErrorCode::ConstraintViolation
                | rusqlite::ErrorCode::TypeMismatch
        ) {
            return LedgerError::with_sqlite_codes(
                R::Migration,
                sqlite.extended_code & 0xff,
                sqlite.extended_code,
            );
        }
    }
    mapped
}

fn history_attempt_read_error(
    error: &rusqlite::Error,
) -> bridge_core::workflow_history::LedgerError {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};

    // The statement itself is schema-sensitive, while a value that cannot be
    // decoded into the declared projection is persisted-row corruption.
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("invalid column type")
        || message.contains("conversion error")
        || message.contains("out of range")
        || message.contains("utf-8")
    {
        LedgerError::new(R::Corruption)
    } else {
        history_migration_error(error)
    }
}

fn schema_migration_error(
    error: &SchemaMigrationError,
) -> bridge_core::workflow_history::LedgerError {
    match error {
        SchemaMigrationError::Sqlite(error) => history_migration_error(error),
        SchemaMigrationError::Validation(
            MigrationValidationError::MalformedLocator
            | MigrationValidationError::ConflictingAuthority,
        ) => bridge_core::workflow_history::LedgerError::new(
            bridge_core::workflow_history::LedgerUnavailableReason::Migration,
        ),
    }
}

#[async_trait::async_trait]
impl bridge_core::workflow_history::WorkflowHistoryStore for SqliteStore {
    async fn reserve(
        &self,
        row: &bridge_core::workflow_history::AttemptReservation,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, MAX_CHARGED_BYTES, MAX_TERMINAL_JSON_BYTES,
            MAX_TERMINAL_ROWS, PERMANENT_IDENTITY_CHARGE, RESERVED_ROW_CHARGE, RETENTION_DAYS,
        };
        row.validate()?;
        let reservation_json =
            serde_json::to_string(row).map_err(|_| LedgerError::new(R::Schema))?;
        let _admission_lease = self.acquire_history_admission_lease()?;
        self.checkpoint_and_verify_history_size()?;
        let lease_acquired = self.acquire_history_attempt_lease(&row.identity.attempt_id)?;
        let result = (|| {
            let mut conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| history_error(&e))?;
            if matches!(
                row.surface,
                bridge_core::workflow_history::ExecutionSurface::ServedTask
                    | bridge_core::workflow_history::ExecutionSurface::DirectUnary
                    | bridge_core::workflow_history::ExecutionSurface::Mcp
                    | bridge_core::workflow_history::ExecutionSurface::Smoke
            ) && row.task_id.as_ref().map(|task| task.as_str())
                != Some(row.identity.execution_id.as_str())
            {
                return Err(LedgerError::new(R::Schema));
            }
            let served = row.surface == bridge_core::workflow_history::ExecutionSurface::ServedTask;
            let admitted: Option<(String, i64, Option<String>, String, i64)> = tx
                .query_row(
                    "SELECT execution_id, ordinal, task_id, owner_surface, summary_attached
                     FROM attempt_identities WHERE attempt_id=?1",
                    rusqlite::params![row.identity.attempt_id.as_str()],
                    |record| {
                        Ok((
                            record.get(0)?,
                            record.get(1)?,
                            record.get(2)?,
                            record.get(3)?,
                            record.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| history_error(&error))?;
            if served {
                let exact = admitted.as_ref().is_some_and(
                    |(execution, ordinal, task_id, owner_surface, summary_attached)| {
                        execution == row.identity.execution_id.as_str()
                            && *ordinal == i64::from(row.identity.ordinal)
                            && task_id.as_deref() == row.task_id.as_ref().map(|task| task.as_str())
                            && owner_surface == "served_task"
                            && *summary_attached == 0
                    },
                );
                if !exact {
                    return Err(LedgerError::new(R::Collision));
                }
            } else {
                let ordinal_owner: Option<String> = tx
                    .query_row(
                        "SELECT attempt_id FROM attempt_identities
                         WHERE execution_id=?1 AND ordinal=?2",
                        rusqlite::params![
                            row.identity.execution_id.as_str(),
                            i64::from(row.identity.ordinal)
                        ],
                        |record| record.get(0),
                    )
                    .optional()
                    .map_err(|error| history_error(&error))?;
                if admitted.is_some() || ordinal_owner.is_some() {
                    return Err(LedgerError::new(R::Collision));
                }
            }
            if let Some(parent) = row.identity.parent_attempt_id.as_ref() {
                let prior: Option<(String, i64, String, Option<String>)> = tx
                    .query_row(
                        "SELECT execution_id, ordinal, status, task_id
                         FROM workflow_attempt_summaries WHERE attempt_id=?1",
                        rusqlite::params![parent.as_str()],
                        |record| {
                            Ok((
                                record.get(0)?,
                                record.get(1)?,
                                record.get(2)?,
                                record.get(3)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|error| history_error(&error))?;
                match prior {
                    Some((execution, ordinal, status, task_id)) => {
                        let expected_ordinal = ordinal.checked_add(1);
                        if execution != row.identity.execution_id.as_str()
                            || expected_ordinal != Some(i64::from(row.identity.ordinal))
                            || status != "terminal"
                            || task_id.as_deref() != row.task_id.as_ref().map(|task| task.as_str())
                        {
                            return Err(LedgerError::new(R::Collision));
                        }
                    }
                    None if row.surface
                        != bridge_core::workflow_history::ExecutionSurface::ServedTask =>
                    {
                        return Err(LedgerError::new(R::Collision));
                    }
                    None => {}
                }
            } else {
                let existing: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM workflow_attempt_summaries WHERE execution_id=?1",
                        rusqlite::params![row.identity.execution_id.as_str()],
                        |record| record.get(0),
                    )
                    .map_err(|error| history_error(&error))?;
                if existing != 0 {
                    return Err(LedgerError::new(R::Collision));
                }
            }
            let cutoff = row
                .started_ms
                .saturating_sub(RETENTION_DAYS * 24 * 60 * 60 * 1000);
            let expired = {
                let mut statement = tx
                    .prepare(
                        "SELECT attempt_id, charged_bytes FROM workflow_attempt_summaries
                         WHERE status='terminal' AND pinned=0 AND completed_ms < ?1
                         ORDER BY completed_ms, attempt_id LIMIT 1",
                    )
                    .map_err(|error| history_error(&error))?;
                let rows = statement
                    .query_map(rusqlite::params![cutoff], |record| {
                        Ok((record.get::<_, String>(0)?, record.get::<_, i64>(1)?))
                    })
                    .map_err(|error| history_error(&error))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|error| history_error(&error))?
            };
            for (attempt_id, charged_bytes) in expired {
                let attempt_id = bridge_core::ids::AttemptId::parse(attempt_id)
                    .map_err(|_| LedgerError::new(R::Corruption))?;
                let charged_bytes =
                    u64::try_from(charged_bytes).map_err(|_| LedgerError::new(R::Corruption))?;
                let changed = tx
                    .execute(
                        "DELETE FROM workflow_attempt_summaries
                         WHERE attempt_id=?1 AND status='terminal' AND pinned=0",
                        rusqlite::params![attempt_id.as_str()],
                    )
                    .map_err(|error| history_error(&error))?;
                if changed != 1 {
                    return Err(LedgerError::new(R::Corruption));
                }
                let _ = charged_bytes;
                self.remove_history_attempt_lock_file(&attempt_id)?;
            }

            loop {
                let allocated_rows: i64 = tx
                    .query_row("SELECT COUNT(*) FROM workflow_attempt_summaries", [], |r| {
                        r.get(0)
                    })
                    .map_err(|e| history_error(&e))?;
                let charged: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(charged_bytes), 0) FROM workflow_attempt_summaries",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| history_error(&e))?;
                let identity_rows: i64 = tx
                    .query_row("SELECT COUNT(*) FROM attempt_identities", [], |record| {
                        record.get(0)
                    })
                    .map_err(|error| history_error(&error))?;
                let allocated_rows =
                    u64::try_from(allocated_rows).map_err(|_| LedgerError::new(R::Corruption))?;
                let charged =
                    u64::try_from(charged).map_err(|_| LedgerError::new(R::Corruption))?;
                let identity_rows =
                    u64::try_from(identity_rows).map_err(|_| LedgerError::new(R::Corruption))?;
                let incoming_identities = u64::from(!served);
                let authority_charge = identity_rows
                    .saturating_add(incoming_identities)
                    .saturating_mul(PERMANENT_IDENTITY_CHARGE);
                let logical_fits = charged
                    .saturating_add(RESERVED_ROW_CHARGE)
                    .saturating_add(authority_charge)
                    <= MAX_CHARGED_BYTES;
                let physical_charge = RESERVED_ROW_CHARGE
                    .saturating_add(incoming_identities.saturating_mul(PERMANENT_IDENTITY_CHARGE));
                let page_size: i64 = tx
                    .query_row("PRAGMA page_size", [], |record| record.get(0))
                    .map_err(|error| history_error(&error))?;
                let page_size =
                    u64::try_from(page_size).map_err(|_| LedgerError::new(R::Corruption))?;
                let rewrite_provision = u64::try_from(MAX_TERMINAL_JSON_BYTES)
                    .map_err(|_| LedgerError::new(R::Schema))?
                    .saturating_add(page_size.saturating_mul(4));
                let physical_fits = self
                    .history_growth_fits(&tx, physical_charge.saturating_add(rewrite_provision))?;
                if allocated_rows < MAX_TERMINAL_ROWS && logical_fits && physical_fits {
                    break;
                }
                let victim: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT attempt_id, charged_bytes FROM workflow_attempt_summaries
                         WHERE status='terminal' AND pinned=0
                         ORDER BY completed_ms, attempt_id LIMIT 1",
                        [],
                        |record| Ok((record.get(0)?, record.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| history_error(&e))?;
                let Some((victim, victim_charge)) = victim else {
                    return Err(LedgerError::new(R::CapacityProtected));
                };
                let victim = bridge_core::ids::AttemptId::parse(victim)
                    .map_err(|_| LedgerError::new(R::Corruption))?;
                let victim_charge =
                    u64::try_from(victim_charge).map_err(|_| LedgerError::new(R::Corruption))?;
                let changed = tx
                    .execute(
                        "DELETE FROM workflow_attempt_summaries
                         WHERE attempt_id=?1 AND status='terminal' AND pinned=0",
                        rusqlite::params![victim.as_str()],
                    )
                    .map_err(|e| history_error(&e))?;
                if changed != 1 {
                    return Err(LedgerError::new(R::Corruption));
                }
                let _ = victim_charge;
                self.remove_history_attempt_lock_file(&victim)?;
                // Re-evaluate logical charge and proven reusable pages after
                // every eviction; permanent identity authority remains charged.
            }

            if served {
                let changed = tx
                    .execute(
                        "UPDATE attempt_identities SET summary_attached=1
                         WHERE attempt_id=?1 AND execution_id=?2 AND ordinal=?3
                           AND task_id=?4 AND owner_surface='served_task'
                           AND summary_attached=0",
                        rusqlite::params![
                            row.identity.attempt_id.as_str(),
                            row.identity.execution_id.as_str(),
                            i64::from(row.identity.ordinal),
                            row.task_id.as_ref().map(|task| task.as_str())
                        ],
                    )
                    .map_err(|error| history_error(&error))?;
                if changed != 1 {
                    return Err(LedgerError::new(R::Collision));
                }
            } else {
                tx.execute(
                    "INSERT INTO attempt_identities(
                         attempt_id, execution_id, ordinal, task_id,
                         owner_surface, summary_attached)
                     VALUES(?1, ?2, ?3, ?4, ?5, 1)",
                    rusqlite::params![
                        row.identity.attempt_id.as_str(),
                        row.identity.execution_id.as_str(),
                        i64::from(row.identity.ordinal),
                        row.task_id.as_ref().map(|task| task.as_str()),
                        row.surface.as_str()
                    ],
                )
                .map_err(|error| {
                    if matches!(
                        error,
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error {
                                code: rusqlite::ErrorCode::ConstraintViolation,
                                ..
                            },
                            _
                        )
                    ) {
                        LedgerError::new(R::Collision)
                    } else {
                        history_error(&error)
                    }
                })?;
            }
            let result = tx.execute(
                "INSERT INTO workflow_attempt_summaries(
                attempt_id, execution_id, parent_attempt_id, ordinal, task_id,
                workflow, task_class, surface, policy, workload_fingerprint,
                workload_fingerprint_complete, started_ms, status, prompt_acceptance,
                pinned, charged_bytes, reservation_json, terminal_reserve)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'active',?13,?14,?15,?16,zeroblob(?17))",
                rusqlite::params![
                    row.identity.attempt_id.as_str(),
                    row.identity.execution_id.as_str(),
                    row.identity.parent_attempt_id.as_ref().map(|v| v.as_str()),
                    i64::from(row.identity.ordinal),
                    row.task_id.as_ref().map(|v| v.as_str()),
                    row.workflow,
                    row.task_class,
                    row.surface.as_str(),
                    row.policy,
                    row.workload_fingerprint,
                    row.workload_fingerprint_complete,
                    row.started_ms,
                    row.prompt_acceptance,
                    row.pinned,
                    RESERVED_ROW_CHARGE as i64,
                    reservation_json,
                    i64::try_from(MAX_TERMINAL_JSON_BYTES)
                        .map_err(|_| LedgerError::new(R::Schema))?,
                ],
            );
            match result {
                Ok(_) => {
                    self.ensure_terminal_rewrite_headroom(&tx)?;
                    tx.commit().map_err(|e| history_error(&e))
                }
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(LedgerError::new(R::Collision))
                }
                Err(error) => Err(history_error(&error)),
            }
        })();
        match result {
            Ok(()) => {
                self.checkpoint_after_committed_history_mutation("reserve");
                Ok(())
            }
            Err(error) => {
                if lease_acquired {
                    if let Err(cleanup_error) =
                        self.release_history_attempt_lease(&row.identity.attempt_id)
                    {
                        tracing::warn!(
                            attempt = row.identity.attempt_id.as_str(),
                            reason = cleanup_error.reason.as_str(),
                            "failed reservation lease cleanup deferred"
                        );
                    }
                }
                Err(error)
            }
        }
    }

    async fn mark_prompt_acceptance(
        &self,
        id: &bridge_core::ids::AttemptId,
        acceptance: &str,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
        if acceptance != "not_dispatched" && acceptance != "dispatch_uncertain" {
            return Err(LedgerError::new(R::Schema));
        }
        let _admission_lease = self.acquire_history_admission_lease()?;
        self.checkpoint_and_verify_history_size()?;
        let mut conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| history_error(&error))?;
        let growth = u64::try_from(acceptance.len()).unwrap_or(u64::MAX);
        if !self.history_growth_fits(&tx, growth)? {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        let changed = tx
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance=?2
                 WHERE attempt_id=?1 AND status='active'
                   AND prompt_acceptance IN ('not_dispatched','unknown','dispatch_uncertain')
                   AND (prompt_acceptance=?2 OR ?2='dispatch_uncertain')",
                rusqlite::params![id.as_str(), acceptance],
            )
            .map_err(|error| history_error(&error))?;
        if changed != 1 {
            return Err(LedgerError::new(R::Schema));
        }
        tx.commit().map_err(|error| history_error(&error))?;
        drop(conn);
        self.checkpoint_after_committed_history_mutation("mark_prompt_acceptance");
        Ok(())
    }

    async fn terminalize(
        &self,
        id: &bridge_core::ids::AttemptId,
        terminal: &bridge_core::workflow_history::AttemptTerminal,
    ) -> Result<
        bridge_core::workflow_history::TerminalWrite,
        bridge_core::workflow_history::LedgerError,
    > {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, TerminalWrite,
        };
        terminal.validate()?;
        let _admission_lease = self.acquire_history_admission_lease()?;
        self.checkpoint_and_verify_history_size()?;
        let result = (|| {
            let mut conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|error| history_error(&error))?;
            let existing: Option<(String, Option<String>, String, Option<i64>)> = tx
                .query_row(
                    "SELECT status, terminal_json, prompt_acceptance,
                            length(terminal_reserve)
                     FROM workflow_attempt_summaries WHERE attempt_id=?1",
                    rusqlite::params![id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| history_error(&error))?;
            let Some((status, prior, persisted_prompt_acceptance, terminal_reserve)) = existing
            else {
                return Err(LedgerError::new(R::Schema));
            };
            let mut canonical = terminal.clone();
            canonical.prompt_acceptance =
                bridge_core::workflow_history::conservative_prompt_acceptance(
                    &persisted_prompt_acceptance,
                    &terminal.prompt_acceptance,
                )?;
            canonical.validate()?;
            let json =
                serde_json::to_string(&canonical).map_err(|_| LedgerError::new(R::Schema))?;
            if status == "terminal" {
                let write = if prior.as_deref() == Some(json.as_str()) {
                    TerminalWrite::Replayed
                } else {
                    TerminalWrite::Conflict
                };
                tx.commit().map_err(|error| history_error(&error))?;
                return Ok(write);
            }
            let terminal_reserve = terminal_reserve
                .map(u64::try_from)
                .transpose()
                .map_err(|_| LedgerError::new(R::Corruption))?
                .unwrap_or(0);
            let terminal_growth = u64::try_from(json.len())
                .unwrap_or(u64::MAX)
                .saturating_sub(terminal_reserve);
            self.ensure_terminal_rewrite_headroom(&tx)?;
            if !self.history_growth_fits(&tx, terminal_growth)? {
                return Err(LedgerError::new(R::CapacityProtected));
            }
            let changed = tx
                .execute(
                    "UPDATE workflow_attempt_summaries SET status='terminal', completed_ms=?2,
                outcome=?3, degraded=?4, producer_terminal=?5, final_message=?6,
                process_liveness=?7, terminal_evidence_capability=?8,
                terminal_evidence_version=?9, terminal_evidence_source=?10,
                terminal_evidence_complete=?11, telemetry_complete=?12,
                prompt_acceptance=?13, terminal_json=?14, terminal_reserve=NULL
             WHERE attempt_id=?1 AND status='active'",
                    rusqlite::params![
                        id.as_str(),
                        canonical.completed_ms,
                        canonical.outcome,
                        canonical.degraded,
                        canonical.producer_terminal,
                        canonical.final_message,
                        canonical.process_liveness,
                        canonical.terminal_evidence_capability,
                        canonical.terminal_evidence_version,
                        canonical.terminal_evidence_source,
                        canonical.terminal_evidence_complete,
                        canonical.telemetry_complete,
                        canonical.prompt_acceptance,
                        json
                    ],
                )
                .map_err(|error| history_error(&error))?;
            if changed == 1 {
                self.ensure_terminal_rewrite_headroom(&tx)?;
                tx.commit().map_err(|error| history_error(&error))?;
                return Ok(TerminalWrite::Applied);
            }
            Err(LedgerError::new(R::Io))
        })();
        match result {
            Ok(write) => {
                self.checkpoint_after_committed_history_mutation("terminalize");
                if let Err(error) = self.release_history_attempt_lease(id) {
                    tracing::warn!(
                        attempt = id.as_str(),
                        reason = error.reason.as_str(),
                        "terminalization committed; stale lease-file cleanup deferred"
                    );
                }
                Ok(write)
            }
            Err(error) => Err(error),
        }
    }

    async fn set_pinned(
        &self,
        id: &bridge_core::ids::AttemptId,
        pinned: bool,
    ) -> Result<bool, bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            AttemptReservation, LedgerError, LedgerUnavailableReason as R,
        };

        let _admission_lease = self.acquire_history_admission_lease()?;
        self.checkpoint_and_verify_history_size()?;
        let mut conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| history_error(&error))?;
        let persisted: Option<(i64, String)> = tx
            .query_row(
                "SELECT pinned, reservation_json
                 FROM workflow_attempt_summaries WHERE attempt_id=?1",
                rusqlite::params![id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| history_error(&error))?;
        let Some((persisted_pinned, reservation_json)) = persisted else {
            return Err(LedgerError::new(R::Schema));
        };
        let mut reservation: AttemptReservation =
            serde_json::from_str(&reservation_json).map_err(|_| LedgerError::new(R::Corruption))?;
        if reservation.identity.attempt_id != *id
            || !matches!(persisted_pinned, 0 | 1)
            || (persisted_pinned == 1) != reservation.pinned
        {
            return Err(LedgerError::new(R::Corruption));
        }
        let changed = reservation.pinned != pinned;
        if changed {
            reservation.pinned = pinned;
            reservation.validate()?;
            let updated_reservation_json =
                serde_json::to_string(&reservation).map_err(|_| LedgerError::new(R::Schema))?;
            let growth = u64::try_from(
                updated_reservation_json
                    .len()
                    .saturating_sub(reservation_json.len()),
            )
            .unwrap_or(u64::MAX);
            if !self.history_growth_fits(&tx, growth)? {
                return Err(LedgerError::new(R::CapacityProtected));
            }
            let updated = tx
                .execute(
                    "UPDATE workflow_attempt_summaries
                     SET pinned=?2, reservation_json=?3 WHERE attempt_id=?1",
                    rusqlite::params![id.as_str(), pinned, updated_reservation_json],
                )
                .map_err(|error| history_error(&error))?;
            if updated != 1 {
                return Err(LedgerError::new(R::Io));
            }
        }
        tx.commit().map_err(|error| history_error(&error))?;
        drop(conn);
        self.checkpoint_after_committed_history_mutation("set_pinned");
        Ok(changed)
    }

    async fn interrupt_active(
        &self,
        completed_ms: i64,
    ) -> Result<u64, bridge_core::workflow_history::LedgerError> {
        self.interrupt_active_sync(completed_ms, &[])
    }

    async fn interrupt_active_excluding(
        &self,
        completed_ms: i64,
        excluded: &[bridge_core::ids::AttemptId],
    ) -> Result<u64, bridge_core::workflow_history::LedgerError> {
        self.interrupt_active_sync(completed_ms, excluded)
    }

    async fn latest_reservation_for_task(
        &self,
        task: &bridge_core::ids::TaskId,
    ) -> Result<
        Option<bridge_core::workflow_history::AttemptReservation>,
        bridge_core::workflow_history::LedgerError,
    > {
        use bridge_core::workflow_history::{
            AttemptReservation, LedgerError, LedgerUnavailableReason as R,
        };
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let json: Option<String> = conn
            .query_row(
                "SELECT reservation_json FROM workflow_attempt_summaries
                 WHERE task_id=?1 ORDER BY ordinal DESC, started_ms DESC LIMIT 1",
                rusqlite::params![task.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| history_error(&error))?;
        json.map(|value| {
            serde_json::from_str::<AttemptReservation>(&value)
                .map_err(|_| LedgerError::new(R::Corruption))
        })
        .transpose()
    }

    async fn attempt(
        &self,
        id: &bridge_core::ids::AttemptId,
    ) -> Result<
        Option<bridge_core::workflow_history::AttemptRecord>,
        bridge_core::workflow_history::LedgerError,
    > {
        use bridge_core::workflow_history::{
            AttemptRecord, AttemptReservation, AttemptTerminal, LedgerError,
            LedgerUnavailableReason as R,
        };

        struct PersistedAttempt {
            attempt_id: String,
            execution_id: String,
            parent_attempt_id: Option<String>,
            ordinal: i64,
            task_id: Option<String>,
            workflow: String,
            task_class: String,
            surface: String,
            policy: String,
            workload_fingerprint: String,
            workload_fingerprint_complete: i64,
            started_ms: i64,
            completed_ms: Option<i64>,
            status: String,
            prompt_acceptance: String,
            producer_terminal: String,
            final_message: String,
            process_liveness: String,
            terminal_evidence_capability: String,
            terminal_evidence_version: String,
            terminal_evidence_source: String,
            terminal_evidence_complete: i64,
            telemetry_complete: i64,
            outcome: Option<String>,
            degraded: i64,
            pinned: i64,
            reservation_json: String,
            terminal_json: Option<String>,
            authority_attempt_id: Option<String>,
            authority_execution_id: Option<String>,
            authority_ordinal: Option<i64>,
            authority_task_id: Option<String>,
            authority_surface: Option<String>,
            authority_summary_attached: Option<i64>,
        }

        let corruption = || LedgerError::new(R::Corruption);
        let persisted_bool = |value| -> Result<bool, LedgerError> {
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(corruption()),
            }
        };
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let row: Option<PersistedAttempt> = conn
            .query_row(
                "SELECT summary.attempt_id, summary.execution_id,
                        summary.parent_attempt_id, summary.ordinal, summary.task_id,
                        summary.workflow, summary.task_class, summary.surface, summary.policy,
                        summary.workload_fingerprint, summary.workload_fingerprint_complete,
                        summary.started_ms, summary.completed_ms, summary.status,
                        summary.prompt_acceptance, summary.producer_terminal,
                        summary.final_message, summary.process_liveness,
                        summary.terminal_evidence_capability,
                        summary.terminal_evidence_version, summary.terminal_evidence_source,
                        summary.terminal_evidence_complete, summary.telemetry_complete,
                        summary.outcome, summary.degraded, summary.pinned,
                        summary.reservation_json, summary.terminal_json,
                        authority.attempt_id, authority.execution_id, authority.ordinal,
                        authority.task_id, authority.owner_surface, authority.summary_attached
                 FROM workflow_attempt_summaries summary
                 LEFT JOIN attempt_identities authority
                   ON authority.attempt_id=summary.attempt_id
                 WHERE summary.attempt_id=?1",
                rusqlite::params![id.as_str()],
                |row| {
                    Ok(PersistedAttempt {
                        attempt_id: row.get(0)?,
                        execution_id: row.get(1)?,
                        parent_attempt_id: row.get(2)?,
                        ordinal: row.get(3)?,
                        task_id: row.get(4)?,
                        workflow: row.get(5)?,
                        task_class: row.get(6)?,
                        surface: row.get(7)?,
                        policy: row.get(8)?,
                        workload_fingerprint: row.get(9)?,
                        workload_fingerprint_complete: row.get(10)?,
                        started_ms: row.get(11)?,
                        completed_ms: row.get(12)?,
                        status: row.get(13)?,
                        prompt_acceptance: row.get(14)?,
                        producer_terminal: row.get(15)?,
                        final_message: row.get(16)?,
                        process_liveness: row.get(17)?,
                        terminal_evidence_capability: row.get(18)?,
                        terminal_evidence_version: row.get(19)?,
                        terminal_evidence_source: row.get(20)?,
                        terminal_evidence_complete: row.get(21)?,
                        telemetry_complete: row.get(22)?,
                        outcome: row.get(23)?,
                        degraded: row.get(24)?,
                        pinned: row.get(25)?,
                        reservation_json: row.get(26)?,
                        terminal_json: row.get(27)?,
                        authority_attempt_id: row.get(28)?,
                        authority_execution_id: row.get(29)?,
                        authority_ordinal: row.get(30)?,
                        authority_task_id: row.get(31)?,
                        authority_surface: row.get(32)?,
                        authority_summary_attached: row.get(33)?,
                    })
                },
            )
            .optional()
            .map_err(|error| history_attempt_read_error(&error))?;
        row.map(|row| {
            let mut reservation = serde_json::from_str::<AttemptReservation>(&row.reservation_json)
                .map_err(|_| corruption())?;
            reservation.validate().map_err(|_| corruption())?;

            let projected_fingerprint_complete = persisted_bool(row.workload_fingerprint_complete)?;
            let projected_pinned = persisted_bool(row.pinned)?;
            let reservation_task_id = reservation.task_id.as_ref().map(|value| value.as_str());
            let reservation_parent = reservation
                .identity
                .parent_attempt_id
                .as_ref()
                .map(|value| value.as_str());
            if row.attempt_id != id.as_str()
                || reservation.identity.attempt_id != *id
                || row.execution_id != reservation.identity.execution_id.as_str()
                || row.parent_attempt_id.as_deref() != reservation_parent
                || row.ordinal != i64::from(reservation.identity.ordinal)
                || row.task_id.as_deref() != reservation_task_id
                || row.workflow != reservation.workflow
                || row.task_class != reservation.task_class
                || row.surface != reservation.surface.as_str()
                || row.policy != reservation.policy
                || row.workload_fingerprint != reservation.workload_fingerprint
                || projected_fingerprint_complete != reservation.workload_fingerprint_complete
                || row.started_ms != reservation.started_ms
                || projected_pinned != reservation.pinned
            {
                return Err(corruption());
            }

            if row.authority_attempt_id.as_deref() != Some(row.attempt_id.as_str())
                || row.authority_execution_id.as_deref() != Some(row.execution_id.as_str())
                || row.authority_ordinal != Some(row.ordinal)
                || row.authority_task_id.as_deref() != row.task_id.as_deref()
                || row.authority_surface.as_deref() != Some(row.surface.as_str())
                || row.authority_summary_attached != Some(1)
            {
                return Err(corruption());
            }

            if !matches!(
                row.prompt_acceptance.as_str(),
                "not_dispatched" | "dispatch_uncertain" | "unknown"
            ) {
                return Err(corruption());
            }
            let projected_prompt_acceptance =
                bridge_core::workflow_history::conservative_prompt_acceptance(
                    &reservation.prompt_acceptance,
                    &row.prompt_acceptance,
                )
                .map_err(|_| corruption())?;
            if projected_prompt_acceptance != row.prompt_acceptance {
                return Err(corruption());
            }
            if row.status == "active"
                && reservation.prompt_acceptance != row.prompt_acceptance
                && row.prompt_acceptance != "dispatch_uncertain"
            {
                return Err(corruption());
            }
            let terminal = row
                .terminal_json
                .as_deref()
                .map(|value| {
                    serde_json::from_str::<AttemptTerminal>(value).map_err(|_| corruption())
                })
                .transpose()?;
            let legacy_conservative_prompt_projection = matches!(
                (row.status.as_str(), terminal.as_ref()),
                ("terminal", Some(terminal))
                    if row.prompt_acceptance == "not_dispatched"
                        && terminal.prompt_acceptance == "unknown"
                        && terminal.terminal_reason == "prompt_barrier_failed"
                        && matches!(terminal.outcome.as_str(), "failed" | "interrupted")
                        && terminal.producer_terminal == "unknown"
                        && terminal.final_message == "unknown"
                        && terminal.process_liveness == "unknown"
                        && terminal.terminal_evidence_capability == "unsupported"
                        && terminal.terminal_evidence_version == "none"
                        && terminal.terminal_evidence_source == "none"
                        && !terminal.terminal_evidence_complete
                        && terminal.degraded
                        && !terminal.telemetry_complete
            );
            // Prompt state may advance conservatively from the immutable
            // reservation snapshot, but the projected column may never erase
            // evidence from it. The one legacy seed shape above predates the
            // projected prompt-column update after a failed dispatch barrier;
            // exact reads conservatively expose its immutable terminal value.
            reservation.prompt_acceptance = if legacy_conservative_prompt_projection {
                "unknown".to_owned()
            } else {
                row.prompt_acceptance.clone()
            };
            reservation.pinned = projected_pinned;
            reservation.validate().map_err(|_| corruption())?;
            match (row.status.as_str(), terminal.as_ref()) {
                ("active", None) => {
                    if row.completed_ms.is_some()
                        || row.outcome.is_some()
                        || row.producer_terminal != "unknown"
                        || row.final_message != "unknown"
                        || row.process_liveness != "unknown"
                        || row.terminal_evidence_capability != "unsupported"
                        || row.terminal_evidence_version != "none"
                        || row.terminal_evidence_source != "none"
                        || persisted_bool(row.terminal_evidence_complete)?
                        || persisted_bool(row.telemetry_complete)?
                        || persisted_bool(row.degraded)?
                    {
                        return Err(corruption());
                    }
                }
                ("terminal", Some(terminal)) => {
                    terminal.validate().map_err(|_| corruption())?;
                    if row.completed_ms != Some(terminal.completed_ms)
                        || row.outcome.as_deref() != Some(terminal.outcome.as_str())
                        || persisted_bool(row.degraded)? != terminal.degraded
                        || row.producer_terminal != terminal.producer_terminal
                        || row.final_message != terminal.final_message
                        || row.process_liveness != terminal.process_liveness
                        || row.terminal_evidence_capability != terminal.terminal_evidence_capability
                        || row.terminal_evidence_version != terminal.terminal_evidence_version
                        || row.terminal_evidence_source != terminal.terminal_evidence_source
                        || persisted_bool(row.terminal_evidence_complete)?
                            != terminal.terminal_evidence_complete
                        || persisted_bool(row.telemetry_complete)? != terminal.telemetry_complete
                        || (row.prompt_acceptance != terminal.prompt_acceptance
                            && !legacy_conservative_prompt_projection)
                    {
                        return Err(corruption());
                    }
                }
                _ => return Err(corruption()),
            }
            let terminal = terminal.map(|terminal| {
                bridge_core::workflow_history::compatibility_project_terminal(
                    reservation.surface,
                    &terminal,
                )
                .into_owned()
            });

            Ok(AttemptRecord {
                reservation,
                terminal,
            })
        })
        .transpose()
    }

    async fn completed_between(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<
        Vec<bridge_core::workflow_history::CompletedAttempt>,
        bridge_core::workflow_history::LedgerError,
    > {
        use bridge_core::workflow_history::{
            CompletedAttempt, LedgerError, LedgerUnavailableReason as R,
        };
        if start_ms < 0 || end_ms < start_ms {
            return Err(LedgerError::new(R::Schema));
        }
        struct PersistedCompleted {
            reservation_json: String,
            terminal_json: String,
            surface: String,
            completed_ms: i64,
            outcome: String,
            degraded: i64,
            prompt_acceptance: String,
            producer_terminal: String,
            final_message: String,
            process_liveness: String,
            terminal_evidence_capability: String,
            terminal_evidence_version: String,
            terminal_evidence_source: String,
            terminal_evidence_complete: i64,
            telemetry_complete: i64,
        }

        let corruption = || LedgerError::new(R::Corruption);
        let persisted_bool = |value| -> Result<bool, LedgerError> {
            match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(corruption()),
            }
        };
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        // Keep this statement and its rows alive through validation and
        // compatibility projection so retention cannot split selection from
        // decoding across SQLite snapshots.
        let mut stmt = conn
            .prepare(
                "SELECT reservation_json, terminal_json, surface, completed_ms,
                        outcome, degraded, prompt_acceptance, producer_terminal,
                        final_message, process_liveness, terminal_evidence_capability,
                        terminal_evidence_version, terminal_evidence_source,
                        terminal_evidence_complete, telemetry_complete
                 FROM workflow_attempt_summaries
                 WHERE status='terminal' AND completed_ms>=?1 AND completed_ms<=?2
                 ORDER BY completed_ms, attempt_id",
            )
            .map_err(|error| history_error(&error))?;
        let rows = stmt
            .query_map(rusqlite::params![start_ms, end_ms], |row| {
                Ok(PersistedCompleted {
                    reservation_json: row.get(0)?,
                    terminal_json: row.get(1)?,
                    surface: row.get(2)?,
                    completed_ms: row.get(3)?,
                    outcome: row.get(4)?,
                    degraded: row.get(5)?,
                    prompt_acceptance: row.get(6)?,
                    producer_terminal: row.get(7)?,
                    final_message: row.get(8)?,
                    process_liveness: row.get(9)?,
                    terminal_evidence_capability: row.get(10)?,
                    terminal_evidence_version: row.get(11)?,
                    terminal_evidence_source: row.get(12)?,
                    terminal_evidence_complete: row.get(13)?,
                    telemetry_complete: row.get(14)?,
                })
            })
            .map_err(|error| history_error(&error))?;

        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(|error| history_attempt_read_error(&error))?;
            let mut reservation = serde_json::from_str::<
                bridge_core::workflow_history::AttemptReservation,
            >(&row.reservation_json)
            .map_err(|_| corruption())?;
            reservation.validate().map_err(|_| corruption())?;
            let projected_prompt_acceptance =
                bridge_core::workflow_history::conservative_prompt_acceptance(
                    &reservation.prompt_acceptance,
                    &row.prompt_acceptance,
                )
                .map_err(|_| corruption())?;
            if projected_prompt_acceptance != row.prompt_acceptance {
                return Err(corruption());
            }

            let terminal = serde_json::from_str::<bridge_core::workflow_history::AttemptTerminal>(
                &row.terminal_json,
            )
            .map_err(|_| corruption())?;
            terminal.validate().map_err(|_| corruption())?;

            let legacy_conservative_prompt_projection = row.prompt_acceptance == "not_dispatched"
                && terminal.prompt_acceptance == "unknown"
                && terminal.terminal_reason == "prompt_barrier_failed"
                && matches!(terminal.outcome.as_str(), "failed" | "interrupted")
                && terminal.producer_terminal == "unknown"
                && terminal.final_message == "unknown"
                && terminal.process_liveness == "unknown"
                && terminal.terminal_evidence_capability == "unsupported"
                && terminal.terminal_evidence_version == "none"
                && terminal.terminal_evidence_source == "none"
                && !terminal.terminal_evidence_complete
                && terminal.degraded
                && !terminal.telemetry_complete;
            if row.surface != reservation.surface.as_str()
                || row.completed_ms != terminal.completed_ms
                || row.outcome != terminal.outcome
                || persisted_bool(row.degraded)? != terminal.degraded
                || row.producer_terminal != terminal.producer_terminal
                || row.final_message != terminal.final_message
                || row.process_liveness != terminal.process_liveness
                || row.terminal_evidence_capability != terminal.terminal_evidence_capability
                || row.terminal_evidence_version != terminal.terminal_evidence_version
                || row.terminal_evidence_source != terminal.terminal_evidence_source
                || persisted_bool(row.terminal_evidence_complete)?
                    != terminal.terminal_evidence_complete
                || persisted_bool(row.telemetry_complete)? != terminal.telemetry_complete
                || (row.prompt_acceptance != terminal.prompt_acceptance
                    && !legacy_conservative_prompt_projection)
            {
                return Err(corruption());
            }
            reservation.prompt_acceptance = if legacy_conservative_prompt_projection {
                "unknown".to_owned()
            } else {
                row.prompt_acceptance
            };
            reservation.validate().map_err(|_| corruption())?;

            let completed = CompletedAttempt {
                reservation,
                terminal,
            };
            out.push(
                bridge_core::workflow_history::compatibility_project_completed(&completed)
                    .into_owned(),
            );
        }
        Ok(out)
    }
}

/// Owner-private history path used only when no configured durable store exists.
pub fn platform_history_path(
) -> Result<std::path::PathBuf, bridge_core::workflow_history::LedgerError> {
    use bridge_core::workflow_history::{LedgerError, LedgerUnavailableReason as R};
    if let Some(path) = std::env::var_os("A2A_BRIDGE_STATE_DIR") {
        if path.is_empty() {
            return Err(LedgerError::new(R::Open));
        }
        return Ok(std::path::PathBuf::from(path).join("workflow-history.sqlite"));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| LedgerError::new(R::Open))?;
    let base = if cfg!(target_os = "macos") {
        std::path::PathBuf::from(home).join("Library/Application Support/a2a-bridge")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(home).join(".local/state"))
            .join("a2a-bridge")
    };
    Ok(base.join("workflow-history.sqlite"))
}

// A disk-backed SQLite transaction can copy every main-database page into its
// rollback journal. Keep the main database below 56 MiB and retain a separate
// 68 MiB per-transaction reserve (plus 4 MiB for journal framing), so even a
// whole-database rewrite cannot cross the aggregate 128 MiB ceiling.
const HISTORY_SIDECAR_HEADROOM_BYTES: u64 = 72 * 1024 * 1024;
const HISTORY_DISK_TRANSACTION_HEADROOM_BYTES: u64 = 68 * 1024 * 1024;

impl SqliteStore {
    /// Apply the main-database page ceiling before schema migration or any
    /// history mutation. Aggregate filesystem admission additionally accounts
    /// for every live WAL/journal/SHM sidecar.
    fn configure_history_physical_limit(
        &self,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, MAX_CHARGED_BYTES,
        };
        if self.history_path.is_none() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let page_count: i64 = conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        let page_size = u64::try_from(page_size).map_err(|_| LedgerError::new(R::Corruption))?;
        let page_count = u64::try_from(page_count).map_err(|_| LedgerError::new(R::Corruption))?;
        if page_size == 0 {
            return Err(LedgerError::new(R::Corruption));
        }
        let main_budget = MAX_CHARGED_BYTES
            .checked_sub(HISTORY_SIDECAR_HEADROOM_BYTES)
            .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
        let max_pages = main_budget / page_size;
        if max_pages == 0 || page_count > max_pages {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        // Refuse before a legacy WAL checkpoint or journal-mode transition can
        // temporarily expand an already near-boundary allocation.
        if !self.history_growth_fits(&conn, 0)? {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        let applied: i64 = conn
            .query_row(&format!("PRAGMA max_page_count = {max_pages}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| history_error(&error))?;
        let applied = u64::try_from(applied).map_err(|_| LedgerError::new(R::Corruption))?;
        if applied > max_pages {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        // A live reader can prevent WAL checkpointing while later writers keep
        // appending frames, so WAL plus max_page_count is not a hard physical
        // bound. Rollback journals serialize writers and contain at most one
        // before-image per main-database page. Disabling cache spill keeps the
        // corresponding write set bounded until the commit boundary.
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode = TRUNCATE", [], |row| row.get(0))
            .map_err(|error| history_error(&error))?;
        if !matches!(journal_mode.as_str(), "delete" | "truncate" | "persist") {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        conn.execute_batch(
            "PRAGMA cache_spill = OFF;
             PRAGMA journal_size_limit = 0;",
        )
        .map_err(|error| history_error(&error))?;
        drop(conn);
        self.checkpoint_and_verify_history_size()?;
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        if !self.history_growth_fits(&conn, 0)? {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        Ok(())
    }

    fn checkpoint_and_verify_history_size(
        &self,
    ) -> Result<(), bridge_core::workflow_history::LedgerError> {
        use bridge_core::workflow_history::{
            LedgerError, LedgerUnavailableReason as R, MAX_CHARGED_BYTES,
        };
        if self.history_path.is_none() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|_| LedgerError::new(R::Io))?;
        let (busy, _, _): (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|error| history_error(&error))?;
        if busy != 0 {
            return Err(LedgerError::new(R::Locked));
        }
        drop(conn);
        if self.checked_history_file_bytes()? > MAX_CHARGED_BYTES {
            return Err(LedgerError::new(R::CapacityProtected));
        }
        Ok(())
    }

    /// The transaction is already durable, so a checkpoint failure must not be
    /// reported as a failed mutation. The next serialized mutation performs a
    /// strict preflight checkpoint before it can write again.
    fn checkpoint_after_committed_history_mutation(&self, operation: &str) {
        if let Err(error) = self.checkpoint_and_verify_history_size() {
            tracing::warn!(
                operation,
                reason = error.reason.as_str(),
                "history mutation committed; post-commit checkpoint deferred"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{PeerTaskId, PendingKind, PendingRequest};
    use bridge_core::ids::{BatchId, ContextId, SessionId, TaskId, TurnId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{FailureClass, SessionStore, TraceParent, TurnContext, TurnOutcome};
    use bridge_core::task_store::{
        BatchRecord, BatchStatus, ChildClaim, MemoryTaskStore, TaskRecord, TaskRecordStatus,
        TaskStore, TurnLogFinalized, TurnLogFinished, TurnUsageFinalization,
        RETENTION_NEVER_ELIGIBLE_MS,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    type LegacyTurnColumns = (
        Option<i64>,
        String,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<String>,
        Option<String>,
    );

    fn trec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
            last_artifact_ms: None,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    fn ctx(turn: &str, attempt: u32) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: Some(TaskId::parse("task-1").unwrap()),
            workflow: Some("code-review".to_string()),
            node: Some("reviewer".to_string()),
            attempt,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: Some("default".to_string()),
            prompt_id: Some("prompt/eval".to_string()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        }
    }

    fn ctx_for(turn: &str, session: &str, task: &str, completed_attempt: u32) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse(session).unwrap(),
            task_id: Some(TaskId::parse(task).unwrap()),
            workflow: Some("code-review".to_string()),
            node: Some("reviewer".to_string()),
            attempt: completed_attempt,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: Some("default".to_string()),
            prompt_id: Some("prompt/eval".to_string()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        }
    }

    fn sqlite_usage(input: u64, output: u64, at_ms: i64) -> UsageSnapshot {
        UsageSnapshot {
            used: None,
            size: None,
            cost: Some(UsageCost {
                amount: 0.42,
                currency: "USD".to_string(),
            }),
            terminal: Some(TerminalUsage {
                total_tokens: input + output,
                input_tokens: input,
                output_tokens: output,
                thought_tokens: Some(1),
                cached_read_tokens: Some(2),
                cached_write_tokens: Some(3),
            }),
            at_ms,
        }
    }

    fn sqlite_finished(ctx: TurnContext, completed_ms: i64) -> TurnLogFinished {
        TurnLogFinished {
            ctx,
            started_ms: completed_ms - 10,
            completed_ms,
            latency: std::time::Duration::from_millis(10),
            ttft: None,
            outcome: TurnOutcome::Success,
        }
    }

    async fn write_sqlite_turn(
        store: &SqliteStore,
        ctx: TurnContext,
        completed_ms: i64,
        input: u64,
        output: u64,
        cost: Option<(&str, f64)>,
    ) {
        store
            .upsert_turn_finished(&TurnLogFinished {
                ctx: ctx.clone(),
                started_ms: completed_ms - 10,
                completed_ms,
                latency: std::time::Duration::from_millis(10),
                ttft: Some(std::time::Duration::from_millis(2)),
                outcome: TurnOutcome::Success,
            })
            .await
            .unwrap();
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx,
                finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                    used: None,
                    size: None,
                    cost: cost.map(|(currency, amount)| UsageCost {
                        amount,
                        currency: currency.to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 999,
                        input_tokens: input,
                        output_tokens: output,
                        thought_tokens: Some(1),
                        cached_read_tokens: Some(2),
                        cached_write_tokens: None,
                    }),
                    at_ms: completed_ms,
                }),
            })
            .await
            .unwrap();
    }

    fn sample_batch(bid: &BatchId, status: BatchStatus, total: u32, ms: i64) -> BatchRecord {
        BatchRecord {
            id: bid.clone(),
            workflow: "code-review".into(),
            concurrency: 2,
            total,
            status,
            items_json: r#"{"v":1,"items":[]}"#.into(),
            error: None,
            created_ms: ms,
            updated_ms: ms,
        }
    }

    fn batch_child_record(tid: &TaskId, bid: &BatchId, item: &str) -> TaskRecord {
        TaskRecord {
            id: tid.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 0,
            updated_ms: 0,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: Some(r#"{"v":1,"nodes":[]}"#.into()),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(bid.clone()),
            item_id: Some(item.to_string()),
            artifacts_purged_at: None,
        }
    }

    #[tokio::test]
    async fn sqlite_usage_finalized_some_updates_usage_and_barrier_atomically() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 12_345)).unwrap();
        let ctx = ctx_for("sqlite-final-usage", "ctx-final-usage", "task-1", 0);
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::Usage(sqlite_usage(5, 7, 1)),
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.input_tokens, Some(5));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.cost_amount, Some(0.42));
        assert_eq!(row.usage_finalized_ms, Some(12_345));
        assert_eq!(row.usage_finalization_kind, "usage");
    }

    #[tokio::test]
    async fn sqlite_usage_finalization_uses_persistence_time_not_old_event_time() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 86_400_001)).unwrap();
        let ctx = ctx_for("sqlite-persist-time", "ctx-persist-time", "task-1", 0);
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::Usage(sqlite_usage(5, 7, 1)),
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(86_400_001));
        assert_ne!(row.usage_finalized_ms, Some(1));
    }

    #[tokio::test]
    async fn sqlite_usage_finalized_none_sets_no_usage_barrier() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 12_346)).unwrap();
        let ctx = ctx_for("sqlite-final-none", "ctx-final-none", "task-1", 0);
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.input_tokens, None);
        assert_eq!(row.cost_amount, None);
        assert_eq!(row.usage_finalized_ms, Some(12_346));
        assert_eq!(row.usage_finalization_kind, "no_usage");
    }

    #[tokio::test]
    async fn sqlite_usage_finalization_invalid_clock_uses_never_eligible_timestamp() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 0)).unwrap();
        let ctx = ctx_for("sqlite-final-zero", "ctx-final-zero", "task-1", 0);
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(RETENTION_NEVER_ELIGIBLE_MS));
        assert_ne!(row.usage_finalized_ms, Some(0));
    }

    #[tokio::test]
    async fn sqlite_no_usage_finalization_rejects_existing_usage_columns() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 12_347)).unwrap();
        let ctx = ctx_for(
            "sqlite-final-contradict",
            "ctx-final-contradict",
            "task-1",
            0,
        );
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE turn_log SET input_tokens=1 WHERE turn_id=?1",
                rusqlite::params![ctx.turn_id.as_str()],
            )
            .unwrap();
        }

        assert!(store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .is_err());
        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.input_tokens, Some(1));
        assert_eq!(row.usage_finalized_ms, None);
        assert_eq!(row.usage_finalization_kind, "pending");
    }

    #[tokio::test]
    async fn sqlite_turn_finished_upsert_does_not_clear_finalization() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 12_348)).unwrap();
        let ctx = ctx_for("sqlite-final-replay", "ctx-final-replay", "task-1", 0);
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 250))
            .await
            .unwrap();
        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(12_348));
        assert_eq!(row.usage_finalization_kind, "no_usage");
    }

    #[tokio::test]
    async fn sqlite_turn_finished_task_linked_bumps_artifact_recency() {
        let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 50_000)).unwrap();
        let task = TaskId::parse("task-recency-finish").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        let ctx = ctx_for(
            "sqlite-recency-finish",
            "ctx-recency-finish",
            task.as_str(),
            0,
        );

        store
            .upsert_turn_finished(&sqlite_finished(ctx, 200))
            .await
            .unwrap();

        let row = store.get(&task).await.unwrap().unwrap();
        assert_eq!(row.last_artifact_ms, Some(50_000));
    }

    #[tokio::test]
    async fn sqlite_finalize_turn_usage_task_linked_bumps_artifact_recency() {
        let clock = Arc::new(AtomicI64::new(60_000));
        let store = SqliteStore::open_in_memory_with_clock({
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        })
        .unwrap();
        let task = TaskId::parse("task-recency-finalize").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        let ctx = ctx_for(
            "sqlite-recency-finalize",
            "ctx-recency-finalize",
            task.as_str(),
            0,
        );
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();
        clock.store(70_000, Ordering::SeqCst);

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx,
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.get(&task).await.unwrap().unwrap();
        assert_eq!(row.last_artifact_ms, Some(70_000));
    }

    async fn finish_for_finalization<S: TaskStore + ?Sized>(store: &S, ctx: &TurnContext) {
        store
            .upsert_turn_finished(&sqlite_finished(ctx.clone(), 200))
            .await
            .unwrap();
    }

    async fn parity_turn_row(
        memory: &MemoryTaskStore,
        sqlite: &SqliteStore,
        turn: &TurnId,
    ) -> bridge_core::task_store::TurnLogRow {
        let memory_row = memory.turn_log_row(turn).await.unwrap().unwrap();
        let sqlite_row = sqlite.turn_log_row(turn).await.unwrap().unwrap();
        assert_eq!(memory_row, sqlite_row);
        memory_row
    }

    async fn finalize_both(
        memory: &MemoryTaskStore,
        sqlite: &SqliteStore,
        row: &TurnLogFinalized,
    ) -> (Result<(), BridgeError>, Result<(), BridgeError>) {
        (
            memory.finalize_turn_usage(row).await,
            sqlite.finalize_turn_usage(row).await,
        )
    }

    #[tokio::test]
    async fn memory_finalization_matches_sqlite() {
        let clock = Arc::new(AtomicI64::new(88_000));
        let memory = MemoryTaskStore::with_clock({
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        });
        let sqlite = SqliteStore::open_in_memory_with_clock({
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        })
        .unwrap();

        // Usage finalization persists all usage fields at storage time, not event time.
        let usage_ctx = ctx_for("parity-usage", "ctx-final-parity", "task-1", 0);
        finish_for_finalization(&memory, &usage_ctx).await;
        finish_for_finalization(&sqlite, &usage_ctx).await;
        let usage = TurnLogFinalized {
            ctx: usage_ctx.clone(),
            finalization: TurnUsageFinalization::Usage(sqlite_usage(3, 4, 1)),
        };
        let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &usage).await;
        assert!(memory_result.is_ok() && sqlite_result.is_ok());
        let first_usage_row = parity_turn_row(&memory, &sqlite, &usage_ctx.turn_id).await;
        assert_eq!(first_usage_row.input_tokens, Some(3));
        assert_eq!(first_usage_row.output_tokens, Some(4));
        assert_eq!(first_usage_row.usage_finalized_ms, Some(88_000));
        assert_ne!(first_usage_row.usage_finalized_ms, Some(1));
        assert_eq!(first_usage_row.usage_finalization_kind, "usage");

        // A same-kind duplicate is idempotent: later storage time and payload do not overwrite.
        clock.store(88_100, Ordering::SeqCst);
        let duplicate_usage = TurnLogFinalized {
            ctx: usage_ctx.clone(),
            finalization: TurnUsageFinalization::Usage(sqlite_usage(30, 40, 2)),
        };
        let (memory_result, sqlite_result) =
            finalize_both(&memory, &sqlite, &duplicate_usage).await;
        assert!(memory_result.is_ok() && sqlite_result.is_ok());
        let duplicate_usage_row = parity_turn_row(&memory, &sqlite, &usage_ctx.turn_id).await;
        assert_eq!(duplicate_usage_row, first_usage_row);

        // A contradictory finalization kind is rejected without changing the stored usage row.
        let contradictory = TurnLogFinalized {
            ctx: usage_ctx.clone(),
            finalization: TurnUsageFinalization::NoUsage,
        };
        let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &contradictory).await;
        assert!(memory_result.is_err() && sqlite_result.is_err());
        assert_eq!(
            parity_turn_row(&memory, &sqlite, &usage_ctx.turn_id).await,
            first_usage_row
        );

        // Explicit no-usage and its duplicate have the same barrier semantics.
        clock.store(89_000, Ordering::SeqCst);
        let no_usage_ctx = ctx_for("parity-no-usage", "ctx-final-parity", "task-1", 0);
        finish_for_finalization(&memory, &no_usage_ctx).await;
        finish_for_finalization(&sqlite, &no_usage_ctx).await;
        let no_usage = TurnLogFinalized {
            ctx: no_usage_ctx.clone(),
            finalization: TurnUsageFinalization::NoUsage,
        };
        let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &no_usage).await;
        assert!(memory_result.is_ok() && sqlite_result.is_ok());
        let first_no_usage_row = parity_turn_row(&memory, &sqlite, &no_usage_ctx.turn_id).await;
        assert_eq!(first_no_usage_row.input_tokens, None);
        assert_eq!(first_no_usage_row.cost_amount, None);
        assert_eq!(first_no_usage_row.usage_finalized_ms, Some(89_000));
        assert_eq!(first_no_usage_row.usage_finalization_kind, "no_usage");

        clock.store(89_100, Ordering::SeqCst);
        let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &no_usage).await;
        assert!(memory_result.is_ok() && sqlite_result.is_ok());
        assert_eq!(
            parity_turn_row(&memory, &sqlite, &no_usage_ctx.turn_id).await,
            first_no_usage_row
        );

        // Unknown turns fail identically and do not synthesize rows.
        let unknown_ctx = ctx_for("parity-unknown", "ctx-final-parity", "task-1", 0);
        let unknown = TurnLogFinalized {
            ctx: unknown_ctx.clone(),
            finalization: TurnUsageFinalization::NoUsage,
        };
        let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &unknown).await;
        assert!(memory_result.is_err() && sqlite_result.is_err());
        assert!(memory
            .turn_log_row(&unknown_ctx.turn_id)
            .await
            .unwrap()
            .is_none());
        assert!(sqlite
            .turn_log_row(&unknown_ctx.turn_id)
            .await
            .unwrap()
            .is_none());

        // An invalid zero clock and the explicit sentinel both fail closed to the sentinel.
        for (turn, clock_value) in [
            ("parity-invalid-clock", 0),
            ("parity-sentinel-clock", RETENTION_NEVER_ELIGIBLE_MS),
        ] {
            clock.store(clock_value, Ordering::SeqCst);
            let ctx = ctx_for(turn, "ctx-final-parity", "task-1", 0);
            finish_for_finalization(&memory, &ctx).await;
            finish_for_finalization(&sqlite, &ctx).await;
            let finalized = TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            };
            let (memory_result, sqlite_result) = finalize_both(&memory, &sqlite, &finalized).await;
            assert!(memory_result.is_ok() && sqlite_result.is_ok());
            let row = parity_turn_row(&memory, &sqlite, &ctx.turn_id).await;
            assert_eq!(row.usage_finalized_ms, Some(RETENTION_NEVER_ELIGIBLE_MS));
            assert_ne!(row.usage_finalized_ms, Some(0));
            assert_eq!(row.usage_finalization_kind, "no_usage");
        }
    }

    async fn assert_task_recency<S: TaskStore + ?Sized>(
        store: &S,
        task: &TaskId,
        expected_ms: i64,
    ) {
        assert_eq!(
            store.get(task).await.unwrap().unwrap().last_artifact_ms,
            Some(expected_ms),
            "unexpected storage-authored recency for {}",
            task.as_str()
        );
    }

    async fn assert_all_seven_writer_recencies<S: TaskStore + ?Sized>(
        store: &S,
        clock: &Arc<AtomicI64>,
    ) {
        const STALE_CALLER_MS: i64 = 1;

        let checkpoint_task = TaskId::parse("recency-legacy-checkpoint").unwrap();
        store
            .create(&trec(checkpoint_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_001, Ordering::SeqCst);
        store
            .put_node_checkpoint(
                &checkpoint_task,
                &bridge_core::ids::NodeId::parse("legacy-node").unwrap(),
                "checkpoint",
                true,
                STALE_CALLER_MS,
            )
            .await
            .unwrap();
        assert_task_recency(store, &checkpoint_task, 100_001).await;

        let start_task = TaskId::parse("recency-node-start").unwrap();
        store
            .create(&trec(start_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_002, Ordering::SeqCst);
        store
            .record_node_started(
                &start_task,
                &bridge_core::ids::NodeId::parse("started-node").unwrap(),
                &OperationId::parse("op-recency-node-start").unwrap(),
                STALE_CALLER_MS,
            )
            .await
            .unwrap();
        assert_task_recency(store, &start_task, 100_002).await;

        let sequenced_checkpoint_task = TaskId::parse("recency-sequenced-checkpoint").unwrap();
        store
            .create(&trec(sequenced_checkpoint_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_003, Ordering::SeqCst);
        store
            .put_node_checkpoint_sequenced(
                &sequenced_checkpoint_task,
                &bridge_core::ids::NodeId::parse("sequenced-node").unwrap(),
                &OperationId::parse("op-recency-sequenced-checkpoint").unwrap(),
                "checkpoint",
                true,
                STALE_CALLER_MS,
                None,
            )
            .await
            .unwrap();
        assert_task_recency(store, &sequenced_checkpoint_task, 100_003).await;

        let terminal_task = TaskId::parse("recency-terminal").unwrap();
        store
            .create(&trec(terminal_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_004, Ordering::SeqCst);
        store
            .set_terminal_sequenced(
                &terminal_task,
                &OperationId::parse("op-recency-terminal").unwrap(),
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                STALE_CALLER_MS,
            )
            .await
            .unwrap();
        assert_task_recency(store, &terminal_task, 100_004).await;

        let rich_event_task = TaskId::parse("recency-rich-event").unwrap();
        store
            .create(&trec(rich_event_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_005, Ordering::SeqCst);
        store
            .record_event_sequenced(
                &rich_event_task,
                &OperationId::parse("op-recency-rich-event").unwrap(),
                STALE_CALLER_MS,
                bridge_core::orch::OrchEventKind::Plan { entries: vec![] },
            )
            .await
            .unwrap();
        assert_task_recency(store, &rich_event_task, 100_005).await;

        let finished_task = TaskId::parse("recency-turn-finished").unwrap();
        store
            .create(&trec(finished_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        let finished_ctx = ctx_for(
            "recency-turn-finished-row",
            "ctx-recency-writers",
            finished_task.as_str(),
            0,
        );
        clock.store(100_006, Ordering::SeqCst);
        store
            .upsert_turn_finished(&sqlite_finished(finished_ctx, STALE_CALLER_MS))
            .await
            .unwrap();
        assert_task_recency(store, &finished_task, 100_006).await;

        let finalized_task = TaskId::parse("recency-turn-finalized").unwrap();
        store
            .create(&trec(finalized_task.as_str(), STALE_CALLER_MS))
            .await
            .unwrap();
        let finalized_ctx = ctx_for(
            "recency-turn-finalized-row",
            "ctx-recency-writers",
            finalized_task.as_str(),
            0,
        );
        clock.store(100_006, Ordering::SeqCst);
        store
            .upsert_turn_finished(&sqlite_finished(finalized_ctx.clone(), STALE_CALLER_MS))
            .await
            .unwrap();
        clock.store(100_007, Ordering::SeqCst);
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: finalized_ctx,
                finalization: TurnUsageFinalization::Usage(sqlite_usage(1, 2, STALE_CALLER_MS)),
            })
            .await
            .unwrap();
        assert_task_recency(store, &finalized_task, 100_007).await;
    }

    #[tokio::test]
    async fn storage_authored_recency_covers_all_seven_writer_families() {
        let memory_clock = Arc::new(AtomicI64::new(0));
        let memory = MemoryTaskStore::with_clock({
            let clock = Arc::clone(&memory_clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        });
        assert_all_seven_writer_recencies(&memory, &memory_clock).await;

        let sqlite_clock = Arc::new(AtomicI64::new(0));
        let sqlite = SqliteStore::open_in_memory_with_clock({
            let clock = Arc::clone(&sqlite_clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        })
        .unwrap();
        assert_all_seven_writer_recencies(&sqlite, &sqlite_clock).await;
    }

    #[tokio::test]
    async fn sqlite_legacy_migration_is_ddl_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY,
                    workflow TEXT NOT NULL,
                    status TEXT NOT NULL,
                    result TEXT,
                    error TEXT,
                    created_ms INTEGER NOT NULL,
                    updated_ms INTEGER NOT NULL,
                    input TEXT NOT NULL DEFAULT '',
                    workflow_spec_json TEXT,
                    resume_attempts INTEGER NOT NULL DEFAULT 0,
                    session_cwd TEXT,
                    last_event_seq INTEGER NOT NULL DEFAULT 0,
                    terminal_seq INTEGER,
                    journal_complete_from_birth INTEGER NOT NULL DEFAULT 0,
                    batch_id TEXT,
                    item_id TEXT
                );
                CREATE TABLE turn_log (
                    turn_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    task_id TEXT,
                    workflow TEXT,
                    node TEXT,
                    attempt INTEGER NOT NULL,
                    agent TEXT NOT NULL,
                    model TEXT,
                    effort TEXT,
                    mode TEXT,
                    prompt_id TEXT,
                    started_ms INTEGER,
                    completed_ms INTEGER,
                    latency_ms INTEGER,
                    ttft_ms INTEGER,
                    outcome TEXT,
                    failure_class TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    thought_tokens INTEGER,
                    cached_read_tokens INTEGER,
                    cached_write_tokens INTEGER,
                    cost_amount REAL,
                    cost_currency TEXT,
                    traceparent TEXT
                );
                INSERT INTO tasks(id, workflow, status, created_ms, updated_ms, input)
                VALUES('task-legacy', 'code-review', 'completed', 1, 2, 'input');
                INSERT INTO turn_log(
                    turn_id, session_id, task_id, workflow, node, attempt, agent,
                    completed_ms, outcome, input_tokens, output_tokens, cost_amount, cost_currency
                ) VALUES(
                    'turn-legacy', 'ctx-legacy', 'task-legacy', 'code-review', 'reviewer', 0,
                    'codex', 3, 'success', 5, 7, 0.42, 'USD'
                );",
            )
            .unwrap();
        }

        let store = SqliteStore::open(&path).unwrap();
        let conn = store.conn.lock().unwrap();
        let task_cols: (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT last_artifact_ms, artifacts_purged_at FROM tasks WHERE id='task-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(task_cols, (None, None));
        let turn: LegacyTurnColumns = conn
            .query_row(
                "SELECT usage_finalized_ms, usage_finalization_kind, input_tokens, output_tokens,
                        cost_amount, cost_currency, task_id
                 FROM turn_log WHERE turn_id='turn-legacy'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            turn,
            (
                None,
                "pending".to_string(),
                Some(5),
                Some(7),
                Some(0.42),
                Some("USD".to_string()),
                Some("task-legacy".to_string())
            )
        );
    }

    #[tokio::test]
    async fn sqlite_migration_idempotent_and_batch_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let _s = SqliteStore::open(&path).unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        let bid = BatchId::parse("b1").unwrap();
        s.create_batch(&sample_batch(&bid, BatchStatus::Working, 2, 0))
            .await
            .unwrap();

        let got = s.get_batch(&bid).await.unwrap().unwrap();
        assert_eq!(got.total, 2);
        assert_eq!(got.status, BatchStatus::Working);
    }

    #[tokio::test]
    async fn sqlite_claim_is_atomic_single_runner() {
        let s = SqliteStore::open_in_memory().unwrap();
        let bid = BatchId::parse("b1").unwrap();
        s.create_batch(&sample_batch(&bid, BatchStatus::Working, 1, 0))
            .await
            .unwrap();

        let a = s
            .claim_batch_child(
                &bid,
                "x",
                &batch_child_record(&TaskId::parse("t1").unwrap(), &bid, "x"),
            )
            .await
            .unwrap();
        let b = s
            .claim_batch_child(
                &bid,
                "x",
                &batch_child_record(&TaskId::parse("t2").unwrap(), &bid, "x"),
            )
            .await
            .unwrap();

        assert_eq!((a, b), (ChildClaim::Created, ChildClaim::ExistingWorking));
        let children = s.batch_children(&bid).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].batch_id.as_ref(), Some(&bid));
        assert_eq!(children[0].item_id.as_deref(), Some("x"));
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
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: Some("{\"v\":1}".into()),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
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
    async fn node_checkpoint_roundtrips_usage_and_old_rows_read_none() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("t-usage").unwrap();
        let op = OperationId::parse("op-t-usage").unwrap();
        store.create(&trec("t-usage", 1)).await.unwrap();
        let node = NodeId::parse("member").unwrap();
        let usage = bridge_core::orch::UsageSnapshot {
            used: Some(15071),
            size: Some(258400),
            cost: None,
            terminal: None,
            at_ms: 7,
        };
        store
            .put_node_checkpoint_sequenced(&task, &node, &op, "OUT", true, 7, Some(&usage))
            .await
            .unwrap();

        let cps = store.node_checkpoints(&task).await.unwrap();
        assert_eq!(cps.len(), 1);
        let (n, out, ok, got) = &cps[0];
        assert_eq!(n.as_str(), "member");
        assert_eq!(out, "OUT");
        assert!(ok);
        assert_eq!(got.as_ref().unwrap().used, Some(15071));

        let evs = store.journal_from(&task, -1).await.unwrap();
        assert!(matches!(
            &evs[0].kind,
            bridge_core::orch::OrchEventKind::NodeFinished { usage: Some(got), .. } if got == &usage
        ));

        let node2 = NodeId::parse("legacy").unwrap();
        store
            .put_node_checkpoint_sequenced(&task, &node2, &op, "L", true, 8, None)
            .await
            .unwrap();
        let cps = store.node_checkpoints(&task).await.unwrap();
        let legacy = cps
            .iter()
            .find(|(node, ..)| node.as_str() == "legacy")
            .unwrap();
        assert!(legacy.3.is_none(), "absent usage reads back as None");
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
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: Some("/req".to_string()),
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
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
                assert!(
                    cp_cols.contains("usage_json"),
                    "task_node_checkpoints.usage_json must exist after migration"
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
            let cps = s.node_checkpoints(&old).await.unwrap();
            let legacy = cps.iter().find(|(node, ..)| node.as_str() == "n").unwrap();
            assert!(legacy.3.is_none(), "legacy NULL usage_json maps to None");
            // A seq write on the freshly-migrated task works from the DEFAULT 0 baseline.
            let op = OperationId::parse("op-old").unwrap();
            let first = s
                .record_node_started(&old, &NodeId::parse("m").unwrap(), &op, 10)
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
    async fn migration_adds_journal_table_and_birth_flag() {
        let store = SqliteStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let tbl: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='task_journal'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tbl, 1);
        let col: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('tasks') WHERE name='journal_complete_from_birth'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(col, 1);
    }

    #[tokio::test]
    async fn sqlite_seq_and_snapshot() {
        let s = SqliteStore::open_in_memory().unwrap();
        use bridge_core::ids::NodeId;
        let t = TaskId::parse("t").unwrap();
        s.create(&trec("t", 1)).await.unwrap();
        let op = OperationId::parse("op-t").unwrap();
        let s1 = s
            .record_node_started(&t, &NodeId::parse("a").unwrap(), &op, 1)
            .await
            .unwrap();
        let s2 = s
            .put_node_checkpoint_sequenced(
                &t,
                &NodeId::parse("a").unwrap(),
                &op,
                "OUT",
                true,
                2,
                None,
            )
            .await
            .unwrap();
        assert!(s2 > s1);
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.checkpoints[0].3, s2); // seq carried
        assert!(snap.starts.is_empty()); // start cleared on finish
                                         // record_node_started is an UPSERT (resume re-emit): no PK error
        let r1 = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), &op, 3)
            .await
            .unwrap();
        let r2 = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), &op, 4)
            .await
            .unwrap();
        assert!(r2 > r1);
        let term = s
            .set_terminal_sequenced(&t, &op, TaskRecordStatus::Completed, Some("R"), None, 5)
            .await
            .unwrap();
        assert_eq!(
            s.progress_snapshot(&t).await.unwrap().terminal_seq,
            Some(term)
        );
    }

    async fn journal_write_matches_typed<S: bridge_core::task_store::TaskStore>(store: S) {
        use bridge_core::ids::{NodeId, OperationId};
        use bridge_core::orch::OrchEventKind;
        let t = TaskId::parse("task-j").unwrap();
        store.create(&trec("task-j", 1)).await.unwrap();
        let a = NodeId::parse("a").unwrap();
        let op = OperationId::parse("op-task-j").unwrap();
        let usage = bridge_core::orch::UsageSnapshot {
            used: Some(15071),
            size: Some(258400),
            cost: None,
            terminal: None,
            at_ms: 7,
        };
        let s1 = store.record_node_started(&t, &a, &op, 1).await.unwrap();
        let s2 = store
            .put_node_checkpoint_sequenced(&t, &a, &op, "oA", true, 2, Some(&usage))
            .await
            .unwrap();
        let evs = store.journal_from(&t, -1).await.unwrap();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0].kind, OrchEventKind::NodeStarted { .. }) && evs[0].seq == s1);
        assert!(
            matches!(&evs[1].kind, OrchEventKind::NodeFinished { output, usage: Some(got), .. } if output == "oA" && got == &usage)
                && evs[1].seq == s2
        );
        assert_eq!(evs[0].operation_id.as_str(), "op-task-j");
    }

    #[tokio::test]
    async fn sqlite_journal_write() {
        journal_write_matches_typed(SqliteStore::open_in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_journal_write() {
        journal_write_matches_typed(bridge_core::task_store::MemoryTaskStore::new()).await;
    }

    async fn rich_event_journals<S: bridge_core::task_store::TaskStore>(store: S) {
        use bridge_core::orch::OrchEventKind;

        let t = TaskId::parse("task-r").unwrap();
        store.create(&trec("task-r", 1)).await.unwrap();
        let op = OperationId::parse("op-task-r").unwrap();
        let seq = store
            .record_event_sequenced(&t, &op, 7, OrchEventKind::Plan { entries: vec![] })
            .await
            .unwrap();
        let evs = store.journal_from(&t, -1).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert!(
            matches!(evs[0].kind, OrchEventKind::Plan { .. })
                && evs[0].seq == seq
                && evs[0].operation_id.as_str() == "op-task-r"
        );
        let snap = store.progress_snapshot(&t).await.unwrap();
        assert!(snap.checkpoints.is_empty() && snap.starts.is_empty());
    }

    #[tokio::test]
    async fn sqlite_rich_event() {
        rich_event_journals(SqliteStore::open_in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_rich_event() {
        rich_event_journals(bridge_core::task_store::MemoryTaskStore::new()).await;
    }

    async fn diagnostic_event_journals<S: bridge_core::task_store::TaskStore>(store: S) {
        use bridge_core::diagnostics::{
            DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
            PersistedPhaseTransitionInput, PhaseStatus,
        };
        use bridge_core::orch::OrchEventKind;

        let task = TaskId::parse("task-diagnostic").unwrap();
        let operation = OperationId::parse("op-task-diagnostic").unwrap();
        store.create(&trec("task-diagnostic", 1)).await.unwrap();
        let diagnostic = DiagnosticEvent::new(
            PersistedPhaseTransition::build(
                PersistedPhaseTransitionInput {
                    phase: DiagnosticPhase::Initialize,
                    status: PhaseStatus::Started,
                    at_ms: 7,
                    operation: None,
                    code: Some("acp.initialize.started".into()),
                    auth: None,
                },
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
            None,
        )
        .unwrap();
        let seq = store
            .record_event_sequenced(
                &task,
                &operation,
                7,
                OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::diagnostic(diagnostic),
                },
            )
            .await
            .unwrap();

        let events = store.journal_from(&task, -1).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].kind,
            OrchEventKind::Progress { progress }
                if progress.text() == "diagnostic transition"
                    && progress.diagnostic_event().is_some()
        ));
        assert_eq!(events[0].seq, seq);
        let snapshot = store.progress_snapshot(&task).await.unwrap();
        assert!(snapshot.checkpoints.is_empty() && snapshot.starts.is_empty());
    }

    #[tokio::test]
    async fn sqlite_diagnostic_event() {
        diagnostic_event_journals(SqliteStore::open_in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_diagnostic_event() {
        diagnostic_event_journals(bridge_core::task_store::MemoryTaskStore::new()).await;
    }

    #[tokio::test]
    async fn sqlite_journal_jsonl_bounded_body_and_counts() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-journal").unwrap();
        let op = OperationId::parse("op-journal").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();

        store
            .record_event_sequenced(
                &task,
                &op,
                10,
                bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("one"),
                },
            )
            .await
            .unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                11,
                bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("two"),
                },
            )
            .await
            .unwrap();

        let read = store
            .journal_jsonl_bounded(&task, 10, 10_000)
            .await
            .unwrap();

        match read {
            bridge_core::task_store::JournalRead::Body {
                jsonl,
                events,
                bytes,
            } => {
                assert_eq!(events, 2);
                assert_eq!(bytes as usize, jsonl.len());
                assert!(jsonl.ends_with('\n'));
                let parsed = jsonl
                    .lines()
                    .map(|line| serde_json::from_str::<bridge_core::orch::OrchEvent>(line).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(parsed.len(), 2);
                assert_eq!(parsed[0].seq, 1);
                assert_eq!(parsed[1].seq, 2);
            }
            other => panic!("expected body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sqlite_journal_jsonl_bounded_too_large_over_events() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-journal").unwrap();
        let op = OperationId::parse("op-journal").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                10,
                bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("one"),
                },
            )
            .await
            .unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                11,
                bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("two"),
                },
            )
            .await
            .unwrap();

        assert!(matches!(
            store.journal_jsonl_bounded(&task, 1, 10_000).await.unwrap(),
            bridge_core::task_store::JournalRead::TooLarge { events: 2, .. }
        ));
    }

    #[tokio::test]
    async fn sqlite_journal_jsonl_bounded_too_large_over_bytes() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-journal").unwrap();
        let op = OperationId::parse("op-journal").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                10,
                bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("one"),
                },
            )
            .await
            .unwrap();

        assert!(matches!(
            store.journal_jsonl_bounded(&task, 10, 1).await.unwrap(),
            bridge_core::task_store::JournalRead::TooLarge { events: 1, .. }
        ));
    }

    #[tokio::test]
    async fn sqlite_node_checkpoint_nodes_metadata_only() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-artifact").unwrap();
        let op = OperationId::parse("op-artifact").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();

        store
            .put_node_checkpoint(
                &task,
                &NodeId::parse("legacy").unwrap(),
                "legacy output",
                true,
                10,
            )
            .await
            .unwrap();
        store
            .put_node_checkpoint_sequenced(
                &task,
                &NodeId::parse("later").unwrap(),
                &op,
                "later output",
                true,
                11,
                None,
            )
            .await
            .unwrap();

        let nodes = store.node_checkpoint_nodes(&task).await.unwrap();

        assert_eq!(
            nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["legacy", "later"]
        );
    }

    #[tokio::test]
    async fn sqlite_node_checkpoint_output_too_large_single_statement() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-artifact").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        store
            .put_node_checkpoint(&task, &NodeId::parse("node-a").unwrap(), "abcdef", true, 10)
            .await
            .unwrap();

        let found = store
            .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 6)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found,
            bridge_core::task_store::NodeCheckpointOutput::Found {
                output: "abcdef".into(),
                ok: true,
                usage: None,
                bytes: 6
            }
        );

        let too_large = store
            .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 5)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            too_large,
            bridge_core::task_store::NodeCheckpointOutput::TooLarge { bytes: 6 }
        );

        assert!(store
            .node_checkpoint_output(&task, &NodeId::parse("missing").unwrap(), 5)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn sqlite_node_checkpoint_output_huge_cap_does_not_wrap_negative() {
        // Regression: `max_bytes as i64` would wrap usize::MAX to -1, making the
        // `<= ?3` gate reject every artifact. Saturating to i64::MAX keeps small
        // outputs `Found` even under an absurd (config-reachable) cap.
        let store = SqliteStore::open_in_memory().unwrap();
        let task = TaskId::parse("task-artifact-huge").unwrap();
        store.create(&trec(task.as_str(), 1)).await.unwrap();
        store
            .put_node_checkpoint(&task, &NodeId::parse("node-a").unwrap(), "a", true, 10)
            .await
            .unwrap();

        let found = store
            .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), usize::MAX)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found,
            bridge_core::task_store::NodeCheckpointOutput::Found {
                output: "a".into(),
                ok: true,
                usage: None,
                bytes: 1
            }
        );
    }

    #[tokio::test]
    async fn sqlite_turn_log_upserts_finished_then_usage_and_keeps_attempts_separate() {
        let store = SqliteStore::open_in_memory().unwrap();
        let first = TurnLogFinished {
            ctx: ctx("turn-a", 0),
            started_ms: 100,
            completed_ms: 250,
            latency: std::time::Duration::from_millis(150),
            ttft: Some(std::time::Duration::from_millis(12)),
            outcome: TurnOutcome::Failed(FailureClass::TimedOut),
        };
        store.upsert_turn_finished(&first).await.unwrap();
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: first.ctx.clone(),
                finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                    used: Some(50),
                    size: Some(1000),
                    cost: Some(UsageCost {
                        amount: 0.42,
                        currency: "USD".to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 12,
                        input_tokens: 5,
                        output_tokens: 7,
                        thought_tokens: Some(1),
                        cached_read_tokens: Some(2),
                        cached_write_tokens: Some(3),
                    }),
                    at_ms: 251,
                }),
            })
            .await
            .unwrap();

        let retry = TurnLogFinished {
            ctx: ctx("turn-b", 1),
            started_ms: 300,
            completed_ms: 450,
            latency: std::time::Duration::from_millis(150),
            ttft: None,
            outcome: TurnOutcome::Success,
        };
        store.upsert_turn_finished(&retry).await.unwrap();

        let rows = store.turn_log_rows().await.unwrap();
        assert_eq!(rows.len(), 2);
        let row = rows
            .iter()
            .find(|r| r.turn_id.as_str() == "turn-a")
            .unwrap();
        assert_eq!(row.session_id.as_str(), "ctx-1");
        assert_eq!(row.task_id.as_ref().unwrap().as_str(), "task-1");
        assert_eq!(row.workflow.as_deref(), Some("code-review"));
        assert_eq!(row.node.as_deref(), Some("reviewer"));
        assert_eq!(row.attempt, 0);
        assert_eq!(row.outcome.as_deref(), Some("failed"));
        assert_eq!(row.failure_class.as_deref(), Some("timed_out"));
        assert_eq!(row.input_tokens, Some(5));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.thought_tokens, Some(1));
        assert_eq!(row.cached_read_tokens, Some(2));
        assert_eq!(row.cached_write_tokens, Some(3));
        assert_eq!(row.cost_amount, Some(0.42));
        assert_eq!(row.cost_currency.as_deref(), Some("USD"));
        assert_eq!(
            row.traceparent.as_ref().unwrap().to_header_value(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        assert!(rows
            .iter()
            .any(|r| r.turn_id.as_str() == "turn-b" && r.attempt == 1));
    }

    #[tokio::test]
    async fn sqlite_duplicate_usage_finalization_is_idempotent() {
        let store = SqliteStore::open_in_memory().unwrap();
        let base = TurnLogFinished {
            ctx: ctx("turn-duplicate-finalization", 0),
            started_ms: 100,
            completed_ms: 200,
            latency: std::time::Duration::from_millis(100),
            ttft: Some(std::time::Duration::from_millis(10)),
            outcome: TurnOutcome::Success,
        };
        store.upsert_turn_finished(&base).await.unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: base.ctx.clone(),
                finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                    used: None,
                    size: None,
                    cost: Some(UsageCost {
                        amount: 1.23,
                        currency: "USD".to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 12,
                        input_tokens: 6,
                        output_tokens: 6,
                        thought_tokens: Some(2),
                        cached_read_tokens: Some(0),
                        cached_write_tokens: None,
                    }),
                    at_ms: 205,
                }),
            })
            .await
            .unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: base.ctx.clone(),
                finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                    used: None,
                    size: None,
                    cost: Some(UsageCost {
                        amount: 9.99,
                        currency: "EUR".to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: 200,
                        input_tokens: 100,
                        output_tokens: 100,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    at_ms: 210,
                }),
            })
            .await
            .unwrap();

        let row = store
            .turn_log_rows()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.turn_id.as_str() == "turn-duplicate-finalization")
            .unwrap();
        assert_eq!(row.input_tokens, Some(6));
        assert_eq!(row.output_tokens, Some(6));
        assert_eq!(row.thought_tokens, Some(2));
        assert_eq!(row.cached_read_tokens, Some(0));
        assert_eq!(row.cost_amount, Some(1.23));
        assert_eq!(row.cost_currency.as_deref(), Some("USD"));
        assert!(row.usage_finalized_ms.is_some());
        assert_eq!(row.usage_finalization_kind, "usage");
    }

    #[tokio::test]
    async fn sqlite_turn_log_row_lookup() {
        let store = SqliteStore::open_in_memory().unwrap();
        write_sqlite_turn(
            &store,
            ctx_for("turn-a", "ctx-a", "task-a", 0),
            20,
            2,
            4,
            Some(("USD", 0.25)),
        )
        .await;

        let row = store
            .turn_log_row(&TurnId::parse("turn-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(row.turn_id.as_str(), "turn-a");
        assert_eq!(row.session_id.as_str(), "ctx-a");
        assert_eq!(row.task_id.as_ref().unwrap().as_str(), "task-a");
        assert_eq!(row.input_tokens, Some(2));
        assert_eq!(row.output_tokens, Some(4));
        assert_eq!(row.cost_currency.as_deref(), Some("USD"));
        assert_eq!(
            row.traceparent.unwrap().to_header_value(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );

        assert!(store
            .turn_log_row(&TurnId::parse("missing").unwrap())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn sqlite_turn_log_rows_for_task_orders_and_limits() {
        let store = SqliteStore::open_in_memory().unwrap();
        write_sqlite_turn(
            &store,
            ctx_for("turn-c", "ctx-a", "task-a", 0),
            30,
            1,
            1,
            None,
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-a", "ctx-a", "task-a", 0),
            10,
            1,
            1,
            None,
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-b", "ctx-a", "task-a", 0),
            20,
            1,
            1,
            None,
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-x", "ctx-a", "task-x", 0),
            5,
            1,
            1,
            None,
        )
        .await;

        let rows = store
            .turn_log_rows_for_task(&TaskId::parse("task-a").unwrap(), 2)
            .await
            .unwrap();

        assert_eq!(
            rows.iter().map(|r| r.turn_id.as_str()).collect::<Vec<_>>(),
            vec!["turn-a", "turn-b"]
        );
    }

    #[tokio::test]
    async fn sqlite_turn_log_usage_for_task_sums_all_rows() {
        let store = SqliteStore::open_in_memory().unwrap();
        for i in 0..513 {
            write_sqlite_turn(
                &store,
                ctx_for(&format!("turn-{i:03}"), "ctx-a", "task-a", 0),
                i,
                2,
                3,
                Some(("USD", 0.01)),
            )
            .await;
        }

        let agg = store
            .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(agg.rows, 513);
        assert_eq!(agg.input_tokens, 1026);
        assert_eq!(agg.output_tokens, 1539);
        assert_eq!(agg.thought_tokens, Some(513));
        assert_eq!(agg.cached_read_tokens, Some(1026));
        assert_eq!(agg.cached_write_tokens, None);
        assert_eq!(agg.cost.as_ref().unwrap().currency, "USD");
        assert!((agg.cost.as_ref().unwrap().amount - 5.13).abs() < 0.000_001);
        assert_eq!(agg.at_ms, 512);
    }

    #[tokio::test]
    async fn sqlite_turn_log_usage_for_task_cost_none_on_mixed_currency() {
        let store = SqliteStore::open_in_memory().unwrap();
        write_sqlite_turn(
            &store,
            ctx_for("turn-usd", "ctx-a", "task-a", 0),
            10,
            2,
            3,
            Some(("USD", 0.10)),
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-eur", "ctx-a", "task-a", 0),
            20,
            5,
            7,
            Some(("EUR", 0.20)),
        )
        .await;

        let agg = store
            .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(agg.input_tokens, 7);
        assert_eq!(agg.output_tokens, 10);
        assert!(agg.cost.is_none());
    }

    #[tokio::test]
    async fn sqlite_latest_turn_log_row_for_session_returns_latest() {
        let store = SqliteStore::open_in_memory().unwrap();
        write_sqlite_turn(
            &store,
            ctx_for("turn-old", "ctx-a", "task-a", 0),
            10,
            1,
            1,
            None,
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-new", "ctx-a", "task-a", 0),
            20,
            1,
            1,
            None,
        )
        .await;
        write_sqlite_turn(
            &store,
            ctx_for("turn-other", "ctx-b", "task-a", 0),
            30,
            1,
            1,
            None,
        )
        .await;

        let row = store
            .latest_turn_log_row_for_session(&ContextId::parse("ctx-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(row.turn_id.as_str(), "turn-new");
    }

    #[tokio::test]
    async fn sqlite_finalize_turn_usage_unknown_turn_returns_error() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx("missing-turn", 0),
                finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                    used: None,
                    size: None,
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                }),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::StoreFailure));
    }

    #[tokio::test]
    async fn create_sets_birth_flag_and_fold_inputs_consistent() {
        let store = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("task-b").unwrap();
        store.create(&trec("task-b", 1)).await.unwrap();
        let a = bridge_core::ids::NodeId::parse("a").unwrap();
        let op = OperationId::parse("op-task-b").unwrap();
        store.record_node_started(&t, &a, &op, 1).await.unwrap();
        store
            .put_node_checkpoint_sequenced(&t, &a, &op, "oA", true, 2, None)
            .await
            .unwrap();

        let fi = store.journal_fold_inputs(&t).await.unwrap();
        assert!(fi.complete_from_birth);
        assert_eq!(fi.events.len(), 2);
        assert_eq!(fi.scalars.cut_seq, 2);
    }

    async fn duplicate_sequenced_checkpoint_is_write_once<S: bridge_core::task_store::TaskStore>(
        store: S,
    ) {
        use bridge_core::ids::{NodeId, OperationId};
        let t = TaskId::parse("task-dup-seq").unwrap();
        store.create(&trec("task-dup-seq", 1)).await.unwrap();
        let a = NodeId::parse("a").unwrap();
        let op = OperationId::parse("op-task-dup-seq").unwrap();
        let first = store
            .put_node_checkpoint_sequenced(&t, &a, &op, "first", true, 1, None)
            .await
            .unwrap();
        let duplicate = store
            .put_node_checkpoint_sequenced(&t, &a, &op, "second", true, 2, None)
            .await;
        assert!(duplicate.is_err());
        let snap = store.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.checkpoints.len(), 1);
        assert_eq!(snap.checkpoints[0].1, "first");
        assert_eq!(snap.checkpoints[0].3, first);
        let evs = store.journal_from(&t, -1).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].seq, first);
    }

    #[tokio::test]
    async fn sqlite_duplicate_sequenced_checkpoint_is_write_once() {
        duplicate_sequenced_checkpoint_is_write_once(SqliteStore::open_in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn memory_duplicate_sequenced_checkpoint_is_write_once() {
        duplicate_sequenced_checkpoint_is_write_once(
            bridge_core::task_store::MemoryTaskStore::new(),
        )
        .await;
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
        let op = OperationId::parse("op-t").unwrap();
        let a = s
            .put_node_checkpoint_sequenced(
                &t,
                &NodeId::parse("a").unwrap(),
                &op,
                "A",
                true,
                1,
                None,
            )
            .await
            .unwrap();
        let b = s
            .record_node_started(&t, &NodeId::parse("b").unwrap(), &op, 2)
            .await
            .unwrap();
        assert!(b > a, "seq continues across a resumed run, not reset");
    }

    #[tokio::test]
    async fn file_backed_open_sets_wal_synchronous_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pragmas.db");
        let s = SqliteStore::open(&path).unwrap();
        let conn = s.conn.lock().unwrap();
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 1);
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
    }

    #[tokio::test]
    async fn in_memory_open_sets_busy_timeout_only() {
        let s = SqliteStore::open_in_memory().unwrap();
        let conn = s.conn.lock().unwrap();
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy_timeout, 5000);
    }
}

#[cfg(test)]
mod r2f0a_history_tests {
    use super::*;
    use bridge_core::workflow_history::{
        AttemptReservation, AttemptTerminal, DirectAttemptBarrier, ExecutionSurface, NodeCounts,
        TerminalWrite, WorkflowHistoryStore,
    };

    fn reservation() -> AttemptReservation {
        AttemptReservation {
            identity: bridge_core::ids::AttemptIdentity::initial().unwrap(),
            task_id: None,
            workflow: "code-review".into(),
            task_class: "review".into(),
            surface: ExecutionSurface::Offline,
            policy: "r2f0a".into(),
            workload_fingerprint: "shape_abc123".into(),
            started_ms: 1_000,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        }
    }

    fn reservation_with_ids(execution_id: &str, attempt_id: &str) -> AttemptReservation {
        let mut row = reservation();
        row.identity = bridge_core::ids::AttemptIdentity {
            execution_id: bridge_core::ids::ExecutionId::parse(execution_id).unwrap(),
            attempt_id: bridge_core::ids::AttemptId::parse(attempt_id).unwrap(),
            ordinal: 0,
            parent_attempt_id: None,
        };
        row
    }

    fn child_reservation() -> AttemptReservation {
        reservation_with_ids(
            "exec-11111111111111111111111111111111",
            "attempt-22222222222222222222222222222222",
        )
    }

    fn parent_reservation() -> AttemptReservation {
        reservation_with_ids(
            "exec-33333333333333333333333333333333",
            "attempt-44444444444444444444444444444444",
        )
    }

    fn primary_task_record(
        id: &bridge_core::ids::TaskId,
        ms: i64,
    ) -> bridge_core::task_store::TaskRecord {
        bridge_core::task_store::TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: bridge_core::task_store::TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
            last_artifact_ms: None,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    fn attempt_lock_path(path: &std::path::Path, row: &AttemptReservation) -> std::path::PathBuf {
        history_attempt_lock_dir(path).join(format!("{}.lock", row.identity.attempt_id.as_str()))
    }

    fn terminal() -> AttemptTerminal {
        AttemptTerminal {
            completed_ms: 2_000,
            work_ms: 700,
            end_to_end_ms: 1_000,
            queue_ms: 100,
            cancellation_ms: 0,
            cleanup_ms: 100,
            finalization_ms: 100,
            outcome: "completed".into(),
            terminal_reason: "completed".into(),
            producer_terminal: "unknown".into(),
            final_message: "unknown".into(),
            process_liveness: "unknown".into(),
            terminal_evidence_capability: "unsupported".into(),
            terminal_evidence_version: "none".into(),
            terminal_evidence_source: "none".into(),
            terminal_evidence_complete: false,
            degraded: false,
            prompt_acceptance: "unknown".into(),
            cleanup_disposition: "complete".into(),
            node_counts: NodeCounts {
                completed: 2,
                ..NodeCounts::default()
            },
            phase_durations: vec![],
            telemetry_complete: true,
            monotonic_clock: true,
        }
    }

    fn followup(parent: &AttemptReservation, attempt_id: &str) -> AttemptReservation {
        let mut row = reservation();
        row.identity = bridge_core::ids::AttemptIdentity {
            execution_id: parent.identity.execution_id.clone(),
            attempt_id: bridge_core::ids::AttemptId::parse(attempt_id).unwrap(),
            ordinal: parent.identity.ordinal + 1,
            parent_attempt_id: Some(parent.identity.attempt_id.clone()),
        };
        row.started_ms = parent.started_ms + 1;
        row
    }

    fn served_followup(parent: &AttemptReservation, attempt_id: &str) -> AttemptReservation {
        let mut row = followup(parent, attempt_id);
        row.surface = ExecutionSurface::ServedTask;
        row.task_id = Some(
            bridge_core::ids::TaskId::parse(row.identity.execution_id.as_str().to_owned()).unwrap(),
        );
        row
    }

    #[tokio::test]
    async fn pending_terminal_projection_survives_restart_and_stays_hidden_until_evidence() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskRecordStatus, TaskStore};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared.sqlite");
        let identity = parent_reservation().identity;
        let task = bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap();
        let locator = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        let mut summary = reservation();
        summary.identity = identity.clone();
        summary.task_id = Some(task.clone());
        summary.surface = ExecutionSurface::ServedTask;
        let terminal = terminal();
        let operation = bridge_core::ids::OperationId::parse("op-pending-restart").unwrap();

        let store = SqliteStore::open_shared_history(&path).unwrap();
        store
            .create_with_attempt_locator(&primary_task_record(&task, 1), &locator)
            .await
            .unwrap();
        store.reserve(&summary).await.unwrap();
        assert_eq!(
            store
                .terminalize(&identity.attempt_id, &terminal)
                .await
                .unwrap(),
            TerminalWrite::Applied
        );

        // Model a first boot where summary evidence committed but the primary
        // terminal transaction lost a writer race. The next attempt must use the
        // same immutable evidence and receive Replayed, not Collision.
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker
            .execute_batch("PRAGMA busy_timeout=0; BEGIN IMMEDIATE")
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch("PRAGMA busy_timeout=0")
            .unwrap();
        assert!(store
            .set_terminal_sequenced_pending(
                &task,
                &operation,
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                terminal.completed_ms,
                &identity.attempt_id,
                &terminal,
            )
            .await
            .is_err());
        assert!(store
            .pending_terminal_projection(&task)
            .await
            .unwrap()
            .is_none());
        blocker.execute_batch("ROLLBACK").unwrap();
        let terminal_seq = store
            .set_terminal_sequenced_pending(
                &task,
                &operation,
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                terminal.completed_ms,
                &identity.attempt_id,
                &terminal,
            )
            .await
            .unwrap();

        assert!(
            store
                .set_terminal(
                    &task,
                    TaskRecordStatus::Completed,
                    Some("bypass"),
                    None,
                    terminal.completed_ms,
                )
                .await
                .is_err(),
            "an ordinary terminal writer cannot bypass exact evidence"
        );
        assert!(
            store
                .set_terminal_sequenced(
                    &task,
                    &operation,
                    TaskRecordStatus::Completed,
                    Some("bypass"),
                    None,
                    terminal.completed_ms,
                )
                .await
                .is_err(),
            "a sequenced ordinary writer cannot bypass exact evidence"
        );
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working,
            "a completed primary row remains private until exact evidence is reconciled"
        );
        assert!(store.journal_from(&task, -1).await.unwrap().is_empty());
        let snapshot = store.progress_snapshot(&task).await.unwrap();
        assert_eq!(snapshot.status, TaskRecordStatus::Working);
        assert_eq!(snapshot.terminal_seq, None);
        assert_eq!(snapshot.cut_seq, terminal_seq - 1);
        drop(store);

        let reopened = SqliteStore::open_shared_history(&path).unwrap();
        let pending = reopened
            .pending_terminal_projection(&task)
            .await
            .unwrap()
            .expect("the exact immutable terminal must survive restart");
        assert_eq!(pending.attempt_id, identity.attempt_id);
        assert_eq!(pending.terminal, terminal);
        assert_eq!(pending.task.status, TaskRecordStatus::Completed);
        assert_eq!(
            reopened.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );

        let wrong_attempt =
            bridge_core::ids::AttemptId::parse("attempt-ffffffffffffffffffffffffffffffff").unwrap();
        assert!(reopened
            .mark_terminal_projection_ready(&task, &wrong_attempt)
            .await
            .is_err());
        assert_eq!(
            reopened.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working,
            "an unrelated completion cannot publish the pending row"
        );

        assert_eq!(
            reopened
                .terminalize(&identity.attempt_id, &pending.terminal)
                .await
                .unwrap(),
            TerminalWrite::Replayed
        );
        reopened
            .mark_terminal_projection_ready(&task, &identity.attempt_id)
            .await
            .unwrap();
        assert_eq!(
            reopened.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
        let events = reopened
            .journal_from(&task, terminal_seq - 1)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].kind,
            bridge_core::orch::OrchEventKind::Terminal { .. }
        ));
    }

    #[tokio::test]
    async fn conflicting_summary_cannot_publish_completed_without_exact_marker() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskRecordStatus, TaskStore};
        use bridge_core::workflow_history::LedgerUnavailableReason;

        let store = SqliteStore::open_in_memory().unwrap();
        let identity = parent_reservation().identity;
        let task = bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap();
        let locator = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        store
            .create_with_attempt_locator(&primary_task_record(&task, 1), &locator)
            .await
            .unwrap();
        let mut summary = reservation();
        summary.identity = identity.clone();
        summary.task_id = Some(task.clone());
        summary.surface = ExecutionSurface::ServedTask;
        store.reserve(&summary).await.unwrap();

        let mut prior = terminal();
        prior.terminal_reason = "prior_owner".into();
        store
            .terminalize(&identity.attempt_id, &prior)
            .await
            .unwrap();
        let expected = terminal();
        store
            .set_terminal_sequenced_pending(
                &task,
                &bridge_core::ids::OperationId::parse("op-conflicting-summary").unwrap(),
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                expected.completed_ms,
                &identity.attempt_id,
                &expected,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .terminalize(&identity.attempt_id, &expected)
                .await
                .unwrap(),
            TerminalWrite::Conflict
        );
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working,
            "a conflict alone is not durable evidence for a public completion"
        );
        assert_eq!(
            store
                .get_attempt_locator(&task)
                .await
                .unwrap()
                .unwrap()
                .telemetry_unavailable,
            None
        );

        store
            .mark_attempt_telemetry_unavailable(
                &task,
                &identity.attempt_id,
                LedgerUnavailableReason::Collision,
            )
            .await
            .unwrap();
        store
            .mark_terminal_projection_ready(&task, &identity.attempt_id)
            .await
            .unwrap();
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
    }

    #[test]
    fn malformed_locator_migration_uses_typed_validation_without_sqlite_codes() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskStore};
        use bridge_core::workflow_history::LedgerUnavailableReason;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("malformed-locator.sqlite");
        let identity = parent_reservation().identity;
        let task = bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = SqliteStore::open_shared_history(&path).unwrap();
        runtime
            .block_on(store.create_with_attempt_locator(
                &primary_task_record(&task, 1),
                &TaskAttemptLocator {
                    identity,
                    telemetry_unavailable: None,
                },
            ))
            .unwrap();
        drop(store);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX idx_tasks_terminal_projection;
                 UPDATE task_attempt_locators SET locator_json='{';",
            )
            .unwrap();
        drop(connection);

        let error = SqliteStore::open_shared_history(&path)
            .err()
            .expect("malformed locator migration must refuse opening");
        assert_eq!(error.reason, LedgerUnavailableReason::Migration);
        assert_eq!(error.sqlite_primary_code, None);
        assert_eq!(error.sqlite_extended_code, None);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let recreated: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_tasks_terminal_projection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            recreated, 0,
            "typed validation failure must roll back every earlier migration write"
        );
    }

    #[test]
    fn conflicting_legacy_identity_authority_uses_typed_migration_validation() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskStore};
        use bridge_core::workflow_history::LedgerUnavailableReason;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conflicting-authority.sqlite");
        let identity = parent_reservation().identity;
        let task = bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = SqliteStore::open_shared_history(&path).unwrap();
        runtime
            .block_on(store.create_with_attempt_locator(
                &primary_task_record(&task, 1),
                &TaskAttemptLocator {
                    identity: identity.clone(),
                    telemetry_unavailable: None,
                },
            ))
            .unwrap();
        drop(store);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE task_attempt_identities(
                     task_id TEXT PRIMARY KEY,
                     attempt_id TEXT NOT NULL,
                     execution_id TEXT NOT NULL,
                     ordinal INTEGER NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO task_attempt_identities(task_id, attempt_id, execution_id, ordinal)
                 VALUES(?1, ?2, 'exec-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0)",
                rusqlite::params![task.as_str(), identity.attempt_id.as_str()],
            )
            .unwrap();
        drop(connection);

        let error = SqliteStore::open_shared_history(&path)
            .err()
            .expect("conflicting legacy authority must refuse opening");
        assert_eq!(error.reason, LedgerUnavailableReason::Migration);
        assert_eq!(error.sqlite_primary_code, None);
        assert_eq!(error.sqlite_extended_code, None);
    }

    #[test]
    fn corrupt_database_remains_distinct_from_typed_migration_validation() {
        use bridge_core::workflow_history::LedgerUnavailableReason;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("corrupt.sqlite");
        std::fs::write(&path, b"not a sqlite database").unwrap();
        let error = SqliteStore::open_shared_history(&path)
            .err()
            .expect("corrupt database must refuse opening");
        assert_eq!(error.reason, LedgerUnavailableReason::Corruption);
        assert!(error.sqlite_primary_code.is_some());
        assert!(error.sqlite_extended_code.is_some());
    }
    #[tokio::test]
    async fn served_resume_accepts_one_summary_gap_and_rejects_forks() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskStore};

        let store = SqliteStore::open_in_memory().unwrap();
        let parent = parent_reservation();
        let child = served_followup(&parent, "attempt-55555555555555555555555555555555");
        let task = child.task_id.clone().unwrap();
        let parent_locator = TaskAttemptLocator {
            identity: parent.identity.clone(),
            telemetry_unavailable: None,
        };
        let child_locator = TaskAttemptLocator {
            identity: child.identity.clone(),
            telemetry_unavailable: None,
        };
        store
            .create_with_attempt_locator(&primary_task_record(&task, 1), &parent_locator)
            .await
            .unwrap();
        assert_eq!(
            store
                .claim_resume_attempt_with_locator(&task, 3, 2, &parent_locator, &child_locator)
                .await
                .unwrap(),
            bridge_core::task_store::ResumeClaim::Resumable { attempt: 1 }
        );

        store.reserve(&child).await.unwrap();

        let fork = served_followup(&parent, "attempt-66666666666666666666666666666666");
        assert_eq!(
            store.reserve(&fork).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Collision,
            "a missing optional summary cannot be used to fork the durable task lineage"
        );
    }

    #[tokio::test]
    async fn present_resume_parent_must_be_terminal_and_cannot_fork() {
        let store = SqliteStore::open_in_memory().unwrap();
        let parent = parent_reservation();
        let child = followup(&parent, "attempt-55555555555555555555555555555555");
        store.reserve(&parent).await.unwrap();
        assert_eq!(
            store.reserve(&child).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Collision,
            "provider work cannot resume while its parent is still active"
        );
        store
            .terminalize(&parent.identity.attempt_id, &terminal())
            .await
            .unwrap();
        store.reserve(&child).await.unwrap();

        let fork = followup(&parent, "attempt-66666666666666666666666666666666");
        assert_eq!(
            store.reserve(&fork).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Collision,
            "a terminal parent can have only one direct successor"
        );
    }

    #[tokio::test]
    async fn persisted_prompt_and_terminal_core_evidence_are_sticky() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.prompt_acceptance = "unknown".into();
        store.reserve(&row).await.unwrap();
        assert!(store
            .mark_prompt_acceptance(&row.identity.attempt_id, "not_dispatched")
            .await
            .is_err());
        store
            .mark_prompt_acceptance(&row.identity.attempt_id, "dispatch_uncertain")
            .await
            .unwrap();
        let mut value = terminal();
        value.prompt_acceptance = "not_dispatched".into();
        value.producer_terminal = "completed".into();
        value.final_message = "nonempty".into();
        value.process_liveness = "exited".into();
        store
            .terminalize(&row.identity.attempt_id, &value)
            .await
            .unwrap();

        let completed = store.completed_between(0, 3_000).await.unwrap();
        assert_eq!(completed.len(), 1);
        let persisted = &completed[0].terminal;
        assert_eq!(persisted.prompt_acceptance, "dispatch_uncertain");
        assert_eq!(persisted.producer_terminal, "completed");
        assert_eq!(persisted.final_message, "nonempty");
        assert_eq!(persisted.process_liveness, "exited");
        assert_eq!(persisted.terminal_evidence_capability, "unsupported");
        assert!(!persisted.terminal_evidence_complete);
    }

    #[tokio::test]
    async fn exact_attempt_reads_reject_corrupt_identity_authority_and_projections() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        let substituted = reservation_with_ids(
            "exec-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "attempt-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        let substituted_json = serde_json::to_string(&substituted).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET reservation_json=?2
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str(), substituted_json],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "the exact lookup must bind the requested, projected, and JSON identities"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE attempt_identities SET summary_attached=0 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "a detached or inconsistent permanent authority row is corrupt"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET workflow='different'
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "an immutable reservation projection must match its JSON snapshot"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .terminalize(&row.identity.attempt_id, &terminal())
            .await
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET outcome='failed'
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "terminal projections must match the exact terminal JSON"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET ordinal='not-an-integer'
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "a projection value with an incompatible persisted type is corrupt"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET pinned=1 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "the mutable pin projection must match its rewritten reservation JSON"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET status='terminal' WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "terminal status without immutable terminal evidence is corrupt"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .terminalize(&row.identity.attempt_id, &terminal())
            .await
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET status='active' WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "active status with immutable terminal evidence is corrupt"
        );
    }

    #[tokio::test]
    async fn exact_attempt_read_accepts_only_legacy_conservative_prompt_projection() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        fn legacy_terminal() -> AttemptTerminal {
            let mut terminal = terminal();
            terminal.outcome = "failed".into();
            terminal.terminal_reason = "prompt_barrier_failed".into();
            terminal.degraded = true;
            terminal.prompt_acceptance = "unknown".into();
            terminal.telemetry_complete = false;
            terminal
        }

        async fn persisted_legacy_shape(
            terminal: &AttemptTerminal,
        ) -> (SqliteStore, AttemptReservation) {
            let store = SqliteStore::open_in_memory().unwrap();
            let row = reservation();
            store.reserve(&row).await.unwrap();
            store
                .terminalize(&row.identity.attempt_id, terminal)
                .await
                .unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE workflow_attempt_summaries SET prompt_acceptance='not_dispatched'
                     WHERE attempt_id=?1",
                    rusqlite::params![row.identity.attempt_id.as_str()],
                )
                .unwrap();
            (store, row)
        }

        // Exercise the real direct-attempt state machine and durable writer. The
        // second trigger models the live seed owner, which committed terminal
        // evidence without advancing the projected prompt column.
        let store = std::sync::Arc::new(SqliteStore::open_in_memory().unwrap());
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_seed_prompt_barrier
                 BEFORE UPDATE OF prompt_acceptance ON workflow_attempt_summaries
                 WHEN NEW.prompt_acceptance='dispatch_uncertain'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected seed prompt barrier failure');
                 END;
                 CREATE TRIGGER preserve_seed_prompt_projection
                 AFTER UPDATE OF terminal_json ON workflow_attempt_summaries
                 WHEN OLD.terminal_json IS NULL AND NEW.terminal_json IS NOT NULL
                 BEGIN
                     UPDATE workflow_attempt_summaries
                     SET prompt_acceptance=OLD.prompt_acceptance
                     WHERE attempt_id=NEW.attempt_id;
                 END;",
            )
            .unwrap();
        let row = reservation();
        let mut barrier = DirectAttemptBarrier::admit(store.clone(), row.clone(), "caller_aborted")
            .await
            .unwrap();
        assert!(barrier.mark_prompt_dispatch().await.is_err());
        let (write, barrier_terminal) = barrier
            .finish("interrupted", "caller_aborted", true, "unknown", true)
            .await
            .unwrap();
        assert_eq!(write, TerminalWrite::Applied);
        let persisted: (String, String) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT prompt_acceptance, terminal_json
                 FROM workflow_attempt_summaries WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(persisted.0, "not_dispatched");
        assert_eq!(
            serde_json::from_str::<AttemptTerminal>(&persisted.1).unwrap(),
            barrier_terminal
        );
        let record = store
            .attempt(&row.identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.reservation.prompt_acceptance, "unknown");
        let exact_terminal = record.terminal.unwrap();
        assert_eq!(exact_terminal.prompt_acceptance, "unknown");
        assert_eq!(exact_terminal.outcome, "interrupted");

        let failed_terminal = legacy_terminal();
        let (store, row) = persisted_legacy_shape(&failed_terminal).await;
        let record = store
            .attempt(&row.identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.reservation.prompt_acceptance, "unknown");
        assert_eq!(record.terminal.unwrap().outcome, "failed");

        for (name, contradiction) in [
            ("wrong_reason", {
                let mut value = legacy_terminal();
                value.terminal_reason = "provider_failed".into();
                value
            }),
            ("wrong_outcome", {
                let mut value = legacy_terminal();
                value.outcome = "completed".into();
                value
            }),
            ("not_degraded", {
                let mut value = legacy_terminal();
                value.degraded = false;
                value
            }),
            ("telemetry_complete", {
                let mut value = legacy_terminal();
                value.telemetry_complete = true;
                value
            }),
        ] {
            let (store, row) = persisted_legacy_shape(&contradiction).await;
            assert_eq!(
                store
                    .attempt(&row.identity.attempt_id)
                    .await
                    .unwrap_err()
                    .reason,
                R::Corruption,
                "{name}"
            );
        }

        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance='unknown' WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "an active projection cannot invent unknown prompt evidence"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.prompt_acceptance = "dispatch_uncertain".into();
        store.reserve(&row).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance='not_dispatched' WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "an active projection cannot erase immutable dispatch evidence"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.prompt_acceptance = "dispatch_uncertain".into();
        store.reserve(&row).await.unwrap();
        let terminal_contradiction = legacy_terminal();
        store
            .terminalize(&row.identity.attempt_id, &terminal_contradiction)
            .await
            .unwrap();
        let terminal_json = serde_json::to_string(&terminal_contradiction).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance='not_dispatched', terminal_json=?2
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str(), terminal_json],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "the legacy exception cannot erase immutable terminal dispatch evidence"
        );

        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.prompt_acceptance = "unknown".into();
        store.reserve(&row).await.unwrap();
        let mut reverse = terminal();
        reverse.prompt_acceptance = "not_dispatched".into();
        store
            .terminalize(&row.identity.attempt_id, &reverse)
            .await
            .unwrap();
        let reverse_json = serde_json::to_string(&reverse).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance='unknown', terminal_json=?2
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str(), reverse_json],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            R::Corruption,
            "the compatibility rule is one-way only"
        );
    }

    #[tokio::test]
    async fn active_rows_are_protected_and_charged_during_capacity_admission() {
        let store = SqliteStore::open_in_memory().unwrap();
        let active = child_reservation();
        store.reserve(&active).await.unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET charged_bytes=?2 WHERE attempt_id=?1",
                rusqlite::params![
                    active.identity.attempt_id.as_str(),
                    bridge_core::workflow_history::MAX_CHARGED_BYTES as i64
                ],
            )
            .unwrap();
        let error = store.reserve(&parent_reservation()).await.unwrap_err();
        assert_eq!(
            error.reason,
            bridge_core::workflow_history::LedgerUnavailableReason::CapacityProtected
        );
        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM workflow_attempt_summaries WHERE status='active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "capacity refusal never evicts an active row");
    }
    #[tokio::test]
    async fn proven_reusable_pages_bound_physical_reclamation() {
        use bridge_core::workflow_history::{
            MAX_CHARGED_BYTES, PERMANENT_IDENTITY_CHARGE, RESERVED_ROW_CHARGE, RETENTION_DAYS,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let store = SqliteStore::open_history(&path).unwrap();
        let expired = child_reservation();
        store.reserve(&expired).await.unwrap();
        store
            .terminalize(&expired.identity.attempt_id, &terminal())
            .await
            .unwrap();

        let expired_lock = attempt_lock_path(&path, &expired);
        std::fs::write(&expired_lock, b"legacy-terminal-lock").unwrap();
        let mode: String = store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode=MEMORY", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "memory");

        // Keep the standalone allocation below the hard cap but with less than one
        // fresh reservation charge available. Collecting the expired row creates
        // freelist pages that SQLite can prove are reusable by the new reservation.
        let mut journal = path.as_os_str().to_os_string();
        journal.push("-journal");
        let journal = std::path::PathBuf::from(journal);
        let filler = std::fs::File::create(&journal).unwrap();
        let base_bytes = store.live_history_file_bytes();
        let target_bytes = MAX_CHARGED_BYTES - RESERVED_ROW_CHARGE / 2 - PERMANENT_IDENTITY_CHARGE;
        assert!(base_bytes < target_bytes);
        filler.set_len(target_bytes - base_bytes).unwrap();
        assert!(
            store.live_history_file_bytes() + RESERVED_ROW_CHARGE + PERMANENT_IDENTITY_CHARGE
                > MAX_CHARGED_BYTES
        );

        let mut admitted = parent_reservation();
        admitted.started_ms = 2_000 + RETENTION_DAYS * 24 * 60 * 60 * 1_000 + 1;
        store.reserve(&admitted).await.unwrap();
        assert!(store.live_history_file_bytes() <= MAX_CHARGED_BYTES);
        assert_eq!(
            store.completed_between(0, i64::MAX).await.unwrap().len(),
            0,
            "the expired terminal row is reclaimed before admission"
        );
        let active: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM workflow_attempt_summaries WHERE status='active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, 1);
        let future_start = admitted.started_ms;
        let mut current = admitted;
        for cycle in 0_u64..32 {
            store
                .terminalize(&current.identity.attempt_id, &terminal())
                .await
                .unwrap();
            assert!(store.live_history_file_bytes() <= MAX_CHARGED_BYTES);

            let execution = format!("exec-{:032x}", 0x100_u64 + cycle);
            let attempt = format!("attempt-{:032x}", 0x200_u64 + cycle);
            let mut next = reservation_with_ids(&execution, &attempt);
            next.started_ms = future_start + i64::try_from(cycle).unwrap() + 1;
            store.reserve(&next).await.unwrap();
            assert!(
                store.live_history_file_bytes() <= MAX_CHARGED_BYTES,
                "cycle {cycle} crossed the physical database-plus-sidecar ceiling"
            );
            current = next;
        }
        let identities: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM attempt_identities", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(identities, 34);
        assert_eq!(store.completed_between(0, i64::MAX).await.unwrap().len(), 0);
        let _ = expired_lock;
    }

    #[tokio::test]
    async fn concurrent_capacity_admission_allows_only_one_remaining_slot() {
        use bridge_core::workflow_history::{
            MAX_CHARGED_BYTES, PERMANENT_IDENTITY_CHARGE, RESERVED_ROW_CHARGE, RETENTION_DAYS,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let store_a = SqliteStore::open_history(&path).unwrap();

        let protected = child_reservation();
        store_a.reserve(&protected).await.unwrap();
        store_a
            .terminalize(&protected.identity.attempt_id, &terminal())
            .await
            .unwrap();
        store_a
            .set_pinned(&protected.identity.attempt_id, true)
            .await
            .unwrap();
        store_a
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET charged_bytes=?2 WHERE attempt_id=?1",
                rusqlite::params![
                    protected.identity.attempt_id.as_str(),
                    (MAX_CHARGED_BYTES - RESERVED_ROW_CHARGE - 3 * PERMANENT_IDENTITY_CHARGE)
                        as i64,
                ],
            )
            .unwrap();

        let expired = parent_reservation();
        store_a.reserve(&expired).await.unwrap();
        store_a
            .terminalize(&expired.identity.attempt_id, &terminal())
            .await
            .unwrap();
        let store_b = SqliteStore::open_history(&path).unwrap();

        let future_start = 2_000 + RETENTION_DAYS * 24 * 60 * 60 * 1_000 + 1;
        let mut first = reservation_with_ids(
            "exec-77777777777777777777777777777777",
            "attempt-88888888888888888888888888888888",
        );
        first.started_ms = future_start;
        let mut second = reservation_with_ids(
            "exec-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "attempt-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        second.started_ms = future_start;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let spawn = |store: SqliteStore, row: AttemptReservation| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                barrier.wait();
                runtime
                    .block_on(store.reserve(&row))
                    .map_err(|error| error.reason)
            })
        };

        let first = spawn(store_a, first);
        let second = spawn(store_b, second);
        barrier.wait();
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    matches!(
                        result,
                        Err(bridge_core::workflow_history::LedgerUnavailableReason::CapacityProtected)
                    )
                })
                .count(),
            1
        );
        let inspected = SqliteStore::open_history(&path).unwrap();
        assert!(
            inspected.live_history_file_bytes() <= bridge_core::workflow_history::MAX_CHARGED_BYTES,
            "serialized concurrent admission must preserve the physical cap"
        );
    }

    #[tokio::test]
    async fn sqlite_atomic_primary_admission_rejects_completed_cross_execution_attempt_reuse() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskStore};

        let store = SqliteStore::open_in_memory().unwrap();
        let first_identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let first_task =
            bridge_core::ids::TaskId::parse(first_identity.execution_id.as_str()).unwrap();
        let first_locator = TaskAttemptLocator {
            identity: first_identity.clone(),
            telemetry_unavailable: None,
        };
        store
            .create_with_attempt_locator(&primary_task_record(&first_task, 1), &first_locator)
            .await
            .unwrap();
        store
            .set_terminal(
                &first_task,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                2,
            )
            .await
            .unwrap();

        let mut colliding_identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let colliding_task =
            bridge_core::ids::TaskId::parse(colliding_identity.execution_id.as_str()).unwrap();
        colliding_identity.attempt_id = first_identity.attempt_id;
        let colliding_locator = TaskAttemptLocator {
            identity: colliding_identity,
            telemetry_unavailable: None,
        };
        assert!(store
            .create_with_attempt_locator(
                &primary_task_record(&colliding_task, 3),
                &colliding_locator,
            )
            .await
            .is_err());
        assert_eq!(store.get(&colliding_task).await.unwrap(), None);
        assert_eq!(
            store.get_attempt_locator(&colliding_task).await.unwrap(),
            None,
            "the transaction must roll back both the task and locator"
        );
        assert_eq!(
            store.get_attempt_locator(&first_task).await.unwrap(),
            Some(first_locator)
        );
    }
    #[tokio::test]
    async fn task_and_history_surfaces_share_one_attempt_identity_authority() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskStore};

        let store = SqliteStore::open_in_memory().unwrap();
        let shared_attempt =
            bridge_core::ids::AttemptId::parse("attempt-cccccccccccccccccccccccccccccccc").unwrap();
        let direct_identity = bridge_core::ids::AttemptIdentity {
            execution_id: bridge_core::ids::ExecutionId::parse(
                "exec-dddddddddddddddddddddddddddddddd",
            )
            .unwrap(),
            attempt_id: shared_attempt.clone(),
            ordinal: 0,
            parent_attempt_id: None,
        };
        let mut direct = reservation_with_ids(
            direct_identity.execution_id.as_str(),
            direct_identity.attempt_id.as_str(),
        );
        direct.surface = ExecutionSurface::DirectUnary;
        direct.task_id =
            Some(bridge_core::ids::TaskId::parse(direct_identity.execution_id.as_str()).unwrap());
        store.reserve(&direct).await.unwrap();

        let colliding_identity = bridge_core::ids::AttemptIdentity {
            execution_id: bridge_core::ids::ExecutionId::parse(
                "exec-eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            )
            .unwrap(),
            attempt_id: shared_attempt,
            ordinal: 0,
            parent_attempt_id: None,
        };
        let colliding_task =
            bridge_core::ids::TaskId::parse(colliding_identity.execution_id.as_str()).unwrap();
        let colliding_locator = TaskAttemptLocator {
            identity: colliding_identity,
            telemetry_unavailable: None,
        };
        assert!(store
            .create_with_attempt_locator(
                &primary_task_record(&colliding_task, 2),
                &colliding_locator,
            )
            .await
            .is_err());
        assert_eq!(store.get(&colliding_task).await.unwrap(), None);

        let task_identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = bridge_core::ids::TaskId::parse(task_identity.execution_id.as_str()).unwrap();
        let locator = TaskAttemptLocator {
            identity: task_identity.clone(),
            telemetry_unavailable: None,
        };
        store
            .create_with_attempt_locator(&primary_task_record(&task, 3), &locator)
            .await
            .unwrap();

        let other_execution =
            bridge_core::ids::ExecutionId::parse("exec-ffffffffffffffffffffffffffffffff").unwrap();
        let colliding_history =
            reservation_with_ids(other_execution.as_str(), task_identity.attempt_id.as_str());
        assert_eq!(
            store.reserve(&colliding_history).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Collision
        );

        let mut exact_served = reservation_with_ids(
            task_identity.execution_id.as_str(),
            task_identity.attempt_id.as_str(),
        );
        exact_served.surface = ExecutionSurface::ServedTask;
        exact_served.task_id = Some(task.clone());
        store.reserve(&exact_served).await.unwrap();
        assert_eq!(
            store.get_attempt_locator(&task).await.unwrap(),
            Some(locator)
        );
    }

    #[tokio::test]
    async fn sqlite_resume_locator_cas_is_atomic_and_attempt_scoped() {
        use bridge_core::task_store::{
            TaskAttemptLocator, TaskRecord, TaskRecordStatus, TaskStore,
        };

        let store = SqliteStore::open_in_memory().unwrap();
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: String::new(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();
        let first = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        let next = TaskAttemptLocator {
            identity: identity.resume().unwrap(),
            telemetry_unavailable: None,
        };
        store.put_attempt_locator(&task, &first).await.unwrap();
        assert!(store.put_attempt_locator(&task, &first).await.is_err());

        let mut forged = next.clone();
        forged.identity.ordinal += 1;
        assert!(store
            .claim_resume_attempt_with_locator(&task, 3, 2, &first, &forged)
            .await
            .is_err());
        assert_eq!(
            store.get_attempt_locator(&task).await.unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            store
                .claim_resume_attempt_with_locator(&task, 3, 2, &first, &next)
                .await
                .unwrap(),
            bridge_core::task_store::ResumeClaim::Resumable { attempt: 1 }
        );
        assert!(store
            .mark_attempt_telemetry_unavailable(
                &task,
                &first.identity.attempt_id,
                bridge_core::workflow_history::LedgerUnavailableReason::Io,
            )
            .await
            .is_err());
        assert_eq!(store.get_attempt_locator(&task).await.unwrap(), Some(next));
    }

    #[tokio::test]
    async fn reserve_collision_and_terminal_replay_are_not_prompt_replay() {
        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        assert_eq!(
            store.reserve(&row).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Collision
        );
        let value = terminal();
        assert_eq!(
            store
                .terminalize(&row.identity.attempt_id, &value)
                .await
                .unwrap(),
            TerminalWrite::Applied
        );
        assert_eq!(
            store
                .terminalize(&row.identity.attempt_id, &value)
                .await
                .unwrap(),
            TerminalWrite::Replayed
        );
        let mut conflict = value;
        conflict.work_ms += 1;
        assert_eq!(
            store
                .terminalize(&row.identity.attempt_id, &conflict)
                .await
                .unwrap(),
            TerminalWrite::Conflict
        );
    }

    #[tokio::test]
    async fn legacy_direct_timing_projection_preserves_raw_bytes_and_replay_authority() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.surface = ExecutionSurface::DirectUnary;
        row.task_id =
            Some(bridge_core::ids::TaskId::parse(row.identity.execution_id.as_str()).unwrap());
        store.reserve(&row).await.unwrap();

        let mut legacy = terminal();
        legacy.work_ms = legacy.end_to_end_ms;
        legacy.queue_ms = 0;
        legacy.cancellation_ms = 0;
        legacy.cleanup_ms = 0;
        legacy.finalization_ms = 0;
        legacy.phase_durations = vec![bridge_core::workflow_history::PhaseDuration {
            phase: "work".into(),
            duration_ms: legacy.work_ms,
        }];
        assert_eq!(
            store
                .terminalize(&row.identity.attempt_id, &legacy)
                .await
                .unwrap(),
            TerminalWrite::Applied
        );

        let raw_before: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT terminal_json FROM workflow_attempt_summaries WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
                |sqlite_row| sqlite_row.get(0),
            )
            .unwrap();

        let exact = store
            .attempt(&row.identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(exact.end_to_end_ms, legacy.end_to_end_ms);
        assert_eq!(exact.work_ms, 0);
        assert!(exact.phase_durations.is_empty());
        assert!(!exact.telemetry_complete);

        let completed = store.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].terminal, exact);
        let report = bridge_core::workflow_history::report(0, i64::MAX, &completed);
        assert_eq!(report.excluded["telemetry_incomplete"], 1);

        let raw_after: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT terminal_json FROM workflow_attempt_summaries WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
                |sqlite_row| sqlite_row.get(0),
            )
            .unwrap();
        assert_eq!(raw_after.as_bytes(), raw_before.as_bytes());
        assert_eq!(
            store
                .terminalize(&row.identity.attempt_id, &legacy)
                .await
                .unwrap(),
            TerminalWrite::Replayed
        );
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET telemetry_complete=0 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap_err()
                .reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption
        );
        assert_eq!(
            store
                .completed_between(0, i64::MAX)
                .await
                .unwrap_err()
                .reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption,
            "completed recovery must validate raw projections before compatibility projection"
        );
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET telemetry_complete='invalid' WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            store
                .completed_between(0, i64::MAX)
                .await
                .unwrap_err()
                .reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption,
            "invalid persisted projection types are row corruption, not transient I/O"
        );
    }

    #[tokio::test]
    async fn completed_range_recovery_preserves_prompt_monotonicity() {
        let store = SqliteStore::open_in_memory().unwrap();
        let mut row = reservation();
        row.prompt_acceptance = "dispatch_uncertain".into();
        store.reserve(&row).await.unwrap();

        let mut value = terminal();
        value.prompt_acceptance = "dispatch_uncertain".into();
        store
            .terminalize(&row.identity.attempt_id, &value)
            .await
            .unwrap();

        let completed = store.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(
            completed[0].reservation.prompt_acceptance,
            "dispatch_uncertain"
        );
        assert_eq!(
            completed[0].terminal.prompt_acceptance,
            "dispatch_uncertain"
        );

        let mut downgraded = value;
        downgraded.prompt_acceptance = "not_dispatched".into();
        let downgraded_json = serde_json::to_string(&downgraded).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries
                 SET prompt_acceptance='not_dispatched', terminal_json=?2
                 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str(), downgraded_json],
            )
            .unwrap();
        assert_eq!(
            store
                .completed_between(0, i64::MAX)
                .await
                .unwrap_err()
                .reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption,
            "range recovery must not erase immutable dispatch-uncertain evidence"
        );
    }

    #[tokio::test]
    async fn live_admin_pin_unpin_is_durable_idempotent_and_attempt_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("configured.sqlite");
        let primary = SqliteStore::open_shared_history(&path).unwrap();
        let row = reservation();
        primary.reserve(&row).await.unwrap();
        primary
            .terminalize(&row.identity.attempt_id, &terminal())
            .await
            .unwrap();

        let admin = SqliteStore::open_history_admin(&path).unwrap();
        assert_eq!(admin.history_path.as_deref(), Some(path.as_path()));
        assert!(
            admin.live_history_file_bytes() <= bridge_core::workflow_history::MAX_CHARGED_BYTES
        );
        assert!(admin
            .set_pinned(&row.identity.attempt_id, true)
            .await
            .unwrap());
        assert!(!admin
            .set_pinned(&row.identity.attempt_id, true)
            .await
            .unwrap());
        let completed = primary.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(completed.len(), 1);
        assert!(completed[0].reservation.pinned);

        assert!(admin
            .set_pinned(&row.identity.attempt_id, false)
            .await
            .unwrap());
        assert!(
            !primary.completed_between(0, i64::MAX).await.unwrap()[0]
                .reservation
                .pinned
        );

        let missing =
            bridge_core::ids::AttemptId::parse("attempt-99999999999999999999999999999999").unwrap();
        assert_eq!(
            admin.set_pinned(&missing, true).await.unwrap_err().reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Schema
        );
        primary
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_attempt_summaries SET pinned=1 WHERE attempt_id=?1",
                rusqlite::params![row.identity.attempt_id.as_str()],
            )
            .unwrap();
        assert_eq!(
            admin
                .set_pinned(&row.identity.attempt_id, false)
                .await
                .unwrap_err()
                .reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption
        );
    }

    #[tokio::test]
    async fn pinned_terminal_survives_age_collection_until_explicitly_unpinned() {
        use bridge_core::workflow_history::RETENTION_DAYS;

        let store = SqliteStore::open_in_memory().unwrap();
        let old = child_reservation();
        store.reserve(&old).await.unwrap();
        store
            .terminalize(&old.identity.attempt_id, &terminal())
            .await
            .unwrap();
        store
            .set_pinned(&old.identity.attempt_id, true)
            .await
            .unwrap();

        let future_start = 2_000 + RETENTION_DAYS * 24 * 60 * 60 * 1_000 + 1;
        let mut protected_admission = parent_reservation();
        protected_admission.started_ms = future_start;
        store.reserve(&protected_admission).await.unwrap();
        assert!(store
            .completed_between(0, i64::MAX)
            .await
            .unwrap()
            .iter()
            .any(|row| row.reservation.identity.attempt_id == old.identity.attempt_id));

        store
            .set_pinned(&old.identity.attempt_id, false)
            .await
            .unwrap();
        let mut collection = reservation_with_ids(
            "exec-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "attempt-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        collection.started_ms = future_start + 1;
        store.reserve(&collection).await.unwrap();
        assert!(store
            .completed_between(0, i64::MAX)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn boot_interrupts_active_rows_and_completed_query_is_bounded() {
        let store = SqliteStore::open_in_memory().unwrap();
        let row = reservation();
        store.reserve(&row).await.unwrap();
        store
            .mark_prompt_acceptance(&row.identity.attempt_id, "dispatch_uncertain")
            .await
            .unwrap();
        assert_eq!(store.interrupt_active(3_000).await.unwrap(), 1);
        assert_eq!(store.interrupt_active(4_000).await.unwrap(), 0);
        let rows = store.completed_between(0, 3_000).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].terminal.outcome, "interrupted");
        assert_eq!(
            rows[0].terminal.prompt_acceptance, "dispatch_uncertain",
            "boot interruption preserves the sticky pre-dispatch evidence"
        );
    }

    #[tokio::test]
    async fn terminal_marker_scan_excludes_only_the_exact_active_summary_on_every_boot() {
        use bridge_core::task_store::{TaskAttemptLocator, TaskRecordStatus, TaskStore};

        let store = SqliteStore::open_in_memory().unwrap();
        let mut marked = parent_reservation();
        let task = bridge_core::ids::TaskId::parse(marked.identity.execution_id.as_str()).unwrap();
        marked.task_id = Some(task.clone());
        marked.surface = ExecutionSurface::ServedTask;
        let locator = TaskAttemptLocator {
            identity: marked.identity.clone(),
            telemetry_unavailable: None,
        };
        store
            .create_with_attempt_locator(&primary_task_record(&task, 1_000), &locator)
            .await
            .unwrap();
        store.reserve(&marked).await.unwrap();
        store
            .mark_attempt_telemetry_unavailable(
                &task,
                &marked.identity.attempt_id,
                bridge_core::workflow_history::LedgerUnavailableReason::Io,
            )
            .await
            .unwrap();
        assert!(
            store
                .terminal_attempts_with_telemetry_markers()
                .await
                .unwrap()
                .is_empty(),
            "a marker on a Working primary cannot suppress restart interruption"
        );
        store
            .set_terminal(
                &task,
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                2_000,
            )
            .await
            .unwrap();

        let excluded = store
            .terminal_attempts_with_telemetry_markers()
            .await
            .unwrap();
        assert_eq!(excluded, vec![marked.identity.attempt_id.clone()]);

        let unrelated = child_reservation();
        store.reserve(&unrelated).await.unwrap();
        assert_eq!(
            store
                .interrupt_active_excluding(3_000, &excluded)
                .await
                .unwrap(),
            1,
            "unrelated active attempts remain conservatively interrupted"
        );
        assert!(store
            .attempt(&marked.identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .is_none());
        assert_eq!(
            store
                .attempt(&unrelated.identity.attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap()
                .terminal_reason,
            "process_restart"
        );
        assert_eq!(
            store
                .interrupt_active_excluding(4_000, &excluded)
                .await
                .unwrap(),
            0,
            "the marker-only attempt stays unmodified on later boots"
        );
    }

    #[tokio::test]
    async fn platform_connections_reserve_concurrently_and_reconcile_only_dead_owners() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let store_a = SqliteStore::open_history(&path).unwrap();
        let store_b = SqliteStore::open_history(&path).unwrap();
        let row_a = child_reservation();
        let row_b = parent_reservation();

        store_a.reserve(&row_a).await.unwrap();
        store_b.reserve(&row_b).await.unwrap();
        assert_eq!(
            store_b.interrupt_active(1_500).await.unwrap(),
            0,
            "both process leases are live"
        );

        drop(store_a);
        assert_eq!(
            store_b.interrupt_active(1_600).await.unwrap(),
            1,
            "only the row whose owner disappeared is interrupted"
        );
        assert_eq!(
            store_b
                .terminalize(&row_b.identity.attempt_id, &terminal())
                .await
                .unwrap(),
            TerminalWrite::Applied
        );
        let completed = store_b.completed_between(0, 2_000).await.unwrap();
        assert_eq!(completed.len(), 2);
        assert_eq!(
            completed
                .iter()
                .filter(|row| row.terminal.outcome == "interrupted")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn platform_primary_enforces_physical_cap_without_eager_reconciliation() {
        use bridge_core::workflow_history::MAX_CHARGED_BYTES;
        let assert_transaction_budget = |store: &SqliteStore| {
            let conn = store.conn.lock().unwrap();
            let page_size: i64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .unwrap();
            let max_pages: i64 = conn
                .query_row("PRAGMA max_page_count", [], |row| row.get(0))
                .unwrap();
            let journal_mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            drop(conn);
            let page_size = u64::try_from(page_size).unwrap();
            let max_pages = u64::try_from(max_pages).unwrap();
            assert!(
                max_pages.saturating_mul(page_size)
                    <= MAX_CHARGED_BYTES - HISTORY_SIDECAR_HEADROOM_BYTES
            );
            assert!(
                store
                    .live_history_file_bytes()
                    .saturating_add(HISTORY_DISK_TRANSACTION_HEADROOM_BYTES)
                    <= MAX_CHARGED_BYTES
            );
            assert!(matches!(
                journal_mode.as_str(),
                "delete" | "truncate" | "persist"
            ));
            assert!(store.acquire_history_admission_lease().unwrap().is_some());
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("platform-primary.sqlite");
        let store = SqliteStore::open_platform_history(&path).unwrap();
        assert_eq!(store.history_path.as_deref(), Some(path.as_path()));
        assert!(store._lock.is_some());
        assert_transaction_budget(&store);

        let row = reservation();
        store.reserve(&row).await.unwrap();
        assert!(store.live_history_file_bytes() <= MAX_CHARGED_BYTES);
        drop(store);

        let reopened = SqliteStore::open_platform_history(&path).unwrap();
        assert!(
            reopened
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .is_none(),
            "platform construction must leave checkpoint-first reconciliation to the coordinator"
        );
        assert!(reopened.live_history_file_bytes() <= MAX_CHARGED_BYTES);

        let shared_path = directory.path().join("configured-shared.sqlite");
        let shared = SqliteStore::open_shared_history(&shared_path).unwrap();
        assert!(
            shared.history_path.as_deref() == Some(shared_path.as_path()),
            "the shared store must enforce the same physical database and sidecar cap"
        );
        assert!(shared.live_history_file_bytes() <= MAX_CHARGED_BYTES);
        assert_transaction_budget(&shared);
    }
    #[tokio::test]
    async fn committed_reservation_does_not_report_deferred_checkpoint_as_failure() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("post-commit.sqlite");
        let store = SqliteStore::open_history(&path).unwrap();

        let mode: String = {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch("PRAGMA busy_timeout=0").unwrap();
            conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(mode, "wal");

        let reader = rusqlite::Connection::open(&path).unwrap();
        reader
            .execute_batch("PRAGMA busy_timeout=0; BEGIN")
            .unwrap();
        let visible: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM workflow_attempt_summaries",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(visible, 0);

        let row = parent_reservation();
        store
            .reserve(&row)
            .await
            .expect("a durable commit must not be reported as a checkpoint failure");
        assert!(
            store
                .attempt(&row.identity.attempt_id)
                .await
                .unwrap()
                .is_some(),
            "the reservation committed before the reader blocked WAL truncation"
        );

        reader.execute_batch("ROLLBACK").unwrap();
        store.checkpoint_and_verify_history_size().unwrap();
    }

    #[tokio::test]
    async fn writable_history_open_reconciles_dead_owners_and_preserves_live_owners() {
        let directory = tempfile::tempdir().unwrap();
        let platform_path = directory.path().join("platform.sqlite");
        let live_store = SqliteStore::open_history(&platform_path).unwrap();
        let live = child_reservation();
        live_store.reserve(&live).await.unwrap();
        let live_lock = attempt_lock_path(&platform_path, &live);
        assert!(live_lock.exists());

        let peer = SqliteStore::open_history(&platform_path).unwrap();
        assert!(peer
            .completed_between(0, i64::MAX)
            .await
            .unwrap()
            .is_empty());
        drop(peer);
        drop(live_store);

        let recovered = SqliteStore::open_history(&platform_path).unwrap();
        assert!(
            !live_lock.exists(),
            "boot reconciliation removes the dead owner's exact attempt lock"
        );
        let rows = recovered.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].terminal.outcome, "interrupted");
        assert_eq!(
            rows[0].terminal.terminal_reason, "process_restart",
            "a new standalone process reconciles a dead owner before admission"
        );

        let configured_path = directory.path().join("configured.sqlite");
        let configured = SqliteStore::open_shared_history(&configured_path).unwrap();
        let configured_row = reservation();
        configured.reserve(&configured_row).await.unwrap();
        drop(configured);

        let configured = SqliteStore::open_shared_history(&configured_path).unwrap();
        assert!(
            configured
                .attempt(&configured_row.identity.attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .is_none(),
            "opening a shared primary cannot interrupt before task checkpoint reconciliation"
        );
        configured.interrupt_active(3_000).await.unwrap();
        let rows = configured.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].terminal.outcome, "interrupted");
    }

    #[test]
    fn startup_reconciliation_failure_is_bounded_and_fails_opening() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        {
            let store = SqliteStore::open_history(&path).unwrap();
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO workflow_attempt_summaries(
                    attempt_id, execution_id, ordinal, workflow, task_class, surface,
                    policy, workload_fingerprint, started_ms, status, charged_bytes,
                    reservation_json)
                 VALUES('corrupt-attempt','exec-11111111111111111111111111111111',0,
                    'review','workflow','offline','r2f0a','shape-a',1,'active',1,'{}')",
                [],
            )
            .unwrap();
        }

        let error = SqliteStore::open_history(&path)
            .err()
            .expect("corrupt active row must fail opening");
        assert_eq!(
            error.reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Corruption
        );
    }

    #[test]
    fn platform_history_process_helper() {
        let Some(path) = std::env::var_os("A2A_BRIDGE_TEST_HISTORY_PATH") else {
            return;
        };
        let ready = std::path::PathBuf::from(
            std::env::var_os("A2A_BRIDGE_TEST_HISTORY_READY").expect("ready path"),
        );
        let release = std::path::PathBuf::from(
            std::env::var_os("A2A_BRIDGE_TEST_HISTORY_RELEASE").expect("release path"),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = SqliteStore::open_history(std::path::Path::new(&path)).unwrap();
        let row = child_reservation();
        runtime.block_on(store.reserve(&row)).unwrap();
        std::fs::write(&ready, b"reserved").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !release.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "parent did not release child helper"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        runtime
            .block_on(store.terminalize(&row.identity.attempt_id, &terminal()))
            .unwrap();
    }

    #[test]
    fn independent_processes_share_platform_history_without_false_lock_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let ready = directory.path().join("child.ready");
        let release = directory.path().join("child.release");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("sqlite::r2f0a_history_tests::platform_history_process_helper")
            .arg("--exact")
            .arg("--nocapture")
            .env("A2A_BRIDGE_TEST_HISTORY_PATH", &path)
            .env("A2A_BRIDGE_TEST_HISTORY_READY", &ready)
            .env("A2A_BRIDGE_TEST_HISTORY_RELEASE", &release)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let ready_result = loop {
            if ready.exists() {
                break Ok(());
            }
            if let Some(status) = child.try_wait().unwrap() {
                break Err(format!("child exited before reservation: {status}"));
            }
            if std::time::Instant::now() >= deadline {
                break Err("child reservation timed out".to_owned());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let parent_result = ready_result.and_then(|()| {
            let store = SqliteStore::open_history(&path)
                .map_err(|error| format!("parent open failed: {error:?}"))?;
            let row = parent_reservation();
            runtime
                .block_on(store.reserve(&row))
                .map_err(|error| format!("parent reservation failed: {error:?}"))?;
            let interrupted = runtime
                .block_on(store.interrupt_active(1_500))
                .map_err(|error| format!("reconciliation failed: {error:?}"))?;
            if interrupted != 0 {
                return Err(format!(
                    "reconciliation interrupted {interrupted} live attempts"
                ));
            }
            runtime
                .block_on(store.terminalize(&row.identity.attempt_id, &terminal()))
                .map_err(|error| format!("parent terminalization failed: {error:?}"))?;
            Ok(store)
        });

        std::fs::write(&release, b"release").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let store = parent_result.unwrap_or_else(|error| panic!("{error}"));
        let completed = runtime.block_on(store.completed_between(0, 2_000)).unwrap();
        assert_eq!(completed.len(), 2);
        assert!(completed
            .iter()
            .all(|row| row.terminal.outcome == "completed"));
    }

    #[test]
    fn configured_primary_history_keeps_its_exclusive_database_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("configured.sqlite");
        let _primary = SqliteStore::open_shared_history(&path).unwrap();
        let error = SqliteStore::open_shared_history(&path)
            .err()
            .expect("second configured primary must be refused");
        assert_eq!(
            error.reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Locked
        );
    }

    #[test]
    fn platform_schema_lock_contention_is_bounded_and_fail_closed() {
        use fs2::FileExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let schema_lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(history_schema_lock_path(&path))
            .unwrap();
        schema_lock.try_lock_exclusive().unwrap();
        let error =
            SqliteStore::open_history_with_schema_lock_timeout(&path, std::time::Duration::ZERO)
                .err()
                .expect("held schema lock must refuse open");
        assert_eq!(
            error.reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Locked
        );
    }

    #[tokio::test]
    async fn sqlite_writer_contention_maps_to_locked_and_releases_failed_reservation_lease() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let blocker = SqliteStore::open_history(&path).unwrap();
        let contender = SqliteStore::open_history(&path).unwrap();
        blocker
            .conn
            .lock()
            .unwrap()
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        contender
            .conn
            .lock()
            .unwrap()
            .execute_batch("PRAGMA busy_timeout = 0")
            .unwrap();

        let row = parent_reservation();
        let error = contender.reserve(&row).await.unwrap_err();
        assert_eq!(
            error.reason,
            bridge_core::workflow_history::LedgerUnavailableReason::Locked
        );
        assert!(
            !attempt_lock_path(&path, &row).exists(),
            "a refused reservation removes its newly acquired attempt lock"
        );
        blocker
            .conn
            .lock()
            .unwrap()
            .execute_batch("ROLLBACK")
            .unwrap();
        contender.reserve(&row).await.unwrap();
        contender
            .terminalize(&row.identity.attempt_id, &terminal())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn near_boundary_terminal_expansion_stays_bounded_for_pinned_lifecycle() {
        use bridge_core::workflow_history::{
            MAX_CHARGED_BYTES, MAX_PHASES, MAX_TERMINAL_JSON_BYTES,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let store = SqliteStore::open_history(&path).unwrap();
        assert!(store.live_history_file_bytes() <= MAX_CHARGED_BYTES);

        let mut row = parent_reservation();
        row.pinned = true;
        store.reserve(&row).await.unwrap();
        assert!(
            store.live_history_file_bytes() <= MAX_CHARGED_BYTES,
            "reservation must keep the main database plus live WAL/journal/SHM under the hard cap"
        );
        let mode: String = store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode=MEMORY", [], |record| record.get(0))
            .unwrap();
        assert_eq!(mode, "memory");
        let mut journal = path.as_os_str().to_os_string();
        journal.push("-journal");
        let journal = std::path::PathBuf::from(journal);
        let filler = std::fs::File::create(&journal).unwrap();
        let base_bytes = store.live_history_file_bytes();
        assert!(base_bytes < MAX_CHARGED_BYTES);
        filler.set_len(MAX_CHARGED_BYTES - base_bytes).unwrap();
        assert_eq!(store.live_history_file_bytes(), MAX_CHARGED_BYTES);

        let mut expanded = terminal();
        expanded.terminal_reason = "x".repeat(bridge_core::workflow_history::MAX_DIMENSION_LEN);
        expanded.phase_durations = (0..MAX_PHASES)
            .map(|_| bridge_core::workflow_history::PhaseDuration {
                phase: "p".repeat(bridge_core::workflow_history::MAX_DIMENSION_LEN),
                duration_ms: 1,
            })
            .collect();
        let encoded = serde_json::to_vec(&expanded).unwrap();
        assert!(encoded.len() <= MAX_TERMINAL_JSON_BYTES);
        store
            .terminalize(&row.identity.attempt_id, &expanded)
            .await
            .unwrap();
        let terminal_bytes = store.live_history_file_bytes();
        assert!(
            terminal_bytes <= MAX_CHARGED_BYTES,
            "terminal expansion of a protected row must remain inside its reservation: observed {terminal_bytes} bytes against {MAX_CHARGED_BYTES}"
        );
        assert!(store
            .completed_between(0, i64::MAX)
            .await
            .unwrap()
            .into_iter()
            .all(|completed| completed.reservation.pinned));
    }

    #[test]
    fn sqlite_failure_classifier_preserves_primary_and_extended_codes() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let extended_io = rusqlite::ffi::SQLITE_IOERR | (1 << 8);
        for (code, reason) in [
            (rusqlite::ffi::SQLITE_BUSY, R::Locked),
            (rusqlite::ffi::SQLITE_READONLY, R::ReadOnlyDatabase),
            (rusqlite::ffi::SQLITE_PERM, R::Permission),
            (rusqlite::ffi::SQLITE_SCHEMA, R::Migration),
            (rusqlite::ffi::SQLITE_CORRUPT, R::Corruption),
            (rusqlite::ffi::SQLITE_CANTOPEN, R::Open),
            (rusqlite::ffi::SQLITE_PROTOCOL, R::AdvisoryLockUnsupported),
            (extended_io, R::Io),
        ] {
            let error = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some("bounded test error".into()),
            );
            let classified = history_error(&error);
            assert_eq!(classified.reason, reason, "SQLite code {code}");
            assert_eq!(classified.sqlite_primary_code, Some(code & 0xff));
            assert_eq!(classified.sqlite_extended_code, Some(code));
        }
    }

    #[test]
    fn migration_classifier_preserves_non_schema_sqlite_codes() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let extended_io = rusqlite::ffi::SQLITE_IOERR | (7 << 8);
        for (code, reason) in [
            (extended_io, R::Io),
            (rusqlite::ffi::SQLITE_CANTOPEN, R::Open),
            (rusqlite::ffi::SQLITE_READONLY, R::ReadOnlyDatabase),
            (rusqlite::ffi::SQLITE_CORRUPT, R::Corruption),
        ] {
            let error = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                Some("bounded migration test error".into()),
            );
            let classified = history_migration_error(&error);
            assert_eq!(classified.reason, reason, "SQLite code {code}");
            assert_eq!(classified.sqlite_primary_code, Some(code & 0xff));
            assert_eq!(classified.sqlite_extended_code, Some(code));
        }

        let schema_error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("bounded schema test error".into()),
        );
        let classified = history_migration_error(&schema_error);
        assert_eq!(classified.reason, R::Migration);
        assert_eq!(
            classified.sqlite_primary_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
        assert_eq!(
            classified.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );

        let directory = tempfile::tempdir().unwrap();
        let incompatible_path = directory.path().join("admin-incompatible-schema.sqlite");
        let connection = rusqlite::Connection::open(&incompatible_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE workflow_attempt_summaries (
                     attempt_id TEXT PRIMARY KEY
                 );",
            )
            .unwrap();
        drop(connection);
        let readonly_error = SqliteStore::open_history_read_only(&incompatible_path)
            .err()
            .expect("a read-only allocation with missing columns must be rejected");
        assert_eq!(readonly_error.reason, R::Migration);
        assert_eq!(
            readonly_error.sqlite_primary_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
        assert_eq!(
            readonly_error.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );

        let admin_error = SqliteStore::open_history_admin(&incompatible_path)
            .err()
            .expect("an admin allocation with missing columns must be rejected");
        assert_eq!(admin_error.reason, R::Migration);
        assert_eq!(
            admin_error.sqlite_primary_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
        assert_eq!(
            admin_error.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
    }

    #[test]
    fn migration_open_preserves_sqlite_primary_and_extended_codes() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("migration.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE VIEW workflow_attempt_summaries AS
                 SELECT 'attempt-placeholder' AS attempt_id;",
            )
            .unwrap();
        drop(connection);

        let error = SqliteStore::open_shared_history(&path)
            .err()
            .expect("the conflicting schema must fail migration");
        assert_eq!(error.reason, R::Migration);
        assert_eq!(error.sqlite_primary_code, Some(rusqlite::ffi::SQLITE_ERROR));
        assert_eq!(
            error.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
    }

    #[test]
    fn readonly_and_admin_schema_probes_preserve_typed_migration_codes() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("admin-missing-schema.sqlite");
        drop(rusqlite::Connection::open(&path).unwrap());
        let readonly_error = SqliteStore::open_history_read_only(&path)
            .err()
            .expect("a read-only allocation without the history schema must be rejected");
        assert_eq!(readonly_error.reason, R::Migration);
        assert_eq!(
            readonly_error.sqlite_primary_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
        assert_eq!(
            readonly_error.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );

        let error = SqliteStore::open_history_admin(&path)
            .err()
            .expect("an allocation without the history schema must be rejected");
        assert_eq!(error.reason, R::Migration);
        assert_eq!(error.sqlite_primary_code, Some(rusqlite::ffi::SQLITE_ERROR));
        assert_eq!(
            error.sqlite_extended_code,
            Some(rusqlite::ffi::SQLITE_ERROR)
        );
    }

    #[test]
    fn filesystem_lock_failures_keep_distinct_typed_categories() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        for (kind, reason) in [
            (std::io::ErrorKind::WouldBlock, R::Locked),
            (std::io::ErrorKind::Unsupported, R::AdvisoryLockUnsupported),
            (std::io::ErrorKind::PermissionDenied, R::ReadOnlyLock),
            (std::io::ErrorKind::Other, R::AdvisoryLockIo),
        ] {
            let error = std::io::Error::from(kind);
            assert_eq!(history_lock_error(&error).reason, reason);
        }

        let permission = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        for reason in [R::ReadOnlyDatabase, R::ReadOnlyLock, R::ReadOnlyParent] {
            assert_eq!(
                history_io_error_with_permission(&permission, R::Io, reason).reason,
                reason
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn readonly_history_open_process_helper() {
        use bridge_core::workflow_history::LedgerUnavailableReason as R;

        let Some(path) = std::env::var_os("A2A_BRIDGE_TEST_READONLY_PATH") else {
            return;
        };
        let expected = match std::env::var("A2A_BRIDGE_TEST_READONLY_EXPECT")
            .unwrap()
            .as_str()
        {
            "parent" => R::ReadOnlyParent,
            "lock" => R::ReadOnlyLock,
            "database" => R::ReadOnlyDatabase,
            other => panic!("unknown read-only fixture kind: {other}"),
        };
        let error = match std::env::var("A2A_BRIDGE_TEST_READONLY_OPEN")
            .unwrap_or_else(|_| "shared".into())
            .as_str()
        {
            "shared" => SqliteStore::open_shared_history(std::path::Path::new(&path)),
            "concurrent" => SqliteStore::open_history(std::path::Path::new(&path)),
            "admin" => SqliteStore::open_history_admin(std::path::Path::new(&path)),
            other => panic!("unknown read-only fixture opener: {other}"),
        }
        .err()
        .expect("read-only fixture must refuse opening");
        assert_eq!(error.reason, expected);
    }

    #[cfg(unix)]
    #[test]
    fn real_readonly_parent_lock_and_database_failures_remain_distinct() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::os::unix::process::CommandExt;

        fn set_mode(path: &std::path::Path, mode: u32) {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        fn run_fixture(
            path: &std::path::Path,
            expected: &str,
            opener: &str,
            drop_privileges: bool,
        ) {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .arg("sqlite::r2f0a_history_tests::readonly_history_open_process_helper")
                .arg("--exact")
                .arg("--nocapture")
                .env("A2A_BRIDGE_TEST_READONLY_PATH", path)
                .env("A2A_BRIDGE_TEST_READONLY_EXPECT", expected)
                .env("A2A_BRIDGE_TEST_READONLY_OPEN", opener)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if drop_privileges {
                command.uid(65_534).gid(65_534);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{expected} fixture failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let directory = tempfile::tempdir().unwrap();
        set_mode(directory.path(), 0o755);
        let drop_privileges = std::fs::metadata(directory.path()).unwrap().uid() == 0;

        let readonly_parent = directory.path().join("readonly-parent");
        std::fs::create_dir(&readonly_parent).unwrap();
        set_mode(&readonly_parent, 0o555);
        run_fixture(
            &readonly_parent.join("missing-lock.sqlite"),
            "parent",
            "shared",
            drop_privileges,
        );
        set_mode(&readonly_parent, 0o755);

        // The schema lock is deliberately present and writable in both cases:
        // failure comes from creating the absent database beneath the parent,
        // not from either auxiliary-lock path.
        let missing_lifetime_parent = directory.path().join("missing-lifetime-database");
        std::fs::create_dir(&missing_lifetime_parent).unwrap();
        let missing_lifetime_db = missing_lifetime_parent.join("history.sqlite");
        let missing_lifetime_lock = history_schema_lock_path(&missing_lifetime_db);
        std::fs::write(&missing_lifetime_lock, b"lock").unwrap();
        set_mode(&missing_lifetime_lock, 0o666);
        set_mode(&missing_lifetime_parent, 0o555);
        run_fixture(&missing_lifetime_db, "parent", "shared", drop_privileges);
        set_mode(&missing_lifetime_parent, 0o755);

        let missing_concurrent_parent = directory.path().join("missing-concurrent-database");
        std::fs::create_dir(&missing_concurrent_parent).unwrap();
        let missing_concurrent_db = missing_concurrent_parent.join("history.sqlite");
        let missing_concurrent_lock = history_schema_lock_path(&missing_concurrent_db);
        std::fs::write(&missing_concurrent_lock, b"lock").unwrap();
        set_mode(&missing_concurrent_lock, 0o666);
        let missing_concurrent_attempts = history_attempt_lock_dir(&missing_concurrent_db);
        std::fs::create_dir(&missing_concurrent_attempts).unwrap();
        set_mode(&missing_concurrent_attempts, 0o777);
        set_mode(&missing_concurrent_parent, 0o555);
        run_fixture(
            &missing_concurrent_db,
            "parent",
            "concurrent",
            drop_privileges,
        );
        set_mode(&missing_concurrent_parent, 0o755);

        let readonly_lock_parent = directory.path().join("readonly-lock");
        std::fs::create_dir(&readonly_lock_parent).unwrap();
        set_mode(&readonly_lock_parent, 0o777);
        let readonly_lock_db = readonly_lock_parent.join("history.sqlite");
        let readonly_lock = history_schema_lock_path(&readonly_lock_db);
        std::fs::write(&readonly_lock, b"lock").unwrap();
        set_mode(&readonly_lock, 0o400);
        run_fixture(&readonly_lock_db, "lock", "shared", drop_privileges);
        set_mode(&readonly_lock, 0o600);

        let readonly_db_parent = directory.path().join("readonly-database");
        std::fs::create_dir(&readonly_db_parent).unwrap();
        let readonly_db = readonly_db_parent.join("history.sqlite");
        drop(SqliteStore::open_shared_history(&readonly_db).unwrap());
        set_mode(&readonly_db_parent, 0o777);
        set_mode(&history_schema_lock_path(&readonly_db), 0o666);
        set_mode(&readonly_db, 0o444);
        run_fixture(&readonly_db, "database", "shared", drop_privileges);
        set_mode(&readonly_db, 0o600);

        let readonly_concurrent_parent = directory.path().join("readonly-concurrent-database");
        std::fs::create_dir(&readonly_concurrent_parent).unwrap();
        let readonly_concurrent_db = readonly_concurrent_parent.join("history.sqlite");
        drop(SqliteStore::open_history(&readonly_concurrent_db).unwrap());
        set_mode(&readonly_concurrent_parent, 0o777);
        set_mode(&history_schema_lock_path(&readonly_concurrent_db), 0o666);
        set_mode(&history_attempt_lock_dir(&readonly_concurrent_db), 0o777);
        set_mode(&readonly_concurrent_db, 0o444);
        run_fixture(
            &readonly_concurrent_db,
            "database",
            "concurrent",
            drop_privileges,
        );
        set_mode(&readonly_concurrent_db, 0o600);

        let readonly_admission_parent = directory.path().join("readonly-admission-parent");
        std::fs::create_dir(&readonly_admission_parent).unwrap();
        let readonly_admission_db = readonly_admission_parent.join("history.sqlite");
        drop(SqliteStore::open_shared_history(&readonly_admission_db).unwrap());
        std::fs::remove_file(history_admission_lock_path(&readonly_admission_db)).unwrap();
        set_mode(&readonly_admission_db, 0o666);
        set_mode(&readonly_admission_parent, 0o555);
        run_fixture(&readonly_admission_db, "parent", "admin", drop_privileges);
        set_mode(&readonly_admission_parent, 0o755);
        set_mode(&readonly_admission_db, 0o600);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn platform_history_files_and_attempt_leases_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let store = SqliteStore::open_history(&path).unwrap();
        let row = parent_reservation();
        store.reserve(&row).await.unwrap();
        let attempt_lock = attempt_lock_path(&path, &row);
        assert!(attempt_lock.exists());
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(std::path::PathBuf::from(lock_path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(history_admission_lock_path(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(history_attempt_lock_dir(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(
                history_attempt_lock_dir(&path)
                    .join(format!("{}.lock", row.identity.attempt_id.as_str()))
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        store
            .terminalize(&row.identity.attempt_id, &terminal())
            .await
            .unwrap();
        assert!(
            !attempt_lock.exists(),
            "successful terminalization removes the exact attempt lock"
        );
    }

    #[test]
    fn allocation_and_privacy_bounds_are_closed() {
        assert_eq!(bridge_core::workflow_history::RETENTION_DAYS, 180);
        assert_eq!(bridge_core::workflow_history::MAX_TERMINAL_ROWS, 100_000);
        assert_eq!(
            bridge_core::workflow_history::MAX_CHARGED_BYTES,
            128 * 1024 * 1024
        );
        let mut row = reservation();
        row.workflow = "/private/repository/prompt".into();
        assert!(row.validate().is_err());
        let mut value = terminal();
        value.phase_durations = (0..=bridge_core::workflow_history::MAX_PHASES)
            .map(|n| bridge_core::workflow_history::PhaseDuration {
                phase: format!("p{n}"),
                duration_ms: 1,
            })
            .collect();
        assert!(value.validate().is_err());
    }
}
