use bridge_core::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
use bridge_core::execution_policy::{
    FanOutPolicyV1, NodeCauseV1, NodeCleanupDispositionV1, NodeCleanupV1, NodePrimaryDispositionV1,
    NodeTerminalV1, PolicyNodeRefV1,
};
use bridge_core::ids::{AttemptId, NodeId};
use bridge_core::workflow_history::LedgerUnavailableReason;
use bridge_workflow::fanout::{
    classify_offline_barrier_error_v1, FanOutControllerV1, PolicyActionV1,
    PolicyTriggerBarrierResultV1, ReadyNodeTerminalV1,
};
use std::collections::BTreeMap;

fn terminal(primary: NodePrimaryDispositionV1) -> NodeTerminalV1 {
    NodeTerminalV1 {
        schema_version: 1,
        primary,
        cleanup: NodeCleanupV1 {
            disposition: NodeCleanupDispositionV1::Complete,
            duration_ms: 1,
        },
        cause: Some(NodeCauseV1 {
            failure_class: DiagnosticFailureClass::Transport,
            code: DiagnosticCode::build("transport.failed", &DiagnosticRedactor::default())
                .unwrap(),
            deepest_cause: Some("socket closed".into()),
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        }),
        prompt_may_have_been_accepted: false,
        degraded_ancestry: false,
        policy_trigger_id: None,
    }
}

fn node_refs() -> BTreeMap<NodeId, PolicyNodeRefV1> {
    BTreeMap::from([
        (
            NodeId::parse("a").unwrap(),
            PolicyNodeRefV1::from_node_id(0, "a"),
        ),
        (
            NodeId::parse("z").unwrap(),
            PolicyNodeRefV1::from_node_id(1, "z"),
        ),
    ])
}

fn attempt() -> AttemptId {
    AttemptId::parse("attempt-11111111111111111111111111111111").unwrap()
}

#[test]
fn simultaneous_failures_select_lowest_node_and_attach_trigger_before_emission() {
    let mut controller = FanOutControllerV1::new(FanOutPolicyV1::FailFast);
    let selection = controller
        .finalize_ready_batch(
            &attempt(),
            vec![
                ReadyNodeTerminalV1 {
                    node: NodeId::parse("z").unwrap(),
                    terminal: terminal(NodePrimaryDispositionV1::Failed),
                },
                ReadyNodeTerminalV1 {
                    node: NodeId::parse("a").unwrap(),
                    terminal: terminal(NodePrimaryDispositionV1::TimedOut),
                },
            ],
            &node_refs(),
            false,
        )
        .unwrap();
    assert_eq!(selection.terminals[0].node.as_str(), "a");
    assert_eq!(selection.terminals[1].node.as_str(), "z");
    let trigger = selection.trigger.unwrap();
    assert_eq!(trigger.node, PolicyNodeRefV1::from_node_id(0, "a"));
    assert_eq!(
        selection.terminals[0].terminal.policy_trigger_id,
        Some(trigger.id.clone())
    );
    assert!(selection.terminals[1].terminal.policy_trigger_id.is_none());
    assert_eq!(
        controller
            .acknowledge_barrier(PolicyTriggerBarrierResultV1::OfflineHistoryCommitted, false,),
        PolicyActionV1::CancelRunningSiblings
    );
}

#[test]
fn identity_and_configuration_errors_refuse_targeted_policy_action() {
    assert_eq!(
        classify_offline_barrier_error_v1(LedgerUnavailableReason::Collision),
        PolicyTriggerBarrierResultV1::PrimaryFailed
    );
    assert_eq!(
        classify_offline_barrier_error_v1(LedgerUnavailableReason::UnsupportedConfiguration,),
        PolicyTriggerBarrierResultV1::PrimaryFailed
    );
    assert!(matches!(
        classify_offline_barrier_error_v1(LedgerUnavailableReason::Io),
        PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { .. }
    ));
    let mut controller = FanOutControllerV1::new(FanOutPolicyV1::FailFast);
    controller
        .finalize_ready_batch(
            &attempt(),
            vec![ReadyNodeTerminalV1 {
                node: NodeId::parse("a").unwrap(),
                terminal: terminal(NodePrimaryDispositionV1::Failed),
            }],
            &node_refs(),
            false,
        )
        .unwrap();
    assert_eq!(
        controller.acknowledge_barrier(PolicyTriggerBarrierResultV1::PrimaryFailed, false),
        PolicyActionV1::GlobalCancelAndDrain
    );
}

#[test]
fn workflow_cancel_suppresses_trigger_and_manual_grace_expires_once() {
    let mut canceled = FanOutControllerV1::new(FanOutPolicyV1::FailFast);
    let selection = canceled
        .finalize_ready_batch(
            &attempt(),
            vec![ReadyNodeTerminalV1 {
                node: NodeId::parse("a").unwrap(),
                terminal: terminal(NodePrimaryDispositionV1::Failed),
            }],
            &node_refs(),
            true,
        )
        .unwrap();
    assert!(selection.trigger.is_none());

    let mut grace = FanOutControllerV1::new(FanOutPolicyV1::FixedGrace { grace_ms: 10 });
    grace
        .finalize_ready_batch(
            &attempt(),
            vec![ReadyNodeTerminalV1 {
                node: NodeId::parse("a").unwrap(),
                terminal: terminal(NodePrimaryDispositionV1::Failed),
            }],
            &node_refs(),
            false,
        )
        .unwrap();
    assert_eq!(
        grace.acknowledge_barrier(PolicyTriggerBarrierResultV1::OfflineHistoryCommitted, false,),
        PolicyActionV1::ArmManualGrace { grace_ms: 10 }
    );
    assert_eq!(
        grace.expire_manual_grace(),
        PolicyActionV1::CancelRunningSiblings
    );
    assert_eq!(grace.expire_manual_grace(), PolicyActionV1::None);
}
