use std::sync::Arc;
use std::time::Duration;

use bridge_core::domain::{Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendResourceFlightV1, DiagnosticObserver,
};
use tokio::sync::Mutex;

use crate::turn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Death {
    Transient,
    Fatal,
}

pub fn classify_death(e: &BridgeError) -> Death {
    use BridgeError::*;
    match e {
        AgentFailure { diagnostic } => match diagnostic.disposition() {
            bridge_core::diagnostics::FailureDisposition::Fatal => Death::Fatal,
            bridge_core::diagnostics::FailureDisposition::RetrySameTarget => Death::Transient,
            bridge_core::diagnostics::FailureDisposition::ContainerFallbackCandidate => {
                Death::Fatal
            }
        },
        AgentCrashed { .. } | AgentOverloaded | SessionNotFound | CancelTimeout | FrameError => {
            Death::Transient
        }
        TaskSpecInvalid { .. } => Death::Fatal,
        _ => Death::Fatal,
    }
}

#[async_trait::async_trait]
pub trait WarmRebuild: Send + Sync {
    async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError>;
}

pub type ResetWorktree = dyn Fn() -> Result<(), String> + Send + Sync;

pub struct ResilientWarm {
    backend: Mutex<Arc<dyn AgentBackend>>,
    rebuild: Arc<dyn WarmRebuild>,
    reset_worktree: Arc<ResetWorktree>,
    spec: SessionSpec,
    /// Run-wide respawn budget shared by the edit turn and all fix turns; 0 disables auto-respawn.
    respawns_left: Mutex<u32>,
    clone_config_guard: bool,
}

impl ResilientWarm {
    pub fn new(
        backend: Arc<dyn AgentBackend>,
        rebuild: Arc<dyn WarmRebuild>,
        spec: SessionSpec,
        max_respawns: u32,
        reset_worktree: Arc<ResetWorktree>,
    ) -> Self {
        Self {
            backend: Mutex::new(backend),
            rebuild,
            reset_worktree,
            spec,
            respawns_left: Mutex::new(max_respawns),
            clone_config_guard: true,
        }
    }

    pub fn with_clone_config_guard(mut self, enabled: bool) -> Self {
        self.clone_config_guard = enabled;
        self
    }

    pub async fn acquire_host_custody(&self) -> Result<(), String> {
        if !self.clone_config_guard {
            return Ok(());
        }
        self.backend
            .lock()
            .await
            .clone()
            .set_host_custody(true)
            .await
            .map_err(|_| {
                let reason =
                    "implement: cannot freeze clone writers; stopping before host git (clone kept)";
                eprintln!("{reason}");
                reason.into()
            })
    }

    pub async fn retire(&self) -> Result<(), BridgeError> {
        let backend = self.backend.lock().await.clone();
        backend.resource_flight_v1()?;
        backend.retire().await
    }
}

#[async_trait::async_trait]
impl turn::TurnRunner for ResilientWarm {
    async fn acquire_host_custody(&self) -> Result<(), String> {
        ResilientWarm::acquire_host_custody(self).await
    }

