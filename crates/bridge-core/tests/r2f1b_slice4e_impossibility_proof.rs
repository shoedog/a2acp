use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1, SchedulerActivationReadinessV1,
};

#[test]
fn disarmed_production_still_cannot_construct_automatic_attempt() {
    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, SchedulerActivationReadinessV1::Disarmed);
    assert_eq!(
        deadline_activation_v2_for(readiness, PolicyActivationV1::Production),
        DeadlineActivationV2::ManualOnlyR2f1a
    );
}
