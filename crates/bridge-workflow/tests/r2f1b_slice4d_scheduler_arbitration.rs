use std::collections::HashSet;

use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1, SchedulerActivationReadinessV1,
};
use bridge_core::ids::NodeId;
use bridge_workflow::scheduler_arbitration::{
    arbitrate_scheduler_v1, ReadyNodeCompletionV1, SchedulerArbitrationReadinessV1, SchedulerArmV1,
    SchedulerTieFactsV1,
};

#[derive(Clone, Copy)]
struct ReadyFlags {
    completion: bool,
    barrier: bool,
    cancellation: bool,
    fixed_grace: bool,
    cutoff: bool,
    impossibility: bool,
    warning: bool,
}

const NO_SIGNALS: ReadyFlags = ReadyFlags {
    completion: false,
    barrier: false,
    cancellation: false,
    fixed_grace: false,
    cutoff: false,
    impossibility: false,
    warning: false,
};

#[derive(Clone, Copy)]
struct PriorityTieFacts {
    completion_ready_at_ms: u64,
    absolute_cutoff_at_ms: u64,
}

const BEFORE_CUTOFF: PriorityTieFacts = PriorityTieFacts {
    completion_ready_at_ms: 9,
    absolute_cutoff_at_ms: 10,
};

struct PriorityCase {
    name: &'static str,
    ready: ReadyFlags,
    expected: SchedulerArmV1,
    tie: PriorityTieFacts,
}

fn node(id: &str) -> NodeId {
    NodeId::parse(id).unwrap()
}

fn completion(id: &str, ready_at_ms: u64) -> ReadyNodeCompletionV1 {
    ReadyNodeCompletionV1 {
        node_id: node(id),
        ready_at_ms,
    }
}

fn readiness(flags: ReadyFlags, tie: PriorityTieFacts) -> SchedulerArbitrationReadinessV1 {
    SchedulerArbitrationReadinessV1 {
        ready_node_completions: if flags.completion {
            vec![completion("completed", tie.completion_ready_at_ms)]
        } else {
            Vec::new()
        },
        durable_trigger_barrier_acknowledgements: flags.barrier,
        workflow_or_external_cancellation: flags.cancellation,
        fixed_grace_expired: flags.fixed_grace,
        absolute_cutoff_reached: flags.cutoff,
        mechanical_impossibility_proved: flags.impossibility,
        no_progress_snapshot_due: flags.warning,
    }
}

fn tie_facts(inflight_nodes: Vec<NodeId>, absolute_cutoff_at_ms: u64) -> SchedulerTieFactsV1 {
    SchedulerTieFactsV1 {
        absolute_cutoff_at_ms,
        inflight_nodes,
    }
}

