//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::ApiConfig;
use crate::provider::{classify_http_error, MAX_ERROR_BODY_BYTES};
use crate::wire::{ChatRequest, Message, SseAccumulator, ToolCall};
use bridge_core::catalog::is_blocked_model_id;
use bridge_core::diagnostics::{
    diagnostic_timestamp_ms, DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor,
    FailureDiagnostic, FailureDiagnosticInput, FailureDisposition, PersistedPhaseTransition,
    PersistedPhaseTransitionInput, PhaseStatus,
};
use bridge_core::domain::{
    Part, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, PolicyEngine, RichEventSink,
    Update, STOP_REASON_CANCELLED,
};
use bridge_core::provider::ProviderEvidence;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::watch;

#[derive(Clone)]
struct ApiLifecycle {
    observer: Arc<dyn DiagnosticObserver>,
    redactor: DiagnosticRedactor,
}

impl ApiLifecycle {
    fn new(observer: Arc<dyn DiagnosticObserver>, api_key: Option<&str>) -> Self {
        Self {
            observer,
            redactor: DiagnosticRedactor::new(api_key),
        }
    }

    async fn record(&self, phase: DiagnosticPhase, status: PhaseStatus) -> Result<(), BridgeError> {
        let transition = PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            None,
            &self.redactor,
        )
        .map_err(|_| BridgeError::InvalidStateTransition)?;
        let event = bridge_core::diagnostics::DiagnosticEvent::new(transition, None)
            .map_err(|_| BridgeError::InvalidStateTransition)?;
        self.observer.record(event).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn failure(
        &self,
        class: DiagnosticFailureClass,
        code: &'static str,
        summary: &'static str,
        cause: Option<String>,
        retry_after_ms: Option<u64>,
        reset_at_ms: Option<i64>,
    ) -> BridgeError {
        let failure = match FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStream,
                last_completed_phase: Some(DiagnosticPhase::PromptStart),
                class,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: summary.to_owned(),
                causes: cause.into_iter().collect(),
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms,
                reset_at_ms,
                prompt_may_have_been_accepted: true,
            },
            code,
            &self.redactor,
        ) {
            Ok(failure) => failure,
            Err(_) => return BridgeError::InvalidStateTransition,
        };
        let transition = match PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase: DiagnosticPhase::PromptStream,
                status: PhaseStatus::Failed,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            Some(code),
            &self.redactor,
        ) {
            Ok(transition) => transition,
            Err(_) => return BridgeError::InvalidStateTransition,
        };
        let event =
            match bridge_core::diagnostics::DiagnosticEvent::new(transition, Some(failure.clone()))
            {
                Ok(event) => event,
                Err(_) => return BridgeError::InvalidStateTransition,
            };
        match self.observer.record(event).await {
            Ok(()) => BridgeError::agent_failure(failure),
            Err(error) => error,
        }
    }
}

struct BoundedErrorBody {
    bytes: Vec<u8>,
    oversized: bool,
}

async fn read_bounded_error_body(
    response: reqwest::Response,
) -> Result<BoundedErrorBody, reqwest::Error> {
    let mut chunks = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            return Ok(BoundedErrorBody {
                bytes,
                oversized: true,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(BoundedErrorBody {
        bytes,
        oversized: false,
    })
}

fn request_failure(
    error: &reqwest::Error,
    transport_code: &'static str,
) -> (DiagnosticFailureClass, &'static str, &'static str) {
    if error.is_timeout() {
        (
            DiagnosticFailureClass::Timeout,
            "api.prompt.timeout",
            "Upstream API request timed out",
        )
    } else {
        (
            DiagnosticFailureClass::Transport,
            transport_code,
            "Upstream API transport failed",
        )
    }
}

async fn complete_prompt_lifecycle(lifecycle: &ApiLifecycle) -> Result<(), BridgeError> {
    lifecycle
        .record(DiagnosticPhase::PromptStream, PhaseStatus::Completed)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptFinish, PhaseStatus::Started)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptFinish, PhaseStatus::Completed)
        .await
}

/// Install the first request future before publishing the post-barrier phase
/// transitions. The returned future has not been polled yet.
async fn install_first_send<F>(
    lifecycle: &ApiLifecycle,
    install: impl FnOnce() -> F,
) -> Result<F, BridgeError> {
    let send = install();
    lifecycle
        .record(DiagnosticPhase::PromptStart, PhaseStatus::Completed)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptStream, PhaseStatus::Started)
        .await?;
    Ok(send)
}

