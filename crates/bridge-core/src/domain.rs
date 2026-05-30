// domain.rs — minimal shared domain value types (spec §5.2/§5.3).

use crate::ids::CallerId;

#[derive(Debug, Default, Clone)]
pub struct Part;

#[derive(Debug, Default, Clone)]
pub struct Artifact;

#[derive(Debug, Default, Clone)]
pub struct PromptOutcome;

#[derive(Debug, Default, Clone)]
pub struct TaskMeta;

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
    pub interactive: bool,
}

impl PermissionRequest {
    pub fn read() -> Self {
        Self { interactive: false }
    }
    pub fn interactive() -> Self {
        Self { interactive: true }
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
