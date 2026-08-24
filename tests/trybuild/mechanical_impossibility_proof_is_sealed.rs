use bridge_core::mechanical_impossibility::{
    prove_mechanical_impossibility_v1, ContainerSpawnSettlementV1,
    MechanicalImpossibilityObservationV1, MechanicalImpossibilityProofV1,
    ProducerFinalRouteObservationV1, ProducerResultObservationV1, RouteStateV1,
    TerminalResultObservationV1,
};

fn main() {
    let _ = MechanicalImpossibilityProofV1::default();
    let _: MechanicalImpossibilityProofV1 = false.into();
    let _ = MechanicalImpossibilityProofV1 {};
}