/// Per-session state: the stashed effective model + a `watch` channel used as the
/// cancel signal. A `watch` (level-triggered, version-counted) lets the turn loop
/// `select!` on cancellation even while parked awaiting the next SSE chunk — an
/// `AtomicBool` polled only between chunks cannot cancel during a stall.
struct SessionState {
    model: Option<String>,
    cancel: watch::Sender<bool>,
}
impl Default for SessionState {
    fn default() -> Self {
        Self {
            model: None,
            cancel: watch::channel(false).0,
        }
    }
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>,
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
}

/// Default policy: approve everything (mirrors AcpBackend's default auto-approver).
struct AutoApprove;
impl PolicyEngine for AutoApprove {
    fn decide(
        &self,
        _: &PermissionRequest,
        _: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .expect("reqwest client builds");
        Self {
            cfg,
            client,
            policy: Arc::new(StdMutex::new(Arc::new(AutoApprove) as Arc<dyn PolicyEngine>)),
            sessions: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() {
            *p = policy;
        }
        self
    }

    /// Test/inspection helper: the stashed effective model for a session.
    pub fn session_model(&self, s: &SessionId) -> Option<String> {
        self.sessions
            .lock()
            .ok()?
            .get(s)
            .and_then(|st| st.model.clone())
    }

    /// The session's cancel sender (creating the slot if absent).
    fn session_cancel(&self, s: &SessionId) -> watch::Sender<bool> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(s.clone()).or_default().cancel.clone()
    }

    fn resolve_api_key(&self) -> Option<String> {
        self.cfg
            .api_key_env
            .as_ref()
            .and_then(|var| std::env::var(var).ok())
    }
    fn resolve_model(&self, s: &SessionId) -> Option<String> {
        self.session_model(s).or_else(|| self.cfg.model.clone())
    }

    fn reject_blocked_model(model: Option<&str>) -> Result<(), BridgeError> {
        if let Some(model) = model.filter(|model| is_blocked_model_id(model)) {
            return Err(BridgeError::config_invalid(format!(
                "api model={model} is blocked by this bridge"
            )));
        }
        Ok(())
    }
}

