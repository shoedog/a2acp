// Before G-A2.2 activation: add checkpoint replay and the complete diagnostic,
// terminal-evidence, and fallible lifecycle forwarding matrix.
use bridge_core::{accounting::*, ids::*, orch::*, task_store::TaskStore};
use bridge_store::sqlite::SqliteStore;
use std::sync::Arc;
fn path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("tasks.sqlite")
}
fn usage_path(path: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.usage-v1.sqlite", path.display()))
}
fn tokens() -> UsageSnapshot {
    UsageSnapshot {
        terminal: Some(TerminalUsage {
            total_tokens: 15,
            input_tokens: 10,
            output_tokens: 5,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        }),
        ..Default::default()
    }
}
fn cost(amount: f64, currency: &str) -> UsageSnapshot {
    UsageSnapshot {
        cost: Some(UsageCost {
            amount,
            currency: currency.into(),
        }),
        ..Default::default()
    }
}
struct Mapping(AtomicUsize);
impl BillingIdentityProviderV1 for Mapping {
    fn identity(&self, _: &str, _: Option<&str>) -> Option<BillingIdentityV1> {
        Some(BillingIdentityV1 {
            billing_mode: "api".into(),
            account_group: self.0.load(SeqCst).to_string(),
        })
    }
}
#[tokio::test]
async fn snapshots_reopen_retries_and_frozen_attribution() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let owner = AttemptIdentity::initial().unwrap();
    let mut service = AccountingV1::new(store.clone());
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(86_399_999));
    let clock_copy = clock.clone();
    service.clock = Arc::new(move || clock_copy.load(SeqCst));
    let mapping = Arc::new(Mapping(AtomicUsize::new(0)));
    service.billing = Some(mapping.clone());
    let query = UsageQueryV1 {
        group_by_utc_day: true,
        ..Default::default()
    };
    let attr = UsageAttributionV1 {
        effective_effort: Some("high".into()),
        ..Default::default()
    };
    for turn in 0..2 {
        let attempt = service
            .start(&owner, AttemptId::mint().unwrap(), attr.clone())
            .await
            .unwrap();
        let id = attempt.id().to_owned();
        clock.store(86_400_001, SeqCst);
        mapping.0.store(1, SeqCst);
        assert!(service
            .start(&owner, AttemptId::parse(&id).unwrap(), Default::default())
            .await
            .is_err());
        let mut usage = tokens();
        usage.terminal.as_mut().unwrap().cached_read_tokens = Some(7);
        let complement = cost(0.0, "USD");
        let backend = AccountingBackendV1::new(script(
            vec![
                Ok(Update::Usage(usage)),
                Ok(Update::Usage(tokens())),
                Ok(Update::Usage(complement)),
                Ok(Update::done("end_turn")),
            ],
            None,
        ));
        let mut stream = dispatch(&backend, attempt).await.unwrap();
        while let Some(item) = next(&mut stream).await {
            item.unwrap();
        }
        store
            .update_usage_attempt_v1(&id, false, Some(tokens()), true)
            .await
            .unwrap();
        let b = &store.usage_summary_v1(&query).await.unwrap().buckets[turn];
        assert_eq!(b.total_tokens.reported_sum, 15);
        assert_eq!(b.cached_read_tokens.reported_sum, 7);
        assert_eq!(b.keys.utc_day, Some(turn as i64));
        assert_eq!(
            b.keys.billing.as_ref().unwrap().account_group,
            turn.to_string()
        );
    }
    let summary = store.usage_summary_v1(&query).await.unwrap();
    assert!(summary.historical_incomplete);
    assert_eq!(summary.buckets.iter().map(|b| b.attempted).sum::<u64>(), 2);
    drop(service);
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    let reopened = store.usage_summary_v1(&query).await.unwrap();
    assert_eq!(
        serde_json::to_value(summary).unwrap(),
        serde_json::to_value(reopened).unwrap()
    );
}
#[tokio::test]
async fn cost_quality_and_occupancy_are_not_charges() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let service = AccountingV1::new(store.clone());
    let owner = AttemptIdentity::initial().unwrap();
    for (amount, currency, mode) in [
        (Some(0.0), "USD", "api"),
        (None, "USD", "api"),
        (Some(-1.0), "USD", "api"),
        (Some(f64::NAN), "USD", "api"),
        (Some(f64::INFINITY), "USD", "api"),
        (Some(2.0), "", "api"),
        (Some(3.0), "USD", "subscription"),
        (Some(4.0), "EUR", "api"),
    ] {
        let attr = UsageAttributionV1 {
            billing: Some(BillingIdentityV1 {
                billing_mode: mode.into(),
                account_group: "a".into(),
            }),
            ..Default::default()
        };
        let a = service
            .start(&owner, AttemptId::mint().unwrap(), attr)
            .await
            .unwrap();
        let mut usage = amount.map(|v| cost(v, currency)).unwrap_or_default();
        usage.used = Some(99999);
        store
            .update_usage_attempt_v1(a.id(), true, Some(usage), false)
            .await
            .unwrap();
        if amount.is_some_and(|v| !v.is_finite() || v < 0.0) || currency.is_empty() {
            store
                .update_usage_attempt_v1(a.id(), false, Some(cost(1.0, "EUR")), false)
                .await
                .unwrap();
        }
        store
            .update_usage_attempt_v1(a.id(), false, None, true)
            .await
            .unwrap();
    }
    let s = store.usage_summary_v1(&Default::default()).await.unwrap();
    assert_eq!(s.buckets.len(), 2);
    let b = &s.buckets[0];
    assert_eq!((b.cost_known, b.cost_missing, b.cost_invalid), (2, 1, 4));
    assert_eq!(b.total_tokens.reported_sum, 0);
    assert_eq!(b.total_tokens.missing_count, 7);
    assert_eq!(b.cost_by_currency.len(), 2);
    assert_eq!(b.cost_by_currency["USD"], 0.0);
    assert_eq!(b.cost_by_currency["EUR"], 4.0);
}
#[cfg(unix)]
#[tokio::test]
async fn aliases_and_retained_identity_refuse_without_sentinel_mutation() {
    use std::os::unix::fs::symlink;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        for hard in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = path(&dir);
            let store = SqliteStore::open_history(&path).unwrap();
            let sentinel = dir.path().join("sentinel.sqlite");
            let db = rusqlite::Connection::open(&sentinel).unwrap();
            db.execute_batch("CREATE TABLE sentinel(x); INSERT INTO sentinel VALUES(7);")
                .unwrap();
            drop(db);
            let before = std::fs::read(&sentinel).unwrap();
            let alias =
                std::path::PathBuf::from(format!("{}{suffix}", usage_path(&path).display()));
            if hard {
                std::fs::hard_link(&sentinel, alias).unwrap();
            } else {
                symlink(&sentinel, alias).unwrap();
            }
            assert!(
                store.usage_summary_v1(&Default::default()).await.is_err(),
                "accepted alias {suffix} hard={hard}"
            );
            let reader = SqliteStore::open_history_read_only(&path).unwrap();
            assert!(reader.usage_summary_v1(&Default::default()).await.is_err());
            assert_eq!(std::fs::read(&sentinel).unwrap(), before);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let store = SqliteStore::open(&path).unwrap();
    store.usage_summary_v1(&Default::default()).await.unwrap();
    let usage = usage_path(&path);
    std::fs::rename(&usage, dir.path().join("retained")).unwrap();
    std::fs::write(&usage, b"replacement").unwrap();
    assert!(store.usage_summary_v1(&Default::default()).await.is_err());
    assert_eq!(std::fs::read(usage).unwrap(), b"replacement");
}

