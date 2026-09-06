use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1, SchedulerActivationReadinessV1,
};

#[test]
fn shipped_production_is_armed_while_explicit_disarmed_stays_manual() {
    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, SchedulerActivationReadinessV1::Armed);
    assert_eq!(
        deadline_activation_v2_for(readiness, PolicyActivationV1::Production),
        DeadlineActivationV2::AutomaticR2f1b
    );
    assert_eq!(
        deadline_activation_v2_for(
            SchedulerActivationReadinessV1::Disarmed,
            PolicyActivationV1::Production,
        ),
        DeadlineActivationV2::ManualOnlyR2f1a
    );
}
