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

/// Interactive policy: defers every permission to the operator. `decide` keeps a
/// safe approve fallback for any non-interactive caller.
pub struct DeferPolicy;

impl PolicyEngine for DeferPolicy {
    fn decide(
        &self,
        _req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }

    fn interactive_decide(
        &self,
        _req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> bridge_core::ports::PolicyOutcome {
        bridge_core::ports::PolicyOutcome::Defer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{PermissionDecision, PermissionRequest, SessionContext};
    use bridge_core::error::BridgeError;
    use bridge_core::ports::{PolicyEngine, PolicyOutcome};

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

    #[test]
    fn defer_policy_interactive_decide_defers() {
        assert!(matches!(
            DeferPolicy
                .interactive_decide(&PermissionRequest::interactive(), &SessionContext::test()),
            PolicyOutcome::Defer
        ));
    }
}
