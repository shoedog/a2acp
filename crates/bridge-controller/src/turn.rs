use bridge_core::{
    accounting::{AccountingV1, UsageAttributionV1},
    domain::Part,
    error::BridgeError,
    ids::AttemptIdentity,
    ids::SessionId,
    ports::{AgentBackend, BackendObservers, BackendStream},
    translator::{AccountingFailureV1, AccountingSelectionV1, PreparedAccountingTurnV1},
};
use std::sync::{Arc, Mutex};

/// Opt-in ownership for one edit/fix operation, including any eligible respawns.
/// A prepared initial attempt is consumed once; every later candidate gets a new ID.
pub struct WarmTurnAccountingV1 {
    pub accounting: AccountingV1,
    pub owner: AttemptIdentity,
    pub attribution: UsageAttributionV1,
    pub prepared: Option<PreparedAccountingTurnV1>,
}
impl WarmTurnAccountingV1 {
    pub async fn prepare(
        &self,
    ) -> Result<PreparedAccountingTurnV1, bridge_core::error::BridgeError> {
        AccountingSelectionV1::new(self.owner.clone(), self.attribution.clone())?
            .start(&self.accounting)
            .await
    }
    /// Keep the operation role while distinguishing an eligible replacement candidate.
    pub(crate) async fn prepare_replacement(
        &self,
    ) -> Result<PreparedAccountingTurnV1, BridgeError> {
        let mut attribution = self.attribution.clone();
        attribution.attempt_kind.push_str("_rebuild");
        AccountingSelectionV1::new(self.owner.clone(), attribution)?
            .start(&self.accounting)
            .await
    }
    pub(crate) fn validate(
        &self,
        spec: &bridge_core::domain::SessionSpec,
        prepared: Option<&PreparedAccountingTurnV1>,
    ) -> Result<(), BridgeError> {
        let effort = spec
            .config
            .effort
            .map(|value| format!("{value:?}").to_ascii_lowercase());
        if self.attribution.effective_model != spec.config.model
            || self.attribution.effective_effort != effort
            || self.attribution.effective_mode != spec.config.mode
        {
            return Err(BridgeError::ConfigMismatch {
                field: "warm_effective_config",
            });
        }
        if let Some(prepared) = prepared {
            let frozen = prepared.attribution();
            let mut expected = self.attribution.clone();
            expected.execution = Some(self.owner.execution_id.as_str().into());
            expected.run_attempt = Some(self.owner.attempt_id.as_str().into());
            // These values are minted/resolved by accounting at admission.
            expected.turn = frozen.turn.clone();
            expected.billing = frozen.billing.clone();
            if frozen != &expected {
                return Err(BridgeError::ConfigMismatch {
                    field: "warm_preparation",
                });
            }
        }
        Ok(())
    }
}

/// C2 handoff before legacy boolean projection, including failed candidates that
/// preceded a successful respawn. No routing or fallback authority is implied.
#[derive(Debug, Default)]
pub struct WarmTurnResultV1 {
    pub completed: bool,
    /// A provider cancellation is an observed terminal, not MissingTerminal.
    pub cancelled: bool,
    pub failures: Vec<AccountingFailureV1>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    pub completed: bool,
    pub last_err: Option<bridge_core::error::BridgeError>,
}

/// Drain a warm-session turn's raw `Update` stream -> `TurnOutcome`. STRICTER than the executor (which
/// leaves `ok=true` on a clean end): complete IFF a `Done { stop_reason != CANCELLED }` arrived; a clean
/// end without `Done` or an error-only stream -> incomplete. Completion latches so a trailing teardown
/// error after a successful `Done` does not un-complete the turn or replace the last pre-completion error.
pub async fn drain_turn(mut stream: bridge_core::ports::BackendStream) -> TurnOutcome {
    use bridge_core::ports::{Update, STOP_REASON_CANCELLED};
    use futures::StreamExt;

    let mut completed = false;
    let mut last_err = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Update::Done { stop_reason, .. }) => {
                if stop_reason != STOP_REASON_CANCELLED {
                    completed = true;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[implement] turn: stream error: {e:?}");
                if !completed {
                    last_err = Some(e);
                }
            }
        }
    }
    TurnOutcome {
        completed,
        last_err,
    }
}

#[async_trait::async_trait]
pub trait TurnRunner: Send + Sync {
    async fn run_turn_accounted_v1(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
        telemetry: RecordedTurnTelemetry,
        accounting: WarmTurnAccountingV1,
    ) -> WarmTurnResultV1 {
        let _ = (session, parts, observer);
        telemetry.terminal_evidence.close();
        WarmTurnResultV1 {
            completed: false,
            cancelled: false,
            failures: vec![
                AccountingFailureV1::from(bridge_core::error::BridgeError::BindUnsupported)
                    .settle(accounting.prepared)
                    .await,
            ],
        }
    }

    async fn acquire_host_custody(&self) -> Result<(), String> {
        Err("implement: writer custody unsupported; stopping before host git".into())
    }

