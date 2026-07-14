// ports.rs — port traits for bridge-core (spec §4.2).
// All traits live here so adapter crates depend on core, never the reverse.

use crate::{domain::*, error::BridgeError, ids::*};
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;

use crate::orch::UsageSnapshot;

// Bring new domain types into scope for the registry/config-source traits.
use crate::domain::{AgentEntry, RegistrySnapshot};

/// Wire spelling of the ACP `StopReason::Cancelled` stop-reason string.
///
/// Both the ACP adapter (`bridge-acp`) and the domain translator (`bridge-core`)
/// must agree on this one value: the adapter emits it into `Update::Done`; the
/// translator matches it to drive `TaskOutcome::Canceled`. A single const here
/// is the ONE source of truth — drift between producer and consumer is impossible.
pub const STOP_REASON_CANCELLED: &str = "cancelled";

/// Streaming update from an agent backend.
#[derive(Debug)]
pub enum Update {
    Text(String),
    Permission(PermissionRequest),
    Usage(crate::orch::UsageSnapshot),
    Done { stop_reason: String },
}

/// A pinned, boxed stream of `Result<Update, BridgeError>` items. Send-safe.
pub type BackendStream = Pin<Box<dyn Stream<Item = Result<Update, BridgeError>> + Send>>;

#[async_trait::async_trait]
pub trait RichEventSink: Send + Sync {
    fn record(&self, kind: crate::orch::OrchEventKind);
    async fn flush(&self) -> Result<(), BridgeError>;
}

pub trait RichEventSinkFactory: Send + Sync {
    fn make(&self, node: &NodeId) -> std::sync::Arc<dyn RichEventSink>;
}

/// Operation-scoped lifecycle diagnostic sink.
///
/// Implementations validate transition grammar before accepting an event. A
/// durable implementation returns persistence failures to the lifecycle owner;
/// callers must not turn them into best-effort logging.
#[async_trait::async_trait]
pub trait DiagnosticObserver: Send + Sync {
    async fn record(&self, event: crate::diagnostics::DiagnosticEvent) -> Result<(), BridgeError>;
}

/// Explicit per-node/attempt diagnostic ownership for workflow execution.
/// Correlation ids and rich-event availability never select journal authority.
pub trait DiagnosticObserverFactory: Send + Sync {
    fn make(&self, node: &NodeId, attempt: u32) -> std::sync::Arc<dyn DiagnosticObserver>;
}

/// Composite prompt observers. This keeps the existing rich-event API intact
/// while allowing adapters to opt into lifecycle diagnostics independently.
#[derive(Clone)]
pub struct BackendObservers {
    pub diagnostic: std::sync::Arc<dyn DiagnosticObserver>,
    pub rich: Option<std::sync::Arc<dyn RichEventSink>>,
}

impl BackendObservers {
    pub fn new(
        diagnostic: std::sync::Arc<dyn DiagnosticObserver>,
        rich: Option<std::sync::Arc<dyn RichEventSink>>,
    ) -> Self {
        Self { diagnostic, rich }
    }

    pub fn diagnostic_only(diagnostic: std::sync::Arc<dyn DiagnosticObserver>) -> Self {
        Self::new(diagnostic, None)
    }
}