#[test]
fn scheduler_priority_table_is_exhaustive_and_all_ready_selects_arm_one() {
    let cases = [
        PriorityCase {
            name: "completion",
            ready: ReadyFlags {
                completion: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::DrainReadyNodeCompletions,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "barrier acknowledgement",
            ready: ReadyFlags {
                barrier: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::DurableTriggerBarrierAcknowledgements,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "workflow cancellation",
            ready: ReadyFlags {
                cancellation: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::WorkflowOrExternalCancellation,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "fixed grace",
            ready: ReadyFlags {
                fixed_grace: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::FixedGraceExpiry,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "absolute cutoff",
            ready: ReadyFlags {
                cutoff: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::AbsoluteCutoff,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "mechanical impossibility",
            ready: ReadyFlags {
                impossibility: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::MechanicallyProvedImpossibility,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "no-progress warning",
            ready: ReadyFlags {
                warning: true,
                ..NO_SIGNALS
            },
            expected: SchedulerArmV1::DueNoProgressSnapshots,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "wait fallback",
            ready: NO_SIGNALS,
            expected: SchedulerArmV1::WaitForNodeActivityControlOrClock,
            tie: BEFORE_CUTOFF,
        },
        PriorityCase {
            name: "all eight ready",
            ready: ReadyFlags {
                completion: true,
                barrier: true,
                cancellation: true,
                fixed_grace: true,
                cutoff: true,
                impossibility: true,
                warning: true,
            },
            expected: SchedulerArmV1::DrainReadyNodeCompletions,
            tie: BEFORE_CUTOFF,
        },
    ];

    let mut winners = HashSet::new();
    for case in cases {
        let result = arbitrate_scheduler_v1(
            readiness(case.ready, case.tie),
            tie_facts(vec![node("completed")], case.tie.absolute_cutoff_at_ms),
        );
        assert_eq!(result.winner, case.expected, "table row: {}", case.name);
        winners.insert(result.winner);
    }
    assert_eq!(winners.len(), 8, "every arm must win at least one row");
}

#[test]
fn completion_ready_exactly_at_cutoff_wins_inclusively() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![completion("finished", 7_200_000)],
            absolute_cutoff_reached: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        SchedulerTieFactsV1 {
            absolute_cutoff_at_ms: 7_200_000,
            inflight_nodes: vec![node("finished")],
        },
    );

    assert_eq!(result.winner, SchedulerArmV1::DrainReadyNodeCompletions);
    assert_eq!(result.ready_node_completions[0].ready_at_ms, 7_200_000);
}

#[test]
fn completion_at_cutoff_cancels_unfinished_nodes_after_drain() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![completion("finished", 50)],
            absolute_cutoff_reached: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        SchedulerTieFactsV1 {
            absolute_cutoff_at_ms: 50,
            inflight_nodes: vec![node("unfinished-z"), node("finished"), node("unfinished-a")],
        },
    );

    assert_eq!(result.winner, SchedulerArmV1::DrainReadyNodeCompletions);
    assert_eq!(
        result.nodes_to_cancel_after_winner,
        vec![node("unfinished-a"), node("unfinished-z")]
    );
}

#[test]
fn completion_strictly_after_cutoff_is_dropped_and_its_node_is_cancelled() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![
                completion("before", 49),
                completion("at", 50),
                completion("after", 51),
            ],
            absolute_cutoff_reached: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        SchedulerTieFactsV1 {
            absolute_cutoff_at_ms: 50,
            inflight_nodes: vec![node("after"), node("at"), node("before")],
        },
    );

    assert_eq!(result.winner, SchedulerArmV1::DrainReadyNodeCompletions);
    assert_eq!(
        result
            .ready_node_completions
            .iter()
            .map(|completion| (completion.node_id.as_str(), completion.ready_at_ms))
            .collect::<Vec<_>>(),
        [("at", 50), ("before", 49)]
    );
    assert_eq!(
        result.post_cutoff_completions,
        vec![completion("after", 51)]
    );
    assert_eq!(result.nodes_to_cancel_after_winner, vec![node("after")]);
}

#[test]
fn warning_loses_to_ready_completion() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![completion("finished", 9)],
            no_progress_snapshot_due: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        tie_facts(vec![node("finished")], 10),
    );

    assert_eq!(result.winner, SchedulerArmV1::DrainReadyNodeCompletions);
}

#[test]
fn warning_loses_to_absolute_cutoff() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            absolute_cutoff_reached: true,
            no_progress_snapshot_due: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        tie_facts(vec![node("unfinished")], 10),
    );

    assert_eq!(result.winner, SchedulerArmV1::AbsoluteCutoff);
}

#[test]
fn ready_completion_batch_is_sorted_by_node_id() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![
                completion("z", 1),
                completion("a", 3),
                completion("m", 2),
            ],
            ..SchedulerArbitrationReadinessV1::default()
        },
        tie_facts(vec![node("z"), node("a"), node("m")], 10),
    );

    let ids: Vec<_> = result
        .ready_node_completions
        .iter()
        .map(|ready| ready.node_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "m", "z"]);
}

#[test]
fn completion_outprioritizes_durable_barrier_acknowledgement() {
    let result = arbitrate_scheduler_v1(
        SchedulerArbitrationReadinessV1 {
            ready_node_completions: vec![completion("finished", 9)],
            durable_trigger_barrier_acknowledgements: true,
            ..SchedulerArbitrationReadinessV1::default()
        },
        tie_facts(vec![node("finished")], 10),
    );

    assert_eq!(result.winner, SchedulerArmV1::DrainReadyNodeCompletions);
}

#[test]
fn disarmed_production_still_cannot_construct_automatic_attempt() {
    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, SchedulerActivationReadinessV1::Disarmed);
    assert_eq!(
        deadline_activation_v2_for(readiness, PolicyActivationV1::Production),
        DeadlineActivationV2::ManualOnlyR2f1a
    );
}
