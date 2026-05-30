// ports.rs — port traits for bridge-core (spec §4.2).
// All traits live here so adapter crates depend on core, never the reverse.

use crate::{domain::*, error::BridgeError, ids::*};
use futures::Stream;
use std::pin::Pin;

/// Streaming update from an agent backend.
#[derive(Debug)]
pub enum Update {
    Text(String),
    Permission(PermissionRequest),
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

/// Session store — persists task→session mappings and pending-request state.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn put(&self, task: &TaskId, session: &SessionId) -> Result<(), BridgeError>;
    async fn session_for(&self, task: &TaskId) -> Result<Option<SessionId>, BridgeError>;
    async fn put_pending(&self, task: &TaskId, req: &PendingRequest) -> Result<(), BridgeError>;
    async fn take_pending(&self, task: &TaskId) -> Result<Option<PendingRequest>, BridgeError>;
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
    }
    impl FakeStore {
        fn new() -> Self {
            Self {
                inner: Default::default(),
                pending: Default::default(),
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
}
