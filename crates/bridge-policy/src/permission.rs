// permission.rs — AutoPolicy: approves non-interactive, denies interactive.

use bridge_core::domain::{PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ports::PolicyEngine;

pub struct AutoPolicy;

impl PolicyEngine for AutoPolicy {
    fn decide(
        &self,
        req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        if req.interactive {
            Err(BridgeError::PermissionDenied)
        } else {
            Ok(PermissionDecision::Approve)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{PermissionDecision, PermissionRequest, SessionContext};
    use bridge_core::error::BridgeError;
    use bridge_core::ports::PolicyEngine;

    #[test]
    fn approves_non_interactive() {
        assert_eq!(
            AutoPolicy
                .decide(&PermissionRequest::read(), &SessionContext::test())
                .unwrap(),
            PermissionDecision::Approve,
        );
    }

    #[test]
    fn denies_interactive() {
        assert_eq!(
            AutoPolicy
                .decide(&PermissionRequest::interactive(), &SessionContext::test())
                .unwrap_err(),
            BridgeError::PermissionDenied,
        );
    }
}
