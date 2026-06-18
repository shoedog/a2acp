//! Slice 0 minimal orchestration DTOs (bridge-owned, versioned, Ser+De). Rich variants
//! (Plan/ToolCall/config/mode/commands) + the `session`/`source` envelope fields are deferred
//! (S6/S7); the versioned + `#[serde(flatten)] kind` envelope makes those additions non-breaking.
use crate::ids::OperationId;
use serde::{Deserialize, Serialize};

pub const ORCH_V: u16 = 1;

/// Outcome of reconciling model/effort on a LIVE warm session (Slice 1). Fieldless —
/// the backend LOGS any rejection reason internally (no wire leak).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Applied,
    NotAdvertised,
    Rejected,
}

/// Bridge-owned agent SESSION-LIFECYCLE capabilities (distinct from `catalog::AgentCaps`, which is
/// model-catalog data). Sourced from initialize-time ACP `AgentCapabilities`. `delete` is behind the SDK
/// `unstable_session_delete` feature (NOT enabled) -> always false in Slice 1.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionCaps {
    pub load_session: bool,
    pub resume: bool,
    pub close: bool,
    pub list: bool,
    pub delete: bool,
}

/// ACP usage cost is `{amount, currency}` — NOT guaranteed USD.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageCost {
    pub amount: f64,
    pub currency: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub used: Option<u64>,
    pub size: Option<u64>,
    pub cost: Option<UsageCost>,
    pub at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchEvent {
    pub v: u16,
    pub seq: i64,
    pub ts_ms: i64,
    pub operation_id: OperationId,
    #[serde(flatten)]
    pub kind: OrchEventKind,
}

/// Struct variants only — serde internally-tagged enums reject bare tuple variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    Progress {
        text: String,
    },
    Usage {
        #[serde(flatten)]
        usage: UsageSnapshot,
    },
    Terminal {
        status: TerminalStatus,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerminalStatus {
    Completed,
    Failed { reason: String },
    Canceled,
}

impl TerminalStatus {
    /// ACP `StopReason` → terminal status. `end_turn`→Completed; `cancelled`→Canceled; else→Failed.
    pub fn from_stop_reason(stop_reason: &str) -> Self {
        match stop_reason {
            "end_turn" => TerminalStatus::Completed,
            "cancelled" => TerminalStatus::Canceled,
            other => TerminalStatus::Failed {
                reason: other.to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchResult {
    pub v: u16,
    pub operation_id: OperationId,
    pub status: TerminalStatus,
    pub wall_clock_ms: u64,
    pub usage: UsageSnapshot,
    pub output: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_session_caps_default_is_all_false() {
        let c = AgentSessionCaps::default();
        assert!(!c.load_session && !c.resume && !c.close && !c.list && !c.delete);
    }

    #[test]
    fn reconcile_outcome_eq() {
        assert_eq!(ReconcileOutcome::Applied, ReconcileOutcome::Applied);
        assert_ne!(ReconcileOutcome::Applied, ReconcileOutcome::NotAdvertised);
    }

    #[test]
    fn orch_event_roundtrips_with_internal_kind_tag() {
        let ev = OrchEvent {
            v: ORCH_V,
            seq: 3,
            ts_ms: 100,
            operation_id: crate::ids::OperationId::parse("op-1").unwrap(),
            kind: OrchEventKind::Usage {
                usage: UsageSnapshot {
                    used: Some(10),
                    size: Some(200),
                    cost: None,
                    at_ms: 100,
                },
            },
        };
        let j = serde_json::to_value(&ev).unwrap();
        assert_eq!(j["kind"], "usage");
        assert_eq!(j["used"], 10);
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 3);
    }
    #[test]
    fn usage_cost_carries_amount_and_currency() {
        let j = serde_json::to_value(&UsageCost {
            amount: 1.5,
            currency: "USD".into(),
        })
        .unwrap();
        assert_eq!(j["amount"], 1.5);
        assert_eq!(j["currency"], "USD");
    }
    #[test]
    fn terminal_status_from_each_stop_reason() {
        assert!(matches!(
            TerminalStatus::from_stop_reason("end_turn"),
            TerminalStatus::Completed
        ));
        assert!(matches!(
            TerminalStatus::from_stop_reason("cancelled"),
            TerminalStatus::Canceled
        ));
        for s in ["refusal", "max_tokens", "max_turn_requests", "weird"] {
            assert!(matches!(
                TerminalStatus::from_stop_reason(s),
                TerminalStatus::Failed { .. }
            ));
        }
    }
}
