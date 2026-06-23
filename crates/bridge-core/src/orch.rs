//! Slice 0 minimal orchestration DTOs (bridge-owned, versioned, Ser+De). Rich variants
//! (Plan/ToolCall/config/mode/commands) are deferred (S6/S7); the versioned +
//! `#[serde(flatten)] kind` envelope makes those additions non-breaking.
use crate::ids::{OperationId, SessionHandleRef, SourceId};
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session: Option<SessionHandleRef>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<SourceId>,
    #[serde(flatten)]
    pub kind: OrchEventKind,
}

/// Struct variants only — serde internally-tagged enums reject bare tuple variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    NodeStarted {
        node: String,
    },
    NodeFinished {
        node: String,
        ok: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<UsageSnapshot>,
    },
    Terminal {
        status: TerminalStatus,
        output: String,
    },
    Progress {
        text: String,
    },
    Usage {
        #[serde(flatten)]
        usage: UsageSnapshot,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        #[serde(rename = "tool_kind")]
        kind: String,
        status: String,
        locations: Vec<String>,
        content: Option<ContentSummary>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(rename = "tool_kind", skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<ContentSummary>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentSummary {
    pub item_count: usize,
    pub preview: String,
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
    fn agent_session_caps_roundtrips() {
        let c = AgentSessionCaps {
            load_session: true,
            resume: true,
            close: false,
            list: true,
            delete: false,
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["load_session"], true);
        assert_eq!(j["close"], false);
        let back: AgentSessionCaps = serde_json::from_value(j).unwrap();
        assert_eq!(back, c);
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
            session: None,
            source: None,
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
    fn journaled_orch_event_kinds_roundtrip() {
        let started = OrchEvent {
            v: ORCH_V,
            seq: 1,
            ts_ms: 9,
            operation_id: crate::ids::OperationId::parse("op-t1").unwrap(),
            session: None,
            source: None,
            kind: OrchEventKind::NodeStarted { node: "a".into() },
        };
        let j = serde_json::to_value(&started).unwrap();
        assert_eq!(j["kind"], "node_started");
        assert_eq!(j["node"], "a");
        assert!(j.get("session").is_none());
        assert!(j.get("source").is_none());
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 1);

        let finished = OrchEvent {
            v: ORCH_V,
            seq: 2,
            ts_ms: 10,
            operation_id: crate::ids::OperationId::parse("op-t1").unwrap(),
            session: None,
            source: None,
            kind: OrchEventKind::NodeFinished {
                node: "a".into(),
                ok: true,
                output: "o".into(),
                usage: None,
            },
        };
        let j = serde_json::to_value(&finished).unwrap();
        assert_eq!(j["kind"], "node_finished");
        assert_eq!(j["node"], "a");
        assert_eq!(j["ok"], true);
        assert_eq!(j["output"], "o");
        assert!(j.get("session").is_none());
        assert!(j.get("source").is_none());
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 2);

        let terminal = OrchEvent {
            v: ORCH_V,
            seq: 3,
            ts_ms: 11,
            operation_id: crate::ids::OperationId::parse("op-t1").unwrap(),
            session: None,
            source: None,
            kind: OrchEventKind::Terminal {
                status: TerminalStatus::Failed {
                    reason: "interrupted".into(),
                },
                output: "final".into(),
            },
        };
        let j = serde_json::to_value(&terminal).unwrap();
        assert_eq!(j["kind"], "terminal");
        assert_eq!(j["status"]["status"], "failed");
        assert_eq!(j["status"]["reason"], "interrupted");
        assert_eq!(j["output"], "final");
        assert!(j.get("session").is_none());
        assert!(j.get("source").is_none());
        let back: OrchEvent = serde_json::from_value(j).unwrap();
        assert_eq!(back.seq, 3);
    }

    #[test]
    fn node_finished_carries_optional_usage() {
        // usage present -> serializes; absent -> field omitted (skip_serializing_if).
        let with_usage = OrchEventKind::NodeFinished {
            node: "a".into(),
            ok: true,
            output: "o".into(),
            usage: Some(UsageSnapshot {
                used: Some(15071),
                size: Some(258400),
                cost: None,
                at_ms: 5,
            }),
        };
        let j = serde_json::to_value(&with_usage).unwrap();
        assert_eq!(j["kind"], "node_finished");
        assert_eq!(j["usage"]["used"], 15071);

        let without = OrchEventKind::NodeFinished {
            node: "a".into(),
            ok: true,
            output: "o".into(),
            usage: None,
        };
        let j2 = serde_json::to_value(&without).unwrap();
        assert!(j2.get("usage").is_none(), "absent usage omitted from wire");

        // Old rows on the wire (no `usage` key) must still deserialize (default None).
        let old: OrchEventKind = serde_json::from_value(serde_json::json!({
            "kind": "node_finished",
            "node": "a",
            "ok": true,
            "output": "o"
        }))
        .unwrap();
        assert!(matches!(
            old,
            OrchEventKind::NodeFinished { usage: None, .. }
        ));
    }

    #[test]
    fn rich_kinds_roundtrip() {
        let tc = OrchEventKind::ToolCall {
            tool_call_id: "t1".into(),
            title: "read".into(),
            kind: "read".into(),
            status: "in_progress".into(),
            locations: vec!["a.rs".into()],
            content: Some(ContentSummary {
                item_count: 1,
                preview: "hello".into(),
            }),
        };
        let ev = OrchEvent {
            v: ORCH_V,
            seq: 5,
            ts_ms: 1,
            operation_id: crate::ids::OperationId::parse("op-t").unwrap(),
            session: None,
            source: None,
            kind: tc,
        };
        let j = serde_json::to_value(&ev).unwrap();
        assert_eq!(j["kind"], "tool_call");
        assert_eq!(j["tool_call_id"], "t1");
        let _back: OrchEvent = serde_json::from_value(j).unwrap();

        let up = serde_json::to_value(&OrchEventKind::ToolCallUpdate {
            tool_call_id: "t1".into(),
            title: None,
            kind: None,
            status: Some("completed".into()),
            locations: None,
            content: None,
        })
        .unwrap();
        assert_eq!(up["kind"], "tool_call_update");
        assert_eq!(up["status"], "completed");
        assert!(up.get("title").is_none() && up.get("content").is_none());

        let pl = serde_json::to_value(&OrchEventKind::Plan {
            entries: vec![PlanEntry {
                content: "step".into(),
                priority: "high".into(),
                status: "pending".into(),
            }],
        })
        .unwrap();
        assert_eq!(pl["kind"], "plan");
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