impl ApiBackend {
    async fn prompt_inner(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        _rich_sink: Option<Arc<dyn RichEventSink>>,
        diagnostic_observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendStream, BridgeError> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let model = self.resolve_model(session);
        Self::reject_blocked_model(model.as_deref())?;
        let api_key = self.resolve_api_key();
        let lifecycle = ApiLifecycle::new(diagnostic_observer, api_key.as_deref());
        let do_stream = self.cfg.stream;
        let client = self.client.clone();
        let policy = self.policy.clone();
        let max_rounds = self.cfg.max_tool_rounds;

        // Cancel: reset for this fresh turn, THEN subscribe so a later send(true)
        // is observed as a change. `select!` on `changed()` fires even while parked
        // awaiting the next SSE chunk.
        let cancel_tx = self.session_cancel(session);
        let _ = cancel_tx.send(false);
        let mut cancel_rx = cancel_tx.subscribe();

        let mut messages: Vec<Message> = vec![Message::user(
            parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )];

        let stream = async_stream::try_stream! {
            lifecycle
                .record(DiagnosticPhase::PromptStart, PhaseStatus::Started)
                .await?;

            // This operation-scoped acceptance barrier is crossed immediately
            // before the first HTTP send future is installed. It is deliberately
            // never cleared between tool rounds: once any request may have reached
            // the provider, every later failure is fatal and non-replayable.
            let mut acceptance_barrier_crossed = false;
            for _round in 0..max_rounds {
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: do_stream };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let send = if acceptance_barrier_crossed {
                    builder.send()
                } else {
                    acceptance_barrier_crossed = true;
                    install_first_send(&lifecycle, || builder.send()).await?
                };
                if *cancel_rx.borrow() {
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() }; return;
                }
                let resp = match send.await {
                    Ok(response) => response,
                    Err(error) => {
                        let (class, code, summary) = request_failure(&error, "api.prompt.send");
                        Err(lifecycle
                            .failure(
                                class,
                                code,
                                summary,
                                Some(error.to_string()),
                                None,
                                None,
                            )
                            .await)?
                    }
                };
                if !resp.status().is_success() {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let body = match read_bounded_error_body(resp).await {
                        Ok(body) => body,
                        Err(error) => {
                            let (class, code, summary) =
                                request_failure(&error, "api.prompt.error_body_read");
                            Err(lifecycle
                                .failure(
                                    class,
                                    code,
                                    summary,
                                    Some(error.to_string()),
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    let ProviderEvidence {
                        class,
                        code,
                        retry_after_ms,
                        reset_at_ms,
                    } = classify_http_error(
                        status,
                        &body.bytes,
                        body.oversized,
                        &headers,
                        diagnostic_timestamp_ms(),
                    );
                    Err(lifecycle
                        .failure(
                            class,
                            code,
                            "Upstream API rejected the prompt",
                            Some(format!("upstream HTTP status {}", status.as_u16())),
                            retry_after_ms,
                            reset_at_ms,
                        )
                        .await)?;
                    unreachable!("non-success response always terminates the prompt stream");
                }

                let parsed = if do_stream {
                    let mut acc = SseAccumulator::default();
                    let mut bytes = resp.bytes_stream();
                    let mut buf = String::new();
                    'read: loop {
                        let chunk = tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    None
                                } else {
                                    continue 'read;
                                }
                            }
                            maybe = bytes.next() => maybe,
                        };
                        let Some(chunk) = chunk else {
                            if *cancel_rx.borrow() {
                                complete_prompt_lifecycle(&lifecycle).await?;
                                yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() };
                                return;
                            }
                            break 'read;
                        };
                        let chunk = match chunk {
                            Ok(chunk) => chunk,
                            Err(error) => {
                                let (class, code, summary) =
                                    request_failure(&error, "api.prompt.sse_read");
                                Err(lifecycle
                                    .failure(
                                        class,
                                        code,
                                        summary,
                                        Some(error.to_string()),
                                        None,
                                        None,
                                    )
                                    .await)?
                            }
                        };
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            match acc.push_sse_line(&line) {
                                Ok(Some(text)) => { yield Update::Text(text); }
                                Ok(None) => {}
                                Err(_) => {
                                    Err(lifecycle
                                        .failure(
                                            DiagnosticFailureClass::Protocol,
                                            "api.prompt.sse_frame",
                                            "Upstream API returned a malformed SSE frame",
                                            None,
                                            None,
                                            None,
                                        )
                                        .await)?;
                                }
                            }
                            if acc.is_done() { break 'read; }
                        }
                    }
                    // Flush a trailing line that arrived without a newline at EOF — but
                    // ONLY if no terminal was seen (otherwise `buf` is post-[DONE] noise,
                    // and a chunk-split partial "[DON" would falsely FrameError).
                    if !acc.is_done() && !buf.trim().is_empty() {
                        match acc.push_sse_line(&buf) {
                            Ok(Some(text)) => { yield Update::Text(text); }
                            Ok(None) => {}
                            Err(_) => {
                                Err(lifecycle
                                    .failure(
                                        DiagnosticFailureClass::Protocol,
                                        "api.prompt.sse_frame",
                                        "Upstream API returned a malformed SSE frame",
                                        None,
                                        None,
                                        None,
                                    )
                                    .await)?;
                            }
                        }
                    }
                    if !acc.is_done() {
                        Err(lifecycle
                            .failure(
                                DiagnosticFailureClass::Protocol,
                                "api.prompt.sse_incomplete",
                                "Upstream API ended SSE before terminal evidence",
                                None,
                                None,
                                None,
                            )
                            .await)?;
                    }
                    acc.finish()
                } else {
                    let body = match resp.text().await {
                        Ok(body) => body,
                        Err(error) => {
                            let (class, code, summary) =
                                request_failure(&error, "api.prompt.body_read");
                            Err(lifecycle
                                .failure(
                                    class,
                                    code,
                                    summary,
                                    Some(error.to_string()),
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    let p = match crate::wire::parse_nonstream(&body) {
                        Ok(parsed) => parsed,
                        Err(_) => {
                            Err(lifecycle
                                .failure(
                                    DiagnosticFailureClass::Protocol,
                                    "api.prompt.body_parse",
                                    "Upstream API returned a malformed response body",
                                    None,
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    if !p.text.is_empty() { yield Update::Text(p.text.clone()); }
                    p
                };
                if parsed.tool_calls.is_empty() {
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done { stop_reason: "stop".into() }; return;
                }
                // Tool round: decide each call SILENTLY via the injected policy.
                // NO Update::Permission is yielded — the backend is the sole authority.
                messages.push(Message::assistant_tool_calls(parsed.tool_calls.clone()));
                for tc in &parsed.tool_calls {
                    let result = decide_tool(&policy, tc);
                    messages.push(Message::tool_result(tc.id.clone(), result));
                }
                // continue → re-POST with the appended tool results.
            }
            if !acceptance_barrier_crossed {
                // Preserve the legacy `max_tool_rounds = 0` terminal shape. No
                // provider request exists in this degenerate configuration.
                lifecycle
                    .record(DiagnosticPhase::PromptStart, PhaseStatus::Completed)
                    .await?;
                lifecycle
                    .record(DiagnosticPhase::PromptStream, PhaseStatus::Started)
                    .await?;
            }
            complete_prompt_lifecycle(&lifecycle).await?;
            yield Update::Done { stop_reason: "max_tool_rounds".into() };
        };
        Ok(Box::pin(stream))
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            None,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            Some(sink),
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(session, parts, observers.rich, observers.diagnostic)
            .await
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        // Only signal an EXISTING slot — never mint a new one (a forgotten session
        // has no in-flight turn to cancel; minting a fresh channel here would lose
        // the signal vs the running turn's receiver).
        if let Ok(map) = self.sessions.lock() {
            if let Some(st) = map.get(session) {
                let _ = st.cancel.send(true);
            }
        }
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        Self::reject_blocked_model(spec.config.model.as_deref())?;
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = spec.config.model.clone();
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(session);
        }
    }
}

/// Silent permission decision for one tool call → the `content` of its tool-result
/// message. Approve runs the stub tool; Deny/abstain feed a refusal string.
fn decide_tool(policy: &Arc<StdMutex<Arc<dyn PolicyEngine>>>, tc: &ToolCall) -> String {
    let req = PermissionRequest::with_id(tc.id.clone(), /*interactive=*/ false);
    let decision = policy.lock().ok().map(|p| p.decide(&req, &SessionContext));
    match decision {
        Some(Ok(PermissionDecision::Approve)) => {
            if tc.function.name == crate::tool::TOOL_NAME { crate::tool::run_tool() }
            else { format!("unknown tool: {}", tc.function.name) }
        }
        Some(Err(BridgeError::PermissionDenied)) => "permission denied: tool not executed".into(),
        _ /* abstain / poisoned */ => "permission unavailable: tool not executed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{
        EffectiveConfig, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, DiagnosticObserver, PolicyEngine};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct InstallOrderObserver {
        installed: Arc<AtomicBool>,
        saw_prompt_start_completed: AtomicBool,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for InstallOrderObserver {
        async fn record(
            &self,
            event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            if event.transition().phase() == DiagnosticPhase::PromptStart
                && event.transition().status() == PhaseStatus::Completed
            {
                assert!(
                    self.installed.load(Ordering::SeqCst),
                    "prompt_start completed before the first send future was installed"
                );
                self.saw_prompt_start_completed
                    .store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    struct DenyAll;
    impl PolicyEngine for DenyAll {
        fn decide(
            &self,
            _: &PermissionRequest,
            _: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    #[tokio::test]
    async fn first_send_is_installed_before_post_barrier_transitions() {
        let installed = Arc::new(AtomicBool::new(false));
        let observer = Arc::new(InstallOrderObserver {
            installed: Arc::clone(&installed),
            saw_prompt_start_completed: AtomicBool::new(false),
        });
        let lifecycle = ApiLifecycle::new(observer.clone(), None);
        lifecycle
            .record(DiagnosticPhase::PromptStart, PhaseStatus::Started)
            .await
            .unwrap();

        let send = install_first_send(&lifecycle, || {
            installed.store(true, Ordering::SeqCst);
            std::future::ready(())
        })
        .await
        .unwrap();
        send.await;

        assert!(observer.saw_prompt_start_completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn configure_session_stashes_model_and_object_safe() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(
            &s,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("haiku".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(be.session_model(&s).as_deref(), Some("haiku"));
        be.forget_session(&s).await;
        assert!(be.session_model(&s).is_none());
        let _obj: Arc<dyn AgentBackend> = Arc::new(ApiBackend::new(crate::config::ApiConfig::new(
            "http://127.0.0.1:1",
        )));
    }

    #[tokio::test]
    async fn configure_session_rejects_blocked_fable_family_model() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        let err = be
            .configure_session(
                &s,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("claude-fable-5.1[1m]".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("api model=claude-fable-5.1[1m] is blocked by this bridge"),
            "{err}"
        );
        assert!(be.session_model(&s).is_none());
    }

    #[tokio::test]
    async fn prompt_rejects_static_blocked_fable_family_model_before_http() {
        let mut cfg = crate::config::ApiConfig::new("http://127.0.0.1:1");
        cfg.model = Some("claude-fable-5.1[1m]".into());
        let be = ApiBackend::new(cfg);
        let s = SessionId::parse("s1").unwrap();
        match be.prompt(&s, vec![Part { text: "hi".into() }]).await {
            Err(err) => assert!(
                err.to_string()
                    .contains("api model=claude-fable-5.1[1m] is blocked by this bridge"),
                "{err}"
            ),
            Ok(_) => panic!("blocked API model must fail before creating a stream"),
        }
    }

    #[tokio::test]
    async fn with_policy_swaps_engine() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"))
            .with_policy(Arc::new(DenyAll));
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&be);
    }
}
