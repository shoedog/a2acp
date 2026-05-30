// error.rs — BridgeError with reject-vs-set-state disposition (§6 of design spec).
// A2aState is our *internal* domain enum; we map it to the SDK's a2a::TaskState at the wire
// edge in bridge-a2a-inbound. Do NOT import the a2a SDK here.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2aState {
    Submitted,
    Working,
    InputRequired,
    AuthRequired,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2aDisposition {
    RejectRequest,
    SetState(A2aState),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum BridgeError {
    #[error("a2a version mismatch")]
    A2aVersionMismatch,
    #[error("invalid request: {field}")]
    InvalidRequest { field: &'static str },
    #[error("task not found")]
    TaskNotFound,
    #[error("session not found")]
    SessionNotFound,
    #[error("auth required")]
    AuthRequired { request_id: String },
    #[error("permission required")]
    PermissionRequired { request_id: String },
    #[error("permission denied")]
    PermissionDenied,
    #[error("agent not authenticated")]
    AgentNotAuthenticated,
    #[error("model not available")]
    ModelNotAvailable,
    #[error("cancel timeout")]
    CancelTimeout,
    #[error("frame error")]
    FrameError,
    #[error("message too large")]
    MessageTooLarge,
    #[error("agent crashed")]
    AgentCrashed,
    #[error("upstream a2a error")]
    UpstreamA2aError,
    #[error("store failure")]
    StoreFailure,
    #[error("invalid state transition")]
    InvalidStateTransition,
}

impl BridgeError {
    pub fn disposition(&self) -> A2aDisposition {
        use A2aDisposition::*;
        use A2aState as S;
        use BridgeError::*;
        match self {
            A2aVersionMismatch | InvalidRequest { .. } | TaskNotFound | SessionNotFound => {
                RejectRequest
            }
            AuthRequired { .. } | AgentNotAuthenticated => SetState(S::AuthRequired),
            PermissionRequired { .. } => SetState(S::InputRequired),
            CancelTimeout => SetState(S::Canceled),
            _ => SetState(S::Failed),
        }
    }

    pub fn is_resumable(&self) -> bool {
        matches!(
            self,
            BridgeError::AuthRequired { .. } | BridgeError::PermissionRequired { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_errors_reject_not_fail_task() {
        assert_eq!(
            BridgeError::A2aVersionMismatch.disposition(),
            A2aDisposition::RejectRequest
        );
        assert_eq!(
            BridgeError::TaskNotFound.disposition(),
            A2aDisposition::RejectRequest
        );
        assert_eq!(
            BridgeError::InvalidRequest { field: "x" }.disposition(),
            A2aDisposition::RejectRequest
        );
        assert_eq!(
            BridgeError::SessionNotFound.disposition(),
            A2aDisposition::RejectRequest
        );
    }

    #[test]
    fn suspends_are_resumable_states() {
        assert_eq!(
            BridgeError::PermissionRequired {
                request_id: "r".into()
            }
            .disposition(),
            A2aDisposition::SetState(A2aState::InputRequired)
        );
        assert_eq!(
            BridgeError::AuthRequired {
                request_id: "r".into()
            }
            .disposition(),
            A2aDisposition::SetState(A2aState::AuthRequired)
        );
        assert!(BridgeError::AuthRequired {
            request_id: "r".into()
        }
        .is_resumable());
        assert!(BridgeError::PermissionRequired {
            request_id: "r".into()
        }
        .is_resumable());
        assert!(!BridgeError::FrameError.is_resumable());
    }

    #[test]
    fn runtime_failures_set_failed_state() {
        for e in [
            BridgeError::FrameError,
            BridgeError::AgentCrashed,
            BridgeError::ModelNotAvailable,
            BridgeError::PermissionDenied,
            BridgeError::MessageTooLarge,
            BridgeError::UpstreamA2aError,
            BridgeError::StoreFailure,
            BridgeError::InvalidStateTransition,
        ] {
            assert_eq!(e.disposition(), A2aDisposition::SetState(A2aState::Failed));
        }
    }

    #[test]
    fn agent_not_authenticated_maps_auth_required() {
        assert_eq!(
            BridgeError::AgentNotAuthenticated.disposition(),
            A2aDisposition::SetState(A2aState::AuthRequired)
        );
    }

    #[test]
    fn cancel_timeout_sets_canceled() {
        assert_eq!(
            BridgeError::CancelTimeout.disposition(),
            A2aDisposition::SetState(A2aState::Canceled)
        );
    }
}
