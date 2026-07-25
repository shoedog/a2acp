use bridge_core::diagnostics::{
    DiagnosticBuildError, DiagnosticEvent, DiagnosticFailureClass, DiagnosticOperation,
    DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic, FailureDiagnosticInput,
    FailureDisposition, InMemoryDiagnosticObserver, InMemoryDiagnosticObserverFactory,
    NoopDiagnosticObserver, PersistedPhaseTransition, PersistedPhaseTransitionInput, PhaseStatus,
    TaskJournalDiagnosticObserver, TaskJournalDiagnosticObserverFactory,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{NodeId, OperationId, SessionId, TaskId};
use bridge_core::orch::OrchEventKind;
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, DiagnosticObserverFactory,
    RichEventSink,
};
use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn event(
    phase: DiagnosticPhase,
    status: PhaseStatus,
    operation: Option<DiagnosticOperation>,
    at_ms: i64,
) -> DiagnosticEvent {
    let transition = PersistedPhaseTransition::build(
        PersistedPhaseTransitionInput {
            phase,
            status,
            at_ms,
            operation,
            code: None,
            auth: None,
        },
        &DiagnosticRedactor::default(),
    )
    .unwrap();
    DiagnosticEvent::new(transition, None).unwrap()
}

fn task_record(id: TaskId) -> TaskRecord {
    TaskRecord {
        id,
        workflow: "r2b2-observer".into(),
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
    }
}

#[tokio::test]
async fn bounded_observer_validates_grammar_and_retains_only_the_tail() {
    assert!(matches!(
        InMemoryDiagnosticObserver::new(0),
        Err(DiagnosticBuildError::InvalidObserverCapacity)
    ));

    let observer = InMemoryDiagnosticObserver::new(3).unwrap();
    assert_eq!(
        observer
            .record(event(DiagnosticPhase::Spawn, PhaseStatus::Failed, None, 1,))
            .await,
        Err(BridgeError::InvalidStateTransition),
        "a failed phase without its start must be rejected"
    );

    observer
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Started,
            None,
            2,
        ))
        .await
        .unwrap();
    observer
        .record(event(DiagnosticPhase::Spawn, PhaseStatus::Started, None, 3))
        .await
        .unwrap();
    observer
        .record(event(
            DiagnosticPhase::Spawn,
            PhaseStatus::Completed,
            None,
            4,
        ))
        .await
        .unwrap();
    observer
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Completed,
            None,
            5,
        ))
        .await
        .unwrap();

    let snapshot = observer.snapshot().await;
    assert_eq!(snapshot.len(), 3);
    assert_eq!(observer.dropped_count().await, 1);
    assert_eq!(snapshot[0].transition().phase(), DiagnosticPhase::Spawn);
    assert_eq!(snapshot[0].transition().status(), PhaseStatus::Started);
    assert_eq!(snapshot[2].transition().phase(), DiagnosticPhase::Resolve);
    assert_eq!(snapshot[2].transition().status(), PhaseStatus::Completed);
}

#[tokio::test]
async fn observer_rejects_duplicate_start_but_allows_repeated_config_operations() {
    let observer = InMemoryDiagnosticObserver::new(16).unwrap();
    let start_model = event(
        DiagnosticPhase::ConfigApply,
        PhaseStatus::Started,
        Some(DiagnosticOperation::Model),
        1,
    );
    observer.record(start_model.clone()).await.unwrap();
    assert_eq!(
        observer.record(start_model).await,
        Err(BridgeError::InvalidStateTransition)
    );
    observer
        .record(event(
            DiagnosticPhase::ConfigApply,
            PhaseStatus::Completed,
            Some(DiagnosticOperation::Model),
            2,
        ))
        .await
        .unwrap();
    observer
        .record(event(
            DiagnosticPhase::ConfigApply,
            PhaseStatus::Started,
            Some(DiagnosticOperation::Effort),
            3,
        ))
        .await
        .unwrap();
    observer
        .record(event(
            DiagnosticPhase::ConfigApply,
            PhaseStatus::Skipped,
            Some(DiagnosticOperation::Effort),
            4,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn in_memory_observer_debug_never_exposes_stored_diagnostic_text() {
    const SECRET_TEXT: &str = "diagnostic-secret-that-must-not-reach-debug";
    let observer = InMemoryDiagnosticObserver::new(4).unwrap();
    observer
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Started,
            None,
            1,
        ))
        .await
        .unwrap();
    let transition = PersistedPhaseTransition::build(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Resolve,
            status: PhaseStatus::Failed,
            at_ms: 2,
            operation: None,
            code: Some("resolve.failed".into()),
            auth: None,
        },
        &DiagnosticRedactor::default(),
    )
    .unwrap();
    let failure = FailureDiagnostic::build_at(
        FailureDiagnosticInput {
            failed_phase: DiagnosticPhase::Resolve,
            last_completed_phase: None,
            class: DiagnosticFailureClass::Config,
            disposition: FailureDisposition::Fatal,
            code: "resolve.failed".into(),
            summary: SECRET_TEXT.into(),
            causes: vec![format!("cause: {SECRET_TEXT}")],
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: false,
        },
        &DiagnosticRedactor::default(),
        2,
    )
    .unwrap();
    observer
        .record(DiagnosticEvent::new(transition, Some(failure)).unwrap())
        .await
        .unwrap();

    let rendered = format!("{observer:?}");
    assert!(!rendered.contains(SECRET_TEXT));
    assert!(!rendered.contains("resolve.failed"));
    assert_eq!(rendered, "InMemoryDiagnosticObserver { capacity: 4, .. }");
}

