//! bridge-a2a-outbound — outbound A2A DelegationPort (v1 stub; concrete impl is Increment 2.5).
use bridge_core::domain::TaskMeta;
use bridge_core::error::BridgeError;
use bridge_core::ports::DelegationPort;

/// v1 stub. Not reachable on the inbound happy path (RouteDecision routes to Kiro).
/// Increment 2.5 replaces this with a real remote-A2A-peer client + SSE stream-merge.
pub struct StubDelegation;

#[async_trait::async_trait]
impl DelegationPort for StubDelegation {
    async fn delegate(&self, _meta: &TaskMeta) -> Result<(), BridgeError> {
        Err(BridgeError::UpstreamA2aError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::TaskMeta;
    use bridge_core::error::BridgeError;
    use bridge_core::ports::DelegationPort;
    #[tokio::test]
    async fn stub_reports_unsupported_delegation() {
        assert_eq!(
            StubDelegation.delegate(&TaskMeta).await.unwrap_err(),
            BridgeError::UpstreamA2aError
        );
    }
}
