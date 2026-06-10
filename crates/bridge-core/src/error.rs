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
    #[error("agent crashed: {reason}")]
    AgentCrashed { reason: String },
    #[error("agent overloaded")]
    AgentOverloaded,
    #[error("upstream a2a error")]
    UpstreamA2aError,
    #[error("store failure")]
    StoreFailure,
    #[error("invalid state transition")]
    InvalidStateTransition,
    #[error("unknown agent: {id}")]
    UnknownAgent { id: String },
    #[error("invalid config: {reason}")]
    ConfigInvalid { reason: String },
}

impl BridgeError {
    /// Construct an `AgentCrashed` carrying a short reason describing what failed
    /// (e.g. "spawn failed: …", "handshake timeout"), so the client/log sees WHY
    /// rather than an opaque "agent crashed".
    pub fn agent_crashed(reason: impl Into<String>) -> Self {
        BridgeError::AgentCrashed {
            reason: reason.into(),
        }
    }

    /// Construct a `ConfigInvalid` carrying the operator-facing reason while
    /// keeping [`Self::client_message`] redacted to a static category.
    pub fn config_invalid(reason: impl Into<String>) -> Self {
        BridgeError::ConfigInvalid {
            reason: reason.into(),
        }
    }

    /// The message safe to surface to an inbound A2A client over the wire.
    ///
    /// Internal-failure reasons (`AgentCrashed`/`ConfigInvalid`) can embed infra
    /// detail — upstream URLs (incl. query params), filesystem paths, SDK error
    /// text — so they collapse to a STATIC category here; the full reason stays in
    /// server logs (`tracing`). Client-caused errors (`InvalidRequest{field}`, etc.)
    /// keep their `Display` because it's both safe and helpful to the caller.
    pub fn client_message(&self) -> String {
        match self {
            BridgeError::AgentCrashed { .. } => "agent crashed".to_string(),
            BridgeError::ConfigInvalid { .. } => "invalid config".to_string(),
            other => other.to_string(),
        }
    }

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
            BridgeError::agent_crashed("test"),
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

    #[test]
    fn agent_overloaded_displays() {
        assert_eq!(BridgeError::AgentOverloaded.to_string(), "agent overloaded");
    }

    #[test]
    fn agent_crashed_carries_reason_in_display() {
        // The reason IS in the Display (server logs / tracing see WHY).
        let e = BridgeError::agent_crashed("spawn failed: no such file");
        assert!(e.to_string().contains("spawn failed: no such file"));
    }

    #[test]
    fn config_invalid_carries_reason_in_display_but_redacts_client_message() {
        let e = BridgeError::config_invalid("model bogus not in [default, sonnet]");
        assert!(e.to_string().contains("model bogus not in"));
        assert_eq!(e.client_message(), "invalid config");
    }

    #[test]
    fn client_message_redacts_internal_reason_but_keeps_client_errors() {
        // Internal-failure reason (could embed a URL with a token) is NOT surfaced
        // to the wire — only the static category — while the full reason remains in
        // Display for logs. This is the wire-leak guard.
        let leaky = BridgeError::agent_crashed("HTTP failed: https://api.example/v1?token=SECRET");
        assert_eq!(leaky.client_message(), "agent crashed");
        assert!(leaky.to_string().contains("SECRET")); // full reason still logged
        assert_eq!(
            BridgeError::ConfigInvalid { reason: "x".into() }.client_message(),
            "invalid config"
        );
        // Client-caused errors keep their helpful Display.
        assert_eq!(
            BridgeError::InvalidRequest {
                field: "message: no text"
            }
            .client_message(),
            "invalid request: message: no text"
        );
    }

    #[test]
    fn agent_overloaded_is_failed_disposition() {
        use crate::error::A2aDisposition::*;
        use crate::error::A2aState as S;
        assert_eq!(
            BridgeError::AgentOverloaded.disposition(),
            SetState(S::Failed)
        );
    }
}
