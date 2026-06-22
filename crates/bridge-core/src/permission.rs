use crate::domain::PermitDecision;
use crate::ids::{ContextId, OperationId};

/// Per-turn metadata threaded onto the ACP route so the reverse permission handler can build a gen-stamped key.
#[derive(Debug, Clone)]
pub struct TurnMeta {
    pub context_id: ContextId,
    pub generation: u64,
    pub op: OperationId,
}

/// Gen+op-keyed identity of one pending permission rendezvous.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermKey {
    pub context_id: ContextId,
    pub generation: u64,
    pub op: OperationId,
    pub request_id: String,
}

/// The value sent through the pending oneshot. `Cancelled` is broadcast by resolve_context (Task 3/4).
#[derive(Debug)]
pub enum PermissionResolution {
    Decided(PermitDecision),
    Cancelled,
}

/// One offered permission option, surfaced to the operator via session/status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionOptionView {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// What `session/status` shows for a pending permission (Task 8 reads this from the registry).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingPermissionView {
    pub request_id: String,
    pub tool_call_id: String,
    pub generation: u64,
    pub op: OperationId,
    pub title: String,
    pub options: Vec<PermissionOptionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<String>,
    pub timeout_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_view_round_trips() {
        let v = PendingPermissionView {
            request_id: "r".into(),
            tool_call_id: "t".into(),
            generation: 1,
            op: OperationId::parse("turn-1").unwrap(),
            title: "write /tmp/x".into(),
            options: vec![PermissionOptionView {
                option_id: "approved".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
            raw_input: None,
            timeout_ms: 120_000,
        };
        let s = serde_json::to_string(&v).unwrap();
        let _back: PendingPermissionView = serde_json::from_str(&s).unwrap();
    }
}