/// Streaming agent backend — adapters implement this; core never depends on adapters.
#[async_trait::async_trait]
pub trait AgentBackend: Send + Sync {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError>;
    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        _sink: std::sync::Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt(session, parts).await
    }
    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        match observers.rich {
            Some(sink) => self.prompt_observed(session, parts, sink).await,
            None => self.prompt(session, parts).await,
        }
    }
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError>;
    async fn cancel_observed(
        &self,
        session: &SessionId,
        _observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.cancel(session).await
    }

    /// Stash per-turn metadata for the NEXT prompt on this session (Slice 9 — lets the reverse permission
    /// handler build a gen-stamped key). Default: no-op. The producer calls this immediately before `prompt`.
    async fn configure_turn(&self, _session: &SessionId, _meta: crate::permission::TurnMeta) {}

    /// Stash the per-session spec (config + cwd); applied at lazy ACP mint. Default: no-op. [§4.4]
    async fn configure_session(
        &self,
        _session: &SessionId,
        _spec: &crate::domain::SessionSpec,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
    /// Drop per-session state (config stash, etc.) when a task/session ends. Default: no-op.
    /// MUST be a trait method — the inbound binding eviction (T11) calls it through `Arc<dyn AgentBackend>`.
    async fn forget_session(&self, _session: &SessionId) {}
    /// Result-bearing, observer-free cleanup used by detached cleanup ownership.
    /// Implementors with fallible teardown override this method; the default
    /// preserves source and behavior compatibility with legacy backends.
    async fn forget_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.forget_session(session).await;
        Ok(())
    }
    async fn forget_session_observed(
        &self,
        session: &SessionId,
        _observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.forget_session_checked(session).await
    }
    /// Release a warm session: drop ALL per-session backend state + reap any per-session
    /// resource (e.g. a `:rw` container). Default = `forget_session` (correct for
    /// non-warm/non-process backends). Warm backends override. [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        self.forget_session(session).await;
    }
    /// Result-bearing, observer-free warm release. A cleanup flight owns this
    /// call, while its optional diagnostic waiter retains the operation observer
    /// and records the bounded result after joining.
    async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.release_session(session).await;
        Ok(())
    }
    async fn release_session_observed(
        &self,
        session: &SessionId,
        _observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.release_session_checked(session).await
    }
    /// Reconcile model/effort on a LIVE warm session (Slice 1). Default: NotAdvertised
    /// (non-ACP/non-process backends can't reconcile a live session). cwd/mode are NOT
    /// reconciled here (the caller routes those). [Slice 1]
    async fn reconcile_config(
        &self,
        _session: &SessionId,
        _spec: &crate::domain::SessionSpec,
    ) -> Result<crate::orch::ReconcileOutcome, BridgeError> {
        Ok(crate::orch::ReconcileOutcome::NotAdvertised)
    }
    /// Agent session-lifecycle capabilities (initialize-time). Default: empty. [Slice 1]
    fn capabilities(&self) -> crate::orch::AgentSessionCaps {
        crate::orch::AgentSessionCaps::default()
    }
    /// Graceful async teardown (the registry drains leases before calling this). Default: no-op. [§5.4]
    async fn retire(&self) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Inbound transport abstraction (e.g. A2A JSON-RPC over WebSocket).
#[async_trait::async_trait]
pub trait InboundTransport: Send + Sync {}

/// A pinned, boxed stream of `Result<Event, BridgeError>` items.
pub type DelegationStream =
    Pin<Box<dyn futures::Stream<Item = Result<crate::translator::Event, BridgeError>> + Send>>;

/// The result of delegating: a stream of events plus a watch channel for the peer task id.
pub struct Delegation {
    pub events: DelegationStream,
    pub peer_task: tokio::sync::watch::Receiver<Option<PeerTaskId>>,
}

/// Delegation port — streams tasks to a downstream agent.
#[async_trait::async_trait]
pub trait DelegationPort: Send + Sync {
    async fn delegate(
        &self,
        auth: &AuthContext,
        local_task: &TaskId,
        parts: Vec<Part>,
    ) -> Result<Delegation, BridgeError>;
    async fn cancel(&self, peer_task: &PeerTaskId) -> Result<(), BridgeError>;
}

/// Session store — persists task→session mappings, pending-request state,
/// delegated peer-task ids, and the early-cancel latch.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn put(&self, task: &TaskId, session: &SessionId) -> Result<(), BridgeError>;
    async fn session_for(&self, task: &TaskId) -> Result<Option<SessionId>, BridgeError>;
    async fn put_pending(&self, task: &TaskId, req: &PendingRequest) -> Result<(), BridgeError>;
    async fn take_pending(&self, task: &TaskId) -> Result<Option<PendingRequest>, BridgeError>;

    /// Persist the downstream peer-task id assigned during delegation.
    async fn set_peer_task(&self, task: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError>;
    /// Retrieve the peer-task id, if any.
    async fn peer_task_for(&self, task: &TaskId) -> Result<Option<PeerTaskId>, BridgeError>;
    /// Latch the early-cancel flag for this task (idempotent).
    async fn request_cancel(&self, task: &TaskId) -> Result<(), BridgeError>;
    /// Returns `true` if `request_cancel` has been called for this task.
    async fn cancel_requested(&self, task: &TaskId) -> Result<bool, BridgeError>;
    /// Mark this task as a fan-out task (idempotent). Required to distinguish
    /// fan-out tasks from plain delegate tasks (which also have a peer id).
    async fn set_fanout(&self, task: &TaskId) -> Result<(), BridgeError>;
    /// Returns `true` if `set_fanout` has been called for this task.
    async fn is_fanout(&self, task: &TaskId) -> Result<bool, BridgeError>;
}

