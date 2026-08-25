//! Pure progress-epoch and no-progress warning cadence.
//!
//! Callers supply monotonic elapsed milliseconds and already-recorded activity. This module reads
//! no clock, starts no timer, performs no wait, and has no cancellation, impossibility-proof, or
//! terminal-effect constructor. Silence can therefore produce only a warning observation.

use crate::attempt_activity::{
    activity_reason_supports_meaningful_progress_v1, ActivityKind, AttemptActivity,
};

/// The frozen 30-minute no-progress snapshot cadence.
pub const NO_PROGRESS_WARNING_INTERVAL_MS: u64 = 1_800_000;

/// Compute `floor((now - last_meaningful_progress) / 30m)` from supplied monotonic offsets.
#[must_use]
pub const fn no_progress_warning_ordinal_v1(
    now_elapsed_ms: u64,
    last_meaningful_progress_elapsed_ms: u64,
) -> u64 {
    now_elapsed_ms.saturating_sub(last_meaningful_progress_elapsed_ms)
        / NO_PROGRESS_WARNING_INTERVAL_MS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoProgressWarningV1 {
    pub ordinal: u64,
    /// Earlier due ordinals represented by this warning rather than emitted as a catch-up burst.
    pub superseded_ordinal_count: u64,
    pub observed_at_elapsed_ms: u64,
    pub last_activity_elapsed_ms: u64,
    pub last_meaningful_progress_elapsed_ms: u64,
}

/// The complete output of one cadence poll.
///
/// The three negative queries are deliberately constant: this warning-only type cannot carry or
/// construct a cancellation request, mechanical proof, or terminal effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoProgressWarningPollV1 {
    warning: Option<NoProgressWarningV1>,
}

impl NoProgressWarningPollV1 {
    #[must_use]
    pub const fn warning(self) -> Option<NoProgressWarningV1> {
        self.warning
    }

    #[must_use]
    pub const fn cancellation_requested(self) -> bool {
        false
    }

    #[must_use]
    pub const fn mechanical_impossibility_proved(self) -> bool {
        false
    }

    #[must_use]
    pub const fn has_terminal_effect(self) -> bool {
        false
    }
}

/// One attempt-local progress epoch.
///
/// `last_emitted_ordinal` is reset only by meaningful progress. Ordinary activity updates only
/// `last_activity_elapsed_ms`, so a chatty but stuck producer cannot suppress warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoProgressWarningEpochV1 {
    last_activity_elapsed_ms: u64,
    last_meaningful_progress_elapsed_ms: u64,
    last_emitted_ordinal: u64,
}

impl NoProgressWarningEpochV1 {
    #[must_use]
    pub const fn new(epoch_started_elapsed_ms: u64) -> Self {
        Self {
            last_activity_elapsed_ms: epoch_started_elapsed_ms,
            last_meaningful_progress_elapsed_ms: epoch_started_elapsed_ms,
            last_emitted_ordinal: 0,
        }
    }

    /// Observe a record from the attempt's existing total progress classifier.
    ///
    /// A forged `MeaningfulProgress` heartbeat remains activity-only because the reason-level
    /// classifier is reapplied at this boundary. Non-advancing progress-capable reasons already
    /// arrive as [`ActivityKind::Activity`] from the accumulator and do not reset this epoch.
    pub fn observe_activity(&mut self, activity: &AttemptActivity) {
        self.last_activity_elapsed_ms = self.last_activity_elapsed_ms.max(activity.elapsed_ms);
        if activity.kind == ActivityKind::MeaningfulProgress
            && activity_reason_supports_meaningful_progress_v1(activity.reason)
            && activity.elapsed_ms > self.last_meaningful_progress_elapsed_ms
        {
            self.last_meaningful_progress_elapsed_ms = self
                .last_meaningful_progress_elapsed_ms
                .max(activity.elapsed_ms);
            self.last_emitted_ordinal = 0;
        }
    }

    /// Emit the current positive ordinal once within this progress epoch.
    #[must_use]
    pub fn poll_elapsed(&mut self, now_elapsed_ms: u64) -> NoProgressWarningPollV1 {
        let ordinal = no_progress_warning_ordinal_v1(
            now_elapsed_ms,
            self.last_meaningful_progress_elapsed_ms,
        );
        let warning = if ordinal > self.last_emitted_ordinal {
            let superseded_ordinal_count = ordinal
                .saturating_sub(self.last_emitted_ordinal)
                .saturating_sub(1);
            self.last_emitted_ordinal = ordinal;
            Some(NoProgressWarningV1 {
                ordinal,
                superseded_ordinal_count,
                observed_at_elapsed_ms: now_elapsed_ms,
                last_activity_elapsed_ms: self.last_activity_elapsed_ms,
                last_meaningful_progress_elapsed_ms: self.last_meaningful_progress_elapsed_ms,
            })
        } else {
            None
        };
        NoProgressWarningPollV1 { warning }
    }

    #[must_use]
    pub const fn last_activity_elapsed_ms(&self) -> u64 {
        self.last_activity_elapsed_ms
    }

    #[must_use]
    pub const fn last_meaningful_progress_elapsed_ms(&self) -> u64 {
        self.last_meaningful_progress_elapsed_ms
    }
}
