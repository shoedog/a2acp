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
    fn identity(&self, _: &str, model: Option<&str>) -> Option<BillingIdentityV1> {
        Some(BillingIdentityV1 {
            billing_mode: model.unwrap_or("api").into(),
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
    let mut service = AccountingV1::new(store.clone());
    service.billing = Some(Arc::new(Mapping(AtomicUsize::new(0))));
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
            effective_model: Some(mode.into()),
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

use bridge_core::{
    governance::*,
    workflow_history::{AttemptReservation, ExecutionSurface, WorkflowHistoryStore},
};
fn policy(
    selectors: Vec<BudgetSelectorV1>,
    turns: Option<u64>,
    cap: Option<u64>,
) -> BudgetPolicyV1 {
    BudgetPolicyV1::new(
        selectors
            .into_iter()
            .enumerate()
            .map(|(i, selector)| BudgetRuleV1 {
                id: format!("rule-{i}"),
                selector,
                limits: BudgetLimitsV1 {
                    turns,
                    terminal_tokens: cap,
                },
            })
            .collect(),
        vec![TurnAllowanceV1 {
            target: candidate(),
            terminal_tokens: 10.try_into().unwrap(),
        }],
    )
    .unwrap()
}
fn candidate() -> BudgetCandidateV1 {
    BudgetCandidateV1 {
        agent: AgentId::parse("a").unwrap(),
        model: None,
    }
}
async fn owner_record(store: &SqliteStore, owner: &AttemptIdentity, started: i64) {
    if store.attempt(&owner.attempt_id).await.unwrap().is_none() {
        store
            .reserve(&AttemptReservation {
                identity: owner.clone(),
                task_id: None,
                workflow: "test".into(),
                task_class: "test".into(),
                surface: ExecutionSurface::Offline,
                policy: "test".into(),
                workload_fingerprint: "test".into(),
                started_ms: started.max(1),
                workload_fingerprint_complete: true,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            })
            .await
            .unwrap();
    }
}
async fn prepared(
    store: &SqliteStore,
    owner: &AttemptIdentity,
    started: i64,
) -> (AttemptId, BudgetContextV1) {
    owner_record(store, owner, started).await;
    let id = AttemptId::mint().unwrap();
    store
        .start_usage_attempt_v1(&UsageAttemptV1 {
            id: id.as_str().into(),
            attribution: UsageAttributionV1 {
                execution: Some(owner.execution_id.as_str().into()),
                run_attempt: Some(owner.attempt_id.as_str().into()),
                agent: "a".into(),
                ..Default::default()
            },
            started_ms: started,
            utc_day: started.div_euclid(86_400_000),
            dispatched: false,
            finalized: false,
            usage: Default::default(),
            cost_invalid: false,
        })
        .await
        .unwrap();
    (
        id,
        BudgetContextV1 {
            request: BudgetRequestIdV1::Execution(owner.execution_id.clone()),
            workflow_run: Some(owner.execution_id.clone()),
        },
    )
}
async fn admission(
    store: &SqliteStore,
    p: Option<&BudgetPolicyV1>,
    a: &(AttemptId, BudgetContextV1),
) -> BudgetAdmissionOutcomeV1 {
    store
        .admit_usage_attempt_v1(p, &a.0, &a.1, &candidate())
        .await
}
fn refused(outcome: BudgetAdmissionOutcomeV1, reason: BudgetRefusalReasonV1) {
    match outcome {
        BudgetAdmissionOutcomeV1::Refused(r) => assert_eq!(r.reason, reason),
        other => panic!("expected {reason:?}, got {other:?}"),
    }
}
fn admitted(outcome: BudgetAdmissionOutcomeV1) -> BudgetReservationV1 {
    match outcome {
        BudgetAdmissionOutcomeV1::Admitted(p) => p.into_reservation(),
        other => panic!("{other:?}"),
    }
}
fn saved(db: &rusqlite::Connection, id: &AttemptId) -> serde_json::Value {
    let s: String = db
        .query_row(
            "SELECT record_json FROM governance_reservations_v1 WHERE id=?1",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&s).unwrap()
}
#[tokio::test]
async fn governance_atomic_scopes_identity_and_policy() {
    use BudgetRefusalReasonV1 as R;
    for selector in [
        BudgetSelectorV1::EachWorkflowRun,
        BudgetSelectorV1::UtcDayAgent(candidate().agent),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = path(&dir);
        let store = SqliteStore::open(&path).unwrap();
        let owner = AttemptIdentity::initial().unwrap();
        let a = prepared(&store, &owner, i64::MAX / 2).await;
        let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
        db.execute_batch("PRAGMA journal_mode=PERSIST").unwrap();
        db.execute("UPDATE usage_coverage_v1 SET started_ms=0", [])
            .unwrap();
        let p = BudgetPolicyV1::new(
            vec![
                BudgetRuleV1 {
                    id: "a-request".into(),
                    selector: BudgetSelectorV1::EachRequest,
                    limits: BudgetLimitsV1 {
                        turns: Some(5),
                        terminal_tokens: None,
                    },
                },
                BudgetRuleV1 {
                    id: "z-blocking".into(),
                    selector,
                    limits: BudgetLimitsV1 {
                        turns: Some(0),
                        terminal_tokens: None,
                    },
                },
            ],
            vec![],
        )
        .unwrap();
        refused(admission(&store, Some(&p), &a).await, R::LimitExceeded);
        let count: u64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let row: String = db
            .query_row("SELECT record_json FROM usage_attempts_v1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            !serde_json::from_str::<UsageAttemptV1>(&row)
                .unwrap()
                .dispatched
        );
    }
    let store = SqliteStore::open_in_memory().unwrap();
    let owner = AttemptIdentity::initial().unwrap();
    let a = prepared(&store, &owner, i64::MAX / 2).await;
    let p = policy(
        vec![
            BudgetSelectorV1::EachRequest,
            BudgetSelectorV1::EachWorkflowRun,
        ],
        Some(4),
        None,
    );
    let mut wrong = candidate();
    wrong.model = Some("other".into());
    refused(
        store
            .admit_usage_attempt_v1(Some(&p), &a.0, &a.1, &wrong)
            .await,
        R::IdentityMismatch,
    );
    let mut other = a.1.clone();
    other.request = BudgetRequestIdV1::Execution(ExecutionId::mint().unwrap());
    refused(
        store
            .admit_usage_attempt_v1(Some(&p), &a.0, &other, &candidate())
            .await,
        R::UnsupportedRoute,
    );
    admitted(admission(&store, Some(&p), &a).await);
    refused(admission(&store, Some(&p), &a).await, R::DuplicateAttempt);
    let b = prepared(&store, &owner, i64::MAX / 2).await;
    refused(admission(&store, None, &b).await, R::PolicyMismatch);
    let mut fake_owner = b.1.clone();
    fake_owner.request = BudgetRequestIdV1::Execution(ExecutionId::mint().unwrap());
    fake_owner.workflow_run = None;
    refused(
        store
            .admit_usage_attempt_v1(None, &b.0, &fake_owner, &candidate())
            .await,
        R::IdentityMismatch,
    );
    let changed = policy(vec![BudgetSelectorV1::EachRequest], Some(5), None);
    refused(
        admission(&store, Some(&changed), &b).await,
        R::PolicyMismatch,
    );
    let renamed = BudgetPolicyV1::new(
        p.rules()
            .iter()
            .cloned()
            .map(|mut r| {
                r.id.push('x');
                r
            })
            .collect(),
        vec![],
    )
    .unwrap();
    refused(
        admission(&store, Some(&renamed), &b).await,
        R::PolicyMismatch,
    );
    let mut no_parent = b.1.clone();
    no_parent.workflow_run = None;
    refused(
        store
            .admit_usage_attempt_v1(Some(&p), &b.0, &no_parent, &candidate())
            .await,
        R::IdentityMismatch,
    );
}
#[tokio::test]
async fn governance_reopen_high_water_finalization_and_rollback() {
    let notices = Arc::new(AtomicUsize::new(0));
    let _subscriber = tracing::subscriber::set_default(Notices(notices.clone()));
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let owner = AttemptIdentity::initial().unwrap();
    let p = policy(vec![BudgetSelectorV1::EachRequest], None, Some(u64::MAX));
    let store = SqliteStore::open(&path).unwrap();
    let a = prepared(&store, &owner, i64::MAX / 2).await;
    admitted(admission(&store, Some(&p), &a).await);
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST").unwrap();
    assert_eq!(saved(&db, &a.0)["liability"]["Pending"], 10);
    for n in [20, 5] {
        let mut u = tokens();
        u.terminal.as_mut().unwrap().total_tokens = n;
        store
            .update_usage_attempt_v1(a.0.as_str(), true, Some(u), false)
            .await
            .unwrap();
    }
    assert_eq!(saved(&db, &a.0)["liability"]["Pending"], 20);
    db.execute_batch("CREATE TRIGGER fail BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'failure'); END;").unwrap();
    assert!(store
        .update_usage_attempt_v1(a.0.as_str(), false, Some(tokens()), true)
        .await
        .is_err());
    assert_eq!(saved(&db, &a.0)["liability"]["Pending"], 20);
    db.execute_batch("DROP TRIGGER fail").unwrap();
    for _ in 0..2 {
        store
            .update_usage_attempt_v1(a.0.as_str(), false, Some(tokens()), true)
            .await
            .unwrap();
    }
    assert_eq!(saved(&db, &a.0)["liability"]["Known"], 15);
    assert_eq!(notices.load(SeqCst), 1); // failed settlement and duplicate finalization emit nothing
    let low = prepared(&store, &AttemptIdentity::initial().unwrap(), i64::MAX / 2).await;
    admitted(admission(&store, Some(&p), &low).await);
    let mut u = tokens();
    u.terminal.as_mut().unwrap().total_tokens = 5;
    store
        .update_usage_attempt_v1(low.0.as_str(), false, Some(u), true)
        .await
        .unwrap();
    assert_eq!(saved(&db, &low.0)["liability"]["Known"], 5);
    assert_eq!(notices.load(SeqCst), 1);
    let b = prepared(&store, &owner, i64::MAX / 2).await;
    admitted(admission(&store, Some(&p), &b).await);
    store
        .update_usage_attempt_v1(b.0.as_str(), false, None, true)
        .await
        .unwrap();
    let c = prepared(&store, &owner, i64::MAX / 2).await;
    refused(
        admission(&store, Some(&p), &c).await,
        BudgetRefusalReasonV1::UnknownMeasurement,
    );
    assert_eq!(saved(&db, &b.0)["liability"]["Unknown"]["retained"], 10);
    // Full u64 never narrows.
    let fresh = AttemptIdentity::initial().unwrap();
    let d = prepared(&store, &fresh, i64::MAX / 2).await;
    admitted(admission(&store, Some(&p), &d).await);
    let mut u = tokens();
    u.terminal.as_mut().unwrap().total_tokens = u64::MAX;
    store
        .update_usage_attempt_v1(d.0.as_str(), false, Some(u), true)
        .await
        .unwrap();
    assert_eq!(
        saved(&db, &d.0)["liability"]["Known"].as_u64(),
        Some(u64::MAX)
    );
    let e = prepared(&store, &fresh, i64::MAX / 2).await;
    refused(
        admission(&store, Some(&p), &e).await,
        BudgetRefusalReasonV1::Overflow,
    );
}
#[tokio::test]
async fn governance_coverage_history_day_and_disabled() {
    use BudgetRefusalReasonV1 as R;
    let store = SqliteStore::open_in_memory().unwrap();
    let a = prepared(&store, &AttemptIdentity::initial().unwrap(), 86_399_999).await;
    let p = policy(
        vec![BudgetSelectorV1::UtcDayAgent(candidate().agent)],
        Some(5),
        None,
    );
    refused(admission(&store, Some(&p), &a).await, R::IncompleteCoverage);
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(1));
    let copy = clock.clone();
    let store =
        SqliteStore::open_in_memory_with_clock(Arc::new(move || copy.load(SeqCst))).unwrap();
    store.usage_summary_v1(&Default::default()).await.unwrap();
    let a = prepared(&store, &AttemptIdentity::initial().unwrap(), 86_399_999).await;
    clock.store(86_400_001, SeqCst);
    let r = admitted(admission(&store, Some(&p), &a).await);
    assert_eq!((r.admission_day, r.admitted_ms), (1, 86_400_001));
    clock.store(172_800_001, SeqCst);
    store
        .update_usage_attempt_v1(a.0.as_str(), false, Some(tokens()), true)
        .await
        .unwrap();
    assert_eq!(
        store
            .usage_summary_v1(&Default::default())
            .await
            .unwrap()
            .buckets[0]
            .total_tokens
            .reported_sum,
        15
    );
    // Admission on the next day does not charge preparation's or completion's day.
    let next = prepared(&store, &AttemptIdentity::initial().unwrap(), 172_800_001).await;
    assert_eq!(
        admitted(admission(&store, Some(&p), &next).await).admission_day,
        2
    );
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let store = SqliteStore::open(&path).unwrap();
    let owner = AttemptIdentity::initial().unwrap();
    let a = prepared(&store, &owner, i64::MAX / 2).await;
    assert!(matches!(
        admission(&store, None, &a).await,
        BudgetAdmissionOutcomeV1::Ungoverned
    ));
    let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST").unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        0
    );
    let p = policy(vec![BudgetSelectorV1::EachRequest], Some(1), None);
    store
        .update_usage_attempt_v1(a.0.as_str(), true, Some(tokens()), true)
        .await
        .unwrap();
    let b = prepared(&store, &owner, i64::MAX / 2).await;
    refused(admission(&store, Some(&p), &b).await, R::LimitExceeded);
    let p = policy(vec![BudgetSelectorV1::EachRequest], Some(4), None);
    admitted(admission(&store, Some(&p), &b).await);
    let c = prepared(&store, &owner, i64::MAX / 2).await;
    let d = prepared(&store, &owner, i64::MAX / 2).await;
    store
        .update_usage_attempt_v1(c.0.as_str(), true, None, false)
        .await
        .unwrap();
    refused(admission(&store, Some(&p), &d).await, R::IncompleteCoverage);
    store
        .update_usage_attempt_v1(c.0.as_str(), false, None, true)
        .await
        .unwrap();
    admitted(admission(&store, Some(&p), &d).await); // unknown tokens do not block turns
    db.execute("UPDATE governance_reservations_v1 SET record_json='{}'", [])
        .unwrap();
    refused(
        admission(&store, Some(&p), &d).await,
        R::AccountingUnavailable,
    );
}

#[test]
fn governance_process_child() {
    let Some(path) = std::env::var_os("A2A_GOVERNANCE_TEST_DB") else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let store = SqliteStore::open_history(std::path::Path::new(&path)).unwrap();
        let id = AttemptId::parse(std::env::var("A2A_GOVERNANCE_TEST_ATTEMPT").unwrap()).unwrap();
        let context: BudgetContextV1 =
            serde_json::from_str(&std::env::var("A2A_GOVERNANCE_TEST_CONTEXT").unwrap()).unwrap();
        let p = policy(vec![BudgetSelectorV1::EachRequest], Some(1), Some(10));
        let gate = std::path::PathBuf::from(std::env::var_os("A2A_GOVERNANCE_TEST_GATE").unwrap());
        std::fs::write(gate.with_extension(id.as_str()), b"ready").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !gate.exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        match admission(&store, Some(&p), &(id, context)).await {
            BudgetAdmissionOutcomeV1::Admitted(_) => (),
            BudgetAdmissionOutcomeV1::Refused(r)
                if r.reason == BudgetRefusalReasonV1::LimitExceeded =>
            {
                std::process::exit(23)
            }
            other => panic!("{other:?}"),
        }
    });
}
#[tokio::test]
async fn governance_independent_processes_compete_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let store = SqliteStore::open_history(&path).unwrap();
    let owner = AttemptIdentity::initial().unwrap();
    let a = prepared(&store, &owner, i64::MAX / 2).await;
    let b = prepared(&store, &owner, i64::MAX / 2).await;
    let gate = dir.path().join("gate");
    let mut children: Vec<_> = [&a, &b]
        .into_iter()
        .map(|a| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "governance_process_child"])
                .env("A2A_GOVERNANCE_TEST_DB", &path)
                .env("A2A_GOVERNANCE_TEST_GATE", &gate)
                .env("A2A_GOVERNANCE_TEST_ATTEMPT", a.0.as_str())
                .env(
                    "A2A_GOVERNANCE_TEST_CONTEXT",
                    serde_json::to_string(&a.1).unwrap(),
                )
                .spawn()
                .unwrap()
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while [&a, &b]
        .iter()
        .any(|a| !gate.with_extension(a.0.as_str()).exists())
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    std::fs::write(&gate, b"go").unwrap();
    let mut codes: Vec<_> = children
        .iter_mut()
        .map(|c| c.wait().unwrap().code().unwrap())
        .collect();
    codes.sort();
    assert_eq!(codes, [0, 23]);
    drop(store);
    let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST").unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM governance_reservations_v1", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(db.query_row("SELECT count(*) FROM usage_attempts_v1 WHERE json_extract(record_json, '$.dispatched')=1", [], |r| r.get::<_, u64>(0)).unwrap(), 1);
    assert_eq!(db.query_row("SELECT json_extract(record_json, '$.liability.Pending') FROM governance_reservations_v1", [], |r| r.get::<_, u64>(0)).unwrap(), 10);
}

