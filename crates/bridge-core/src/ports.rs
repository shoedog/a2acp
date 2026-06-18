// ports.rs — port traits for bridge-core (spec §4.2).
// All traits live here so adapter crates depend on core, never the reverse.

use crate::{domain::*, error::BridgeError, ids::*};
use futures::Stream;
use std::pin::Pin;

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

/// Streaming agent backend — adapters implement this; core never depends on adapters.
#[async_trait::async_trait]
pub trait AgentBackend: Send + Sync {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError>;
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError>;

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
    /// Release a warm session: drop ALL per-session backend state + reap any per-session
    /// resource (e.g. a `:rw` container). Default = `forget_session` (correct for
    /// non-warm/non-process backends). Warm backends override. [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        self.forget_session(session).await;
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
pub trait PolicyEngine: Send + Sync {
    fn decide(
        &self,
        req: &PermissionRequest,
        ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError>;
}

/// Sync auth middleware — validates an inbound request.
pub trait AuthMiddleware: Send + Sync {
    fn authorize(&self, req: &InboundRequest) -> Result<AuthContext, BridgeError>;
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
    /// Return the default agent id for this registry.
    fn default_id(&self) -> crate::ids::AgentId;
    /// Atomically reconcile the registry to the given snapshot. [§4.5]
    async fn apply(&self, snapshot: RegistrySnapshot) -> Result<(), BridgeError>;
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
        let _ = f
            .reconcile_config(
                &crate::ids::SessionId::parse("s").unwrap(),
                &crate::domain::SessionSpec::from_config(Default::default()),
            )
            .await;
        let _ = f.capabilities();
        f.retire().await.unwrap();
        let _obj: std::sync::Arc<dyn AgentBackend> = std::sync::Arc::new(Fake); // object-safe
    }
}