fn write_lock_test_usage(conn: &rusqlite::Connection, id: &str) {
    assert_eq!(
        conn.execute(
            "UPDATE usage_attempts_v1 SET record_json=json_set(record_json,
             '$.usage', json(?2), '$.dispatched', json('true'), '$.finalized', json('true'))
             WHERE id=?1",
            rusqlite::params![id, serde_json::to_string(&tokens()).unwrap()],
        )
        .unwrap(),
        1
    );
}

// Run in a fresh process: SQLite's process-local inode bookkeeping must not
// conceal a lost POSIX lock in the parent.
#[test]
fn accounting_lock_child() {
    let Some(path) = std::env::var_os("A2A_ACCOUNTING_LOCK_TEST_DB") else {
        return;
    };
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.busy_timeout(std::time::Duration::ZERO).unwrap();
    let begin = conn.execute_batch("BEGIN IMMEDIATE");
    if let Ok(id) = std::env::var("A2A_ACCOUNTING_LOCK_TEST_UPDATE") {
        begin.unwrap();
        write_lock_test_usage(&conn, &id);
        conn.execute_batch("COMMIT").unwrap();
    } else {
        assert_eq!(
            begin.unwrap_err().sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy),
            "another process acquired a writer lock while A1 held BEGIN IMMEDIATE"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn accounting_identity_checks_and_teardown_preserve_process_locks() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let a1 = Arc::new(SqliteStore::open_history(&path).unwrap());
    let service = AccountingV1::new(a1.clone());
    let first = start(&service).await;
    let second = start(&service).await;
    let a2 = SqliteStore::open_history(&path).unwrap();
    a2.usage_summary_v1(&Default::default()).await.unwrap();
    let usage = usage_path(&path);
    let probe = |update: Option<&str>| {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "accounting_lock_child", "--nocapture"])
            .env("A2A_ACCOUNTING_LOCK_TEST_DB", &usage)
            .env_remove("A2A_ACCOUNTING_LOCK_TEST_UPDATE");
        if let Some(id) = update {
            cmd.env("A2A_ACCOUNTING_LOCK_TEST_UPDATE", id);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "child failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // This SQLite connection is A1's paused writer. Do not write until all the
    // probes finish, so a failed regression cannot corrupt the test database.
    let writer = rusqlite::Connection::open(&usage).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    probe(None);
    a2.usage_summary_v1(&Default::default()).await.unwrap();
    probe(None);
    // Admission of a new same-inode reader and teardown of either handle must
    // also leave A1's process locks intact.
    let reader = SqliteStore::open_history_read_only(&path).unwrap();
    reader.usage_summary_v1(&Default::default()).await.unwrap();
    probe(None);
    drop(reader);
    probe(None);
    drop(a2);
    probe(None);

    write_lock_test_usage(&writer, first.id());
    writer.execute_batch("COMMIT").unwrap();
    probe(Some(second.id()));
    drop(writer);
    drop(service);
    drop(a1);
    let reopened = SqliteStore::open_history(&path).unwrap();
    let summary = reopened
        .usage_summary_v1(&Default::default())
        .await
        .unwrap();
    assert_eq!(summary.buckets.len(), 1);
    assert_eq!(summary.buckets[0].attempted, 2);
    assert_eq!(summary.buckets[0].pending, 0);
    assert_eq!(summary.buckets[0].total_tokens.reported_sum, 30);
}

use bridge_core::{
    accounting_backend::AccountingBackendV1, domain::Part, error::BridgeError, ports::*,
};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering::SeqCst},
        Mutex,
    },
    task::{Context, Poll},
};
struct Script(
    Mutex<VecDeque<Result<Update, BridgeError>>>,
    AtomicUsize,
    Option<BridgeError>,
);
impl Stream for Script {
    type Item = Result<Update, BridgeError>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.lock().unwrap().pop_front())
    }
}
#[async_trait::async_trait]
impl AgentBackend for Script {
    async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream, BridgeError> {
        self.1.fetch_add(1, SeqCst);
        if let Some(error) = &self.2 {
            return Err(error.clone());
        }
        Ok(Box::pin(Script(
            Mutex::new(std::mem::take(&mut *self.0.lock().unwrap())),
            AtomicUsize::new(0),
            None,
        )))
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        Err(BridgeError::CancelTimeout)
    }
    fn capabilities(&self) -> AgentSessionCaps {
        AgentSessionCaps {
            resume: true,
            ..Default::default()
        }
    }
    async fn set_host_custody(&self, _: bool) -> Result<(), BridgeError> {
        Err(BridgeError::IdentityUnavailable)
    }
    async fn preserve_checkout_v1(
        &self,
        _: &SessionId,
        _: CheckoutPreservationReasonV1,
    ) -> CheckoutPreservationV1 {
        CheckoutPreservationV1::Preserved
    }
}
async fn next(stream: &mut BackendStream) -> Option<Result<Update, BridgeError>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}
fn script(events: Vec<Result<Update, BridgeError>>, error: Option<BridgeError>) -> Arc<Script> {
    Arc::new(Script(
        Mutex::new(events.into()),
        AtomicUsize::new(0),
        error,
    ))
}
async fn dispatch(
    backend: &dyn AgentBackend,
    attempt: PreparedUsageAttemptV1,
) -> Result<BackendStream, BridgeError> {
    backend
        .prompt_accounted_v1(&SessionId::parse("s").unwrap(), vec![], attempt, None)
        .await
}
async fn start(service: &AccountingV1) -> PreparedUsageAttemptV1 {
    service
        .start(
            &AttemptIdentity::initial().unwrap(),
            AttemptId::mint().unwrap(),
            Default::default(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn interrupted_capture_stays_pending_after_reopen() {
    for (error, ending) in [
        (Some(BridgeError::AgentTimedOut), "stream"),
        (Some(BridgeError::CancelTimeout), "stream"),
        (Some(BridgeError::FrameError), "stream"),
        (Some(BridgeError::agent_crashed("closed")), "stream"),
        (Some(BridgeError::AgentTimedOut), "prompt"),
        (None, "eof"),
        (None, "drop"),
        (None, "done"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = path(&dir);
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let service = AccountingV1::new(store.clone());
        let immediate = ending == "prompt";
        let eof = ending == "eof";
        let dropped = ending == "drop";
        let mut events = vec![Ok(Update::Usage(cost(1.25, "USD")))];
        if let Some(error) = &error {
            events.push(Err(error.clone()));
        } else if !eof {
            events.push(Ok(Update::done("end_turn")));
        }
        let pending = ending != "done";
        let backend =
            AccountingBackendV1::new(script(events, immediate.then(|| error.clone().unwrap())));
        let result = dispatch(&backend, start(&service).await).await;
        if immediate {
            assert_eq!(result.err(), error);
        } else {
            let mut stream = result.unwrap();
            let item = next(&mut stream).await.unwrap().unwrap();
            assert!(matches!(item, Update::Usage(_)));
            if !dropped {
                let item = next(&mut stream).await.unwrap();
                if let Some(error) = error {
                    assert_eq!(item.unwrap_err(), error);
                } else if eof {
                    assert!(item.is_err());
                } else {
                    assert!(matches!(item, Ok(Update::Done { .. })));
                }
            }
        }
        drop(service);
        drop(store);
        let store = SqliteStore::open(&path).unwrap();
        let s = store.usage_summary_v1(&Default::default()).await.unwrap();
        let b = &s.buckets[0];
        assert_eq!(b.pending, u64::from(pending));
        assert_eq!(b.cost_partial, pending);
        if !immediate {
            let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
            let json: String = db
                .query_row("SELECT record_json FROM usage_attempts_v1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            let row: UsageAttemptV1 = serde_json::from_str(&json).unwrap();
            assert_eq!(row.usage.cost.unwrap().amount, 1.25);
        }
    }
}
#[tokio::test]
async fn capture_persistence_failures_and_context_free_refusals() {
    for phase in ["start", "usage", "completion", "preparation"] {
        let dir = tempfile::tempdir().unwrap();
        let path = path(&dir);
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        store.usage_summary_v1(&Default::default()).await.unwrap();
        let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
        db.execute_batch("PRAGMA journal_mode=PERSIST").unwrap();
        let predicate = if phase == "start" { "INSERT" } else { "UPDATE" };
        let when = match phase {
            "usage" => "WHEN json_extract(NEW.record_json, '$.usage.cost') IS NOT NULL",
            "completion" => "WHEN json_extract(NEW.record_json, '$.finalized')=1",
            _ => "",
        };
        if phase != "preparation" {
            db.execute_batch(&format!("CREATE TRIGGER refuse BEFORE {predicate} ON usage_attempts_v1 {when} BEGIN SELECT RAISE(ABORT, 'test failure'); END;")).unwrap();
        }
        let service = AccountingV1::new(store.clone());
        let inner = script(
            vec![
                Ok(Update::Usage(cost(1.0, "USD"))),
                Ok(Update::done("end_turn")),
            ],
            None,
        );
        let backend = AccountingBackendV1::new(inner.clone());
        let session = SessionId::parse("s").unwrap();
        assert!(backend.prompt(&session, vec![]).await.is_err());
        let observers = BackendObservers::diagnostic_only(Arc::new(
            bridge_core::diagnostics::NoopDiagnosticObserver::default(),
        ));
        assert!(backend
            .prompt_with_observers(&session, vec![], observers.clone())
            .await
            .is_err());
        let attempt = service
            .start(
                &AttemptIdentity::initial().unwrap(),
                AttemptId::mint().unwrap(),
                Default::default(),
            )
            .await;
        if phase == "start" {
            assert!(attempt.is_err());
            assert_eq!(inner.1.load(SeqCst), 0);
            continue;
        }
        if phase == "preparation" {
            attempt.unwrap().preparation_failed().await.unwrap();
        } else {
            let mut stream = backend
                .prompt_accounted_v1(&session, vec![], attempt.unwrap(), Some(observers))
                .await
                .unwrap();
            if phase == "completion" {
                let item = next(&mut stream).await.unwrap().unwrap();
                assert!(matches!(item, Update::Usage(_)));
            }
            assert_eq!(
                next(&mut stream).await.unwrap().unwrap_err(),
                BridgeError::StoreFailure
            );
        }
        assert!(backend.capabilities().resume);
        let cancelled = backend.cancel(&session).await;
        assert_eq!(cancelled, Err(BridgeError::CancelTimeout));
        let custody = backend.set_host_custody(true).await;
        assert_eq!(custody, Err(BridgeError::IdentityUnavailable));
        let reason = CheckoutPreservationReasonV1::NodeFailure;
        let preserved = backend.preserve_checkout_v1(&session, reason).await;
        assert_eq!(preserved, CheckoutPreservationV1::Preserved);
        drop(service);
        drop(store);
        drop(db);
        let store = SqliteStore::open(&path).unwrap();
        let s = store.usage_summary_v1(&Default::default()).await.unwrap();
        assert_eq!(s.buckets[0].pending, u64::from(phase != "preparation"));
    }
}
