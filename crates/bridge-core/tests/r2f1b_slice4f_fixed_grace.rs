use bridge_core::execution_policy::{
    resolve_execution_policy_v1, resolve_execution_policy_with_readiness_v1,
    scheduler_activation_readiness_v1, ControlEventIdV1, DeadlineActivationV2,
    ExecutionPolicyError as PolicyError, ExecutionPolicyInvocationV1, FanOutPolicyNameV1,
    FanOutPolicyV1, PolicyActivationV1, PolicyNodeRefV1,
    SchedulerActivationReadinessV1 as Readiness, WorkflowControlDefaultsV1,
};
use bridge_core::fixed_grace_timer::{FixedGraceTimerErrorV1 as TimerError, FixedGraceTimerV1};
use bridge_core::ids::AttemptId;

const GRACE_MS: u64 = 30_000;
const ARM_AT_MS: u64 = 1_000;
const NODE_DEADLINE_MS: u64 = 7_200_000;

fn workflow(grace_ms: u64) -> WorkflowControlDefaultsV1 {
    WorkflowControlDefaultsV1 {
        fan_out: Some(FanOutPolicyV1::FixedGrace { grace_ms }),
        ..WorkflowControlDefaultsV1::default()
    }
}

fn resolve(grace_ms: u64, readiness: Readiness) -> Result<DeadlineActivationV2, PolicyError> {
    resolve_execution_policy_with_readiness_v1(
        &workflow(grace_ms),
        &ExecutionPolicyInvocationV1::default(),
        false,
        readiness,
        PolicyActivationV1::Production,
    )
    .map(|controls| controls.deadline_activation)
}

fn trigger_id(ordinal: u32) -> ControlEventIdV1 {
    let attempt = AttemptId::parse("attempt-11111111111111111111111111111111").unwrap();
    ControlEventIdV1::for_attempt(&attempt, ordinal)
}

fn arm(timer: &mut FixedGraceTimerV1, ordinal: u32) -> Result<(), TimerError> {
    timer.arm(
        PolicyNodeRefV1::from_node_id(2, "sibling-review"),
        trigger_id(ordinal),
        GRACE_MS,
        ARM_AT_MS,
        NODE_DEADLINE_MS,
    )
}

fn armed_timer(ordinal: u32) -> FixedGraceTimerV1 {
    let mut timer = FixedGraceTimerV1::new();
    arm(&mut timer, ordinal).unwrap();
    timer
}

#[test]
fn fixed_grace_admission_and_shipped_refusal_are_gated_by_frozen_activation() {
    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, Readiness::Disarmed);
    let inactive = resolve(GRACE_MS, readiness).unwrap_err();
    assert_eq!(inactive, PolicyError::FixedGraceInactive);
    let shipped = resolve_execution_policy_v1(
        &workflow(GRACE_MS),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    );
    assert_eq!(shipped.unwrap_err(), PolicyError::FixedGraceInactive);
    let activation = resolve(GRACE_MS, Readiness::Armed).unwrap();
    assert_eq!(activation, DeadlineActivationV2::AutomaticR2f1b);
    for grace_ms in [0, NODE_DEADLINE_MS + 1] {
        let invalid = resolve(grace_ms, Readiness::Armed).unwrap_err();
        assert_eq!(invalid, PolicyError::InvalidFixedGrace);
    }
}

#[test]
fn timer_arms_once_and_fires_once_without_renewal() {
    let mut timer = armed_timer(0);
    let refused = Err(TimerError::AlreadyArmedOrFired);
    let expires = ARM_AT_MS + GRACE_MS;
    assert_eq!(arm(&mut timer, 1), refused);
    let deadline = timer.recorded_node_deadline_elapsed_ms();
    assert_eq!(deadline, Some(NODE_DEADLINE_MS));
    assert_eq!(timer.observe_elapsed(expires - 1), Ok(None));
    let first = timer.observe_elapsed(expires).unwrap().unwrap();
    assert_eq!(timer.recorded_node_deadline_elapsed_ms(), deadline);
    assert_eq!(first.policy, FanOutPolicyNameV1::FixedGrace);
    assert_eq!(first.grace_ms, Some(GRACE_MS));
    assert_eq!(first.id, trigger_id(0));
    let second_fire = timer.observe_elapsed(expires);
    assert_eq!(second_fire, Err(TimerError::AlreadyFired));
    assert_eq!(arm(&mut timer, 1), refused);
    let second = armed_timer(1).observe_elapsed(expires).unwrap().unwrap();
    assert_ne!(first.id, second.id);
}

#[test]
fn timer_state_accepts_canonical_and_rejects_invalid_literal_wire_bytes() {
    const ARMED: &[u8] = br#"{"state":"armed","schema_version":1,"trigger_id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"grace_ms":30000,"armed_at_elapsed_ms":1000,"recorded_node_deadline_elapsed_ms":7200000}"#;
    const FIRED: &[u8] = br#"{"state":"fired","schema_version":1,"trigger":{"schema_version":1,"id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"policy":"fixed_grace","grace_ms":30000},"armed_at_elapsed_ms":1000,"fired_at_elapsed_ms":31000,"recorded_node_deadline_elapsed_ms":7200000}"#;
    let mut timer: FixedGraceTimerV1 = serde_json::from_slice(ARMED).unwrap();
    assert_eq!(serde_json::to_vec(&timer).unwrap(), ARMED);
    timer.observe_elapsed(ARM_AT_MS + GRACE_MS).unwrap();
    assert_eq!(serde_json::to_vec(&timer).unwrap(), FIRED);
    let restored = serde_json::from_slice::<FixedGraceTimerV1>(FIRED).unwrap();
    assert_eq!(restored, timer);
    const BAD_ARMED_SCHEMA: &[u8] = br#"{"state":"armed","schema_version":2,"trigger_id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"grace_ms":30000,"armed_at_elapsed_ms":1000,"recorded_node_deadline_elapsed_ms":7200000}"#;
    const ZERO_GRACE: &[u8] = br#"{"state":"armed","schema_version":1,"trigger_id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"grace_ms":0,"armed_at_elapsed_ms":1000,"recorded_node_deadline_elapsed_ms":7200000}"#;
    const OVERFLOWING_ARMED: &[u8] = br#"{"state":"armed","schema_version":1,"trigger_id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"grace_ms":1,"armed_at_elapsed_ms":18446744073709551615,"recorded_node_deadline_elapsed_ms":7200000}"#;
    const BAD_FIRED_TRIGGER: &[u8] = br#"{"state":"fired","schema_version":1,"trigger":{"schema_version":1,"id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"policy":"fail_fast","grace_ms":null},"armed_at_elapsed_ms":1000,"fired_at_elapsed_ms":31000,"recorded_node_deadline_elapsed_ms":7200000}"#;
    const EARLY_FIRE: &[u8] = br#"{"state":"fired","schema_version":1,"trigger":{"schema_version":1,"id":"attempt-11111111111111111111111111111111:policy:0","node":{"sorted_ordinal":2,"id_sha256":"8c68e22d2163006919f6c6e467b06fde8983f7841391112c06d7be23377d37fc"},"policy":"fixed_grace","grace_ms":30000},"armed_at_elapsed_ms":1000,"fired_at_elapsed_ms":30999,"recorded_node_deadline_elapsed_ms":7200000}"#;
    for bytes in [
        BAD_ARMED_SCHEMA,
        ZERO_GRACE,
        OVERFLOWING_ARMED,
        BAD_FIRED_TRIGGER,
        EARLY_FIRE,
    ] {
        assert!(serde_json::from_slice::<FixedGraceTimerV1>(bytes).is_err());
    }
}