    async fn run_turn(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> bool;

    async fn run_turn_observed(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        _observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
    ) -> bool {
        self.run_turn(session, parts).await
    }

    async fn run_turn_recorded(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
        _activity: std::sync::Arc<dyn bridge_core::attempt_activity::AttemptRecorder>,
    ) -> bool {
        self.run_turn_observed(session, parts, observer).await
    }

    async fn run_turn_with_telemetry(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
        telemetry: RecordedTurnTelemetry,
    ) -> bool {
        let completed = self
            .run_turn_recorded(session, parts, observer, telemetry.activity)
            .await;
        telemetry.terminal_evidence.close();
        completed
    }
}

#[derive(Clone)]
pub struct RecordedTurnTelemetry {
    pub activity: std::sync::Arc<dyn bridge_core::attempt_activity::AttemptRecorder>,
    pub terminal_evidence: std::sync::Arc<dyn bridge_core::terminal_evidence::TerminalEvidenceSink>,
    pub turn_meta: Option<bridge_core::permission::TurnMeta>,
}

/// Observe the raw error/end before the accounting boundary can replace it with
/// a persistence error. This temporary view is never retained in a warm session.
#[derive(Default)]
struct RawTurnEvidence {
    error: Option<BridgeError>,
    ended: bool,
    cancelled: bool,
}
struct RawTurnView {
    inner: Arc<dyn AgentBackend>,
    evidence: Arc<Mutex<RawTurnEvidence>>,
    stream: Arc<tokio::sync::Mutex<Option<BackendStream>>>,
}
#[async_trait::async_trait]
impl AgentBackend for RawTurnView {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_with_observers(
            session,
            parts,
            BackendObservers::diagnostic_only(Arc::new(
                bridge_core::diagnostics::NoopDiagnosticObserver::default(),
            )),
        )
        .await
    }
    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        let stream = self
            .inner
            .prompt_with_observers(session, parts, observers)
            .await
            .inspect_err(|error| {
                self.evidence.lock().unwrap().error = Some(error.clone());
            })?;
        *self.stream.lock().await = Some(stream);
        let stream = self.stream.clone();
        let evidence = self.evidence.clone();
        use futures::StreamExt;
        Ok(Box::pin(futures::stream::unfold(
            (stream, evidence),
            |(stream, evidence)| async move {
                let mut guard = stream.lock().await;
                let raw = guard.as_mut().expect("raw turn installed");
                let Some(item) = raw.next().await else {
                    evidence.lock().unwrap().ended = true;
                    return None;
                };
                match &item {
                    Err(error) => {
                        evidence.lock().unwrap().error = Some(error.clone());
                        // Capture stops on the first error too. Await raw stream
                        // settlement before the caller requests writer custody.
                        while raw.next().await.is_some() {}
                    }
                    Ok(bridge_core::ports::Update::Done { stop_reason, .. }) => {
                        evidence.lock().unwrap().cancelled =
                            stop_reason == bridge_core::ports::STOP_REASON_CANCELLED;
                        // Capture stops at Done. Finish the raw warm stream first so
                        // its awaited custody/active-turn settlement is not detached
                        // by capture dropping it. Post-terminal items cannot change
                        // the already observed provider terminal.
                        while raw.next().await.is_some() {}
                        // Only proxy EOF marks ended: capture may still fail its
                        // terminal write and must retain that persistence error.
                    }
                    _ => {}
                }
                drop(guard);
                Some((item, (stream, evidence)))
            },
        )))
    }
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.inner.cancel(session).await
    }
}

pub(crate) async fn drain_accounted_turn_v1(
    backend: Arc<dyn AgentBackend>,
    session: &SessionId,
    parts: Vec<Part>,
    observers: BackendObservers,
    prepared: PreparedAccountingTurnV1,
) -> (TurnOutcome, Option<BridgeError>, bool) {
    let evidence = Arc::new(Mutex::new(RawTurnEvidence::default()));
    let raw_stream = Arc::new(tokio::sync::Mutex::new(None));
    let capture =
        bridge_core::accounting_backend::AccountingBackendV1::new(Arc::new(RawTurnView {
            inner: backend.clone(),
            evidence: evidence.clone(),
            stream: raw_stream.clone(),
        }));
    let mut outcome = match capture
        .prompt_accounted_v1(session, parts, prepared.into_attempt(), Some(observers))
        .await
    {
        Ok(stream) => drain_turn(stream).await,
        Err(error) => TurnOutcome {
            completed: false,
            last_err: Some(error),
        },
    };
    let persistence = {
        let raw = evidence.lock().unwrap();
        if outcome.last_err.is_some() && raw.error.is_none() && !raw.ended {
            outcome.last_err.clone()
        } else {
            None
        }
    };
    // Capture may stop on a failed usage write before the raw terminal. Retain
    // that stream outside capture so its TurnGuard cannot detach on drop; cancel
    // and await its settlement before returning to host Git custody.
    if persistence.is_some() {
        let _ = backend.cancel(session).await;
        if let Some(mut stream) = raw_stream.lock().await.take() {
            use futures::StreamExt;
            while stream.next().await.is_some() {}
        }
    }
    let raw = evidence.lock().unwrap();
    if raw.error.is_some() {
        outcome.last_err = raw.error.clone();
    }
    (outcome, persistence, raw.cancelled)
}
