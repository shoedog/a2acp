// domain.rs — minimal shared domain value types (spec §5.2/§5.3).

use crate::ids::CallerId;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct Artifact;

#[derive(Debug, Default, Clone)]
pub struct PromptOutcome;

#[derive(Debug, Clone, Default)]
pub struct TaskMeta {
    pub skill: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTaskId(pub String);

#[derive(Debug, Clone)]
pub enum RouteTarget {
    Local(crate::ids::AgentId),
    Delegate,
}

// --- Types added by Task 4 ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKind {
    Permission,
    Auth,
}

#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub request_id: String,
    pub kind: PendingKind,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: String,
    pub interactive: bool,
}

impl PermissionRequest {
    pub fn read() -> Self {
        Self {
            request_id: String::new(),
            interactive: false,
        }
    }
    pub fn interactive() -> Self {
        Self {
            request_id: String::new(),
            interactive: true,
        }
    }
    pub fn with_id(request_id: impl Into<String>, interactive: bool) -> Self {
        Self {
            request_id: request_id.into(),
            interactive,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
}

#[derive(Debug, Clone, Default)]
pub struct SessionContext;

impl SessionContext {
    pub fn test() -> Self {
        Self
    }
}

#[derive(Debug, Clone)]
pub struct InboundRequest {
    pub token: Option<String>,
}

impl InboundRequest {
    pub fn anon() -> Self {
        Self { token: None }
    }
    pub fn with_token(t: &str) -> Self {
        Self {
            token: Some(t.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    caller: CallerId,
}

impl AuthContext {
    pub fn new(caller: CallerId) -> Self {
        Self { caller }
    }
    pub fn caller_id(&self) -> &CallerId {
        &self.caller
    }
}

#[cfg(test)]
mod v25 {
    use super::*;
    use crate::ids::{AgentId, CallerId};

    #[test]
    fn part_carries_text() {
        assert_eq!(Part { text: "hi".into() }.text, "hi");
    }
    #[test]
    fn task_meta_skill() {
        assert_eq!(
            TaskMeta {
                skill: Some("delegate".into())
            }
            .skill
            .as_deref(),
            Some("delegate")
        );
    }
    #[test]
    fn peer_task_id_holds_string() {
        let p = PeerTaskId("peer-1".into());
        assert_eq!(p.0, "peer-1");
    }
    #[test]
    fn route_target_delegate_variant() {
        let r = RouteTarget::Delegate;
        assert!(matches!(r, RouteTarget::Delegate));
    }
    #[test]
    fn auth_context_roundtrips_caller() {
        let caller = CallerId::parse("alice").unwrap();
        let ctx = AuthContext::new(caller.clone());
        assert_eq!(ctx.caller_id().as_str(), "alice");
    }
    #[test]
    fn inbound_request_with_token() {
        let req = InboundRequest::with_token("tok-123");
        assert_eq!(req.token.as_deref(), Some("tok-123"));
    }
    #[test]
    fn permission_request_read_is_non_interactive() {
        let req = PermissionRequest::read();
        assert!(!req.interactive);
        assert!(req.request_id.is_empty());
    }
    #[test]
    fn session_context_test_ctor() {
        let _ctx = SessionContext::test();
    }
    #[test]
    fn route_target_local() {
        let r = RouteTarget::Local(AgentId::parse("kiro").unwrap());
        assert!(matches!(r, RouteTarget::Local(a) if a.as_str() == "kiro"));
    }
}