#[tokio::test]
async fn task_journal_observer_requires_a_real_row_and_persists_before_return() {
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let task = TaskId::parse("task-r2b2-observer").unwrap();
    let operation = OperationId::parse("op-r2b2-observer").unwrap();

    let missing =
        TaskJournalDiagnosticObserver::new(store.clone(), task.clone(), operation.clone()).await;
    assert!(matches!(missing, Err(BridgeError::StoreFailure)));
    assert!(matches!(
        TaskJournalDiagnosticObserverFactory::new(store.clone(), task.clone(), operation.clone(),)
            .await,
        Err(BridgeError::StoreFailure)
    ));

    store.create(&task_record(task.clone())).await.unwrap();
    let observer =
        TaskJournalDiagnosticObserver::new(store.clone(), task.clone(), operation.clone())
            .await
            .unwrap();
    observer
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Started,
            None,
            42,
        ))
        .await
        .unwrap();

    let journal = store.journal_from(&task, -1).await.unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].operation_id, operation);
    assert_eq!(journal[0].ts_ms, 42);
    match &journal[0].kind {
        OrchEventKind::Progress { progress } => {
            let diagnostic = progress
                .diagnostic_event()
                .expect("journal row must carry the diagnostic event");
            assert_eq!(diagnostic.transition().phase(), DiagnosticPhase::Resolve);
            assert_eq!(diagnostic.transition().status(), PhaseStatus::Started);
        }
        other => panic!("unexpected journal kind: {other:?}"),
    }

    let factory = TaskJournalDiagnosticObserverFactory::new(
        store.clone(),
        task.clone(),
        OperationId::parse("op-r2b2-factory").unwrap(),
    )
    .await
    .unwrap();
    factory
        .make(&NodeId::parse("node-a").unwrap(), 2)
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Started,
            None,
            43,
        ))
        .await
        .unwrap();
    assert_eq!(store.journal_from(&task, -1).await.unwrap().len(), 2);
}

#[derive(Default)]
struct CountingSink;

#[async_trait::async_trait]
impl RichEventSink for CountingSink {
    fn record(&self, _kind: OrchEventKind) {}

    async fn flush(&self) -> Result<(), BridgeError> {
        Ok(())
    }
}

#[derive(Default)]
struct LegacyBackend {
    prompt: AtomicUsize,
    prompt_observed: AtomicUsize,
    cancel: AtomicUsize,
    forget: AtomicUsize,
    release: AtomicUsize,
}

#[async_trait::async_trait]
impl AgentBackend for LegacyBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<bridge_core::domain::Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn prompt_observed(
        &self,
        _session: &SessionId,
        _parts: Vec<bridge_core::domain::Part>,
        _sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_observed.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        self.cancel.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn forget_session(&self, _session: &SessionId) {
        self.forget.fetch_add(1, Ordering::SeqCst);
    }

    async fn release_session(&self, _session: &SessionId) {
        self.release.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn composite_and_cleanup_defaults_preserve_legacy_backend_behavior() {
    let backend = LegacyBackend::default();
    let session = SessionId::parse("session-r2b2").unwrap();
    let diagnostic: Arc<dyn DiagnosticObserver> = Arc::new(NoopDiagnosticObserver::default());

    let _rich_stream = backend
        .prompt_with_observers(
            &session,
            vec![],
            BackendObservers::new(diagnostic.clone(), Some(Arc::new(CountingSink))),
        )
        .await
        .unwrap();
    let _plain_stream = backend
        .prompt_with_observers(
            &session,
            vec![],
            BackendObservers::diagnostic_only(diagnostic.clone()),
        )
        .await
        .unwrap();
    backend
        .cancel_observed(&session, diagnostic.clone())
        .await
        .unwrap();
    backend
        .forget_session_observed(&session, diagnostic.clone())
        .await
        .unwrap();
    backend
        .release_session_observed(&session, diagnostic)
        .await
        .unwrap();

    assert_eq!(backend.prompt_observed.load(Ordering::SeqCst), 1);
    assert_eq!(backend.prompt.load(Ordering::SeqCst), 1);
    assert_eq!(backend.cancel.load(Ordering::SeqCst), 1);
    assert_eq!(backend.forget.load(Ordering::SeqCst), 1);
    assert_eq!(backend.release.load(Ordering::SeqCst), 1);

    let factory = InMemoryDiagnosticObserverFactory::new(4).unwrap();
    factory
        .make(&NodeId::parse("node-b").unwrap(), 1)
        .record(event(
            DiagnosticPhase::Resolve,
            PhaseStatus::Started,
            None,
            1,
        ))
        .await
        .unwrap();
}
