use bridge_core::attempt_activity::{
    activity_reason_supports_meaningful_progress_v1, ActivityKind, ActivityReason, AttemptActivity,
    AttemptPhase,
};
use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1, SchedulerActivationReadinessV1,
};
use bridge_core::no_progress_warning::{
    no_progress_warning_ordinal_v1, NoProgressWarningEpochV1, NO_PROGRESS_WARNING_INTERVAL_MS,
};

fn activity(reason: ActivityReason, kind: ActivityKind, elapsed_ms: u64) -> AttemptActivity {
    AttemptActivity {
        phase: AttemptPhase::Waiter,
        reason,
        kind,
        elapsed_ms,
        advance: 1,
    }
}

#[test]
fn ordinal_boundaries_are_exact() {
    let interval = NO_PROGRESS_WARNING_INTERVAL_MS;
    assert_eq!(no_progress_warning_ordinal_v1(interval - 1, 0), 0);
    assert_eq!(no_progress_warning_ordinal_v1(interval, 0), 1);
    assert_eq!(no_progress_warning_ordinal_v1(2 * interval - 1, 0), 1);
    assert_eq!(no_progress_warning_ordinal_v1(2 * interval, 0), 2);
}

#[test]
fn ordinal_zero_emits_nothing() {
    let mut epoch = NoProgressWarningEpochV1::new(0);
    let poll = epoch.poll_elapsed(NO_PROGRESS_WARNING_INTERVAL_MS - 1);
    assert_eq!(poll.warning(), None);
}

#[test]
fn positive_ordinal_emits_once_and_duplicate_poll_does_not_reemit() {
    let mut epoch = NoProgressWarningEpochV1::new(0);
    let first = epoch.poll_elapsed(NO_PROGRESS_WARNING_INTERVAL_MS);
    assert_eq!(first.warning().map(|warning| warning.ordinal), Some(1));
    let duplicate = epoch.poll_elapsed(NO_PROGRESS_WARNING_INTERVAL_MS);
    assert_eq!(duplicate.warning(), None);
}

#[test]
fn non_progress_activity_updates_only_activity_clock_and_epoch_keeps_climbing() {
    let interval = NO_PROGRESS_WARNING_INTERVAL_MS;
    let mut epoch = NoProgressWarningEpochV1::new(0);
    assert_eq!(
        epoch
            .poll_elapsed(interval)
            .warning()
            .map(|warning| warning.ordinal),
        Some(1)
    );

    for (offset, reason) in [
        ActivityReason::UsageHighWater,
        ActivityReason::OwnedChildOutput,
        ActivityReason::Heartbeat,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(!activity_reason_supports_meaningful_progress_v1(reason));
        epoch.observe_activity(&activity(
            reason,
            ActivityKind::Activity,
            interval + offset as u64 + 1,
        ));
    }
    assert_eq!(epoch.last_activity_elapsed_ms(), interval + 3);
    assert_eq!(epoch.last_meaningful_progress_elapsed_ms(), 0);
    assert_eq!(
        epoch
            .poll_elapsed(2 * interval)
            .warning()
            .map(|warning| warning.ordinal),
        Some(2)
    );
}

#[test]
fn meaningful_progress_resets_epoch_and_allows_ordinal_to_emit_again() {
    let interval = NO_PROGRESS_WARNING_INTERVAL_MS;
    let mut epoch = NoProgressWarningEpochV1::new(0);
    assert_eq!(
        epoch
            .poll_elapsed(interval)
            .warning()
            .map(|warning| warning.ordinal),
        Some(1)
    );

    let progressed_at = interval + 1;
    epoch.observe_activity(&activity(
        ActivityReason::MessageDelta,
        ActivityKind::MeaningfulProgress,
        progressed_at,
    ));
    assert_eq!(epoch.last_activity_elapsed_ms(), progressed_at);
    assert_eq!(epoch.last_meaningful_progress_elapsed_ms(), progressed_at);
    assert_eq!(
        epoch
            .poll_elapsed(progressed_at + interval)
            .warning()
            .map(|warning| warning.ordinal),
        Some(1),
        "ordinal one may emit again only after a new progress epoch begins"
    );
}

#[test]
fn stale_meaningful_progress_cannot_reopen_an_emitted_ordinal() {
    let interval = NO_PROGRESS_WARNING_INTERVAL_MS;
    let mut epoch = NoProgressWarningEpochV1::new(0);
    let progressed_at = interval + 1;
    epoch.observe_activity(&activity(
        ActivityReason::MessageDelta,
        ActivityKind::MeaningfulProgress,
        progressed_at,
    ));
    assert_eq!(
        epoch
            .poll_elapsed(progressed_at + interval)
            .warning()
            .map(|warning| warning.ordinal),
        Some(1)
    );

    epoch.observe_activity(&activity(
        ActivityReason::MessageDelta,
        ActivityKind::MeaningfulProgress,
        interval,
    ));
    assert_eq!(epoch.last_meaningful_progress_elapsed_ms(), progressed_at);
    assert_eq!(epoch.poll_elapsed(progressed_at + interval).warning(), None);
}

#[test]
fn arbitrarily_long_silence_warns_without_cancellation_proof_or_terminal_effect() {
    let mut epoch = NoProgressWarningEpochV1::new(0);
    let poll = epoch.poll_elapsed(u64::MAX);
    assert_eq!(
        poll.warning().map(|warning| warning.ordinal),
        Some(u64::MAX / NO_PROGRESS_WARNING_INTERVAL_MS)
    );
    assert!(!poll.cancellation_requested());
    assert!(!poll.mechanical_impossibility_proved());
    assert!(!poll.has_terminal_effect());
}

#[test]
fn every_activity_reason_has_an_explicit_progress_classification() {
    let cases = [
        (ActivityReason::PhaseTransition, true),
        (ActivityReason::MessageDelta, true),
        (ActivityReason::ThoughtDelta, true),
        (ActivityReason::UsageHighWater, false),
        (ActivityReason::ToolTransition, true),
        (ActivityReason::OwnedChildTransition, true),
        (ActivityReason::OwnedChildOutput, false),
        (ActivityReason::RepositoryOrdinal, true),
        (ActivityReason::GateStarted, true),
        (ActivityReason::GateExited, true),
        (ActivityReason::CompletedSetGrowth, true),
        (ActivityReason::ProducerTerminal, true),
        (ActivityReason::Heartbeat, false),
    ];
    assert_eq!(cases.len(), 13);
    for (reason, expected) in cases {
        assert_eq!(
            activity_reason_supports_meaningful_progress_v1(reason),
            expected,
            "unexpected classification for {reason:?}"
        );
    }
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
