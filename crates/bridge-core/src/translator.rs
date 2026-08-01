// translator.rs — the anti-corruption core (spec §5.3, Task 11).
//
// Drives an `AgentBackend`'s `Update` stream and produces a stream of A2A-facing
// `Event`s for the SSE layer, applying the anti-corruption rules:
//   * Coalesce consecutive `Update::Text` into `Event{kind:Status}` chunks; flush
//     when accumulated text reaches `max_chunk` chars, a non-text update arrives,
//     or the stream ends.
//   * `Update::Done` -> emit a final `Event{kind:Artifact}`, then end.
//   * `Update::Permission(req)` -> consult the `PolicyEngine`. On `Ok(Approve)`
//     continue; on `Err(PermissionDenied)` persist a `PendingRequest` and end the
//     stream with `Err(PermissionRequired { request_id })` (suspend; resumable).
//   * A backend error (e.g. `FrameError`) ends the stream with that error after
//     flushing any pending coalesced text (fail the task; NO silent restart).

use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};

use crate::attestation::HarvestSanitizationMode;
use crate::domain::{Part, PendingKind, PendingRequest, SessionContext};
use crate::error::BridgeError;
use crate::harvest::{
    commit_harvested_completion, CompletionBodyOrigin, HarvestAuditStore, NoopHarvestAuditStore,
};
use crate::ids::{ContextId, SessionId, TaskId, TurnId};
use crate::orch::UsageSnapshot;
use crate::ports::{
    AgentBackend, BackendObservers, DiagnosticObserver, PolicyEngine, SessionStore, Update,
    STOP_REASON_CANCELLED,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Status,
    Artifact,
    Terminal,
    Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub struct Event {
    kind: EventKind,
    text: String,
    source: Option<String>,
    outcome: Option<TaskOutcome>,
    usage: Option<UsageSnapshot>,
}

impl Event {
    pub fn kind(&self) -> &EventKind {
        &self.kind
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn text_len(&self) -> usize {
        self.text.chars().count()
    }
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }
    pub fn with_source(mut self, s: impl Into<String>) -> Self {
        self.source = Some(s.into());
        self
    }
    pub fn outcome(&self) -> Option<TaskOutcome> {
        self.outcome
    }
    pub fn usage_snapshot(&self) -> Option<&UsageSnapshot> {
        self.usage.as_ref()
    }
    pub fn status(t: impl Into<String>) -> Self {
        Self {
            kind: EventKind::Status,
            text: t.into(),
            source: None,
            outcome: None,
            usage: None,
        }
    }
    pub fn artifact(t: impl Into<String>) -> Self {
        let text = t.into();
        Self {
            kind: EventKind::Artifact,
            text,
            source: None,
            outcome: None,
            usage: None,
        }
    }
    pub fn terminal(o: TaskOutcome) -> Self {
        Self {
            kind: EventKind::Terminal,
            text: String::new(),
            source: None,
            outcome: Some(o),
            usage: None,
        }
    }
    pub fn usage(snap: UsageSnapshot) -> Self {
        Self {
            kind: EventKind::Usage,
            text: String::new(),
            source: None,
            outcome: None,
            usage: Some(snap),
        }
    }
}

pub struct Translator {
    max_chunk: usize,
}

fn default_turn_context(task: &TaskId, session: &SessionId) -> crate::ports::TurnContext {
    crate::ports::TurnContext {
        turn_id: TurnId::parse("translator-run").expect("fallback turn id is valid"),
        session_id: ContextId::parse(session.as_str()).unwrap_or_else(|_| {
            ContextId::parse("translator-run").expect("fallback context id is valid")
        }),
        task_id: Some(task.clone()),
        workflow: None,
        node: None,
        attempt: 0,
        agent: "translator".to_string(),
        model: None,
        effort: None,
        mode: None,
        prompt_id: None,
        traceparent: None,
    }
}

impl Translator {
    pub fn new() -> Self {
        Self { max_chunk: 1200 }
    }

    /// Drives `backend.prompt(session, parts)` and returns a stream of translated events.
    /// On permission-suspend, persists a PendingRequest to `store` for `task` and ends with Err.
    pub fn run<'a>(
        &self,
        backend: &'a dyn AgentBackend,
        store: &'a dyn SessionStore,
        policy: &'a dyn PolicyEngine,
        task: &'a TaskId,
        session: &'a SessionId,
        parts: Vec<Part>,
    ) -> Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send + 'a>> {
        self.run_observed(
            backend,
            store,
            policy,
            task,
            session,
            parts,
            Arc::new(crate::diagnostics::NoopDiagnosticObserver::default()),
            default_turn_context(task, session),
            Arc::new(NoopHarvestAuditStore),
        )
    }

    /// Drives the same translation pipeline while transferring one
    /// operation-scoped lifecycle observer into backend resolution/prompt
    /// ownership. The returned stream owns the observer for exactly as long as
    /// the backend prompt/stream is live; no backend cache or task binding may
    /// retain it after the stream is dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn run_observed<'a>(
        &self,
        backend: &'a dyn AgentBackend,
        store: &'a dyn SessionStore,
        policy: &'a dyn PolicyEngine,
        task: &'a TaskId,
        session: &'a SessionId,
        parts: Vec<Part>,
        diagnostic: Arc<dyn DiagnosticObserver>,
        turn_context: crate::ports::TurnContext,
        harvest_audit_store: Arc<dyn HarvestAuditStore>,
    ) -> Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send + 'a>> {
        self.run_observed_with_attempt_telemetry(
            backend,
            store,
            policy,
            task,
            session,
            parts,
            diagnostic,
            turn_context,
            harvest_audit_store,
            Arc::new(crate::attempt_activity::NoopAttemptRecorder),
            Arc::new(crate::terminal_evidence::SharedTurnEvidence::unsupported()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_observed_with_attempt_telemetry<'a>(
        &self,
        backend: &'a dyn AgentBackend,
        store: &'a dyn SessionStore,
        policy: &'a dyn PolicyEngine,
        task: &'a TaskId,
        session: &'a SessionId,
        parts: Vec<Part>,
        diagnostic: Arc<dyn DiagnosticObserver>,
        turn_context: crate::ports::TurnContext,
        harvest_audit_store: Arc<dyn HarvestAuditStore>,
        activity: Arc<dyn crate::attempt_activity::AttemptRecorder>,
        terminal_evidence: Arc<dyn crate::terminal_evidence::TerminalEvidenceSink>,
    ) -> Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send + 'a>> {
        let max_chunk = self.max_chunk;
        Box::pin(async_stream::try_stream! {
            let mut stream = backend
                .prompt_with_observers(
                    session,
                    parts,
                    BackendObservers::diagnostic_only(diagnostic)
                        .with_attempt_telemetry(activity, terminal_evidence.clone()),
                )
                .await?;
            // Accumulated text awaiting flush as a Status chunk.
            let mut acc = String::new();
            // Full text emitted by the backend, used as the Artifact payload on Done.
            let mut artifact_text = String::new();
            let mut saw_text_delta = false;

            while let Some(item) = stream.next().await {
                match item {
                    Ok(Update::Text(t)) => {
                        saw_text_delta = true;
                        artifact_text.push_str(&t);
                        acc.push_str(&t);
                        // Flush as many full chunks as the accumulator allows.
                        while acc.chars().count() >= max_chunk {
                            let chunk: String = acc.chars().take(max_chunk).collect();
                            acc = acc.chars().skip(max_chunk).collect();
                            yield Event::status(chunk);
                        }
                    }
                    Ok(Update::FinalAnswer(t)) => {
                        saw_text_delta = true;
                        if !t.is_empty() {
                            terminal_evidence.record_deliverable_final();
                        }
                        artifact_text.push_str(&t);
                        acc.push_str(&t);
                        while acc.chars().count() >= max_chunk {
                            let chunk: String = acc.chars().take(max_chunk).collect();
                            acc = acc.chars().skip(max_chunk).collect();
                            yield Event::status(chunk);
                        }
                    }
                    Ok(Update::Permission(req)) => {
                        // Flush pending text before handling the permission boundary.
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event::status(chunk);
                        }
                        let ctx = SessionContext;
                        match policy.decide(&req, &ctx) {
                            Ok(_) => {
                                // Non-interactive / auto-approved: continue the stream.
                            }
                            Err(_) => {
                                // Interactive / unresolvable: persist pending + suspend.
                                let pending = PendingRequest {
                                    request_id: req.request_id.clone(),
                                    kind: PendingKind::Permission,
                                };
                                store.put_pending(task, &pending).await?;
                                Err(BridgeError::PermissionRequired {
                                    request_id: req.request_id.clone(),
                                })?;
                            }
                        }
                    }
                    Ok(Update::Usage(snap)) => {
                        yield Event::usage(snap);
                    }
                    Ok(Update::Done { stop_reason, prefix_attestation }) => {
                        // Flush any pending coalesced text first.
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event::status(chunk);
                        }
                        // A user-cancelled turn ends with stop_reason == STOP_REASON_CANCELLED
                        // (the ACP wire string for StopReason::Cancelled). Detect it
                        // BEFORE moving `stop_reason` into the artifact payload, so we
                        // can emit a terminal Canceled signal after the artifact.
                        let cancelled = stop_reason == STOP_REASON_CANCELLED;
                        // Final artifact carries the committed effective body. The
                        // `saw_text_delta == false` branch audits `stop_reason` as
                        // bridge-synthetic and never submits it to cut validation.
                        let (payload, origin) = if saw_text_delta {
                            (artifact_text, CompletionBodyOrigin::ModelText)
                        } else {
                            (stop_reason, CompletionBodyOrigin::BridgeStopReasonWithoutText)
                        };
                        let committed = commit_harvested_completion(
                            &turn_context,
                            HarvestSanitizationMode::Off,
                            &backend.prefix_attestation_capability(),
                            &turn_context.agent,
                            origin,
                            payload,
                            prefix_attestation,
                            harvest_audit_store.as_ref(),
                        )
                        .await?;
                        yield Event::artifact(committed.effective_body);
                        // A cancelled Done drives a terminal Canceled outcome so the
                        // local-backend producer reports Canceled (not Completed) to the
                        // A2A caller. A normal end_turn emits no terminal here, leaving
                        // the producer's clean-end -> Completed mapping intact.
                        if cancelled {
                            yield Event::terminal(TaskOutcome::Canceled);
                        }
                        return;
                    }
                    Err(e) => {
                        // Flush pending text as a Status event, then fail (no restart).
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event::status(chunk);
                        }
                        Err(e)?;
                    }
                }
            }
            // A direct-unary producer must terminate with Done or a structured
            // error. Preserve already-emitted commentary, then fail explicitly;
            // clean EOF is never implicit completion.
            if !acc.is_empty() {
                yield Event::status(acc);
            }
            Err(BridgeError::MissingTerminal)?;
        })
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod v25ev {
    use super::*;
    #[test]
    fn event_ctors() {
        // Test with &str (str-to-String coercion path).
        assert_eq!(Event::status("a").kind(), &EventKind::Status);
        assert_eq!(Event::artifact("b").text(), "b");
        // Test with String (owned path).
        assert_eq!(Event::status(String::from("c")).kind(), &EventKind::Status);
        assert_eq!(Event::artifact(String::from("d")).text(), "d");
        // Verify text() and text_len() via constructed events.
        let s = Event::status("hello");
        assert_eq!(s.text(), "hello");
        assert_eq!(s.text_len(), 5);
        let a = Event::artifact("world!");
        assert_eq!(a.text(), "world!");
        assert_eq!(a.text_len(), 6);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::error::BridgeError;
    use crate::ids::{SessionId, TaskId};
    use crate::ports::*;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::sync::{Mutex, Weak};

    struct FakeBackend {
        items: Mutex<Option<Vec<Result<Update, BridgeError>>>>,
    }
    impl FakeBackend {
        fn new(v: Vec<Result<Update, BridgeError>>) -> Self {
            Self {
                items: Mutex::new(Some(v)),
            }
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let v = self.items.lock().unwrap().take().unwrap();
            Ok(Box::pin(tokio_stream::iter(v)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MarkerDiagnosticObserver;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnosticObserver {
        async fn record(
            &self,
            _event: crate::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CompositeOnlyBackend {
        diagnostics: Mutex<Vec<Weak<dyn DiagnosticObserver>>>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for CompositeOnlyBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("run_observed must use prompt_with_observers")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            assert!(observers.rich.is_none(), "translator owns no rich sink");
            self.diagnostics
                .lock()
                .unwrap()
                .push(Arc::downgrade(&observers.diagnostic));
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            })])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        pending: Mutex<HashMap<String, PendingRequest>>,
        peer_tasks: Mutex<HashMap<String, crate::domain::PeerTaskId>>,
        cancels: Mutex<std::collections::HashSet<String>>,
    }
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, _t: &TaskId, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn session_for(&self, _t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(None)
        }
        async fn put_pending(&self, t: &TaskId, r: &PendingRequest) -> Result<(), BridgeError> {
            self.pending
                .lock()
                .unwrap()
                .insert(t.as_str().into(), r.clone());
            Ok(())
        }
        async fn take_pending(&self, t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
            Ok(self.pending.lock().unwrap().remove(t.as_str()))
        }
        async fn set_peer_task(
            &self,
            t: &TaskId,
            peer: &crate::domain::PeerTaskId,
        ) -> Result<(), BridgeError> {
            self.peer_tasks
                .lock()
                .unwrap()
                .insert(t.as_str().into(), peer.clone());
            Ok(())
        }
        async fn peer_task_for(
            &self,
            t: &TaskId,
        ) -> Result<Option<crate::domain::PeerTaskId>, BridgeError> {
            Ok(self.peer_tasks.lock().unwrap().get(t.as_str()).cloned())
        }
        async fn request_cancel(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn cancel_requested(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.cancels.lock().unwrap().contains(t.as_str()))
        }
        async fn set_fanout(&self, _t: &TaskId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn is_fanout(&self, _t: &TaskId) -> Result<bool, BridgeError> {
            Ok(false)
        }
    }

    // approves non-interactive, denies interactive (mirrors AutoPolicy)
    struct AutoApprove;
    impl PolicyEngine for AutoApprove {
        fn decide(
            &self,
            req: &PermissionRequest,
            _c: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            if req.interactive {
                Err(BridgeError::PermissionDenied)
            } else {
                Ok(PermissionDecision::Approve)
            }
        }
    }

    fn ids() -> (TaskId, SessionId) {
        (TaskId::parse("t").unwrap(), SessionId::parse("s").unwrap())
    }

    #[tokio::test]
    async fn happy_path_status_then_artifact() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("PONG".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect()
            .await;
        let evs: Vec<Event> = evs.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(evs.first().unwrap().kind(), &EventKind::Status);
        assert!(evs.iter().any(|e| e.kind() == &EventKind::Artifact));
    }

    #[tokio::test]
    async fn run_observed_forwards_exact_diagnostic_and_releases_it_after_stream() {
        let backend = CompositeOnlyBackend::default();
        let store = FakeStore::default();
        let policy = AutoApprove;
        let (task, session) = ids();
        let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnosticObserver);
        let weak = Arc::downgrade(&observer);

        let events = Translator::new()
            .run_observed(
                &backend,
                &store,
                &policy,
                &task,
                &session,
                vec![],
                observer.clone(),
                default_turn_context(&task, &session),
                Arc::new(NoopHarvestAuditStore),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(events.len(), 1);
        assert!(events[0].is_ok());
        let seen = backend.diagnostics.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0]
                .upgrade()
                .as_ref()
                .is_some_and(|captured| Arc::ptr_eq(captured, &observer)),
            "translator must forward the caller-owned observer"
        );
        drop(seen);
        drop(observer);
        assert!(
            weak.upgrade().is_none(),
            "translator/backend must not retain the operation observer"
        );
    }

    #[tokio::test]
    async fn usage_update_emits_usage_event_and_not_artifact_text() {
        let be = FakeBackend::new(vec![
            Ok(Update::Usage(UsageSnapshot {
                used: Some(7),
                size: Some(9),
                cost: None,
                terminal: None,
                at_ms: 0,
            })),
            Ok(Update::Text("body".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let usage = evs
            .iter()
            .find(|e| e.kind() == &EventKind::Usage)
            .expect("usage event");
        assert_eq!(usage.usage_snapshot().and_then(|s| s.used), Some(7));
        assert_eq!(usage.text(), "");

        let artifact = evs
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), "body");
    }

    #[tokio::test]
    async fn multi_chunk_artifact_carries_full_text() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("AL".into())),
            Ok(Update::Text("PH".into())),
            Ok(Update::Text("A".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let artifact = evs
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), "ALPHA");
    }

    #[tokio::test]
    async fn empty_text_delta_produces_empty_artifact() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text(String::new())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let artifact = evs
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), "");
    }

    #[tokio::test]
    async fn coalesces_and_caps_chunks_at_1200() {
        // 50 updates of 40 chars = 2000 chars -> coalesced into >=2 Status chunks,
        // each <=1200 chars, far fewer than 50 events. The final Artifact carries
        // the full text and is not capped.
        let mut v: Vec<Result<Update, BridgeError>> =
            (0..50).map(|_| Ok(Update::Text("x".repeat(40)))).collect();
        v.push(Ok(Update::Done {
            stop_reason: "end_turn".into(),
            prefix_attestation: Default::default(),
        }));
        let be = FakeBackend::new(v);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let status = evs
            .iter()
            .filter(|e| e.kind() == &EventKind::Status)
            .count();
        assert!((2..50).contains(&status), "status events: {status}");
        assert!(evs
            .iter()
            .filter(|e| e.kind() == &EventKind::Status)
            .all(|e| e.text_len() <= 1200));
        let artifact = evs
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text_len(), 2000);
    }

    #[tokio::test]
    async fn large_output_keeps_full_artifact_and_caps_status_chunks() {
        // A single large delta (3001 chars) must coalesce into the 1200/1200/601
        // chunking contract: two Status flushes while the delta is processed, plus
        // a trailing flush of the remainder on Done. The final Artifact still
        // carries the complete, uncapped text.
        let expected = "x".repeat(3_001);
        let be = FakeBackend::new(vec![
            Ok(Update::Text(expected.clone())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let status: Vec<&Event> = evs
            .iter()
            .filter(|e| e.kind() == &EventKind::Status)
            .collect();
        assert_eq!(status.len(), 3, "status events: {status:?}");
        assert!(status.iter().all(|e| e.text_len() <= 1200));
        let joined: String = status.iter().map(|e| e.text()).collect();
        assert_eq!(joined, expected);

        let artifact = evs
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), expected);
    }

    #[tokio::test]
    async fn interactive_permission_suspends_and_persists_pending() {
        let be = FakeBackend::new(vec![Ok(Update::Permission(PermissionRequest::with_id(
            "r1", true,
        )))]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let last = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .pop()
            .unwrap();
        assert_eq!(
            last.unwrap_err(),
            BridgeError::PermissionRequired {
                request_id: "r1".into()
            }
        );
        assert_eq!(st.take_pending(&t).await.unwrap().unwrap().request_id, "r1");
    }

    #[tokio::test]
    async fn non_interactive_permission_is_auto_approved_and_continues() {
        let be = FakeBackend::new(vec![
            Ok(Update::Permission(PermissionRequest::with_id("r2", false))),
            Ok(Update::Text("after".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await;
        assert!(evs.last().unwrap().is_ok()); // completed, not suspended
        assert!(st.take_pending(&t).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_error_fails_no_restart() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("partial".into())),
            Err(BridgeError::FrameError),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let items: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect()
            .await;
        assert_eq!(
            items.last().unwrap().as_ref().unwrap_err(),
            &BridgeError::FrameError
        );
    }

    #[tokio::test]
    async fn stream_ends_without_done_flushes_then_reports_missing_terminal() {
        // No Done frame: trailing commentary remains observable, but EOF is a
        // typed terminal failure rather than an implicit success.
        let be = FakeBackend::new(vec![Ok(Update::Text("trailing".into()))]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs = Translator::default()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await;
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].as_ref().unwrap().kind(), &EventKind::Status);
        assert_eq!(evs[0].as_ref().unwrap().text(), "trailing");
        assert_eq!(evs[1].as_ref().unwrap_err(), &BridgeError::MissingTerminal);
    }

    #[tokio::test]
    async fn r2f0b_commentary_text_does_not_prove_deliverable_final() {
        let backend = FakeBackend::new(vec![
            Ok(Update::Text("commentary only".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let store = FakeStore::default();
        let policy = AutoApprove;
        let (task, session) = ids();
        let evidence = Arc::new(crate::terminal_evidence::SharedTurnEvidence::unsupported());
        let events = Translator::new()
            .run_observed_with_attempt_telemetry(
                &backend,
                &store,
                &policy,
                &task,
                &session,
                vec![],
                Arc::new(MarkerDiagnosticObserver),
                default_turn_context(&task, &session),
                Arc::new(NoopHarvestAuditStore),
                Arc::new(crate::attempt_activity::NoopAttemptRecorder),
                evidence.clone(),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.into_iter().all(|event| event.is_ok()));
        assert!(
            !crate::terminal_evidence::TerminalEvidenceSink::deliverable_final_present(
                evidence.as_ref(),
            ),
            "commentary must not manufacture final delivery"
        );
    }
    #[tokio::test]
    async fn r2f0b_commentary_then_failure_does_not_prove_direct_delivery() {
        let backend = FakeBackend::new(vec![
            Ok(Update::Text("commentary before failure".into())),
            Err(BridgeError::AgentTimedOut),
        ]);
        let store = FakeStore::default();
        let policy = AutoApprove;
        let (task, session) = ids();
        let evidence = Arc::new(crate::terminal_evidence::SharedTurnEvidence::unsupported());
        let events = Translator::new()
            .run_observed_with_attempt_telemetry(
                &backend,
                &store,
                &policy,
                &task,
                &session,
                vec![],
                Arc::new(MarkerDiagnosticObserver),
                default_turn_context(&task, &session),
                Arc::new(NoopHarvestAuditStore),
                Arc::new(crate::attempt_activity::NoopAttemptRecorder),
                evidence.clone(),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.last().is_some_and(Result::is_err));
        assert!(
            !crate::terminal_evidence::TerminalEvidenceSink::deliverable_final_present(
                evidence.as_ref(),
            )
        );
    }

    #[tokio::test]
    async fn r2f0b_only_nonempty_final_answer_proves_direct_delivery() {
        for (final_text, expected) in [("", false), ("answer", true)] {
            let backend = FakeBackend::new(vec![
                Ok(Update::Text("commentary".into())),
                Ok(Update::FinalAnswer(final_text.into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ]);
            let store = FakeStore::default();
            let policy = AutoApprove;
            let (task, session) = ids();
            let evidence = Arc::new(crate::terminal_evidence::SharedTurnEvidence::unsupported());
            let events = Translator::new()
                .run_observed_with_attempt_telemetry(
                    &backend,
                    &store,
                    &policy,
                    &task,
                    &session,
                    vec![],
                    Arc::new(MarkerDiagnosticObserver),
                    default_turn_context(&task, &session),
                    Arc::new(NoopHarvestAuditStore),
                    Arc::new(crate::attempt_activity::NoopAttemptRecorder),
                    evidence.clone(),
                )
                .collect::<Vec<_>>()
                .await;
            assert!(events.into_iter().all(|event| event.is_ok()));
            assert_eq!(
                crate::terminal_evidence::TerminalEvidenceSink::deliverable_final_present(
                    evidence.as_ref(),
                ),
                expected,
            );
        }
    }

    #[tokio::test]
    async fn done_with_no_text_uses_stop_reason_as_artifact() {
        // Done arriving with no prior text -> Artifact payload falls back to
        // stop_reason. A "ran_out_of_turns" (non-cancel) stop_reason yields ONLY the
        // artifact (no terminal frame); the producer maps clean-end -> Completed.
        let be = FakeBackend::new(vec![Ok(Update::Done {
            stop_reason: "ran_out_of_turns".into(),
            prefix_attestation: Default::default(),
        })]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evidence = Arc::new(crate::terminal_evidence::SharedTurnEvidence::unsupported());
        let evs: Vec<Event> = Translator::new()
            .run_observed_with_attempt_telemetry(
                &be,
                &st,
                &pol,
                &t,
                &s,
                vec![],
                Arc::new(MarkerDiagnosticObserver),
                default_turn_context(&t, &s),
                Arc::new(NoopHarvestAuditStore),
                Arc::new(crate::attempt_activity::NoopAttemptRecorder),
                evidence.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind(), &EventKind::Artifact);
        assert_eq!(evs[0].text(), "ran_out_of_turns");
        assert!(
            !crate::terminal_evidence::TerminalEvidenceSink::deliverable_final_present(
                evidence.as_ref(),
            ),
            "a bridge-synthetic stop reason is not delivered model text"
        );
    }

    #[tokio::test]
    async fn done_cancelled_emits_artifact_then_terminal_canceled() {
        // A user-cancelled turn ends with Update::Done{stop_reason:"cancelled"}.
        // The translator emits the final artifact, then a terminal Canceled event so
        // the local-backend producer reports Canceled (not Completed) to the caller.
        let be = FakeBackend::new(vec![
            Ok(Update::Text("partial".into())),
            Ok(Update::Done {
                stop_reason: "cancelled".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        // ... Status("partial"), Artifact("partial"), Terminal(Canceled).
        let artifact = evs
            .iter()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), "partial");
        let last = evs.last().unwrap();
        assert_eq!(last.kind(), &EventKind::Terminal);
        assert_eq!(last.outcome(), Some(TaskOutcome::Canceled));
    }

    #[tokio::test]
    async fn cancelled_multi_chunk_artifact_carries_full_partial_text() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("PAR".into())),
            Ok(Update::Text("TIAL".into())),
            Ok(Update::Done {
                stop_reason: STOP_REASON_CANCELLED.into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let artifact = evs
            .iter()
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact event");
        assert_eq!(artifact.text(), "PARTIAL");
        let last = evs.last().unwrap();
        assert_eq!(last.kind(), &EventKind::Terminal);
        assert_eq!(last.outcome(), Some(TaskOutcome::Canceled));
    }

    #[tokio::test]
    async fn done_end_turn_emits_no_terminal() {
        // A normal end_turn must NOT emit a terminal event — the producer maps the
        // clean stream end to Completed.
        let be = FakeBackend::new(vec![
            Ok(Update::Text("done".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert!(evs.iter().all(|e| e.kind() != &EventKind::Terminal));
    }

    /// Cover the FakeBackend::cancel and FakeStore::put/session_for stubs so
    /// llvm-cov doesn't mark those async-fn stubs as missed lines.
    #[tokio::test]
    async fn fake_helpers_cancel_and_store_stubs_covered() {
        let be = FakeBackend::new(vec![]);
        be.cancel(&SessionId::parse("s").unwrap()).await.unwrap();
        let st = FakeStore::default();
        let t = TaskId::parse("t").unwrap();
        let s = SessionId::parse("s").unwrap();
        st.put(&t, &s).await.unwrap();
        assert!(st.session_for(&t).await.unwrap().is_none());
    }

    /// Cover FakeStore::set_peer_task / peer_task_for / request_cancel / cancel_requested
    /// stubs that are exercised by delegation-path tests but not by translator unit tests.
    #[tokio::test]
    async fn fake_store_peer_task_and_cancel_stubs_covered() {
        use crate::domain::PeerTaskId;
        let st = FakeStore::default();
        let t = TaskId::parse("t").unwrap();
        let peer = PeerTaskId("peer-1".into());
        // peer_task_for before setting returns None.
        assert!(st.peer_task_for(&t).await.unwrap().is_none());
        // set then retrieve.
        st.set_peer_task(&t, &peer).await.unwrap();
        assert_eq!(st.peer_task_for(&t).await.unwrap().unwrap().0, "peer-1");
        // cancel_requested before request returns false.
        assert!(!st.cancel_requested(&t).await.unwrap());
        // request_cancel then check.
        st.request_cancel(&t).await.unwrap();
        assert!(st.cancel_requested(&t).await.unwrap());
    }
}

#[cfg(test)]
mod v26ev {
    use super::*;
    #[test]
    fn event_source_default_none_and_with_source() {
        assert_eq!(Event::status("x").source(), None);
        assert_eq!(
            Event::status("x").with_source("kiro").source(),
            Some("kiro")
        );
    }
    #[test]
    fn terminal_event_carries_outcome() {
        let e = Event::terminal(TaskOutcome::Completed);
        assert_eq!(e.kind(), &EventKind::Terminal);
        assert_eq!(e.outcome(), Some(TaskOutcome::Completed));
    }
    #[test]
    fn outcome_variants() {
        for o in [
            TaskOutcome::Completed,
            TaskOutcome::Failed,
            TaskOutcome::Canceled,
        ] {
            assert_eq!(Event::terminal(o).outcome(), Some(o));
        }
    }
    #[test]
    fn event_clone_and_debug_derive_paths() {
        // Exercise Clone + Debug derives on Event, EventKind, TaskOutcome.
        let s = Event::status("hello");
        let s2 = s.clone();
        assert_eq!(s2.text(), "hello");
        assert_eq!(s2.source(), None);
        assert_eq!(s2.outcome(), None);
        let a = Event::artifact("bye");
        let a2 = a.clone();
        assert_eq!(a2.kind(), &EventKind::Artifact);
        let t = Event::terminal(TaskOutcome::Failed);
        let t2 = t.clone();
        assert_eq!(t2.kind(), &EventKind::Terminal);
        assert_eq!(t2.outcome(), Some(TaskOutcome::Failed));
        // Debug should not panic.
        let _ = format!("{:?}", s2);
        let _ = format!("{:?}", a2);
        let _ = format!("{:?}", t2);
        let _ = format!("{:?}", EventKind::Terminal);
        let _ = format!("{:?}", EventKind::Status);
        let _ = format!("{:?}", EventKind::Artifact);
        let _ = format!("{:?}", TaskOutcome::Completed);
        let _ = format!("{:?}", TaskOutcome::Failed);
        let _ = format!("{:?}", TaskOutcome::Canceled);
    }
    #[test]
    fn event_kind_terminal_eq() {
        assert_eq!(EventKind::Terminal, EventKind::Terminal);
        assert_ne!(EventKind::Terminal, EventKind::Status);
        assert_ne!(EventKind::Terminal, EventKind::Artifact);
    }
    #[test]
    fn status_and_artifact_ctors_have_no_source_or_outcome() {
        // Explicitly verify the new fields on the existing constructors.
        let s = Event::status(String::from("owned"));
        assert_eq!(s.kind(), &EventKind::Status);
        assert_eq!(s.text(), "owned");
        assert_eq!(s.source(), None);
        assert_eq!(s.outcome(), None);
        let a = Event::artifact(String::from("result"));
        assert_eq!(a.kind(), &EventKind::Artifact);
        assert_eq!(a.text(), "result");
        assert_eq!(a.source(), None);
        assert_eq!(a.outcome(), None);
    }
    #[test]
    fn event_drop_with_some_source_exercises_drop_glue() {
        // Create and drop Events with Some(source) to exercise the Drop impl for Option<String>.
        {
            let e = Event::status("text").with_source(String::from("agent-1"));
            assert_eq!(e.source(), Some("agent-1"));
            // e is dropped here, exercising Drop for Option<String>
        }
        {
            let e = Event::artifact("result").with_source(String::from("agent-2"));
            assert_eq!(e.source(), Some("agent-2"));
        }
        {
            let e = Event::terminal(TaskOutcome::Canceled).with_source(String::from("agent-3"));
            assert_eq!(e.source(), Some("agent-3"));
            assert_eq!(e.outcome(), Some(TaskOutcome::Canceled));
        }
    }
}
