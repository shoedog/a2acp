use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1, SchedulerActivationReadinessV1,
};
use bridge_core::mechanical_impossibility::{
    prove_mechanical_impossibility_v1, ContainerSpawnSettlementV1,
    MechanicalImpossibilityObservationV1, ProducerFinalRouteObservationV1,
    ProducerResultObservationV1, RouteStateV1, TerminalResultObservationV1,
};

#[test]
fn wiring_inputs_are_reachable_while_production_remains_disarmed() {
    let routes = ProducerFinalRouteObservationV1 {
        producer_routes: vec![RouteStateV1::IrreversiblyClosed],
        final_routes: vec![RouteStateV1::IrreversiblyClosed],
        terminal_result: TerminalResultObservationV1::Absent,
    };
    assert!(prove_mechanical_impossibility_v1(
        MechanicalImpossibilityObservationV1::ProducerAndFinalRoutes(&routes)
    )
    .is_some());
    let _ = ProducerResultObservationV1::PendingSoleProducer;
    let _ = ContainerSpawnSettlementV1::Settled;

    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, SchedulerActivationReadinessV1::Disarmed);
    assert_eq!(
        deadline_activation_v2_for(readiness, PolicyActivationV1::Production),
        DeadlineActivationV2::ManualOnlyR2f1a
    );
}