    async fn run_turn(&self, session: &SessionId, parts: Vec<Part>) -> bool {
        self.run_turn_observed(
            session,
            parts,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn run_turn_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> bool {
        self.run_turn_recorded(
            session,
            parts,
            observer,
            Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder),
        )
        .await
    }

    async fn run_turn_recorded(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observer: Arc<dyn DiagnosticObserver>,
        activity: Arc<dyn bridge_core::attempt_activity::AttemptRecorder>,
    ) -> bool {
        self.run_turn_with_telemetry(
            session,
            parts,
            observer,
            turn::RecordedTurnTelemetry {
                activity,
                terminal_evidence: Arc::new(
                    bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                ),
                turn_meta: None,
            },
        )
        .await
    }

    async fn run_turn_with_telemetry(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observer: Arc<dyn DiagnosticObserver>,
        telemetry: turn::RecordedTurnTelemetry,
    ) -> bool {
        self.run_turn_inner(session, parts, observer, telemetry, None)
            .await
            .completed
    }

    async fn run_turn_accounted_v1(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observer: Arc<dyn DiagnosticObserver>,
        telemetry: turn::RecordedTurnTelemetry,
        accounting: turn::WarmTurnAccountingV1,
    ) -> turn::WarmTurnResultV1 {
        self.run_turn_inner(session, parts, observer, telemetry, Some(accounting))
            .await
    }
}

impl ResilientWarm {
    async fn run_turn_inner(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observer: Arc<dyn DiagnosticObserver>,
        telemetry: turn::RecordedTurnTelemetry,
        mut accounting: Option<turn::WarmTurnAccountingV1>,
    ) -> turn::WarmTurnResultV1 {
        struct CloseEvidence(Arc<dyn bridge_core::terminal_evidence::TerminalEvidenceSink>);
        impl Drop for CloseEvidence {
            fn drop(&mut self) {
                self.0.close();
            }
        }
        let _close_evidence = CloseEvidence(telemetry.terminal_evidence.clone());
        let mut result = turn::WarmTurnResultV1::default();
        let mut prepared = accounting
            .as_mut()
            .and_then(|context| context.prepared.take());
        if let Some(context) = &accounting {
            if let Err(error) = context.validate(&self.spec, prepared.as_ref()) {
                result.failures.push(
                    bridge_core::translator::AccountingFailureV1::from(error)
                        .settle(prepared.take())
                        .await,
                );
                return result;
            }
        }
        loop {
            if let Some(context) = &accounting {
                if prepared.is_none() {
                    match context.prepare().await {
                        Ok(attempt) => prepared = Some(attempt),
                        Err(error) => {
                            if let Some(prior) = result.failures.last_mut() {
                                prior.persistence = Some(error);
                            } else {
                                let mut failure =
                                    bridge_core::translator::AccountingFailureV1::from(
                                        error.clone(),
                                    );
                                failure.persistence = Some(error);
                                result.failures.push(failure);
                            }
                            return result;
                        }
                    }
                }
            }
            let backend = self.backend.lock().await.clone();
            if self.clone_config_guard {
                if let Err(error) = backend.set_host_custody(false).await {
                    eprintln!("implement: cannot resume clone writers; stopping (clone kept)");
                    result.failures.push(
                        bridge_core::translator::AccountingFailureV1::from(error)
                            .settle(prepared.take())
                            .await,
                    );
                    return result;
                }
            }
            if let Some(meta) = telemetry.turn_meta.clone() {
                backend.configure_turn(session, meta).await;
            }
            let observers = BackendObservers::diagnostic_only(observer.clone())
                .with_attempt_telemetry(
                    telemetry.activity.clone(),
                    telemetry.terminal_evidence.clone(),
                );
            let (outcome, persistence, cancelled) = if let Some(attempt) = prepared.take() {
                turn::drain_accounted_turn_v1(
                    backend.clone(),
                    session,
                    parts.clone(),
                    observers,
                    attempt,
                )
                .await
            } else {
                let outcome = match backend
                    .prompt_with_observers(session, parts.clone(), observers)
                    .await
                {
                    Ok(stream) => turn::drain_turn(stream).await,
                    Err(error) => turn::TurnOutcome {
                        completed: false,
                        last_err: Some(error),
                    },
                };
                (outcome, None, false)
            };
            result.cancelled = cancelled;
            if cancelled && outcome.last_err.is_none() {
                return result;
            }
            if outcome.completed {
                result.completed = true;
                return result;
            }
            let err = outcome.last_err.unwrap_or(BridgeError::MissingTerminal);
            let mut failure = bridge_core::translator::AccountingFailureV1::from(err.clone());
            failure.persistence = persistence;
            if let BridgeError::AgentFailure { diagnostic } = &err {
                if !diagnostic.prompt_may_have_been_accepted() {
                    failure.replay.prompt_acceptance =
                        bridge_core::routing::ExclusionEvidenceV1::Excluded;
                }
            }
            let persistence_failed = failure.persistence.is_some();
            result.failures.push(failure);
            if persistence_failed || classify_death(&err) != Death::Transient {
                return result;
            }
            let flight = match backend.resource_flight_v1() {
                Ok(flight) => flight,
                Err(_) => return result,
            };
            if flight == BackendResourceFlightV1::ProtectedV3 {
                // Protected attempts may stop only through the backend-owned flight.
                // The historical retire -> reset -> rebuild chain is intentionally
                // unreachable for this mode.
                let _ = backend.cancel(session).await;
                return result;
            }
            {
                let mut respawns_left = self.respawns_left.lock().await;
                if *respawns_left == 0 {
                    return result;
                }
                *respawns_left -= 1;
            }

            let _ = backend.cancel(session).await;
            if backend.retire().await.is_err() {
                eprintln!("implement: writer retirement unconfirmed; stopping before host git");
                return result;
            }
            // Generic retirement acknowledges teardown, but does not prove that
            // retained/protected resources were removed. Keep cleanup unknown.
            if let Err(e) = (self.reset_worktree)() {
                eprintln!("[implement] warm respawn reset failed: {e}");
                return result;
            }
            if let Some(context) = &accounting {
                match context.prepare_replacement().await {
                    Ok(attempt) => prepared = Some(attempt),
                    Err(error) => {
                        result
                            .failures
                            .last_mut()
                            .expect("failed turn recorded")
                            .persistence = Some(error);
                        return result;
                    }
                }
            }
            let rebuilt = match self.rebuild.rebuild().await {
                Ok(backend) => backend,
                Err(e) => {
                    eprintln!("[implement] warm respawn failed: {e:?}");
                    result.failures.push(
                        bridge_core::translator::AccountingFailureV1::from(e)
                            .settle(prepared.take())
                            .await,
                    );
                    return result;
                }
            };
            if let Err(e) = rebuilt.configure_session(session, &self.spec).await {
                eprintln!("[implement] warm respawn configure_session failed: {e:?}");
                let failure = bridge_core::translator::AccountingFailureV1::from(e);
                if rebuilt.resource_flight_v1().is_ok() {
                    let _ = rebuilt.retire().await;
                }
                result.failures.push(failure.settle(prepared.take()).await);
                return result;
            }
            *self.backend.lock().await = rebuilt;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::TurnRunner;
    use bridge_core::domain::EffectiveConfig;
    use bridge_core::ports::{BackendResourceFlightV1, BackendStream, Update};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering as BoolOrdering};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    fn done_stream() -> BackendStream {
        Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
            stop_reason: "end_turn".into(),
            prefix_attestation: Default::default(),
        })]))
    }

    fn err_stream(e: BridgeError) -> BackendStream {
        Box::pin(tokio_stream::iter(vec![Err(e)]))
    }

    fn clean_end_stream() -> BackendStream {
        Box::pin(tokio_stream::iter(Vec::<Result<Update, BridgeError>>::new()))
    }

    fn session_spec() -> SessionSpec {
        SessionSpec::from_config(EffectiveConfig::default())
    }

    fn noop_reset() -> Arc<ResetWorktree> {
        Arc::new(|| Ok(()))
    }

    fn table_key(e: &BridgeError) -> &'static str {
        match e {
            BridgeError::A2aVersionMismatch => "A2aVersionMismatch",
            BridgeError::InvalidRequest { .. } => "InvalidRequest",
            BridgeError::IdentityUnavailable => "IdentityUnavailable",
            BridgeError::DurableEvidenceUnavailable { .. } => "DurableEvidenceUnavailable",
            BridgeError::TaskNotFound => "TaskNotFound",
            BridgeError::SessionNotFound => "SessionNotFound",
            BridgeError::AuthRequired { .. } => "AuthRequired",
            BridgeError::PermissionRequired { .. } => "PermissionRequired",
            BridgeError::PermissionDenied => "PermissionDenied",
            BridgeError::AgentNotAuthenticated => "AgentNotAuthenticated",
            BridgeError::ModelNotAvailable => "ModelNotAvailable",
            BridgeError::CancelTimeout => "CancelTimeout",
            BridgeError::AgentTimedOut => "AgentTimedOut",
            BridgeError::FrameError => "FrameError",
            BridgeError::MissingTerminal => "MissingTerminal",
            BridgeError::MessageTooLarge => "MessageTooLarge",
            BridgeError::AgentCrashed { .. } => "AgentCrashed",
            BridgeError::AgentFailure { .. } => "AgentFailure",
            BridgeError::AgentOverloaded => "AgentOverloaded",
            BridgeError::UpstreamA2aError => "UpstreamA2aError",
            BridgeError::StoreFailure => "StoreFailure",
            BridgeError::InvalidStateTransition => "InvalidStateTransition",
            BridgeError::UnknownAgent { .. } => "UnknownAgent",
            BridgeError::ConfigInvalid { .. } => "ConfigInvalid",
            BridgeError::ConfigMismatch { .. } => "ConfigMismatch",
            BridgeError::ConfigReseedRequired { .. } => "ConfigReseedRequired",
            BridgeError::BoundSessionUnsupported => "BoundSessionUnsupported",
            BridgeError::BindUnsupported => "BindUnsupported",
            BridgeError::ResourceFlightUnsupported => "ResourceFlightUnsupported",
            BridgeError::SessionExpired => "SessionExpired",
            BridgeError::HandleBusy => "HandleBusy",
            BridgeError::TaskSpecInvalid { .. } => "TaskSpecInvalid",
            BridgeError::EmptyFinal => "EmptyFinal",
            BridgeError::HarvestAuditPersistFailed { .. } => "HarvestAuditPersistFailed",
        }
    }

    #[test]
    fn classify_death_table_is_exhaustive() {
        use bridge_core::diagnostics::{
            DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
            FailureDiagnosticInput, FailureDisposition,
        };

        let structured = |disposition| {
            BridgeError::agent_failure(
                FailureDiagnostic::build(
                    FailureDiagnosticInput {
                        failed_phase: DiagnosticPhase::Initialize,
                        last_completed_phase: Some(DiagnosticPhase::Spawn),
                        class: if disposition == FailureDisposition::ContainerFallbackCandidate {
                            DiagnosticFailureClass::ContainerRuntime
                        } else {
                            DiagnosticFailureClass::Transport
                        },
                        disposition,
                        code: "acp.initialize.transport".into(),
                        summary: "failed".into(),
                        causes: vec![],
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
                )
                .unwrap(),
            )
        };
        let cases = vec![
            (BridgeError::A2aVersionMismatch, Death::Fatal),
            (BridgeError::InvalidRequest { field: "x" }, Death::Fatal),
            (BridgeError::TaskNotFound, Death::Fatal),
            (BridgeError::SessionNotFound, Death::Transient),
            (
                BridgeError::AuthRequired {
                    request_id: "r".into(),
                },
                Death::Fatal,
            ),
            (
                BridgeError::PermissionRequired {
                    request_id: "r".into(),
                },
                Death::Fatal,
            ),
            (BridgeError::PermissionDenied, Death::Fatal),
            (BridgeError::AgentNotAuthenticated, Death::Fatal),
            (BridgeError::ModelNotAvailable, Death::Fatal),
            (BridgeError::CancelTimeout, Death::Transient),
            (BridgeError::AgentTimedOut, Death::Fatal),
            (BridgeError::FrameError, Death::Transient),
            (BridgeError::MissingTerminal, Death::Fatal),
            (BridgeError::MessageTooLarge, Death::Fatal),
            (BridgeError::agent_crashed("x"), Death::Transient),
            (structured(FailureDisposition::Fatal), Death::Fatal),
            (
                structured(FailureDisposition::RetrySameTarget),
                Death::Transient,
            ),
            (
                structured(FailureDisposition::ContainerFallbackCandidate),
                Death::Fatal,
            ),
            (BridgeError::AgentOverloaded, Death::Transient),
            (BridgeError::UpstreamA2aError, Death::Fatal),
            (BridgeError::StoreFailure, Death::Fatal),
            (BridgeError::InvalidStateTransition, Death::Fatal),
            (BridgeError::UnknownAgent { id: "x".into() }, Death::Fatal),
            (
                BridgeError::ConfigInvalid { reason: "x".into() },
                Death::Fatal,
            ),
            (BridgeError::ConfigMismatch { field: "x" }, Death::Fatal),
            (
                BridgeError::ConfigReseedRequired { field: "x" },
                Death::Fatal,
            ),
            (BridgeError::SessionExpired, Death::Fatal),
            (BridgeError::HandleBusy, Death::Fatal),
            (
                BridgeError::TaskSpecInvalid {
                    message: "x".into(),
                },
                Death::Fatal,
            ),
            // Fatal at every retry layer: a terminal empty final proves prompt
            // acceptance, so no component may replay that prompt.
            (BridgeError::EmptyFinal, Death::Fatal),
        ];
        for (err, want) in cases {
            let _ = table_key(&err);
            assert_eq!(classify_death(&err), want, "{err:?}");
        }
    }

    #[test]
    fn agent_timed_out_is_fatal() {
        assert!(matches!(
            classify_death(&BridgeError::AgentTimedOut),
            Death::Fatal
        ));
    }

    struct FakeBackend {
        prompts: AtomicUsize,
        cancels: AtomicUsize,
        configured: AtomicUsize,
        retired: AtomicUsize,
        flight: BackendResourceFlightV1,
        first: BridgeError,
        fail_first: bool,
        complete: bool,
        clean_end: bool,
        fail_config: bool,
        retire_error: bool,
        retain_on_retire: bool,
        resources_retained: AtomicBool,
        scripts: StdMutex<std::collections::VecDeque<Vec<Result<Update, BridgeError>>>>,
        scratch_write: Option<PathBuf>,
        scratch_absent: Option<(PathBuf, Arc<AtomicBool>)>,
        diagnostics: StdMutex<Vec<Arc<dyn DiagnosticObserver>>>,
        activities: StdMutex<Vec<Arc<dyn bridge_core::attempt_activity::AttemptRecorder>>>,
        terminal_evidence:
            StdMutex<Vec<Arc<dyn bridge_core::terminal_evidence::TerminalEvidenceSink>>>,
        turn_metas: StdMutex<Vec<bridge_core::permission::TurnMeta>>,
    }

    impl FakeBackend {
        fn new(first: BridgeError, fail_first: bool, complete: bool) -> Self {
            Self {
                prompts: AtomicUsize::new(0),
                cancels: AtomicUsize::new(0),
                configured: AtomicUsize::new(0),
                retired: AtomicUsize::new(0),
                flight: BackendResourceFlightV1::LegacyV2,
                first,
                fail_first,
                complete,
                clean_end: false,
                fail_config: false,
                retire_error: false,
                retain_on_retire: true,
                resources_retained: AtomicBool::new(true),
                scripts: StdMutex::new(Default::default()),
                scratch_write: None,
                scratch_absent: None,
                diagnostics: StdMutex::new(Vec::new()),
                activities: StdMutex::new(Vec::new()),
                terminal_evidence: StdMutex::new(Vec::new()),
                turn_metas: StdMutex::new(Vec::new()),
            }
        }

        fn refusal() -> Self {
            Self {
                clean_end: true,
                ..Self::new(BridgeError::agent_crashed("unused"), false, false)
            }
        }

        fn protected_v3(mut self) -> Self {
            self.flight = BackendResourceFlightV1::ProtectedV3;
            self
        }

        fn write_scratch(mut self, path: PathBuf) -> Self {
            self.scratch_write = Some(path);
            self
        }

        fn expect_scratch_absent(mut self, path: PathBuf, seen_clean: Arc<AtomicBool>) -> Self {
            self.scratch_absent = Some((path, seen_clean));
            self
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn set_host_custody(&self, _held: bool) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let n = self.prompts.fetch_add(1, Ordering::SeqCst);
            if let Some(script) = self.scripts.lock().unwrap().pop_front() {
                return Ok(Box::pin(tokio_stream::iter(script)));
            }
            if let Some(path) = &self.scratch_write {
                std::fs::write(path, "scratch\n").expect("write fake scratch");
            }
            if let Some((path, seen_clean)) = &self.scratch_absent {
                seen_clean.store(!path.exists(), BoolOrdering::SeqCst);
            }
            if n == 0 && self.fail_first {
                Ok(err_stream(self.first.clone()))
            } else if self.complete {
                Ok(done_stream())
            } else if self.clean_end {
                Ok(clean_end_stream())
            } else {
                Ok(err_stream(self.first.clone()))
            }
        }

        async fn prompt_with_observers(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            assert!(observers.rich.is_none());
            self.diagnostics.lock().unwrap().push(observers.diagnostic);
            self.activities.lock().unwrap().push(observers.activity);
            self.terminal_evidence
                .lock()
                .unwrap()
                .push(observers.terminal_evidence);
            self.prompt(session, parts).await
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
            Ok(self.flight)
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.configured.fetch_add(1, Ordering::SeqCst);
            if self.fail_config {
                return Err(BridgeError::ModelNotAvailable);
            }
            Ok(())
        }

        async fn configure_turn(
            &self,
            _session: &SessionId,
            meta: bridge_core::permission::TurnMeta,
        ) {
            self.turn_metas.lock().unwrap().push(meta);
        }

        async fn forget_session(&self, _session: &SessionId) {}

        async fn retire(&self) -> Result<(), BridgeError> {
            self.retired.fetch_add(1, Ordering::SeqCst);
            if self.retire_error {
                return Err(BridgeError::CancelTimeout);
            }
            if !self.retain_on_retire {
                self.resources_retained.store(false, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    struct CountingRebuild {
        count: Arc<AtomicUsize>,
        next: Arc<FakeBackend>,
    }

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl WarmRebuild for CountingRebuild {
        async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.next.clone() as Arc<dyn AgentBackend>)
        }
    }

    struct NoCustodyBackend;
    #[async_trait::async_trait]
    impl AgentBackend for NoCustodyBackend {
        async fn set_host_custody(&self, _: bool) -> Result<(), BridgeError> {
            panic!("opt-out must skip both pause and unpause");
        }
        async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream, BridgeError> {
            Ok(done_stream())
        }
        async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn clone_config_opt_out_skips_all_pause_custody() {
        let runner = ResilientWarm::new(
            Arc::new(NoCustodyBackend),
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next: Arc::new(FakeBackend::new(
                    BridgeError::agent_crashed("unused"),
                    false,
                    true,
                )),
            }),
            session_spec(),
            0,
            Arc::new(|| Ok(())),
        )
        .with_clone_config_guard(false);
        runner.acquire_host_custody().await.unwrap();
        assert!(
            runner
                .run_turn(&SessionId::parse("opt-out").unwrap(), vec![])
                .await
        );
    }

    #[tokio::test]
    async fn transient_death_respawns_once_and_completes() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first.clone() as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second.clone(),
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 1);
        assert_eq!(first.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(first.retired.load(Ordering::SeqCst), 1);
        assert_eq!(second.configured.load(Ordering::SeqCst), 1);
        assert_eq!(second.prompts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn protected_transient_never_retires_resets_or_rebuilds() {
        let first = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("gone"), true, false).protected_v3(),
        );
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let resets = Arc::new(AtomicUsize::new(0));
        let reset_count = resets.clone();
        let runner = ResilientWarm::new(
            first.clone(),
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            Arc::new(move || {
                reset_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        );
        let session = SessionId::parse("protected-implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(first.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(first.retired.load(Ordering::SeqCst), 0);
        assert_eq!(resets.load(Ordering::SeqCst), 0);
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn protected_public_retire_delegates_to_the_backend_owned_flight() {
        let backend = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("unused"), false, true).protected_v3(),
        );
        let runner = ResilientWarm::new(
            backend.clone(),
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next: backend.clone(),
            }),
            session_spec(),
            0,
            noop_reset(),
        );

        runner.retire().await.unwrap();
        assert_eq!(backend.retired.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn observed_turn_reuses_one_observer_for_initial_and_rebuilt_backend() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let runner = ResilientWarm::new(
            first.clone(),
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next: second.clone(),
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-observed-test").unwrap();
        let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);

        assert!(
            runner
                .run_turn_observed(
                    &session,
                    vec![Part { text: "fix".into() }],
                    observer.clone(),
                )
                .await
        );

        let first_seen = first.diagnostics.lock().unwrap();
        let second_seen = second.diagnostics.lock().unwrap();
        assert_eq!(first_seen.len(), 1);
        assert_eq!(second_seen.len(), 1);
        assert!(Arc::ptr_eq(&first_seen[0], &observer));
        assert!(Arc::ptr_eq(&second_seen[0], &observer));
    }

    #[tokio::test]
    async fn r2f0b_recorded_turn_reuses_one_activity_recorder_after_respawn() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let runner = ResilientWarm::new(
            first.clone(),
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next: second.clone(),
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-recorded-test").unwrap();
        let activity: Arc<dyn bridge_core::attempt_activity::AttemptRecorder> =
            Arc::new(bridge_core::attempt_activity::SharedAttemptRecorder::new(
                bridge_core::attempt_activity::SystemMonotonicClock::start(),
            ));

        assert!(
            runner
                .run_turn_recorded(
                    &session,
                    vec![Part { text: "fix".into() }],
                    Arc::new(MarkerDiagnostic),
                    activity.clone(),
                )
                .await
        );

        let first_seen = first.activities.lock().unwrap();
        let second_seen = second.activities.lock().unwrap();
        assert_eq!(first_seen.len(), 1);
        assert_eq!(second_seen.len(), 1);
        assert!(Arc::ptr_eq(&first_seen[0], &activity));
        assert!(Arc::ptr_eq(&second_seen[0], &activity));
    }

    #[tokio::test]
    async fn r2f0b_warm_respawn_reuses_exact_turn_evidence_and_metadata_then_seals_it() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let runner = ResilientWarm::new(
            first.clone(),
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next: second.clone(),
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-telemetry-test").unwrap();
        let activity: Arc<dyn bridge_core::attempt_activity::AttemptRecorder> =
            Arc::new(bridge_core::attempt_activity::SharedAttemptRecorder::new(
                bridge_core::attempt_activity::SystemMonotonicClock::start(),
            ));
        let binding = bridge_core::terminal_evidence::TurnEvidenceBinding {
            generation: 4,
            session_id: session.as_str().into(),
            turn_id: "implement-turn-4".into(),
            attempt_id: "implement-attempt".into(),
            marker_nonce: "00112233445566778899aabbccddeeff".into(),
        };
        let concrete_evidence = Arc::new(
            bridge_core::terminal_evidence::SharedTurnEvidence::dormant(binding),
        );
        bridge_core::terminal_evidence::TerminalEvidenceSink::declare_capability(
            concrete_evidence.as_ref(),
            bridge_core::terminal_evidence::EvidenceCapability::V1,
        );
        let evidence: Arc<dyn bridge_core::terminal_evidence::TerminalEvidenceSink> =
            concrete_evidence.clone();
        let meta = bridge_core::permission::TurnMeta {
            context_id: bridge_core::ids::ContextId::parse(session.as_str()).unwrap(),
            generation: 4,
            op: bridge_core::ids::OperationId::parse("implement-provider-4").unwrap(),
            turn_id: bridge_core::ids::TurnId::parse("implement-turn-4").unwrap(),
            requested_mode: bridge_core::attestation::HarvestSanitizationMode::Off,
            prefix_attestation_request:
                bridge_core::attestation::PrefixAttestationRequest::Disabled,
            liveness: bridge_core::turn_liveness::TurnLivenessRequestV1::default(),
        };

        assert!(
            runner
                .run_turn_with_telemetry(
                    &session,
                    vec![Part { text: "fix".into() }],
                    Arc::new(MarkerDiagnostic),
                    crate::turn::RecordedTurnTelemetry {
                        activity,
                        terminal_evidence: evidence.clone(),
                        turn_meta: Some(meta),
                    },
                )
                .await
        );

        let first_evidence = first.terminal_evidence.lock().unwrap();
        let second_evidence = second.terminal_evidence.lock().unwrap();
        assert_eq!(first_evidence.len(), 1);
        assert_eq!(second_evidence.len(), 1);
        assert!(Arc::ptr_eq(&first_evidence[0], &evidence));
        assert!(Arc::ptr_eq(&second_evidence[0], &evidence));
        drop(first_evidence);
        drop(second_evidence);
        for backend in [&first, &second] {
            let metas = backend.turn_metas.lock().unwrap();
            assert_eq!(metas.len(), 1);
            assert_eq!(metas[0].generation, 4);
            assert_eq!(metas[0].turn_id.as_str(), "implement-turn-4");
            assert_eq!(metas[0].op.as_str(), "implement-provider-4");
        }
        assert_eq!(
            bridge_core::terminal_evidence::TerminalEvidenceSink::observation(
                concrete_evidence.as_ref(),
            )
            .0,
            bridge_core::terminal_evidence::EvidenceCompleteness::Missing,
            "the attempt owner must seal negotiated evidence after the warm turn"
        );
    }

    #[tokio::test]
    async fn fatal_death_does_not_respawn() {
        let first = Arc::new(FakeBackend::new(BridgeError::PermissionDenied, true, false));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transient_death_with_exhausted_budget_does_not_respawn() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            0,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refusal_without_error_does_not_respawn() {
        let first = Arc::new(FakeBackend::refusal());
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "t"]);
        git_ok(&repo, &["config", "user.email", "t@t"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git_ok(&repo, &["add", "README.md"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);
        (td, repo)
    }

    #[tokio::test]
    async fn transient_retry_resets_worktree_before_reprompt() {
        let (_td, repo) = temp_repo();
        let scratch = repo.join("scratch.txt");
        let saw_clean = Arc::new(AtomicBool::new(false));
        let first = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("gone"), true, false)
                .write_scratch(scratch.clone()),
        );
        let second = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("unused"), false, true)
                .expect_scratch_absent(scratch.clone(), saw_clean.clone()),
        );
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let reset_repo = repo.clone();
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            Arc::new(move || crate::implement::reset_worktree_to_head(&reset_repo)),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 1);
        assert!(saw_clean.load(BoolOrdering::SeqCst));
        assert!(!scratch.exists());
    }
    use bridge_core::{accounting::*, harvest::*, ids::*, orch::UsageSnapshot, task_store::*};

    #[derive(Default)]
    struct UsageStore {
        rows: StdMutex<Vec<UsageAttemptV1>>,
        writes: AtomicUsize,
        fail_start: AtomicBool,
        fail_update: AtomicBool,
        starts: AtomicUsize,
        update_calls: AtomicUsize,
        fail_start_at: AtomicUsize,
        fail_update_at: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl HarvestAuditStore for UsageStore {
        async fn commit_bundle(
            &self,
            _: HarvestRawRecordV1,
            _: HarvestSanitizationDecisionV1,
        ) -> Result<HarvestAuditCommit, HarvestAuditStoreError> {
            unreachable!()
        }
        async fn get_by_audit_id(
            &self,
            _: &str,
        ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
            unreachable!()
        }
        async fn get_by_attempt_key(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: &str,
        ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
            unreachable!()
        }
        async fn list_by_task_id(
            &self,
            _: &str,
            _: Option<&str>,
            _: u32,
        ) -> Result<Vec<HarvestAuditBundleV1>, HarvestAuditStoreError> {
            unreachable!()
        }
    }
    #[async_trait::async_trait]
    impl TaskStore for UsageStore {
        async fn start_usage_attempt_v1(&self, row: &UsageAttemptV1) -> Result<(), BridgeError> {
            let call = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_start.load(Ordering::SeqCst)
                || self.fail_start_at.load(Ordering::SeqCst) == call
            {
                return Err(BridgeError::StoreFailure);
            }
            self.rows.lock().unwrap().push(row.clone());
            Ok(())
        }
        async fn update_usage_attempt_v1(
            &self,
            id: &str,
            dispatched: bool,
            usage: Option<UsageSnapshot>,
            finalized: bool,
        ) -> Result<(), BridgeError> {
            let call = self.update_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_update.load(Ordering::SeqCst)
                || self.fail_update_at.load(Ordering::SeqCst) == call
            {
                return Err(BridgeError::StoreFailure);
            }
            self.writes.fetch_add(1, Ordering::SeqCst);
            let mut rows = self.rows.lock().unwrap();
            let row = rows.iter_mut().find(|row| row.id == id).unwrap();
            row.dispatched |= dispatched;
            row.finalized |= finalized;
            if let Some(usage) = usage {
                row.observe(usage);
            }
            Ok(())
        }
        async fn create(&self, _: &TaskRecord) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn set_terminal(
            &self,
            _: &TaskId,
            _: TaskRecordStatus,
            _: Option<&str>,
            _: Option<&str>,
            _: i64,
        ) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn get(&self, _: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
            unreachable!()
        }
        async fn list(&self, _: usize) -> Result<Vec<TaskRecord>, BridgeError> {
            unreachable!()
        }
        async fn sweep_interrupted(&self, _: i64) -> Result<u64, BridgeError> {
            unreachable!()
        }
        async fn cancel_if_working(&self, _: &TaskId, _: i64) -> Result<bool, BridgeError> {
            unreachable!()
        }
        async fn put_node_checkpoint(
            &self,
            _: &TaskId,
            _: &NodeId,
            _: &str,
            _: bool,
            _: i64,
        ) -> Result<(), BridgeError> {
            unreachable!()
        }
        async fn node_checkpoints(
            &self,
            _: &TaskId,
        ) -> Result<Vec<(NodeId, String, bool, Option<UsageSnapshot>)>, BridgeError> {
            unreachable!()
        }
        async fn claim_resume_attempt(
            &self,
            _: &TaskId,
            _: u32,
            _: i64,
        ) -> Result<ResumeClaim, BridgeError> {
            unreachable!()
        }
        async fn record_node_started(
            &self,
            _: &TaskId,
            _: &NodeId,
            _: &OperationId,
            _: i64,
        ) -> Result<i64, BridgeError> {
            unreachable!()
        }
        async fn put_node_checkpoint_sequenced(
            &self,
            _: &TaskId,
            _: &NodeId,
            _: &OperationId,
            _: &str,
            _: bool,
            _: i64,
            _: Option<&UsageSnapshot>,
        ) -> Result<i64, BridgeError> {
            unreachable!()
        }
        async fn set_terminal_sequenced(
            &self,
            _: &TaskId,
            _: &OperationId,
            _: TaskRecordStatus,
            _: Option<&str>,
            _: Option<&str>,
            _: i64,
        ) -> Result<i64, BridgeError> {
            unreachable!()
        }
        async fn journal_from(
            &self,
            _: &TaskId,
            _: i64,
        ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
            unreachable!()
        }
        async fn progress_snapshot(&self, _: &TaskId) -> Result<TaskProgressSnapshot, BridgeError> {
            unreachable!()
        }
        async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
            unreachable!()
        }
    }

    fn warm_accounting(store: Arc<UsageStore>, role: &str) -> turn::WarmTurnAccountingV1 {
        turn::WarmTurnAccountingV1 {
            accounting: AccountingV1::new(store),
            owner: AttemptIdentity::initial().unwrap(),
            attribution: UsageAttributionV1 {
                agent: "fixture".into(),
                attempt_kind: role.into(),
                effective_model: Some("fixture-model".into()),
                effective_effort: Some("high".into()),
                effective_mode: Some("write".into()),
                ..Default::default()
            },
            prepared: None,
        }
    }
    async fn accounted(
        runner: &dyn TurnRunner,
        accounting: turn::WarmTurnAccountingV1,
    ) -> turn::WarmTurnResultV1 {
        runner
            .run_turn_accounted_v1(
                &SessionId::parse("accounted").unwrap(),
                vec![],
                Arc::new(MarkerDiagnostic),
                turn::RecordedTurnTelemetry {
                    activity: Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder),
                    terminal_evidence: Arc::new(
                        bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                    ),
                    turn_meta: None,
                },
                accounting,
            )
            .await
    }
    fn accounting_spec() -> SessionSpec {
        SessionSpec::from_config(EffectiveConfig {
            model: Some("fixture-model".into()),
            effort: Some(bridge_core::domain::Effort::High),
            mode: Some("write".into()),
        })
    }
    fn accounting_runner(
        first: Arc<FakeBackend>,
        next: Arc<FakeBackend>,
        budget: u32,
    ) -> ResilientWarm {
        ResilientWarm::new(
            first,
            Arc::new(CountingRebuild {
                count: Arc::new(AtomicUsize::new(0)),
                next,
            }),
            accounting_spec(),
            budget,
            noop_reset(),
        )
    }
    #[tokio::test]
    async fn accounted_preparation_then_fix_uses_one_capture_per_turn() {
        let store = Arc::new(UsageStore::default());
        let backend = Arc::new(FakeBackend::new(BridgeError::PermissionDenied, false, true));
        let runner = accounting_runner(backend.clone(), backend.clone(), 0);
        let mut initial = warm_accounting(store.clone(), "edit");
        initial.prepared = Some(initial.prepare().await.unwrap());
        assert!(accounted(&runner, initial).await.completed);
        for _ in 0..2 {
            assert!(
                accounted(&runner, warm_accounting(store.clone(), "fix"))
                    .await
                    .completed
            );
        }
        let rows = store.rows.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.dispatched && row.finalized));
        assert_eq!(store.writes.load(Ordering::SeqCst), 6);
        assert_eq!(backend.prompts.load(Ordering::SeqCst), 3);
        assert_eq!(rows[0].attribution.attempt_kind, "edit");
        assert_eq!(rows[1].attribution.attempt_kind, "fix");
        assert_ne!(rows[0].id, rows[1].id);
        assert_ne!(
            rows[0].attribution.run_attempt,
            rows[1].attribution.run_attempt
        );
        assert_eq!(
            rows[1].attribution.effective_model.as_deref(),
            Some("fixture-model")
        );
    }
    struct AccountedRebuild {
        store: Arc<UsageStore>,
        next: Arc<FakeBackend>,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl WarmRebuild for AccountedRebuild {
        async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            let rows = self.store.rows.lock().unwrap();
            assert_eq!(rows.len(), 2, "replacement admitted before construction");
            assert_ne!(rows[0].id, rows[1].id);
            assert!(!rows[1].dispatched && !rows[1].finalized);
            if self.fail {
                Err(BridgeError::ModelNotAvailable)
            } else {
                Ok(self.next.clone())
            }
        }
    }
    #[tokio::test]
    async fn accounted_rebuild_admits_then_settles_only_undispatched_failures() {
        for failure in 0..3 {
            let store = Arc::new(UsageStore::default());
            let first = Arc::new(FakeBackend::new(BridgeError::AgentOverloaded, true, false));
            let mut next = FakeBackend::new(BridgeError::PermissionDenied, false, true);
            next.fail_config = failure == 2;
            let next = Arc::new(next);
            let runner = ResilientWarm::new(
                first,
                Arc::new(AccountedRebuild {
                    store: store.clone(),
                    next: next.clone(),
                    fail: failure == 1,
                }),
                accounting_spec(),
                1,
                noop_reset(),
            );
            let outcome = accounted(&runner, warm_accounting(store.clone(), "edit")).await;
            assert_eq!(outcome.completed, failure == 0);
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), 2);
            assert!(rows[0].dispatched && !rows[0].finalized);
            assert!(rows[1].finalized);
            assert_eq!(
                rows[0].attribution.run_attempt,
                rows[1].attribution.run_attempt
            );
            assert_eq!(rows[1].dispatched, failure == 0);
            assert_eq!(
                next.prompts.load(Ordering::SeqCst),
                usize::from(failure == 0)
            );
            assert_eq!(outcome.failures[0].original, BridgeError::AgentOverloaded);
            assert_eq!(
                outcome.failures.last().unwrap().cleanup,
                bridge_core::ports::BackendCleanupDispositionV1::Unknown
            );
            assert!(!outcome.failures.last().unwrap().replay.cleanup_completed);
            assert_eq!(
                outcome.failures[0].cleanup,
                bridge_core::ports::BackendCleanupDispositionV1::Unknown
            );
            assert_eq!(
                outcome.failures[0].replay.prompt_acceptance,
                bridge_core::routing::ExclusionEvidenceV1::Unknown
            );
        }
    }
    #[tokio::test]
    async fn accounted_write_failure_and_protected_flight_never_reprompt() {
        for failure in 0..3 {
            let store = Arc::new(UsageStore::default());
            store.fail_start.store(failure == 0, Ordering::SeqCst);
            store.fail_update.store(failure == 1, Ordering::SeqCst);
            let backend = Arc::new(
                FakeBackend::new(BridgeError::AgentOverloaded, true, false).protected_v3(),
            );
            let runner = accounting_runner(backend.clone(), backend.clone(), 1);
            let outcome = accounted(&runner, warm_accounting(store.clone(), "fix")).await;
            assert!(!outcome.completed);
            assert_eq!(
                backend.prompts.load(Ordering::SeqCst),
                usize::from(failure == 2)
            );
            assert_eq!(backend.retired.load(Ordering::SeqCst), 0);
            assert_eq!(outcome.failures[0].persistence.is_some(), failure != 2);
            assert_eq!(
                outcome.failures[0].cleanup,
                bridge_core::ports::BackendCleanupDispositionV1::Unknown
            );
            assert_eq!(
                outcome.failures[0].replay,
                bridge_core::routing::ReplayEvidenceV1::default()
            );
        }
    }
    fn scripted(items: Vec<Result<Update, BridgeError>>) -> Arc<FakeBackend> {
        let backend = Arc::new(FakeBackend::new(BridgeError::PermissionDenied, false, true));
        backend.scripts.lock().unwrap().push_back(items);
        backend
    }
    #[tokio::test]
    async fn accounted_rejects_foreign_preparation_before_prompt() {
        for mismatch in 0..9 {
            let store = Arc::new(UsageStore::default());
            let backend = scripted(vec![Ok(Update::done("end_turn"))]);
            let runner = accounting_runner(backend.clone(), backend.clone(), 1);
            let mut context = warm_accounting(store.clone(), "edit");
            let attribution = context.attribution.clone();
            match mismatch {
                4 => context.attribution.effective_model = Some("other".into()),
                5 => context.attribution.effective_effort = Some("low".into()),
                6 => context.attribution.effective_mode = None,
                _ => {}
            }
            context.prepared = Some(context.prepare().await.unwrap());
            context.attribution = attribution;
            match mismatch {
                0 => context.owner.execution_id = ExecutionId::mint().unwrap(),
                1 => context.owner.attempt_id = AttemptId::mint().unwrap(),
                2 => context.attribution.agent = "other".into(),
                3 => context.attribution.attempt_kind = "fix".into(),
                4..=6 => {}
                7 => context.attribution.workflow = Some("other".into()),
                _ => context.attribution.node = Some("other".into()),
            }
            let outcome = accounted(&runner, context).await;
            assert!(!outcome.completed, "mismatch {mismatch}");
            assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
            assert_eq!(
                outcome.failures[0].original,
                BridgeError::ConfigMismatch {
                    field: "warm_preparation"
                }
            );
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].finalized && !rows[0].dispatched);
        }
    }
    #[tokio::test]
    async fn accounted_binds_attribution_to_runner_configuration() {
        for mismatch in 0..3 {
            for prepared in [false, true] {
                let store = Arc::new(UsageStore::default());
                let backend = scripted(vec![]);
                let mut spec = accounting_spec();
                match mismatch {
                    0 => spec.config.model = Some("other".into()),
                    1 => spec.config.effort = None,
                    _ => spec.config.mode = None,
                }
                let runner = ResilientWarm::new(
                    backend.clone(),
                    Arc::new(CountingRebuild {
                        count: Arc::new(AtomicUsize::new(0)),
                        next: backend.clone(),
                    }),
                    spec,
                    0,
                    noop_reset(),
                );
                let mut context = warm_accounting(store.clone(), "edit");
                if prepared {
                    context.prepared = Some(context.prepare().await.unwrap());
                }
                let outcome = accounted(&runner, context).await;
                assert!(!outcome.completed);
                assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
                assert!(matches!(
                    outcome.failures[0].original,
                    BridgeError::ConfigMismatch { .. }
                ));
                assert!(store
                    .rows
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|row| row.finalized && !row.dispatched));
            }
        }
    }
    #[tokio::test]
    async fn accounted_cancelled_terminal_is_distinct_from_missing_terminal() {
        for fail_at in [0, 3] {
            let store = Arc::new(UsageStore::default());
            store.fail_update_at.store(fail_at, Ordering::SeqCst);
            let backend = scripted(vec![
                Ok(usage_update()),
                Ok(Update::done(bridge_core::ports::STOP_REASON_CANCELLED)),
            ]);
            let runner = accounting_runner(backend.clone(), backend.clone(), 1);
            let outcome = accounted(&runner, warm_accounting(store.clone(), "edit")).await;
            assert!(!outcome.completed);
            assert!(outcome.cancelled);
            if fail_at == 0 {
                assert!(
                    outcome.failures.is_empty(),
                    "cancellation is a terminal event"
                );
            } else {
                assert_eq!(
                    outcome.failures[0].persistence,
                    Some(BridgeError::StoreFailure)
                );
            }
            assert_eq!(store.rows.lock().unwrap()[0].finalized, fail_at == 0);
            backend
                .scripts
                .lock()
                .unwrap()
                .push_back(vec![Ok(usage_update())]);
            let outcome = accounted(&runner, warm_accounting(store.clone(), "fix")).await;
            assert!(!outcome.completed && !outcome.cancelled);
            assert_eq!(outcome.failures[0].original, BridgeError::MissingTerminal);
            assert!(!store.rows.lock().unwrap()[1].finalized);
            assert!(store.rows.lock().unwrap().iter().all(|row| row
                .usage
                .terminal
                .as_ref()
                .unwrap()
                .total_tokens
                == 13));
        }
    }
    #[tokio::test]
    async fn accounted_retirement_does_not_prove_complete_cleanup() {
        for retirement in 0..3 {
            let retire_error = retirement == 2;
            let store = Arc::new(UsageStore::default());
            let mut first = FakeBackend::new(BridgeError::AgentOverloaded, true, false);
            first.retire_error = retire_error;
            first.retain_on_retire = retirement != 1;
            let first = Arc::new(first);
            let next = scripted(vec![Ok(Update::done("end_turn"))]);
            let runner = accounting_runner(first.clone(), next, 1);
            let outcome = accounted(&runner, warm_accounting(store, "edit")).await;
            assert_eq!(outcome.completed, !retire_error);
            assert_eq!(first.retired.load(Ordering::SeqCst), 1);
            assert_eq!(
                first.resources_retained.load(Ordering::SeqCst),
                retirement != 1
            );
            // Even actual removal is unknown to the runner through retire's unit result.
            assert_eq!(
                outcome.failures[0].cleanup,
                bridge_core::ports::BackendCleanupDispositionV1::Unknown
            );
            assert!(!outcome.failures[0].replay.cleanup_completed);
        }
    }
    fn usage_update() -> Update {
        Update::Usage(UsageSnapshot {
            terminal: Some(bridge_core::orch::TerminalUsage {
                total_tokens: 13,
                input_tokens: 8,
                output_tokens: 5,
                thought_tokens: None,
                cached_read_tokens: None,
                cached_write_tokens: None,
            }),
            ..Default::default()
        })
    }
    #[tokio::test]
    async fn accounted_usage_and_finalization_writes_fail_closed_after_dispatch() {
        for fail_at in 0..=3 {
            let store = Arc::new(UsageStore::default());
            store.fail_update_at.store(fail_at, Ordering::SeqCst);
            let backend = scripted(vec![Ok(usage_update()), Ok(Update::done("end_turn"))]);
            let runner = accounting_runner(backend.clone(), backend.clone(), 2);
            let outcome = accounted(&runner, warm_accounting(store.clone(), "edit")).await;
            assert_eq!(outcome.completed, fail_at == 0);
            assert_eq!(
                backend.prompts.load(Ordering::SeqCst),
                usize::from(fail_at != 1)
            );
            assert_eq!(backend.retired.load(Ordering::SeqCst), 0);
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].dispatched, fail_at != 1);
            assert_eq!(rows[0].finalized, fail_at == 0);
            assert_eq!(
                rows[0].usage.terminal.is_some(),
                fail_at == 0 || fail_at == 3
            );
            if fail_at != 0 {
                assert_eq!(
                    outcome.failures[0].persistence,
                    Some(BridgeError::StoreFailure)
                );
                assert_eq!(outcome.failures[0].original, BridgeError::StoreFailure);
            }
        }
    }
    #[tokio::test]
    async fn accounted_replacement_admission_and_settlement_keep_original_failure() {
        for fail_admission in [false, true] {
            let store = Arc::new(UsageStore::default());
            if fail_admission {
                store.fail_start_at.store(2, Ordering::SeqCst);
            } else {
                store.fail_update_at.store(3, Ordering::SeqCst);
            }
            let first = scripted(vec![Ok(usage_update()), Err(BridgeError::AgentOverloaded)]);
            let next = scripted(vec![]);
            let runner = ResilientWarm::new(
                first,
                Arc::new(AccountedRebuild {
                    store: store.clone(),
                    next: next.clone(),
                    fail: true,
                }),
                accounting_spec(),
                1,
                noop_reset(),
            );
            let outcome = accounted(&runner, warm_accounting(store.clone(), "edit")).await;
            assert!(!outcome.completed);
            let failure = outcome.failures.last().unwrap();
            assert_eq!(failure.persistence, Some(BridgeError::StoreFailure));
            assert_eq!(
                failure.original,
                if fail_admission {
                    BridgeError::AgentOverloaded
                } else {
                    BridgeError::ModelNotAvailable
                }
            );
            assert_eq!(next.prompts.load(Ordering::SeqCst), 0);
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), if fail_admission { 1 } else { 2 });
            assert!(rows[0].dispatched && !rows[0].finalized);
            assert_eq!(rows[0].usage.terminal.as_ref().unwrap().total_tokens, 13);
            if !fail_admission {
                assert!(!rows[1].dispatched && !rows[1].finalized);
            }
        }
    }
    #[tokio::test]
    async fn accounted_repeated_rebuilds_share_owner_budget_and_one_capture() {
        let store = Arc::new(UsageStore::default());
        let backend = scripted(vec![Ok(usage_update()), Err(BridgeError::AgentOverloaded)]);
        for _ in 0..3 {
            backend
                .scripts
                .lock()
                .unwrap()
                .push_back(vec![Ok(usage_update()), Err(BridgeError::AgentOverloaded)]);
        }
        let runner = accounting_runner(backend.clone(), backend.clone(), 2);
        let initial = warm_accounting(store.clone(), "edit");
        let owner = initial.owner.clone();
        let outcome = accounted(&runner, initial).await;
        assert!(!outcome.completed);
        assert_eq!(outcome.failures.len(), 3);
        let mut fix = warm_accounting(store.clone(), "fix");
        fix.owner = owner.clone();
        assert!(!accounted(&runner, fix).await.completed);
        assert_eq!(backend.retired.load(Ordering::SeqCst), 2);
        assert_eq!(backend.prompts.load(Ordering::SeqCst), 4);
        assert_eq!(store.writes.load(Ordering::SeqCst), 8);
        let rows = store.rows.lock().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter()
                .map(|row| &row.id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
        assert!(rows.iter().all(|row| row.dispatched
            && !row.finalized
            && row.attribution.run_attempt.as_deref() == Some(owner.attempt_id.as_str())
            && row.usage.terminal.as_ref().unwrap().total_tokens == 13));
        assert_eq!(rows[3].attribution.attempt_kind, "fix");
    }
    #[tokio::test]
    async fn accounted_settled_usage_is_not_replayed_and_eof_keeps_usage_pending() {
        let store = Arc::new(UsageStore::default());
        let backend = scripted(vec![
            Ok(usage_update()),
            Ok(Update::done("end_turn")),
            Err(BridgeError::AgentOverloaded),
        ]);
        let runner = accounting_runner(backend.clone(), backend.clone(), 2);
        assert!(
            accounted(&runner, warm_accounting(store.clone(), "edit"))
                .await
                .completed
        );
        backend
            .scripts
            .lock()
            .unwrap()
            .push_back(vec![Ok(usage_update())]);
        let outcome = accounted(&runner, warm_accounting(store.clone(), "fix")).await;
        assert_eq!(outcome.failures[0].original, BridgeError::MissingTerminal);
        assert!(outcome.failures[0].persistence.is_none());
        let rows = store.rows.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].finalized && !rows[1].finalized);
        assert!(rows
            .iter()
            .all(|row| row.usage.terminal.as_ref().unwrap().total_tokens == 13));
        assert_eq!(backend.retired.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn accounted_typed_acceptance_and_provider_store_failure_stay_distinct() {
        use bridge_core::diagnostics::*;
        for may_accept in [false, true] {
            let diagnostic = FailureDiagnostic::build(
                FailureDiagnosticInput {
                    failed_phase: DiagnosticPhase::PromptStart,
                    last_completed_phase: None,
                    class: DiagnosticFailureClass::Transport,
                    disposition: FailureDisposition::Fatal,
                    code: "acp.prompt.transport".into(),
                    summary: "fixture".into(),
                    causes: vec![],
                    stderr_observed: false,
                    stderr_line_count: 0,
                    stderr_scope: None,
                    stderr_tail: None,
                    stderr_redaction: None,
                    retry_after_ms: None,
                    reset_at_ms: None,
                    prompt_may_have_been_accepted: may_accept,
                },
                &DiagnosticRedactor::default(),
            )
            .unwrap();
            let error = BridgeError::agent_failure(diagnostic);
            let store = Arc::new(UsageStore::default());
            let backend = scripted(vec![Err(error.clone())]);
            let runner = accounting_runner(backend.clone(), backend.clone(), 0);
            let outcome = accounted(&runner, warm_accounting(store.clone(), "edit")).await;
            let failure = &outcome.failures[0];
            assert_eq!(failure.original, error);
            assert!(failure.persistence.is_none());
            assert_eq!(
                failure.replay.prompt_acceptance,
                if may_accept {
                    bridge_core::routing::ExclusionEvidenceV1::Unknown
                } else {
                    bridge_core::routing::ExclusionEvidenceV1::Excluded
                }
            );
            assert_eq!(
                failure.replay.effects,
                bridge_core::routing::ExclusionEvidenceV1::Unknown
            );
            backend
                .scripts
                .lock()
                .unwrap()
                .push_back(vec![Err(BridgeError::StoreFailure)]);
            let outcome = accounted(&runner, warm_accounting(store, "fix")).await;
            assert_eq!(outcome.failures[0].original, BridgeError::StoreFailure);
            assert!(outcome.failures[0].persistence.is_none());
        }
    }
    struct UnsupportedRunner;
    #[async_trait::async_trait]
    impl TurnRunner for UnsupportedRunner {
        async fn run_turn(&self, _: &SessionId, _: Vec<Part>) -> bool {
            panic!("unsupported accounting must not prompt")
        }
    }
    #[tokio::test]
    async fn accounted_unsupported_default_and_mismatch_settlement_fail_closed() {
        for unsupported in [false, true] {
            let store = Arc::new(UsageStore::default());
            let mut context = warm_accounting(store.clone(), "edit");
            context.prepared = Some(context.prepare().await.unwrap());
            let outcome = if unsupported {
                accounted(&UnsupportedRunner, context).await
            } else {
                context.attribution.agent = "other".into();
                store.fail_update_at.store(1, Ordering::SeqCst);
                let backend = scripted(vec![]);
                let runner = accounting_runner(backend.clone(), backend, 0);
                accounted(&runner, context).await
            };
            assert!(!outcome.completed);
            assert_eq!(
                outcome.failures[0].original,
                if unsupported {
                    BridgeError::BindUnsupported
                } else {
                    BridgeError::ConfigMismatch {
                        field: "warm_preparation",
                    }
                }
            );
            assert_eq!(outcome.failures[0].persistence.is_some(), !unsupported);
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), 1);
            assert!(!rows[0].dispatched);
            assert_eq!(rows[0].finalized, unsupported);
        }
    }
}