// A thread-local subscriber keeps the completion assertion independent of other tests' logs.
struct Notices(Arc<AtomicUsize>);
impl tracing::field::Visit for Notices {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message"
            && format!("{value:?}") == "governed turn completed above its token allowance"
        {
            self.0.fetch_add(1, SeqCst);
        }
    }
}
impl tracing::Subscriber for Notices {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut Notices(self.0.clone()));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
#[tokio::test]
async fn governance_new_token_scope_requires_pending_coverage() {
    use BudgetRefusalReasonV1 as R;
    let store = SqliteStore::open_in_memory_with_clock(Arc::new(|| 100)).unwrap();
    let owner = AttemptIdentity::initial().unwrap();
    let a = prepared(&store, &owner, 100).await;
    let turns = policy(vec![BudgetSelectorV1::EachRequest], Some(10), None);
    admitted(admission(&store, Some(&turns), &a).await);
    let b = prepared(&store, &owner, 100).await;
    let token = policy(vec![BudgetSelectorV1::EachWorkflowRun], None, Some(10));
    let combined = BudgetPolicyV1::new(
        [turns.rules(), token.rules()]
            .concat()
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                r.id = format!("rule-{i}");
                r
            })
            .collect(),
        token.allowances().to_vec(),
    )
    .unwrap();
    refused(
        admission(&store, Some(&combined), &b).await,
        R::IncompleteCoverage,
    );
    let mut u = tokens();
    u.terminal.as_mut().unwrap().total_tokens = 1;
    store
        .update_usage_attempt_v1(a.0.as_str(), false, Some(u), true)
        .await
        .unwrap();
    refused(
        admission(&store, Some(&combined), &b).await,
        R::LimitExceeded,
    ); // finalized actual + new allowance
    let token = policy(vec![BudgetSelectorV1::EachWorkflowRun], None, Some(11));
    let mut rules = turns.rules().to_vec();
    let mut r = token.rules()[0].clone();
    r.id = "rule-1".into();
    rules.push(r);
    let p = BudgetPolicyV1::new(rules, token.allowances().to_vec()).unwrap();
    admitted(admission(&store, Some(&p), &b).await); // failed admissions published no binding
    let c = prepared(&store, &owner, 100).await;
    let changed = BudgetPolicyV1::new(
        p.rules().to_vec(),
        vec![TurnAllowanceV1 {
            target: candidate(),
            terminal_tokens: 11.try_into().unwrap(),
        }],
    )
    .unwrap();
    refused(
        admission(&store, Some(&changed), &c).await,
        R::PolicyMismatch,
    );
    let missing = BudgetPolicyV1::new(p.rules().to_vec(), vec![]).unwrap();
    refused(
        admission(&store, Some(&missing), &c).await,
        R::MissingAllowance,
    );
}
#[tokio::test]
async fn governance_lazy_bootstrap_and_unproven_owners() {
    use BudgetRefusalReasonV1 as R;
    let store = Arc::new(SqliteStore::open_in_memory_with_clock(Arc::new(|| 101)).unwrap());
    let owner = AttemptIdentity::initial().unwrap();
    owner_record(&store, &owner, 90).await;
    let mut accounting = AccountingV1::new(store.clone());
    accounting.clock = Arc::new(|| 100); // captured before lazy sidecar initialization at 101
    let p = policy(vec![BudgetSelectorV1::EachRequest], Some(2), None);
    for _ in 0..2 {
        let a = accounting
            .start(
                &owner,
                AttemptId::mint().unwrap(),
                UsageAttributionV1 {
                    agent: "a".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let ctx = BudgetContextV1 {
            request: BudgetRequestIdV1::Execution(owner.execution_id.clone()),
            workflow_run: Some(owner.execution_id.clone()),
        };
        admitted(
            store
                .admit_usage_attempt_v1(
                    Some(&p),
                    &AttemptId::parse(a.id()).unwrap(),
                    &ctx,
                    &candidate(),
                )
                .await,
        );
    }
    assert_eq!(
        store
            .usage_summary_v1(&Default::default())
            .await
            .unwrap()
            .coverage_start_ms,
        101
    );
    let legacy = AttemptIdentity::initial().unwrap();
    owner_record(&store, &legacy, 90).await;
    store
        .mark_prompt_acceptance(&legacy.attempt_id, "dispatch_uncertain")
        .await
        .unwrap();
    let a = prepared(&store, &legacy, 100).await;
    refused(admission(&store, Some(&p), &a).await, R::IncompleteCoverage);
    // No ledger birth record is also not affirmative evidence of a fresh owner.
    let unknown = AttemptIdentity::initial().unwrap();
    let a = accounting
        .start(
            &unknown,
            AttemptId::mint().unwrap(),
            UsageAttributionV1 {
                agent: "a".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ctx = BudgetContextV1 {
        request: BudgetRequestIdV1::Execution(unknown.execution_id),
        workflow_run: None,
    };
    refused(
        store
            .admit_usage_attempt_v1(
                Some(&p),
                &AttemptId::parse(a.id()).unwrap(),
                &ctx,
                &candidate(),
            )
            .await,
        R::IncompleteCoverage,
    );
}
#[tokio::test]
async fn governance_corruption_replacement_and_unrelated_history() {
    use BudgetRefusalReasonV1 as R;
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let store = SqliteStore::open(&path).unwrap();
    let p = policy(vec![BudgetSelectorV1::EachRequest], Some(2), None);
    for _ in 0..100 {
        let a = prepared(&store, &AttemptIdentity::initial().unwrap(), i64::MAX / 2).await;
        admitted(admission(&store, Some(&p), &a).await);
        store
            .update_usage_attempt_v1(a.0.as_str(), false, Some(tokens()), true)
            .await
            .unwrap();
    }
    let db = rusqlite::Connection::open(usage_path(&path)).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER no_rewrite BEFORE UPDATE ON governance_reservations_v1 BEGIN SELECT RAISE(ABORT, 'unrelated write'); END;").unwrap();
    let a = prepared(&store, &AttemptIdentity::initial().unwrap(), i64::MAX / 2).await;
    admitted(admission(&store, Some(&p), &a).await);
    db.execute_batch("DROP TRIGGER no_rewrite").unwrap();
    let b = prepared(&store, &AttemptIdentity::initial().unwrap(), i64::MAX / 2).await;
    let original = saved(&db, &a.0).to_string();
    for invalid in ["-1", "18446744073709551616", "\"invalid\""] {
        db.execute("UPDATE governance_reservations_v1 SET record_json=json_set(record_json, '$.liability.Pending', json(?1)) WHERE id=?2", rusqlite::params![invalid, a.0.as_str()]).unwrap();
        refused(
            admission(&store, Some(&p), &b).await,
            R::AccountingUnavailable,
        );
    }
    db.execute(
        "UPDATE governance_reservations_v1 SET record_json=?1 WHERE id=?2",
        rusqlite::params![original, a.0.as_str()],
    )
    .unwrap();
    drop(db);
    let usage = usage_path(&path);
    std::fs::rename(&usage, dir.path().join("retained")).unwrap();
    std::fs::write(&usage, b"replacement").unwrap();
    refused(
        admission(&store, Some(&p), &b).await,
        R::AccountingUnavailable,
    );
    assert_eq!(std::fs::read(usage).unwrap(), b"replacement");
}
