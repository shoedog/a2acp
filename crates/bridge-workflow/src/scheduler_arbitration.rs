use bridge_core::ids::NodeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SchedulerArmV1 {
    DrainReadyNodeCompletions,
    DurableTriggerBarrierAcknowledgements,
    WorkflowOrExternalCancellation,
    FixedGraceExpiry,
    AbsoluteCutoff,
    MechanicallyProvedImpossibility,
    DueNoProgressSnapshots,
    WaitForNodeActivityControlOrClock,
}

/// The sole executable representation of scheduler priority.
const SCHEDULER_ARM_PRIORITY_V1: [SchedulerArmV1; 8] = [
    SchedulerArmV1::DrainReadyNodeCompletions,
    SchedulerArmV1::DurableTriggerBarrierAcknowledgements,
    SchedulerArmV1::WorkflowOrExternalCancellation,
    SchedulerArmV1::FixedGraceExpiry,
    SchedulerArmV1::AbsoluteCutoff,
    SchedulerArmV1::MechanicallyProvedImpossibility,
    SchedulerArmV1::DueNoProgressSnapshots,
    SchedulerArmV1::WaitForNodeActivityControlOrClock,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyNodeCompletionV1 {
    pub node_id: NodeId,
    /// A monotonic offset already sampled by the caller; arbitration never reads a clock.
    pub ready_at_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchedulerArbitrationReadinessV1 {
    pub ready_node_completions: Vec<ReadyNodeCompletionV1>,
    pub durable_trigger_barrier_acknowledgements: bool,
    pub workflow_or_external_cancellation: bool,
    pub fixed_grace_expired: bool,
    pub absolute_cutoff_reached: bool,
    pub mechanical_impossibility_proved: bool,
    pub no_progress_snapshot_due: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerTieFactsV1 {
    /// The attempt-relative monotonic cutoff supplied by the frozen policy.
    pub absolute_cutoff_at_ms: u64,
    pub inflight_nodes: Vec<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerArbitrationV1 {
    pub winner: SchedulerArmV1,
    pub ready_node_completions: Vec<ReadyNodeCompletionV1>,
    /// A cutoff tied with the winning completion batch cancels these nodes after the drain.
    pub nodes_to_cancel_after_winner: Vec<NodeId>,
}

impl SchedulerArmV1 {
    fn is_ready(
        self,
        readiness: &SchedulerArbitrationReadinessV1,
        has_ready_completions: bool,
    ) -> bool {
        match self {
            Self::DrainReadyNodeCompletions => has_ready_completions,
            Self::DurableTriggerBarrierAcknowledgements => {
                readiness.durable_trigger_barrier_acknowledgements
            }
            Self::WorkflowOrExternalCancellation => readiness.workflow_or_external_cancellation,
            Self::FixedGraceExpiry => readiness.fixed_grace_expired,
            Self::AbsoluteCutoff => readiness.absolute_cutoff_reached,
            Self::MechanicallyProvedImpossibility => readiness.mechanical_impossibility_proved,
            Self::DueNoProgressSnapshots => readiness.no_progress_snapshot_due,
            Self::WaitForNodeActivityControlOrClock => true,
        }
    }
}

#[must_use]
pub fn arbitrate_scheduler_v1(
    mut readiness: SchedulerArbitrationReadinessV1,
    mut tie_facts: SchedulerTieFactsV1,
) -> SchedulerArbitrationV1 {
    if readiness.absolute_cutoff_reached {
        readiness
            .ready_node_completions
            .retain(|completion| completion.ready_at_ms <= tie_facts.absolute_cutoff_at_ms);
    }
    readiness
        .ready_node_completions
        .sort_by(|left, right| left.node_id.cmp(&right.node_id));

    let winner = SCHEDULER_ARM_PRIORITY_V1
        .into_iter()
        .find(|arm| arm.is_ready(&readiness, !readiness.ready_node_completions.is_empty()))
        .expect("the wait arm is always eligible");

    let cutoff_requires_cancellation = readiness.absolute_cutoff_reached
        && matches!(
            winner,
            SchedulerArmV1::DrainReadyNodeCompletions | SchedulerArmV1::AbsoluteCutoff
        );
    let mut nodes_to_cancel_after_winner = if cutoff_requires_cancellation {
        tie_facts
            .inflight_nodes
            .drain(..)
            .filter(|node| {
                !readiness
                    .ready_node_completions
                    .iter()
                    .any(|completion| completion.node_id == *node)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    nodes_to_cancel_after_winner.sort();

    SchedulerArbitrationV1 {
        winner,
        ready_node_completions: readiness.ready_node_completions,
        nodes_to_cancel_after_winner,
    }
}
