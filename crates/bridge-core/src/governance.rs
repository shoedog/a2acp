//! Budget contracts only. Admission is deliberately unavailable until the atomic store boundary lands.
use crate::{
    ids::{AgentId, AttemptId, BatchId, ExecutionId},
    orch::UsageSnapshot,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, num::NonZeroU64};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetMetricV1 {
    Turns,
    TerminalTokens,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetLimitsV1 {
    pub turns: Option<u64>,
    pub terminal_tokens: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetSelectorV1 {
    EachRequest,
    EachWorkflowRun,
    UtcDayAgent(AgentId),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetRuleV1 {
    pub id: String,
    pub selector: BudgetSelectorV1,
    pub limits: BudgetLimitsV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCandidateV1 {
    pub agent: AgentId,
    /// Exact dispatched selector. None is an absent selector, never a wildcard.
    pub model: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnAllowanceV1 {
    pub target: BudgetCandidateV1,
    /// Admission liability, not a ceiling on a successful provider response.
    pub terminal_tokens: NonZeroU64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetPolicyV1 {
    rules: Vec<BudgetRuleV1>,
    allowances: Vec<TurnAllowanceV1>,
    digest: [u8; 32],
}
impl BudgetPolicyV1 {
    pub fn new(
        mut rules: Vec<BudgetRuleV1>,
        mut allowances: Vec<TurnAllowanceV1>,
    ) -> Result<Self, &'static str> {
        let stable = |s: &str| !s.is_empty() && s.trim() == s;
        let (mut ids, mut selectors, mut targets) =
            (HashSet::new(), HashSet::new(), HashSet::new());
        if rules.is_empty() {
            return Err("governance requires nonempty rules");
        }
        for rule in &rules {
            if !stable(&rule.id)
                || !ids.insert(&rule.id)
                || !selectors.insert(&rule.selector)
                || (rule.limits.turns.is_none() && rule.limits.terminal_tokens.is_none())
            {
                return Err(
                    "governance requires unique stable rule IDs, selectors and nonempty limits",
                );
            }
            if let BudgetSelectorV1::UtcDayAgent(agent) = &rule.selector {
                if !stable(agent.as_str()) {
                    return Err("invalid governance agent");
                }
            }
        }
        for allowance in &allowances {
            if !stable(allowance.target.agent.as_str())
                || allowance
                    .target
                    .model
                    .as_deref()
                    .is_some_and(|m| !stable(m))
                || !targets.insert(&allowance.target)
            {
                return Err("governance requires valid unique allowance targets");
            }
        }
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        allowances.sort_by(|a, b| {
            (a.target.agent.as_str(), &a.target.model)
                .cmp(&(b.target.agent.as_str(), &b.target.model))
        });
        let bytes = serde_json::to_vec(&(1_u32, &rules, &allowances))
            .map_err(|_| "policy encoding failed")?;
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes)
            .as_ref()
            .try_into()
            .map_err(|_| "policy digest failed")?;
        Ok(Self {
            rules,
            allowances,
            digest,
        })
    }
    pub fn rules(&self) -> &[BudgetRuleV1] {
        &self.rules
    }
    pub fn allowances(&self) -> &[TurnAllowanceV1] {
        &self.allowances
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn allowance_for(
        &self,
        candidate: &BudgetCandidateV1,
        workflow: bool,
    ) -> Result<Option<NonZeroU64>, BudgetRefusalV1> {
        let applicable = self.rules.iter().any(|r| {
            r.limits.terminal_tokens.is_some()
                && match &r.selector {
                    BudgetSelectorV1::EachRequest => true,
                    BudgetSelectorV1::EachWorkflowRun => workflow,
                    BudgetSelectorV1::UtcDayAgent(agent) => agent == &candidate.agent,
                }
        });
        if !applicable {
            return Ok(None);
        }
        self.allowances
            .iter()
            .find(|a| &a.target == candidate)
            .map(|a| Some(a.terminal_tokens))
            .ok_or_else(|| BudgetRefusalV1::new(BudgetRefusalReasonV1::MissingAllowance))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetRequestIdV1 {
    Execution(ExecutionId),
    Batch(BatchId),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetContextV1 {
    pub request: BudgetRequestIdV1,
    pub workflow_run: Option<ExecutionId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetScopeOwnerV1 {
    Request(BudgetRequestIdV1),
    WorkflowRun(ExecutionId),
    UtcDayAgent { agent: AgentId, utc_day: i64 },
}
/// Rule ID is policy identity, not a coverage boundary: renaming must not erase historical liability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetScopeKeyV1 {
    pub rule_id: String,
    pub owner: BudgetScopeOwnerV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetReservationV1 {
    pub attempt: AttemptId,
    pub context: BudgetContextV1,
    pub candidate: BudgetCandidateV1,
    pub policy_digest: [u8; 32],
    pub admitted_ms: i64,
    pub admission_day: i64,
    pub scopes: Vec<BudgetScopeKeyV1>,
    pub turns: u64,
    pub terminal_tokens: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetUnknownReasonV1 {
    MissingTerminalTokens,
    InvalidTerminalTokens,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetLiabilityV1 {
    Known(u64),
    Pending(u64),
    Unknown {
        retained: u64,
        reason: BudgetUnknownReasonV1,
    },
}
impl BudgetLiabilityV1 {
    pub fn retained(&self) -> u64 {
        match self {
            Self::Known(n) | Self::Pending(n) | Self::Unknown { retained: n, .. } => *n,
        }
    }
    /// Only valid finalized terminal totals may reduce liability. Components and occupancy never charge.
    pub fn reconcile(
        &self,
        usage: &UsageSnapshot,
        finalized: bool,
        invalid_terminal: bool,
    ) -> Self {
        let total = usage.terminal.as_ref().map(|t| t.total_tokens);
        let retained = self.retained().max(total.unwrap_or(0));
        if !finalized {
            return Self::Pending(retained);
        }
        match (total, invalid_terminal) {
            (Some(n), false) => Self::Known(n),
            _ => Self::Unknown {
                retained,
                reason: if invalid_terminal {
                    BudgetUnknownReasonV1::InvalidTerminalTokens
                } else {
                    BudgetUnknownReasonV1::MissingTerminalTokens
                },
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetRefusalReasonV1 {
    LimitExceeded,
    UnknownMeasurement,
    IncompleteCoverage,
    MissingAllowance,
    IdentityMismatch,
    PolicyMismatch,
    DuplicateAttempt,
    Overflow,
    AccountingUnavailable,
    UnsupportedRoute,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetRefusalV1 {
    pub reason: BudgetRefusalReasonV1,
    pub scope: Option<BudgetScopeKeyV1>,
    pub metric: Option<BudgetMetricV1>,
    /// Settlement failure is separate from the original refusal; public text stays static.
    pub persistence: Option<Box<crate::error::BridgeError>>,
}
impl BudgetRefusalV1 {
    pub fn new(reason: BudgetRefusalReasonV1) -> Self {
        Self {
            reason,
            scope: None,
            metric: None,
            persistence: None,
        }
    }
}
/// No public constructor or Clone: only the future atomic admission boundary can mint authority.
#[derive(Debug)]
pub struct BudgetPermitV1 {
    reservation: BudgetReservationV1,
}
impl BudgetPermitV1 {
    /// Only TaskStore admission implementations may call this after committing admission.
    pub fn from_committed_admission(reservation: BudgetReservationV1) -> Self {
        Self { reservation }
    }
    pub fn into_reservation(self) -> BudgetReservationV1 {
        self.reservation
    }
}
#[derive(Debug)]
pub enum BudgetAdmissionOutcomeV1 {
    Ungoverned,
    Admitted(BudgetPermitV1),
    Refused(BudgetRefusalV1),
}

/// A budget stop deliberately drains siblings instead of selecting a fan-out trigger.
/// Only a failed, undispatched terminal with the typed static budget cause proves it.
pub fn is_budget_stop_terminal_v1(terminal: &crate::execution_policy::NodeTerminalV1) -> bool {
    use crate::{diagnostics::DiagnosticFailureClass, execution_policy::NodePrimaryDispositionV1};
    terminal.primary == NodePrimaryDispositionV1::Failed
        && !terminal.prompt_may_have_been_accepted
        && terminal.cause.as_ref().is_some_and(|cause| {
            cause.failure_class == DiagnosticFailureClass::Config
                && cause.code.as_str() == "bridge.budget_refused"
                && !cause.cause_truncated
                && !cause.evidence_overflow
                && cause.dependency_set.is_none()
        })
}