/// Sync routing decision — no async needed; plain fn.
pub trait RouteDecision: Send + Sync {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError>;
}

/// Sync policy engine — evaluates a permission request against session context.
pub enum PolicyOutcome {
    Decide(Result<PermissionDecision, BridgeError>),
    Defer,
}

pub trait PolicyEngine: Send + Sync {
    fn decide(
        &self,
        req: &PermissionRequest,
        ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError>;

    fn interactive_decide(&self, req: &PermissionRequest, ctx: &SessionContext) -> PolicyOutcome {
        PolicyOutcome::Decide(self.decide(req, ctx))
    }
}

/// Sync auth middleware — validates an inbound request.
pub trait AuthMiddleware: Send + Sync {
    fn authorize(&self, req: &InboundRequest) -> Result<AuthContext, BridgeError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceParent {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub flags: u8,
}

impl TraceParent {
    pub fn parse_header_value(raw: &str) -> Option<Self> {
        let mut parts = raw.split('-');
        let version = parts.next()?;
        let trace = parts.next()?;
        let span = parts.next()?;
        let flags = parts.next()?;
        let is_lower_hex = |part: &str| {
            part.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        if parts.next().is_some()
            || version != "00"
            || trace.len() != 32
            || span.len() != 16
            || flags.len() != 2
            || !is_lower_hex(trace)
            || !is_lower_hex(span)
            || !is_lower_hex(flags)
        {
            return None;
        }
        let mut trace_id = [0_u8; 16];
        let mut span_id = [0_u8; 8];
        for i in 0..16 {
            trace_id[i] = u8::from_str_radix(&trace[i * 2..i * 2 + 2], 16).ok()?;
        }
        for i in 0..8 {
            span_id[i] = u8::from_str_radix(&span[i * 2..i * 2 + 2], 16).ok()?;
        }
        if trace_id.iter().all(|b| *b == 0) || span_id.iter().all(|b| *b == 0) {
            return None;
        }
        Some(Self {
            trace_id,
            span_id,
            flags: u8::from_str_radix(flags, 16).ok()?,
        })
    }

    pub fn to_header_value(&self) -> String {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        }
        format!(
            "00-{}-{}-{:02x}",
            hex(&self.trace_id),
            hex(&self.span_id),
            self.flags
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnContext {
    pub turn_id: TurnId,
    pub session_id: ContextId,
    pub task_id: Option<TaskId>,
    pub workflow: Option<String>,
    pub node: Option<String>,
    pub attempt: u32,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub prompt_id: Option<String>,
    pub traceparent: Option<TraceParent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    AgentCrashed,
    TimedOut,
    Overloaded,
    Config,
    Transport,
    Other,
}

/// Classify a `BridgeError` into a `FailureClass` for observability metrics.
pub fn classify_failure(e: &BridgeError) -> FailureClass {
    match e {
        BridgeError::AgentCrashed { .. } => FailureClass::AgentCrashed,
        BridgeError::AgentFailure { diagnostic } => match diagnostic.class() {
            crate::diagnostics::DiagnosticFailureClass::Config
            | crate::diagnostics::DiagnosticFailureClass::Authentication
            | crate::diagnostics::DiagnosticFailureClass::Model => FailureClass::Config,
            crate::diagnostics::DiagnosticFailureClass::Protocol
            | crate::diagnostics::DiagnosticFailureClass::Transport => FailureClass::Transport,
            crate::diagnostics::DiagnosticFailureClass::AgentProcess
            | crate::diagnostics::DiagnosticFailureClass::ContainerRuntime
            | crate::diagnostics::DiagnosticFailureClass::ContainerImage
            | crate::diagnostics::DiagnosticFailureClass::ContainerNetwork
            | crate::diagnostics::DiagnosticFailureClass::ContainerMount
            | crate::diagnostics::DiagnosticFailureClass::ContainerCredentials => {
                FailureClass::AgentCrashed
            }
            crate::diagnostics::DiagnosticFailureClass::Timeout => FailureClass::TimedOut,
            crate::diagnostics::DiagnosticFailureClass::Overloaded
            | crate::diagnostics::DiagnosticFailureClass::ProviderLimit => FailureClass::Overloaded,
            crate::diagnostics::DiagnosticFailureClass::Persistence
            | crate::diagnostics::DiagnosticFailureClass::Canceled
            | crate::diagnostics::DiagnosticFailureClass::Unknown => FailureClass::Other,
        },
        BridgeError::AgentTimedOut | BridgeError::CancelTimeout => FailureClass::TimedOut,
        BridgeError::AgentOverloaded => FailureClass::Overloaded,
        BridgeError::ConfigMismatch { .. }
        | BridgeError::ConfigReseedRequired { .. }
        | BridgeError::ConfigInvalid { .. }
        | BridgeError::UnknownAgent { .. }
        | BridgeError::ModelNotAvailable => FailureClass::Config,
        BridgeError::FrameError | BridgeError::UpstreamA2aError => FailureClass::Transport,
        _ => FailureClass::Other,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Success,
    Failed(FailureClass),
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageFinalization {
    TurnFinal,
    TaskFinal,
    Partial,
}

#[derive(Debug)]
pub enum ObsEvent<'a> {
    TaskStarted {
        ctx: &'a TurnContext,
    },
    TaskFinished {
        ctx: &'a TurnContext,
        outcome: &'a TurnOutcome,
    },
    NodeStarted {
        ctx: &'a TurnContext,
    },
    NodeFinished {
        ctx: &'a TurnContext,
        outcome: &'a TurnOutcome,
    },
    TurnStarted {
        ctx: &'a TurnContext,
    },
    TurnFinished {
        ctx: &'a TurnContext,
        latency: Duration,
        ttft: Option<Duration>,
        outcome: &'a TurnOutcome,
    },
    QueueChanged {
        in_flight: u64,
        queued: u64,
        wait: Option<Duration>,
    },
    UsageFinalized {
        ctx: &'a TurnContext,
        usage: Option<&'a UsageSnapshot>,
        fin: UsageFinalization,
    },
}

pub trait Observer: Send + Sync {
    fn record(&self, e: &ObsEvent<'_>);
}

// ─── Registry / config-source ports (Increment 3b §4.5) ──────────────────────

/// Lease guard: while held, a registry slot's active-task count is incremented; decremented on drop. [§4.5]
pub trait Lease: Send + Sync {
    /// True once the slot this lease belongs to has been retired/replaced (config reload).
    /// A warm SessionManager checks this to expire a handle. Default `false`. [Slice 0]
    fn is_retired(&self) -> bool {
        false
    }
}

/// Result of resolving an agent: its entry config, the (lazily-spawned) backend, and a lease keeping the slot alive.
pub struct Resolved {
    pub entry: std::sync::Arc<AgentEntry>,
    pub backend: std::sync::Arc<dyn AgentBackend>,
    pub lease: Box<dyn Lease>,
}

#[async_trait::async_trait]
pub trait AgentRegistry: Send + Sync {
    /// Resolve (and lazily spawn) a backend for the given agent id. [§4.5]
    async fn resolve(&self, id: &crate::ids::AgentId) -> Result<Resolved, BridgeError>;
    /// Resolve with operation-scoped lifecycle observation. Existing registries
    /// remain source-compatible and ignore diagnostics through this default.
    async fn resolve_observed(
        &self,
        id: &crate::ids::AgentId,
        _observer: std::sync::Arc<dyn DiagnosticObserver>,
    ) -> Result<Resolved, BridgeError> {
        self.resolve(id).await
    }
    /// Return the default agent id for this registry.
    fn default_id(&self) -> crate::ids::AgentId;
    /// Atomically reconcile the registry to the given snapshot. [§4.5]
    async fn apply(&self, snapshot: RegistrySnapshot) -> Result<(), BridgeError>;
    /// Drop the cached backend for `agent` so the next `resolve` RESPAWNS a fresh process (E6 retry
    /// reset). Best-effort + idempotent; unknown agent ⇒ no-op. Default: no-op (non-spawning registries).
    async fn invalidate(&self, _agent: &crate::ids::AgentId) {}
    /// List all registered agent ids.
    fn list(&self) -> Vec<crate::ids::AgentId>;
    /// Per-agent MCP server names, for agent-card advertisement (ADR-0028) — `(agent_id, [server
    /// names])` for each agent that has any `[[agents.mcp]]`. Read-only over the config (no spawn).
    /// Defaults to empty (test/mocked registries advertise nothing).
    fn mcp_advertisement(&self) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }
}

#[async_trait::async_trait]
pub trait ConfigSource: Send + Sync {
    /// Load the current registry snapshot from the config source.
    async fn load(&self) -> Result<RegistrySnapshot, BridgeError>;
    /// Return a stream of snapshots that fires whenever the source changes.
    fn watch(&self) -> futures::stream::BoxStream<'static, RegistrySnapshot>;
}

#[async_trait::async_trait]
pub trait ConfigStore: ConfigSource {
    /// Upsert an agent entry (3b.2+ admin API write-back — defined now, impl'd later).
    async fn upsert(&self, entry: AgentEntry) -> Result<(), BridgeError>;
    /// Remove an agent entry by id.
    async fn remove(&self, id: &crate::ids::AgentId) -> Result<(), BridgeError>;
}

#[cfg(test)]
mod v25rt {
    use super::*;
    use crate::ids::AgentId;
    #[test]
    fn route_target_local() {
        let r = RouteTarget::Local(AgentId::parse("kiro").unwrap());
        assert!(matches!(r, RouteTarget::Local(a) if a.as_str() == "kiro"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::BridgeError;
    use futures::StreamExt;

    #[test]
    fn default_policy_engine_never_defers() {
        struct OldStyle;
        impl PolicyEngine for OldStyle {
            fn decide(
                &self,
                _: &PermissionRequest,
                _: &SessionContext,
            ) -> Result<PermissionDecision, BridgeError> {
                Ok(PermissionDecision::Approve)
            }
        }
        let out =
            OldStyle.interactive_decide(&PermissionRequest::with_id("r", false), &SessionContext);
        assert!(matches!(
            out,
            PolicyOutcome::Decide(Ok(PermissionDecision::Approve))
        ));
    }

    struct FakeStore {
        inner: std::sync::Mutex<std::collections::HashMap<String, String>>,
        pending: std::sync::Mutex<std::collections::HashMap<String, PendingRequest>>,
        peer_tasks: std::sync::Mutex<std::collections::HashMap<String, PeerTaskId>>,
        cancels: std::sync::Mutex<std::collections::HashSet<String>>,
        fanouts: std::sync::Mutex<std::collections::HashSet<String>>,
    }
    impl FakeStore {
        fn new() -> Self {
            Self {
                inner: Default::default(),
                pending: Default::default(),
                peer_tasks: Default::default(),
                cancels: Default::default(),
                fanouts: Default::default(),
            }
        }
    }
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, t: &TaskId, s: &SessionId) -> Result<(), BridgeError> {
            self.inner
                .lock()
                .unwrap()
                .insert(t.as_str().into(), s.as_str().into());
            Ok(())
        }
        async fn session_for(&self, t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .get(t.as_str())
                .map(|s| SessionId::parse(s.clone()).unwrap()))
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
        async fn set_peer_task(&self, t: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
            self.peer_tasks
                .lock()
                .unwrap()
                .insert(t.as_str().into(), peer.clone());
            Ok(())
        }
        async fn peer_task_for(&self, t: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
            Ok(self.peer_tasks.lock().unwrap().get(t.as_str()).cloned())
        }
        async fn request_cancel(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn cancel_requested(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.cancels.lock().unwrap().contains(t.as_str()))
        }
        async fn set_fanout(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.fanouts.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn is_fanout(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.fanouts.lock().unwrap().contains(t.as_str()))
        }
    }

    struct FakeBackend;
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text("hi".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingSink {
        records: std::sync::atomic::AtomicUsize,
    }

    impl CountingSink {
        fn count(&self) -> usize {
            self.records.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl RichEventSink for CountingSink {
        fn record(&self, _kind: crate::orch::OrchEventKind) {
            self.records
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct AlwaysKiro;
    impl RouteDecision for AlwaysKiro {
        fn route(&self, _t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Local(AgentId::parse("kiro")?))
        }
    }

    #[tokio::test]
    async fn backend_streams_text_then_done() {
        let mut s = FakeBackend
            .prompt(&SessionId::parse("s").unwrap(), vec![])
            .await
            .unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "hi"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn prompt_observed_defaults_to_prompt() {
        let backend = FakeBackend;
        let sink = std::sync::Arc::new(CountingSink::default());
        let dyn_sink: std::sync::Arc<dyn RichEventSink> = sink.clone();

        let mut stream = backend
            .prompt_observed(&SessionId::parse("s").unwrap(), vec![], dyn_sink)
            .await
            .unwrap();

        assert!(matches!(
            stream.next().await,
            Some(Ok(Update::Text(text))) if text == "hi"
        ));
        assert!(matches!(
            stream.next().await,
            Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn"
        ));
        assert!(stream.next().await.is_none());
        assert_eq!(sink.count(), 0);
    }

    #[tokio::test]
    async fn store_put_and_session_for_roundtrip() {
        let st = FakeStore::new();
        let t = TaskId::parse("t-sess").unwrap();
        let s = SessionId::parse("s-abc").unwrap();
        st.put(&t, &s).await.unwrap();
        let found = st.session_for(&t).await.unwrap();
        assert_eq!(found.unwrap().as_str(), "s-abc");
    }

    #[tokio::test]
    async fn backend_cancel_returns_ok() {
        FakeBackend
            .cancel(&SessionId::parse("s").unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn store_pending_roundtrips_and_clears() {
        let st = FakeStore::new();
        let t = TaskId::parse("t").unwrap();
        st.put_pending(
            &t,
            &PendingRequest {
                request_id: "r1".into(),
                kind: PendingKind::Permission,
            },
        )
        .await
        .unwrap();
        assert_eq!(st.take_pending(&t).await.unwrap().unwrap().request_id, "r1");
        assert!(st.take_pending(&t).await.unwrap().is_none());
    }

    #[test]
    fn route_decision_is_sync_and_routes_to_kiro() {
        let r = AlwaysKiro.route(&TaskMeta::default()).unwrap();
        assert!(matches!(r, RouteTarget::Local(a) if a.as_str() == "kiro"));
    }

    #[test]
    fn domain_constructors_exist() {
        let _ = PermissionRequest::read();
        let _ = PermissionRequest::interactive();
        let _ = SessionContext::test();
        let _ = InboundRequest::anon();
        let _ = InboundRequest::with_token("tok");
    }

    #[tokio::test]
    async fn store_fanout_marker_roundtrips() {
        let st = FakeStore::new();
        let t = TaskId::parse("t-fanout").unwrap();
        assert!(!st.is_fanout(&t).await.unwrap());
        st.set_fanout(&t).await.unwrap();
        assert!(st.is_fanout(&t).await.unwrap());
        // Idempotent: setting again must not fail.
        st.set_fanout(&t).await.unwrap();
        assert!(st.is_fanout(&t).await.unwrap());
    }

    #[tokio::test]
    async fn agentbackend_defaults_are_noops_and_object_safe() {
        struct Fake;
        #[async_trait::async_trait]
        impl AgentBackend for Fake {
            async fn prompt(
                &self,
                _: &crate::ids::SessionId,
                _: Vec<crate::domain::Part>,
            ) -> Result<BackendStream, BridgeError> {
                unreachable!()
            }
            async fn cancel(&self, _: &crate::ids::SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let f = Fake;
        f.configure_session(
            &crate::ids::SessionId::parse("s").unwrap(),
            &crate::domain::SessionSpec::from_config(crate::domain::EffectiveConfig::default()),
        )
        .await
        .unwrap();
        f.forget_session(&crate::ids::SessionId::parse("s").unwrap())
            .await;
        f.release_session(&crate::ids::SessionId::parse("s").unwrap())
            .await;
        assert_eq!(
            f.reconcile_config(
                &crate::ids::SessionId::parse("s").unwrap(),
                &crate::domain::SessionSpec::from_config(Default::default()),
            )
            .await
            .unwrap(),
            crate::orch::ReconcileOutcome::NotAdvertised,
        );
        assert_eq!(f.capabilities(), crate::orch::AgentSessionCaps::default());
        f.retire().await.unwrap();
        let _obj: std::sync::Arc<dyn AgentBackend> = std::sync::Arc::new(Fake); // object-safe
    }
}
