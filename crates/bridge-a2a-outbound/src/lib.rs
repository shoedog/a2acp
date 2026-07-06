//! bridge-a2a-outbound — outbound A2A DelegationPort (Increment 2.5).

pub mod client;
pub mod sse;

pub use client::{A2aClient, ClientError, SendOpts, StreamingReply, TaskIdMode};
pub use sse::{sse_events, SseError, SseEvent, SseStream};

#[cfg(test)]
pub(crate) mod testpeer;

use bridge_core::domain::{AuthContext, Part, PeerTaskId};
use bridge_core::error::BridgeError;
use bridge_core::ids::TaskId;
use bridge_core::ports::{Delegation, DelegationPort};

/// Real outbound A2A delegation: opens an SSE stream to a remote peer
/// and returns `Delegation{events, peer_task}`. Implements `DelegationPort`.
///
/// `_auth` and `_local` are accepted for interface compatibility but are not
/// forwarded in this passthrough implementation (v1 caller-identity forwarding
/// is deferred to a later increment, per spec §3.1.1 note).
pub struct PeerDelegation {
    url: String,
    auth: String,
    timeout: std::time::Duration,
}

impl PeerDelegation {
    /// Construct a `PeerDelegation` that will talk to the A2A peer at `url`
    /// using `auth` (prefix `"bearer:TOKEN"`) and `timeout` per request.
    pub fn new(url: &str, auth: &str, timeout: std::time::Duration) -> Self {
        Self {
            url: url.into(),
            auth: auth.into(),
            timeout,
        }
    }
}

#[async_trait::async_trait]
impl DelegationPort for PeerDelegation {
    async fn delegate(
        &self,
        _auth: &AuthContext,
        _local: &TaskId,
        parts: Vec<Part>,
    ) -> Result<Delegation, BridgeError> {
        let (events, peer_task) = A2aClient::new(&self.url, &self.auth, self.timeout)
            .open_stream(&parts)
            .await?;
        Ok(Delegation { events, peer_task })
    }

    async fn cancel(&self, peer_task: &PeerTaskId) -> Result<(), BridgeError> {
        A2aClient::new(&self.url, &self.auth, self.timeout)
            .cancel(&peer_task.0)
            .await
    }
}

/// No-op stub used when no `[delegation]` config is present (Task 10 fallback).
///
/// `delegate` always returns `UpstreamA2aError`; `cancel` is a no-op.
pub struct StubDelegation;

#[async_trait::async_trait]
impl DelegationPort for StubDelegation {
    async fn delegate(
        &self,
        _auth: &bridge_core::domain::AuthContext,
        _local_task: &TaskId,
        _parts: Vec<Part>,
    ) -> Result<Delegation, BridgeError> {
        Err(BridgeError::UpstreamA2aError)
    }
    async fn cancel(&self, _peer_task: &PeerTaskId) -> Result<(), BridgeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{AuthContext, Part, PeerTaskId};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{CallerId, TaskId};
    use bridge_core::ports::DelegationPort;
    use futures::StreamExt;

    fn auth() -> AuthContext {
        AuthContext::new(CallerId::parse("anonymous").unwrap())
    }

    #[tokio::test]
    async fn stub_reports_unsupported_delegation() {
        let auth = AuthContext::new(CallerId::parse("anon").unwrap());
        let task = TaskId::parse("t1").unwrap();
        let result = StubDelegation
            .delegate(&auth, &task, vec![Part::default()])
            .await;
        assert!(matches!(result, Err(BridgeError::UpstreamA2aError)));
    }

    #[tokio::test]
    async fn stub_cancel_returns_ok() {
        assert!(StubDelegation
            .cancel(&PeerTaskId("p1".into()))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn delegate_streams_and_exposes_peer_id() {
        let (url, _p) =
            crate::testpeer::MockPeer::start(crate::testpeer::script_status_then_final("R")).await;
        let pd = PeerDelegation::new(&url, "bearer:T", std::time::Duration::from_secs(30));
        let mut d = pd
            .delegate(
                &auth(),
                &TaskId::parse("t").unwrap(),
                vec![Part { text: "x".into() }],
            )
            .await
            .unwrap();
        while d.events.next().await.is_some() {}
        assert!(d.peer_task.borrow().is_some(), "peer task id captured");
    }

    #[tokio::test]
    async fn cancel_posts_cancel_task_to_peer() {
        let (url, peer) =
            crate::testpeer::MockPeer::start(crate::testpeer::script_status_then_final("R")).await;
        let pd = PeerDelegation::new(&url, "bearer:T", std::time::Duration::from_secs(30));
        pd.cancel(&PeerTaskId("peer-123".into())).await.unwrap();
        assert!(
            peer.received_cancel_for("peer-123"),
            "peer should have received CancelTask for peer-123"
        );
    }
}
