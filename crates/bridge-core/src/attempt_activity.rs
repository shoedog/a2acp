//! Bounded R2f0b activity and meaningful-progress recording.
//!
//! Records contain only closed enums and monotonic numeric observations. The
//! default port is synchronous, nonblocking, and no-op.

pub const MAX_ACTIVITY_RECORDS: u32 = 64;
pub const MAX_ATTACHMENT_ENCODING_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    Admission,
    Checkout,
    Configure,
    Provider,
    Adapter,
    Tool,
    Verification,
    Waiter,
    Cleanup,
    TerminalStore,
}

impl AttemptPhase {
    const fn index(self) -> usize {
        match self {
            Self::Admission => 0,
            Self::Checkout => 1,
            Self::Configure => 2,
            Self::Provider => 3,
            Self::Adapter => 4,
            Self::Tool => 5,
            Self::Verification => 6,
            Self::Waiter => 7,
            Self::Cleanup => 8,
            Self::TerminalStore => 9,
        }
    }
}

pub const ATTEMPT_PHASE_COUNT: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityReason {
    PhaseTransition,
    MessageDelta,
    ThoughtDelta,
    UsageHighWater,
    ToolTransition,
    OwnedChildTransition,
    OwnedChildOutput,
    RepositoryOrdinal,
    GateStarted,
    GateExited,
    CompletedSetGrowth,
    ProducerTerminal,
    Heartbeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Activity,
    MeaningfulProgress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttemptActivity {
    pub phase: AttemptPhase,
    pub reason: ActivityReason,
    pub kind: ActivityKind,
    pub elapsed_ms: u64,
    pub advance: u64,
}

/// Synchronous and nonblocking by contract. Implementations must enqueue or
/// tally locally; they may not perform durable or network I/O in `record`.
pub trait AttemptRecorder: Send + Sync {
    fn record(
        &self,
        _phase: AttemptPhase,
        _reason: ActivityReason,
        _advance: u64,
    ) -> Option<AttemptActivity> {
        None
    }

    fn tally(&self) -> Option<ActivityTally> {
        None
    }

    /// Make the attempt tally's overflow marker sticky after a bounded numeric
    /// counter, aggregation, or translation lost information to arithmetic
    /// overflow. The default no-op port ignores it.
    fn mark_overflowed(&self) {}
}

#[derive(Default)]
pub struct NoopAttemptRecorder;
impl AttemptRecorder for NoopAttemptRecorder {}

/// One attempt-scoped recorder owns the sole monotonic clock and all producer
/// high-water marks. Producers submit only bounded facts; they cannot stamp
/// their own elapsed time or accidentally reset the absolute clock.
pub struct SharedAttemptRecorder<C> {
    accumulator: std::sync::Mutex<AttemptActivityAccumulator<C>>,
}

impl<C: MonotonicClock> SharedAttemptRecorder<C> {
    pub fn new(clock: C) -> Self {
        Self {
            accumulator: std::sync::Mutex::new(AttemptActivityAccumulator::new(clock)),
        }
    }
}

impl<C: MonotonicClock> AttemptRecorder for SharedAttemptRecorder<C> {
    fn record(
        &self,
        phase: AttemptPhase,
        reason: ActivityReason,
        advance: u64,
    ) -> Option<AttemptActivity> {
        self.accumulator
            .lock()
            .ok()
            .map(|mut accumulator| accumulator.observe(phase, reason, advance))
    }

    fn tally(&self) -> Option<ActivityTally> {
        self.accumulator.lock().ok().map(|value| value.tally())
    }

    fn mark_overflowed(&self) {
        if let Ok(mut accumulator) = self.accumulator.lock() {
            accumulator.mark_overflowed();
        }
    }
}

pub trait MonotonicClock: Send + Sync {
    fn elapsed_ms(&self) -> u64;
}

pub struct SystemMonotonicClock(std::time::Instant);

impl SystemMonotonicClock {
    pub fn start() -> Self {
        Self(std::time::Instant::now())
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActivityTally {
    pub activity: u32,
    pub meaningful_progress: u32,
    pub phase_activity: [u32; ATTEMPT_PHASE_COUNT],
    pub last_elapsed_ms: u64,
    pub last_progress_elapsed_ms: u64,
    pub max_advance: u64,
    pub overflowed: bool,
}

impl ActivityTally {
    pub fn encoded_len(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |encoded| encoded.len())
    }
}

/// Monotonic accumulator. Advance is compared independently for each
/// `(phase, reason)` cell; elapsed time alone never becomes progress.
pub struct AttemptActivityAccumulator<C> {
    clock: C,
    tally: ActivityTally,
    high_water: [[u64; 13]; ATTEMPT_PHASE_COUNT],
}

impl<C: MonotonicClock> AttemptActivityAccumulator<C> {
    pub fn new(clock: C) -> Self {
        Self {
            clock,
            tally: ActivityTally::default(),
            high_water: [[0; 13]; ATTEMPT_PHASE_COUNT],
        }
    }

    pub fn observe(
        &mut self,
        phase: AttemptPhase,
        reason: ActivityReason,
        proposed_advance: u64,
    ) -> AttemptActivity {
        let elapsed_ms = self.clock.elapsed_ms();
        let reason_index = reason_index(reason);
        let prior = self.high_water[phase.index()][reason_index];
        let progresses = meaningful_reason(reason) && proposed_advance > prior;
        if progresses {
            self.high_water[phase.index()][reason_index] = proposed_advance;
        }
        let kind = if progresses {
            ActivityKind::MeaningfulProgress
        } else {
            ActivityKind::Activity
        };

        if self
            .tally
            .activity
            .saturating_add(self.tally.meaningful_progress)
            >= MAX_ACTIVITY_RECORDS
        {
            self.tally.overflowed = true;
        } else {
            match kind {
                ActivityKind::Activity => {
                    self.tally.activity = self.tally.activity.saturating_add(1);
                }
                ActivityKind::MeaningfulProgress => {
                    self.tally.meaningful_progress =
                        self.tally.meaningful_progress.saturating_add(1);
                    self.tally.last_progress_elapsed_ms = elapsed_ms;
                    self.tally.max_advance = self.tally.max_advance.max(proposed_advance);
                }
            }
            self.tally.phase_activity[phase.index()] =
                self.tally.phase_activity[phase.index()].saturating_add(1);
            self.tally.last_elapsed_ms = self.tally.last_elapsed_ms.max(elapsed_ms);
        }

        AttemptActivity {
            phase,
            reason,
            kind,
            elapsed_ms,
            advance: proposed_advance,
        }
    }

    pub const fn tally(&self) -> ActivityTally {
        self.tally
    }

    /// Sticky: once telemetry lost information to arithmetic overflow, the
    /// tally stays explicitly incomplete.
    pub fn mark_overflowed(&mut self) {
        self.tally.overflowed = true;
    }
}

const fn reason_index(reason: ActivityReason) -> usize {
    match reason {
        ActivityReason::PhaseTransition => 0,
        ActivityReason::MessageDelta => 1,
        ActivityReason::ThoughtDelta => 2,
        ActivityReason::UsageHighWater => 3,
        ActivityReason::ToolTransition => 4,
        ActivityReason::OwnedChildTransition => 5,
        ActivityReason::OwnedChildOutput => 6,
        ActivityReason::RepositoryOrdinal => 7,
        ActivityReason::GateStarted => 8,
        ActivityReason::GateExited => 9,
        ActivityReason::CompletedSetGrowth => 10,
        ActivityReason::ProducerTerminal => 11,
        ActivityReason::Heartbeat => 12,
    }
}

const fn meaningful_reason(reason: ActivityReason) -> bool {
    !matches!(reason, ActivityReason::Heartbeat)
}

/// One fixed-size attempt-scoping owner. It retains the single attempt-global
/// recorder plus the cumulative translation cells, and mints one fresh bounded
/// local comparison scope per logical provider turn so a later smaller real
/// message/thought/usage/tool advance stays meaningful progress.
#[derive(Clone)]
pub struct AttemptScopeOwner {
    recorder: std::sync::Arc<dyn AttemptRecorder>,
    attempt_high_water: std::sync::Arc<std::sync::Mutex<[u64; SCOPED_CELL_COUNT]>>,
}

impl AttemptScopeOwner {
    pub fn new(recorder: std::sync::Arc<dyn AttemptRecorder>) -> Self {
        Self {
            recorder,
            attempt_high_water: std::sync::Arc::new(std::sync::Mutex::new([0; SCOPED_CELL_COUNT])),
        }
    }

    /// The one attempt-global recorder (shared tally and monotonic clock).
    pub fn recorder(&self) -> std::sync::Arc<dyn AttemptRecorder> {
        self.recorder.clone()
    }

    /// A fresh bounded local comparison scope for one logical provider turn.
    pub fn turn_scope(&self) -> std::sync::Arc<dyn AttemptRecorder> {
        std::sync::Arc::new(ScopedAttemptRecorder {
            recorder: self.recorder.clone(),
            attempt_high_water: self.attempt_high_water.clone(),
            local_high_water: std::sync::Mutex::new([0; SCOPED_CELL_COUNT]),
        })
    }
}

#[derive(Clone)]
pub struct AttemptTelemetrySinkFactory {
    scopes: AttemptScopeOwner,
    evidence: std::sync::Arc<crate::terminal_evidence::WorkflowTurnEvidenceCollector>,
    attempt_id: String,
}

pub type AttemptTurnTelemetry = (
    std::sync::Arc<dyn AttemptRecorder>,
    std::sync::Arc<dyn crate::terminal_evidence::TerminalEvidenceSink>,
);

impl AttemptTelemetrySinkFactory {
    pub fn new(attempt_id: impl Into<String>) -> Self {
        Self {
            scopes: AttemptScopeOwner::new(std::sync::Arc::new(SharedAttemptRecorder::new(
                SystemMonotonicClock::start(),
            ))),
            evidence: std::sync::Arc::new(
                crate::terminal_evidence::WorkflowTurnEvidenceCollector::default(),
            ),
            attempt_id: attempt_id.into(),
        }
    }

    pub fn recorder(&self) -> std::sync::Arc<dyn AttemptRecorder> {
        self.scopes.recorder()
    }

    pub fn evidence(
        &self,
    ) -> std::sync::Arc<crate::terminal_evidence::WorkflowTurnEvidenceCollector> {
        self.evidence.clone()
    }

    pub fn telemetry_for_exact_turn(
        &self,
        capability: crate::terminal_evidence::EvidenceCapability,
        generation: u64,
        session_id: &str,
        turn_id: &str,
    ) -> Result<AttemptTurnTelemetry, crate::error::BridgeError> {
        let nonce = crate::attestation::generate_nonce()?;
        let binding = crate::terminal_evidence::TurnEvidenceBinding {
            generation,
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            attempt_id: self.attempt_id.clone(),
            marker_nonce: crate::attestation::nonce_hex(&nonce),
        };
        if !binding.validate() {
            return Err(crate::error::BridgeError::FrameError);
        }
        Ok((
            self.scopes.turn_scope(),
            self.evidence.register(capability, Some(binding)),
        ))
    }
}

const SCOPED_REASON_COUNT: usize = 4;
const SCOPED_CELL_COUNT: usize = ATTEMPT_PHASE_COUNT * SCOPED_REASON_COUNT;

/// One bounded producer-turn scope translates resetting local counters into
/// attempt-global cumulative high waters. Only four closed progress classes
/// and their numeric high waters are retained; no producer identity or content
/// enters the recorder.
struct ScopedAttemptRecorder {
    recorder: std::sync::Arc<dyn AttemptRecorder>,
    attempt_high_water: std::sync::Arc<std::sync::Mutex<[u64; SCOPED_CELL_COUNT]>>,
    local_high_water: std::sync::Mutex<[u64; SCOPED_CELL_COUNT]>,
}

impl AttemptRecorder for ScopedAttemptRecorder {
    fn record(
        &self,
        phase: AttemptPhase,
        reason: ActivityReason,
        advance: u64,
    ) -> Option<AttemptActivity> {
        let Some(scoped_reason) = scoped_reason_index(reason) else {
            return self.recorder.record(phase, reason, advance);
        };
        let index = phase.index() * SCOPED_REASON_COUNT + scoped_reason;
        let mut local = self.local_high_water.lock().ok()?;
        let mut attempt = self.attempt_high_water.lock().ok()?;
        let prior = local[index];
        let translated = if advance > prior {
            local[index] = advance;
            let delta = advance - prior;
            // Overflow is sticky and explicit: a translation that can no
            // longer represent a genuine increment marks the attempt tally
            // incomplete instead of silently saturating.
            match attempt[index].checked_add(delta) {
                Some(next) => {
                    attempt[index] = next;
                    next
                }
                None => {
                    attempt[index] = u64::MAX;
                    self.recorder.mark_overflowed();
                    u64::MAX
                }
            }
        } else {
            attempt[index]
        };
        drop(local);
        let observation = self.recorder.record(phase, reason, translated);
        drop(attempt);
        observation
    }

    fn tally(&self) -> Option<ActivityTally> {
        self.recorder.tally()
    }

    fn mark_overflowed(&self) {
        self.recorder.mark_overflowed();
    }
}

const fn scoped_reason_index(reason: ActivityReason) -> Option<usize> {
    match reason {
        ActivityReason::MessageDelta => Some(0),
        ActivityReason::ThoughtDelta => Some(1),
        ActivityReason::UsageHighWater => Some(2),
        ActivityReason::ToolTransition => Some(3),
        _ => None,
    }
}

struct AttemptTelemetrySink {
    factory: AttemptTelemetrySinkFactory,
    recorder: std::sync::Arc<dyn AttemptRecorder>,
}

#[async_trait::async_trait]
impl crate::ports::RichEventSink for AttemptTelemetrySink {
    fn record(&self, _kind: crate::orch::OrchEventKind) {}

    async fn flush(&self) -> Result<(), crate::error::BridgeError> {
        Ok(())
    }

    fn attempt_recorder(&self) -> Option<std::sync::Arc<dyn AttemptRecorder>> {
        Some(self.recorder.clone())
    }

    fn terminal_evidence_for_turn(
        &self,
        capability: crate::terminal_evidence::EvidenceCapability,
        generation: u64,
        session_id: &str,
        turn_id: &str,
    ) -> Result<
        Option<std::sync::Arc<dyn crate::terminal_evidence::TerminalEvidenceSink>>,
        crate::error::BridgeError,
    > {
        let nonce = crate::attestation::generate_nonce()?;
        let binding = crate::terminal_evidence::TurnEvidenceBinding {
            generation: generation.saturating_add(1),
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            attempt_id: self.factory.attempt_id.clone(),
            marker_nonce: crate::attestation::nonce_hex(&nonce),
        };
        Ok(Some(
            self.factory.evidence.register(capability, Some(binding)),
        ))
    }
}

impl crate::ports::RichEventSinkFactory for AttemptTelemetrySinkFactory {
    fn make(&self, _node: &crate::ids::NodeId) -> std::sync::Arc<dyn crate::ports::RichEventSink> {
        std::sync::Arc::new(AttemptTelemetrySink {
            factory: self.clone(),
            recorder: self.scopes.turn_scope(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeClock(AtomicU64);
    impl MonotonicClock for FakeClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    impl FakeClock {
        fn set(&self, value: u64) {
            self.0.store(value, Ordering::SeqCst);
        }
    }

    #[test]
    fn duplicate_decreasing_empty_and_elapsed_only_input_are_not_progress() {
        let clock = FakeClock(AtomicU64::new(1));
        let mut accumulator = AttemptActivityAccumulator::new(clock);
        assert_eq!(
            accumulator
                .observe(AttemptPhase::Provider, ActivityReason::MessageDelta, 4)
                .kind,
            ActivityKind::MeaningfulProgress
        );
        accumulator.clock.set(2);
        for advance in [4, 3, 0] {
            assert_eq!(
                accumulator
                    .observe(
                        AttemptPhase::Provider,
                        ActivityReason::MessageDelta,
                        advance
                    )
                    .kind,
                ActivityKind::Activity
            );
        }
        accumulator.clock.set(10_000);
        assert_eq!(
            accumulator
                .observe(AttemptPhase::Waiter, ActivityReason::Heartbeat, 99)
                .kind,
            ActivityKind::Activity
        );
    }

    #[test]
    fn tally_encoding_is_conservative_inside_existing_attachment_charge() {
        let worst = ActivityTally {
            activity: u32::MAX,
            meaningful_progress: u32::MAX,
            phase_activity: [u32::MAX; ATTEMPT_PHASE_COUNT],
            last_elapsed_ms: u64::MAX,
            last_progress_elapsed_ms: u64::MAX,
            max_advance: u64::MAX,
            overflowed: true,
        };
        assert!(
            worst.encoded_len() <= MAX_ATTACHMENT_ENCODING_BYTES,
            "fixed tally JSON including field-name overhead must fit the charged attachment"
        );
    }

    #[test]
    fn r2f0b_scope_translation_overflow_is_sticky_explicit_incompleteness() {
        use crate::ports::RichEventSinkFactory;

        let factory = AttemptTelemetrySinkFactory::new("attempt-overflow");
        let node_a = crate::ids::NodeId::parse("scope-a").expect("node id");
        let node_b = crate::ids::NodeId::parse("scope-b").expect("node id");
        let scope_a = factory
            .make(&node_a)
            .attempt_recorder()
            .expect("telemetry sink exposes a scoped recorder");
        let _ = scope_a.record(
            AttemptPhase::Provider,
            ActivityReason::UsageHighWater,
            u64::MAX,
        );
        let scope_b = factory
            .make(&node_b)
            .attempt_recorder()
            .expect("telemetry sink exposes a scoped recorder");
        let _ = scope_b.record(AttemptPhase::Provider, ActivityReason::UsageHighWater, 5);
        let tally = factory.recorder().tally().expect("shared tally");
        assert!(
            tally.overflowed,
            "a local-to-attempt translation overflow must become sticky explicit incompleteness"
        );
    }

    #[test]
    fn r2f0b_fresh_scope_smaller_real_advance_is_cumulative_progress_without_overflow() {
        use crate::ports::RichEventSinkFactory;

        let factory = AttemptTelemetrySinkFactory::new("attempt-cumulative");
        let node_a = crate::ids::NodeId::parse("turn-a").expect("node id");
        let node_b = crate::ids::NodeId::parse("turn-b").expect("node id");
        let scope_a = factory
            .make(&node_a)
            .attempt_recorder()
            .expect("telemetry sink exposes a scoped recorder");
        assert_eq!(
            scope_a
                .record(AttemptPhase::Provider, ActivityReason::MessageDelta, 100)
                .expect("recorded")
                .kind,
            ActivityKind::MeaningfulProgress
        );
        let scope_b = factory
            .make(&node_b)
            .attempt_recorder()
            .expect("telemetry sink exposes a scoped recorder");
        let fresh = scope_b
            .record(AttemptPhase::Provider, ActivityReason::MessageDelta, 5)
            .expect("recorded");
        assert_eq!(
            fresh.kind,
            ActivityKind::MeaningfulProgress,
            "a later smaller real advance in a fresh scope is meaningful progress"
        );
        assert_eq!(fresh.advance, 105, "translation is attempt-cumulative");
        assert_eq!(
            scope_b
                .record(AttemptPhase::Provider, ActivityReason::MessageDelta, 5)
                .expect("recorded")
                .kind,
            ActivityKind::Activity,
            "a duplicate within one scope stays activity"
        );
        let tally = factory.recorder().tally().expect("shared tally");
        assert!(!tally.overflowed);
    }

    #[test]
    fn overflow_is_sticky_and_saturating() {
        let mut accumulator = AttemptActivityAccumulator::new(FakeClock(AtomicU64::new(1)));
        for advance in 1..=MAX_ACTIVITY_RECORDS + 2 {
            accumulator.observe(
                AttemptPhase::Verification,
                ActivityReason::CompletedSetGrowth,
                u64::from(advance),
            );
        }
        let tally = accumulator.tally();
        assert!(tally.overflowed);
        assert_eq!(
            tally.activity.saturating_add(tally.meaningful_progress),
            MAX_ACTIVITY_RECORDS
        );
    }
}
