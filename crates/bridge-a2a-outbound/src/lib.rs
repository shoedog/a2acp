//! bridge-a2a-outbound — outbound A2A DelegationPort (v1 stub; concrete impl is Increment 2.5).
use bridge_core::domain::{Part, PeerTaskId};
use bridge_core::error::BridgeError;
use bridge_core::ids::TaskId;
use bridge_core::ports::{Delegation, DelegationPort};

/// v1 stub. Not reachable on the inbound happy path (RouteDecision routes to Kiro).
/// Increment 2.5 replaces this with a real remote-A2A-peer client + SSE stream-merge.
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
}
