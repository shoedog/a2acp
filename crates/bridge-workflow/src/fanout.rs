//! Pure R2f1a fan-out trigger controller.
//!
//! The live executor remains responsible for draining `FuturesUnordered`, child cancellation
//! tokens, and durable barrier I/O. This module owns deterministic trigger selection and the
//! closed acknowledgement-to-action transition. Fixed grace is manual-only in R2f1a.

use bridge_core::execution_policy::{
    ControlEventIdV1, FanOutPolicyNameV1, FanOutPolicyV1, NodePrimaryDispositionV1, NodeTerminalV1,
    PolicyNodeRefV1, PolicyTriggerV1, EXECUTION_POLICY_SCHEMA_V1,
};
use bridge_core::ids::{AttemptId, NodeId};
use bridge_core::workflow_history::LedgerUnavailableReason;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyNodeTerminalV1 {
    pub node: NodeId,
    pub terminal: NodeTerminalV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyBatchSelectionV1 {
    pub terminals: Vec<ReadyNodeTerminalV1>,
    pub trigger: Option<PolicyTriggerV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanOutControllerError {
    MissingNodeReference,
    DuplicateReadyNode,
}

impl std::fmt::Display for FanOutControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingNodeReference => "ready node is absent from the frozen graph identity",
            Self::DuplicateReadyNode => "ready terminal batch contains a duplicate node",
        })
    }
}

impl std::error::Error for FanOutControllerError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyTriggerBarrierResultV1 {
    ServedPrimaryCommitted,
    OfflineHistoryCommitted,
    OfflineTelemetryUnavailable { reason: LedgerUnavailableReason },
    PrimaryFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyActionV1 {
    None,
    CancelRunningSiblings,
    ArmManualGrace { grace_ms: u64 },
    GlobalCancelAndDrain,
}

/// Exhaustive classification: availability/capacity stays fail-open; durable identity or
/// ownership collision fails closed and cannot authorize targeted cancellation.
#[must_use]
pub fn classify_offline_barrier_error_v1(
    reason: LedgerUnavailableReason,
) -> PolicyTriggerBarrierResultV1 {
    match reason {
        LedgerUnavailableReason::Open
        | LedgerUnavailableReason::Permission
        | LedgerUnavailableReason::ReadOnlyDatabase
        | LedgerUnavailableReason::ReadOnlyLock
        | LedgerUnavailableReason::ReadOnlyParent
        | LedgerUnavailableReason::AdvisoryLockUnsupported
        | LedgerUnavailableReason::AdvisoryLockIo
        | LedgerUnavailableReason::Locked
        | LedgerUnavailableReason::Migration
        | LedgerUnavailableReason::Schema
        | LedgerUnavailableReason::Corruption
        | LedgerUnavailableReason::Io
        | LedgerUnavailableReason::CapacityProtected => {
            PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { reason }
        }
        LedgerUnavailableReason::Collision => PolicyTriggerBarrierResultV1::PrimaryFailed,
    }
}

#[derive(Clone, Debug)]
pub struct FanOutControllerV1 {
    policy: FanOutPolicyV1,
    trigger: Option<PolicyTriggerV1>,
    barrier_pending: bool,
    admission_stopped: bool,
    manual_grace_armed: bool,
    manual_grace_expired: bool,
}

impl FanOutControllerV1 {
    #[must_use]
    pub fn new(policy: FanOutPolicyV1) -> Self {
        Self {
            policy,
            trigger: None,
            barrier_pending: false,
            admission_stopped: false,
            manual_grace_armed: false,
            manual_grace_expired: false,
        }
    }

    #[must_use]
    pub fn trigger(&self) -> Option<&PolicyTriggerV1> {
        self.trigger.as_ref()
    }

    #[must_use]
    pub fn admission_stopped(&self) -> bool {
        self.admission_stopped
    }

    pub fn finalize_ready_batch(
        &mut self,
        attempt_id: &AttemptId,
        mut terminals: Vec<ReadyNodeTerminalV1>,
        node_refs: &BTreeMap<NodeId, PolicyNodeRefV1>,
        workflow_cancel_observable_before_wait: bool,
    ) -> Result<ReadyBatchSelectionV1, FanOutControllerError> {
        terminals.sort_by(|left, right| left.node.cmp(&right.node));
        if terminals
            .windows(2)
            .any(|pair| pair[0].node == pair[1].node)
        {
            return Err(FanOutControllerError::DuplicateReadyNode);
        }

        if workflow_cancel_observable_before_wait {
            self.admission_stopped = true;
            return Ok(ReadyBatchSelectionV1 {
                terminals,
                trigger: None,
            });
        }
        if self.trigger.is_some() || matches!(self.policy, FanOutPolicyV1::BoundedIndependent) {
            return Ok(ReadyBatchSelectionV1 {
                terminals,
                trigger: None,
            });
        }

        let selected = terminals.iter().position(|ready| {
            matches!(
                ready.terminal.primary,
                NodePrimaryDispositionV1::Failed | NodePrimaryDispositionV1::TimedOut
            )
        });
        let Some(selected) = selected else {
            return Ok(ReadyBatchSelectionV1 {
                terminals,
                trigger: None,
            });
        };
        let node = node_refs
            .get(&terminals[selected].node)
            .cloned()
            .ok_or(FanOutControllerError::MissingNodeReference)?;
        let id = ControlEventIdV1::for_attempt(attempt_id, 0);
        let grace_ms = match self.policy {
            FanOutPolicyV1::FixedGrace { grace_ms } => Some(grace_ms),
            FanOutPolicyV1::BoundedIndependent | FanOutPolicyV1::FailFast => None,
        };
        let trigger = PolicyTriggerV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            id: id.clone(),
            node,
            policy: FanOutPolicyNameV1::from(&self.policy),
            grace_ms,
        };
        terminals[selected].terminal.policy_trigger_id = Some(id);
        self.trigger = Some(trigger.clone());
        self.barrier_pending = true;
        self.admission_stopped = true;
        Ok(ReadyBatchSelectionV1 {
            terminals,
            trigger: Some(trigger),
        })
    }

    #[must_use]
    pub fn acknowledge_barrier(
        &mut self,
        result: PolicyTriggerBarrierResultV1,
        workflow_cancel_observable_after_ack: bool,
    ) -> PolicyActionV1 {
        if !self.barrier_pending {
            return PolicyActionV1::None;
        }
        self.barrier_pending = false;
        if workflow_cancel_observable_after_ack {
            return PolicyActionV1::GlobalCancelAndDrain;
        }
        let authorized = match result {
            PolicyTriggerBarrierResultV1::ServedPrimaryCommitted
            | PolicyTriggerBarrierResultV1::OfflineHistoryCommitted => true,
            PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { reason } => {
                reason != LedgerUnavailableReason::Collision
            }
            PolicyTriggerBarrierResultV1::PrimaryFailed => false,
        };
        if !authorized {
            return PolicyActionV1::GlobalCancelAndDrain;
        }
        match self.policy {
            FanOutPolicyV1::BoundedIndependent => PolicyActionV1::None,
            FanOutPolicyV1::FailFast => PolicyActionV1::CancelRunningSiblings,
            FanOutPolicyV1::FixedGrace { grace_ms } => {
                self.manual_grace_armed = true;
                PolicyActionV1::ArmManualGrace { grace_ms }
            }
        }
    }

    #[must_use]
    pub fn expire_manual_grace(&mut self) -> PolicyActionV1 {
        if !self.manual_grace_armed || self.manual_grace_expired {
            return PolicyActionV1::None;
        }
        self.manual_grace_expired = true;
        PolicyActionV1::CancelRunningSiblings
    }
}
