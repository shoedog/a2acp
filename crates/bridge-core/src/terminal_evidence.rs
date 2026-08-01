//! Bounded, provider-neutral R2f0b terminal-evidence domain.
//!
//! Producer terminality, final-answer presence, prompt RPC disposition, ordered
//! drain, and bridge-owned ACP-child liveness are deliberately independent.

pub const TURN_EVIDENCE_VERSION: &str = "a2a_bridge.turn_evidence.v1";
pub const TURN_EVIDENCE_META_KEY: &str = "a2a_bridge.turn_evidence";
pub const TURN_EVIDENCE_CONTROL_PREFIX: &str = "_a2a_bridge_turn_evidence/";
pub const MAX_CORRELATION_LEN: usize = 128;
pub const MAX_NATIVE_TURN_LEN: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCapability {
    Unsupported,
    MalformedAdvertisement,
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerTerminal {
    Unknown,
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalPresence {
    Unknown,
    Nonempty,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpChildLiveness {
    Unknown,
    Live,
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptRpcObservation {
    Resolved,
    RejectedBeforeAcceptance,
    RejectedAcceptedOrUncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceCompleteness {
    Unsupported,
    Complete,
    Missing,
    Malformed,
    Mismatched,
    Late,
    Conflict,
}

impl EvidenceCompleteness {
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Unsupported | Self::Complete => None,
            Self::Missing => Some("protocol_terminal_evidence_missing"),
            Self::Malformed => Some("protocol_terminal_evidence_malformed"),
            Self::Mismatched => Some("protocol_terminal_evidence_mismatch"),
            Self::Late => Some("protocol_terminal_evidence_late"),
            Self::Conflict => Some("protocol_terminal_evidence_conflict"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnEvidenceBinding {
    pub generation: u64,
    pub session_id: String,
    pub turn_id: String,
    pub attempt_id: String,
    pub marker_nonce: String,
}

impl TurnEvidenceBinding {
    pub fn validate(&self) -> bool {
        self.generation > 0
            && [
                self.session_id.as_str(),
                self.turn_id.as_str(),
                self.attempt_id.as_str(),
                self.marker_nonce.as_str(),
            ]
            .into_iter()
            .all(bounded_correlation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnEvidenceEnvelope {
    pub version: String,
    pub generation: u64,
    pub session_id: String,
    pub turn_id: String,
    pub attempt_id: String,
    pub marker_nonce: String,
    pub native_turn_id: String,
    pub sequence: u32,
    pub producer: ProducerTerminal,
    pub final_presence: FinalPresence,
    pub ordered_notifications_drained: bool,
    pub complete: bool,
}

impl TurnEvidenceEnvelope {
    pub fn intrinsically_valid(&self) -> bool {
        self.version == TURN_EVIDENCE_VERSION
            && self.generation > 0
            && [
                self.session_id.as_str(),
                self.turn_id.as_str(),
                self.attempt_id.as_str(),
                self.marker_nonce.as_str(),
            ]
            .into_iter()
            .all(bounded_correlation)
            && self.sequence > 0
            && bounded(self.native_turn_id.as_str(), MAX_NATIVE_TURN_LEN)
            && self.complete
    }

    pub fn validates_for(&self, binding: &TurnEvidenceBinding) -> bool {
        binding.validate()
            && self.intrinsically_valid()
            && self.generation == binding.generation
            && self.session_id == binding.session_id
            && self.turn_id == binding.turn_id
            && self.attempt_id == binding.attempt_id
            && self.marker_nonce == binding.marker_nonce
    }
}

fn bounded_correlation(value: &str) -> bool {
    bounded(value, MAX_CORRELATION_LEN)
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceAcceptance {
    Accepted,
    IdenticalReplay,
    Rejected(EvidenceCompleteness),
}

/// One ordered, generation-scoped evidence consumer. Any malformed, mismatched,
/// late, or conflicting observation is sticky and cannot be repaired.
#[derive(Clone, Debug)]
pub struct TurnEvidenceConsumer {
    capability: EvidenceCapability,
    declaration: Option<EvidenceCapability>,
    binding: Option<TurnEvidenceBinding>,
    accepted: Option<TurnEvidenceEnvelope>,
    sticky: Option<EvidenceCompleteness>,
    closed: bool,
}

impl TurnEvidenceConsumer {
    pub fn unsupported() -> Self {
        Self {
            capability: EvidenceCapability::Unsupported,
            declaration: None,
            binding: None,
            accepted: None,
            sticky: None,
            closed: false,
        }
    }

    pub fn negotiated_v1(binding: TurnEvidenceBinding) -> Self {
        let mut consumer = Self::dormant(binding);
        consumer.declare_capability(EvidenceCapability::V1);
        consumer
    }

    pub fn dormant(binding: TurnEvidenceBinding) -> Self {
        Self {
            capability: EvidenceCapability::Unsupported,
            declaration: None,
            binding: Some(binding),
            accepted: None,
            sticky: None,
            closed: false,
        }
    }

    fn prepare_binding(&mut self, binding: TurnEvidenceBinding) {
        if self.closed
            || self.accepted.is_some()
            || self.sticky.is_some()
            || self.declaration.is_some()
        {
            return;
        }
        match self.binding.as_ref() {
            None => self.binding = Some(binding),
            Some(existing) if existing == &binding => {}
            Some(_) => {
                self.capability = EvidenceCapability::V1;
                self.sticky = Some(EvidenceCompleteness::Conflict);
            }
        }
    }

    fn declare_capability(&mut self, declaration: EvidenceCapability) {
        if self.closed {
            return;
        }
        if let Some(prior) = self.declaration {
            if prior == declaration {
                return;
            }
            self.capability = EvidenceCapability::V1;
            if self.sticky.is_none() {
                self.sticky = Some(EvidenceCompleteness::Conflict);
            }
            return;
        }
        self.declaration = Some(declaration);
        match declaration {
            EvidenceCapability::Unsupported => {
                self.capability = EvidenceCapability::Unsupported;
            }
            EvidenceCapability::MalformedAdvertisement => {
                self.capability = EvidenceCapability::V1;
                if self.sticky.is_none() {
                    self.sticky = Some(EvidenceCompleteness::Malformed);
                }
            }
            EvidenceCapability::V1 => {
                self.capability = EvidenceCapability::V1;
                let binding_valid = self
                    .binding
                    .as_ref()
                    .is_some_and(TurnEvidenceBinding::validate);
                if !binding_valid && self.sticky.is_none() {
                    self.sticky = Some(EvidenceCompleteness::Malformed);
                }
            }
        }
    }

    pub fn accept(&mut self, envelope: TurnEvidenceEnvelope) -> EvidenceAcceptance {
        if self.capability != EvidenceCapability::V1 {
            return EvidenceAcceptance::Rejected(EvidenceCompleteness::Unsupported);
        }
        if self.closed {
            if let Some(sticky) = self.sticky {
                if sticky != EvidenceCompleteness::Missing {
                    return EvidenceAcceptance::Rejected(sticky);
                }
            }
            let Some(binding) = self.binding.as_ref() else {
                return EvidenceAcceptance::Rejected(EvidenceCompleteness::Malformed);
            };
            let disposition = if envelope.validates_for(binding) {
                EvidenceCompleteness::Late
            } else if !envelope.intrinsically_valid() {
                EvidenceCompleteness::Malformed
            } else {
                EvidenceCompleteness::Mismatched
            };
            // The bounded closed-turn tombstone classifies the frame without
            // reopening the turn or replacing its already sealed observation.
            return EvidenceAcceptance::Rejected(disposition);
        }
        if let Some(sticky) = self.sticky {
            return EvidenceAcceptance::Rejected(sticky);
        }
        let Some(binding) = self.binding.as_ref() else {
            self.sticky = Some(EvidenceCompleteness::Malformed);
            return EvidenceAcceptance::Rejected(EvidenceCompleteness::Malformed);
        };
        if !envelope.validates_for(binding) {
            let disposition = if !envelope.intrinsically_valid() {
                EvidenceCompleteness::Malformed
            } else {
                EvidenceCompleteness::Mismatched
            };
            self.sticky = Some(disposition);
            return EvidenceAcceptance::Rejected(disposition);
        }
        // Duplicate identity is decided on the untouched raw envelope: raw
        // `absent` versus raw `unknown` is a permanent conflict, never an
        // identical replay. The conservative consumer downgrade happens only
        // in `observation`, after this classification.
        match self.accepted.as_ref() {
            None => {
                self.accepted = Some(envelope);
                EvidenceAcceptance::Accepted
            }
            Some(prior) if prior == &envelope => EvidenceAcceptance::IdenticalReplay,
            Some(_) => {
                self.sticky = Some(EvidenceCompleteness::Conflict);
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Conflict)
            }
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
        if self.capability == EvidenceCapability::V1
            && self.accepted.is_none()
            && self.sticky.is_none()
        {
            self.sticky = Some(EvidenceCompleteness::Missing);
        }
    }

    pub fn observation(&self) -> (EvidenceCompleteness, ProducerTerminal, FinalPresence, bool) {
        if self.capability == EvidenceCapability::Unsupported {
            return (
                EvidenceCompleteness::Unsupported,
                ProducerTerminal::Unknown,
                FinalPresence::Unknown,
                false,
            );
        }
        if let Some(sticky) = self.sticky {
            return (
                sticky,
                ProducerTerminal::Unknown,
                FinalPresence::Unknown,
                false,
            );
        }
        match self.accepted.as_ref() {
            Some(value) => {
                // Unauthoritative `final=absent` (producer not completed, or
                // ordered notifications not drained) downgrades to unknown in
                // this consumer observation only; the stored raw envelope
                // stays the untouched replay/conflict identity.
                let final_presence = if value.final_presence == FinalPresence::Absent
                    && (value.producer != ProducerTerminal::Completed
                        || !value.ordered_notifications_drained)
                {
                    FinalPresence::Unknown
                } else {
                    value.final_presence
                };
                (
                    EvidenceCompleteness::Complete,
                    value.producer,
                    final_presence,
                    value.ordered_notifications_drained,
                )
            }
            None => (
                EvidenceCompleteness::Missing,
                ProducerTerminal::Unknown,
                FinalPresence::Unknown,
                false,
            ),
        }
    }
}

/// Synchronous ordered-control sink shared by the adapter and durable attempt
/// owner. The mutex protects only a bounded in-memory state machine.
pub trait TerminalEvidenceSink: Send + Sync {
    /// Additive prompt-time declaration; legacy sinks conservatively ignore it.
    fn declare_capability(&self, _capability: EvidenceCapability) {}

    fn binding(&self) -> Option<TurnEvidenceBinding>;
    fn accept(&self, envelope: TurnEvidenceEnvelope) -> EvidenceAcceptance;
    fn reject(&self, disposition: EvidenceCompleteness);
    fn close(&self);
    fn observation(&self) -> (EvidenceCompleteness, ProducerTerminal, FinalPresence, bool);
    fn capability(&self) -> EvidenceCapability;
    fn record_child_liveness(&self, liveness: AcpChildLiveness);
    fn child_liveness(&self) -> AcpChildLiveness;
    fn record_deliverable_final(&self);
    fn deliverable_final_present(&self) -> bool;
}

#[derive(Debug)]
pub struct SharedTurnEvidence {
    consumer: std::sync::Mutex<TurnEvidenceConsumer>,
    child_liveness: std::sync::Mutex<AcpChildLiveness>,
    deliverable_final_present: std::sync::atomic::AtomicBool,
}

impl SharedTurnEvidence {
    pub fn unsupported() -> Self {
        Self {
            consumer: std::sync::Mutex::new(TurnEvidenceConsumer::unsupported()),
            child_liveness: std::sync::Mutex::new(AcpChildLiveness::Unknown),
            deliverable_final_present: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn dormant(binding: TurnEvidenceBinding) -> Self {
        Self {
            consumer: std::sync::Mutex::new(TurnEvidenceConsumer::dormant(binding)),
            child_liveness: std::sync::Mutex::new(AcpChildLiveness::Unknown),
            deliverable_final_present: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn prepare_binding(&self, binding: TurnEvidenceBinding) {
        if let Ok(mut consumer) = self.consumer.lock() {
            consumer.prepare_binding(binding);
        }
    }

    pub fn configure_v1(&self, binding: TurnEvidenceBinding) {
        if let Ok(mut consumer) = self.consumer.lock() {
            consumer.prepare_binding(binding);
            consumer.declare_capability(EvidenceCapability::V1);
        }
    }

    pub fn configure_malformed_advertisement(&self) {
        if let Ok(mut consumer) = self.consumer.lock() {
            consumer.declare_capability(EvidenceCapability::MalformedAdvertisement);
        }
    }
}

impl TerminalEvidenceSink for SharedTurnEvidence {
    fn declare_capability(&self, capability: EvidenceCapability) {
        if let Ok(mut consumer) = self.consumer.lock() {
            consumer.declare_capability(capability);
        }
    }

    fn binding(&self) -> Option<TurnEvidenceBinding> {
        self.consumer
            .lock()
            .ok()
            .and_then(|value| value.binding.clone())
    }

    fn accept(&self, envelope: TurnEvidenceEnvelope) -> EvidenceAcceptance {
        self.consumer.lock().map_or(
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Malformed),
            |mut value| value.accept(envelope),
        )
    }

    fn reject(&self, disposition: EvidenceCompleteness) {
        if let Ok(mut value) = self.consumer.lock() {
            if value.capability == EvidenceCapability::V1 && !value.closed && value.sticky.is_none()
            {
                value.sticky = Some(disposition);
            }
        }
    }

    fn close(&self) {
        if let Ok(mut value) = self.consumer.lock() {
            value.close();
        }
    }

    fn observation(&self) -> (EvidenceCompleteness, ProducerTerminal, FinalPresence, bool) {
        self.consumer.lock().map_or(
            (
                EvidenceCompleteness::Malformed,
                ProducerTerminal::Unknown,
                FinalPresence::Unknown,
                false,
            ),
            |value| value.observation(),
        )
    }

    fn capability(&self) -> EvidenceCapability {
        self.consumer
            .lock()
            .map_or(EvidenceCapability::Unsupported, |value| value.capability)
    }

    fn record_child_liveness(&self, liveness: AcpChildLiveness) {
        if let Ok(mut value) = self.child_liveness.lock() {
            if *value == AcpChildLiveness::Unknown || liveness != AcpChildLiveness::Unknown {
                *value = liveness;
            }
        }
    }

    fn child_liveness(&self) -> AcpChildLiveness {
        self.child_liveness
            .lock()
            .map_or(AcpChildLiveness::Unknown, |value| *value)
    }

    fn record_deliverable_final(&self) {
        self.deliverable_final_present
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn deliverable_final_present(&self) -> bool {
        self.deliverable_final_present
            .load(std::sync::atomic::Ordering::Acquire)
    }
}

#[derive(Default)]
struct WorkflowTurnEvidenceState {
    turns: Vec<std::sync::Arc<SharedTurnEvidence>>,
    overflowed: bool,
}

#[derive(Default)]
pub struct WorkflowTurnEvidenceCollector {
    state: std::sync::Mutex<WorkflowTurnEvidenceState>,
}

impl WorkflowTurnEvidenceCollector {
    pub fn register(
        &self,
        capability: EvidenceCapability,
        binding: Option<TurnEvidenceBinding>,
    ) -> std::sync::Arc<dyn TerminalEvidenceSink> {
        let sink = std::sync::Arc::new(match binding {
            Some(binding) => SharedTurnEvidence::dormant(binding),
            None => SharedTurnEvidence::unsupported(),
        });
        match capability {
            EvidenceCapability::V1 | EvidenceCapability::MalformedAdvertisement => {
                sink.declare_capability(capability);
            }
            EvidenceCapability::Unsupported => {}
        }
        if let Ok(mut state) = self.state.lock() {
            if state.turns.len()
                < usize::try_from(TerminalEvidenceCounts::MAX_REACHED).unwrap_or(usize::MAX)
            {
                state.turns.push(std::sync::Arc::clone(&sink));
            } else {
                // The collector is a fixed-capacity evidence attachment. Once
                // another real provider turn is reached, preserve that loss of
                // fidelity explicitly without retaining another sink.
                state.overflowed = true;
            }
        }
        sink
    }

    pub fn single_turn(
        &self,
    ) -> Option<(
        EvidenceCapability,
        EvidenceCompleteness,
        ProducerTerminal,
        FinalPresence,
        bool,
        AcpChildLiveness,
        bool,
    )> {
        let state = self.state.lock().ok()?;
        if state.overflowed {
            return None;
        }
        let [turn] = state.turns.as_slice() else {
            return None;
        };
        let (completeness, producer, final_presence, drained) = turn.observation();
        Some((
            turn.capability(),
            completeness,
            producer,
            final_presence,
            drained,
            turn.child_liveness(),
            turn.deliverable_final_present(),
        ))
    }

    pub fn counts(&self) -> TerminalEvidenceCounts {
        let Ok(state) = self.state.lock() else {
            return TerminalEvidenceCounts::default();
        };
        let reached = u32::try_from(state.turns.len())
            .unwrap_or(u32::MAX)
            .min(TerminalEvidenceCounts::MAX_REACHED);
        let mut counts = TerminalEvidenceCounts {
            reached,
            overflowed: state.overflowed,
            ..TerminalEvidenceCounts::default()
        };
        for turn in state
            .turns
            .iter()
            .take(usize::try_from(reached).unwrap_or(usize::MAX))
        {
            match turn.observation().0 {
                EvidenceCompleteness::Complete => {
                    counts.valid = counts.valid.saturating_add(1);
                }
                EvidenceCompleteness::Missing => {
                    counts.missing = counts.missing.saturating_add(1);
                }
                EvidenceCompleteness::Malformed
                | EvidenceCompleteness::Mismatched
                | EvidenceCompleteness::Late
                | EvidenceCompleteness::Conflict => {
                    counts.invalid = counts.invalid.saturating_add(1);
                }
                EvidenceCompleteness::Unsupported => {}
            }
        }
        counts
    }
    pub fn turn_count(&self) -> usize {
        self.state.lock().map_or(0, |state| state.turns.len())
    }

    /// Seal every registered turn so a negotiated leg without an envelope
    /// becomes sticky `Missing` before counts or a single-turn projection are
    /// read. Idempotent.
    pub fn close_all(&self) {
        if let Ok(state) = self.state.lock() {
            for turn in state.turns.iter() {
                TerminalEvidenceSink::close(turn.as_ref());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalEvidenceCounts {
    pub reached: u32,
    pub valid: u32,
    pub missing: u32,
    pub invalid: u32,
    #[serde(default)]
    pub overflowed: bool,
}

impl TerminalEvidenceCounts {
    pub const MAX_REACHED: u32 = 1_024;

    pub fn validate(self) -> bool {
        self.reached <= Self::MAX_REACHED
            && (!self.overflowed || self.reached == Self::MAX_REACHED)
            && self.valid <= self.reached
            && self.missing <= self.reached
            && self.invalid <= self.reached
            && self
                .valid
                .saturating_add(self.missing)
                .saturating_add(self.invalid)
                <= self.reached
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedOutcome {
    Completed,
    Failed,
    Interrupted,
}

impl ResolvedOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalReason {
    CompletedFinal,
    LegacyWireCompleted,
    PromptRejectedBeforeAcceptance,
    ProducerFailed,
    ProducerInterrupted,
    ProtocolIncompleteFinal,
    ProtocolFinalDeliveryConflict,
    ProtocolTerminalUnknown,
    Evidence(EvidenceCompleteness),
}

impl TerminalReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompletedFinal => "completed_final",
            Self::LegacyWireCompleted => "completed",
            Self::PromptRejectedBeforeAcceptance => "prompt_rejected_before_acceptance",
            Self::ProducerFailed => "producer_failed",
            Self::ProducerInterrupted => "producer_interrupted",
            Self::ProtocolIncompleteFinal => "protocol_incomplete_final",
            Self::ProtocolFinalDeliveryConflict => "protocol_final_delivery_conflict",
            Self::ProtocolTerminalUnknown => "protocol_terminal_unknown",
            Self::Evidence(value) => match value.reason() {
                Some(reason) => reason,
                None => "protocol_terminal_unknown",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalObservation {
    pub capability: EvidenceCapability,
    pub completeness: EvidenceCompleteness,
    pub producer: ProducerTerminal,
    pub final_presence: FinalPresence,
    pub prompt_rpc: PromptRpcObservation,
    pub ordered_notifications_drained: bool,
    pub deliverable_final_present: bool,
    pub child_liveness: AcpChildLiveness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalResolution {
    pub outcome: ResolvedOutcome,
    pub reason: TerminalReason,
    pub producer: ProducerTerminal,
    pub final_presence: FinalPresence,
    pub child_liveness: AcpChildLiveness,
}

/// The single pure resolver for terminal facts. Child liveness is projected but
/// intentionally absent from every decision branch.
pub fn resolve_terminal(observation: TerminalObservation) -> TerminalResolution {
    let pair = match observation.completeness {
        EvidenceCompleteness::Malformed
        | EvidenceCompleteness::Mismatched
        | EvidenceCompleteness::Late
        | EvidenceCompleteness::Conflict
        | EvidenceCompleteness::Missing => (
            ResolvedOutcome::Failed,
            TerminalReason::Evidence(observation.completeness),
        ),
        EvidenceCompleteness::Unsupported => match observation.prompt_rpc {
            PromptRpcObservation::Resolved if observation.deliverable_final_present => (
                ResolvedOutcome::Completed,
                TerminalReason::LegacyWireCompleted,
            ),
            PromptRpcObservation::RejectedBeforeAcceptance => (
                ResolvedOutcome::Failed,
                TerminalReason::PromptRejectedBeforeAcceptance,
            ),
            PromptRpcObservation::Resolved | PromptRpcObservation::RejectedAcceptedOrUncertain => (
                ResolvedOutcome::Failed,
                TerminalReason::ProtocolTerminalUnknown,
            ),
        },
        EvidenceCompleteness::Complete => match observation.producer {
            ProducerTerminal::Failed => (ResolvedOutcome::Failed, TerminalReason::ProducerFailed),
            ProducerTerminal::Interrupted => (
                ResolvedOutcome::Interrupted,
                TerminalReason::ProducerInterrupted,
            ),
            ProducerTerminal::Unknown => (
                ResolvedOutcome::Failed,
                TerminalReason::ProtocolTerminalUnknown,
            ),
            ProducerTerminal::Completed => match observation.final_presence {
                FinalPresence::Absent if observation.ordered_notifications_drained => (
                    ResolvedOutcome::Failed,
                    TerminalReason::ProtocolIncompleteFinal,
                ),
                FinalPresence::Nonempty
                    if observation.deliverable_final_present
                        && observation.prompt_rpc == PromptRpcObservation::Resolved =>
                {
                    (ResolvedOutcome::Completed, TerminalReason::CompletedFinal)
                }
                FinalPresence::Nonempty => (
                    ResolvedOutcome::Failed,
                    TerminalReason::ProtocolFinalDeliveryConflict,
                ),
                FinalPresence::Absent | FinalPresence::Unknown => (
                    ResolvedOutcome::Failed,
                    TerminalReason::ProtocolTerminalUnknown,
                ),
            },
        },
    };
    TerminalResolution {
        outcome: pair.0,
        reason: pair.1,
        producer: observation.producer,
        final_presence: observation.final_presence,
        child_liveness: observation.child_liveness,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> TurnEvidenceBinding {
        TurnEvidenceBinding {
            generation: 7,
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            attempt_id: "attempt-1".into(),
            marker_nonce: "nonce-1".into(),
        }
    }

    fn envelope() -> TurnEvidenceEnvelope {
        let binding = binding();
        TurnEvidenceEnvelope {
            version: TURN_EVIDENCE_VERSION.into(),
            generation: binding.generation,
            session_id: binding.session_id,
            turn_id: binding.turn_id,
            attempt_id: binding.attempt_id,
            marker_nonce: binding.marker_nonce,
            native_turn_id: "native-1".into(),
            sequence: 1,
            producer: ProducerTerminal::Completed,
            final_presence: FinalPresence::Nonempty,
            ordered_notifications_drained: true,
            complete: true,
        }
    }

    #[test]
    fn identical_replay_is_idempotent_and_conflict_is_sticky() {
        let mut consumer = TurnEvidenceConsumer::negotiated_v1(binding());
        let first = envelope();
        assert_eq!(consumer.accept(first.clone()), EvidenceAcceptance::Accepted);
        assert_eq!(
            consumer.accept(first.clone()),
            EvidenceAcceptance::IdenticalReplay
        );
        let mut conflict = first;
        conflict.sequence = 2;
        assert_eq!(
            consumer.accept(conflict),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Conflict)
        );
        assert_eq!(
            consumer.accept(envelope()),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Conflict)
        );
    }

    #[test]
    fn missing_mismatch_and_late_cannot_be_repaired() {
        let mut missing = TurnEvidenceConsumer::negotiated_v1(binding());
        missing.close();
        assert_eq!(missing.observation().0, EvidenceCompleteness::Missing);
        assert_eq!(
            missing.accept(envelope()),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Late)
        );

        let mut mismatched = TurnEvidenceConsumer::negotiated_v1(binding());
        let mut wrong = envelope();
        wrong.attempt_id = "attempt-2".into();
        assert_eq!(
            mismatched.accept(wrong),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Mismatched)
        );
        assert_eq!(
            mismatched.accept(envelope()),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Mismatched)
        );

        let mut late = TurnEvidenceConsumer::negotiated_v1(binding());
        late.close();
        assert_eq!(
            late.accept(envelope()),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Late)
        );
    }

    #[test]
    fn r2f0b_intrinsic_shape_matrix_is_malformed_and_correlation_is_mismatched() {
        fn changed(mutate: impl FnOnce(&mut TurnEvidenceEnvelope)) -> TurnEvidenceEnvelope {
            let mut value = envelope();
            mutate(&mut value);
            value
        }

        let malformed = vec![
            ("version", changed(|v| v.version = "v0".into())),
            ("sequence", changed(|v| v.sequence = 0)),
            ("completeness", changed(|v| v.complete = false)),
            ("generation", changed(|v| v.generation = 0)),
            ("native empty", changed(|v| v.native_turn_id.clear())),
            (
                "native overlong",
                changed(|v| v.native_turn_id = "x".repeat(MAX_NATIVE_TURN_LEN + 1)),
            ),
            (
                "native invalid",
                changed(|v| v.native_turn_id = "native/turn".into()),
            ),
            ("session empty", changed(|v| v.session_id.clear())),
            (
                "session overlong",
                changed(|v| v.session_id = "x".repeat(MAX_CORRELATION_LEN + 1)),
            ),
            (
                "session invalid",
                changed(|v| v.session_id = "session/1".into()),
            ),
            ("turn empty", changed(|v| v.turn_id.clear())),
            (
                "turn overlong",
                changed(|v| v.turn_id = "x".repeat(MAX_CORRELATION_LEN + 1)),
            ),
            ("turn invalid", changed(|v| v.turn_id = "turn/1".into())),
            ("attempt empty", changed(|v| v.attempt_id.clear())),
            (
                "attempt overlong",
                changed(|v| v.attempt_id = "x".repeat(MAX_CORRELATION_LEN + 1)),
            ),
            (
                "attempt invalid",
                changed(|v| v.attempt_id = "attempt/1".into()),
            ),
            ("nonce empty", changed(|v| v.marker_nonce.clear())),
            (
                "nonce overlong",
                changed(|v| v.marker_nonce = "x".repeat(MAX_CORRELATION_LEN + 1)),
            ),
            (
                "nonce invalid",
                changed(|v| v.marker_nonce = "nonce/1".into()),
            ),
        ];
        for (name, value) in malformed {
            let mut open = TurnEvidenceConsumer::negotiated_v1(binding());
            assert_eq!(
                open.accept(value.clone()),
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Malformed),
                "open {name}"
            );
            let mut closed = TurnEvidenceConsumer::negotiated_v1(binding());
            closed.close();
            assert_eq!(
                closed.accept(value),
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Malformed),
                "closed {name}"
            );
        }

        let mismatched = vec![
            changed(|v| v.generation = 8),
            changed(|v| v.session_id = "session-2".into()),
            changed(|v| v.turn_id = "turn-2".into()),
            changed(|v| v.attempt_id = "attempt-2".into()),
            changed(|v| v.marker_nonce = "nonce-2".into()),
        ];
        for value in mismatched {
            let mut open = TurnEvidenceConsumer::negotiated_v1(binding());
            assert_eq!(
                open.accept(value.clone()),
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Mismatched)
            );
            let mut closed = TurnEvidenceConsumer::negotiated_v1(binding());
            closed.close();
            assert_eq!(
                closed.accept(value),
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Mismatched)
            );
        }
    }
    #[test]
    fn r2f0b_exact_post_close_envelope_is_late_without_reopening_missing() {
        let mut consumer = TurnEvidenceConsumer::negotiated_v1(binding());
        consumer.close();
        assert_eq!(consumer.observation().0, EvidenceCompleteness::Missing);

        assert_eq!(
            consumer.accept(envelope()),
            EvidenceAcceptance::Rejected(EvidenceCompleteness::Late)
        );
        assert_eq!(consumer.observation().0, EvidenceCompleteness::Missing);
    }

    #[test]
    fn absent_is_downgraded_without_completed_ordered_drain() {
        let mut consumer = TurnEvidenceConsumer::negotiated_v1(binding());
        let mut value = envelope();
        value.producer = ProducerTerminal::Failed;
        value.final_presence = FinalPresence::Absent;
        value.ordered_notifications_drained = false;
        assert_eq!(consumer.accept(value), EvidenceAcceptance::Accepted);
        assert_eq!(consumer.observation().2, FinalPresence::Unknown);

        let mut authoritative = TurnEvidenceConsumer::negotiated_v1(binding());
        let mut value = envelope();
        value.final_presence = FinalPresence::Absent;
        assert_eq!(authoritative.accept(value), EvidenceAcceptance::Accepted);
        assert_eq!(authoritative.observation().2, FinalPresence::Absent);
    }

    #[test]
    fn r2f0b_raw_absent_versus_raw_unknown_duplicate_is_sticky_conflict() {
        // Duplicate identity is decided on the untouched raw envelope: a second
        // envelope whose only difference is raw `absent` versus raw `unknown`
        // final presence is a permanent conflict, never an identical replay.
        for (first_final, second_final) in [
            (FinalPresence::Absent, FinalPresence::Unknown),
            (FinalPresence::Unknown, FinalPresence::Absent),
        ] {
            let mut consumer = TurnEvidenceConsumer::negotiated_v1(binding());
            let mut first = envelope();
            first.producer = ProducerTerminal::Failed;
            first.final_presence = first_final;
            first.ordered_notifications_drained = false;
            assert_eq!(consumer.accept(first.clone()), EvidenceAcceptance::Accepted);
            let mut second = first;
            second.final_presence = second_final;
            assert_eq!(
                consumer.accept(second),
                EvidenceAcceptance::Rejected(EvidenceCompleteness::Conflict),
                "raw {first_final:?} then raw {second_final:?} must be a conflict"
            );
            let observation = consumer.observation();
            assert_eq!(observation.0, EvidenceCompleteness::Conflict);
            assert_eq!(observation.1, ProducerTerminal::Unknown);
            assert_eq!(observation.2, FinalPresence::Unknown);
        }
    }

    #[test]
    fn r2f0b_identical_raw_absent_replay_stays_idempotent_with_conservative_observation() {
        let mut consumer = TurnEvidenceConsumer::negotiated_v1(binding());
        let mut value = envelope();
        value.producer = ProducerTerminal::Failed;
        value.final_presence = FinalPresence::Absent;
        value.ordered_notifications_drained = false;
        assert_eq!(consumer.accept(value.clone()), EvidenceAcceptance::Accepted);
        assert_eq!(consumer.accept(value), EvidenceAcceptance::IdenticalReplay);
        let observation = consumer.observation();
        assert_eq!(observation.0, EvidenceCompleteness::Complete);
        assert_eq!(observation.1, ProducerTerminal::Failed);
        assert_eq!(
            observation.2,
            FinalPresence::Unknown,
            "unauthoritative raw absent is downgraded only in the consumer observation"
        );
        assert!(!observation.3);
    }

    #[test]
    fn unsupported_success_preserves_legacy_wire_outcome_without_inventing_facts() {
        let resolved = resolve_terminal(TerminalObservation {
            capability: EvidenceCapability::Unsupported,
            completeness: EvidenceCompleteness::Unsupported,
            producer: ProducerTerminal::Unknown,
            final_presence: FinalPresence::Unknown,
            prompt_rpc: PromptRpcObservation::Resolved,
            ordered_notifications_drained: true,
            deliverable_final_present: true,
            child_liveness: AcpChildLiveness::Exited,
        });
        assert_eq!(resolved.outcome, ResolvedOutcome::Completed);
        assert_eq!(resolved.reason, TerminalReason::LegacyWireCompleted);
        assert_eq!(resolved.producer, ProducerTerminal::Unknown);
        assert_eq!(resolved.final_presence, FinalPresence::Unknown);
    }

    #[test]
    fn r2f0b_workflow_collector_retains_exact_cap_and_marks_later_loss() {
        let collector = WorkflowTurnEvidenceCollector::default();
        for _ in 0..TerminalEvidenceCounts::MAX_REACHED {
            collector.register(EvidenceCapability::Unsupported, None);
        }
        assert_eq!(
            collector.turn_count(),
            TerminalEvidenceCounts::MAX_REACHED as usize
        );
        let at_cap = collector.counts();
        assert_eq!(at_cap.reached, TerminalEvidenceCounts::MAX_REACHED);
        assert!(!at_cap.overflowed);

        collector.register(EvidenceCapability::Unsupported, None);
        assert_eq!(
            collector.turn_count(),
            TerminalEvidenceCounts::MAX_REACHED as usize,
            "the bounded collector must not retain an unbounded hidden tail"
        );
        let overflowed = collector.counts();
        assert_eq!(overflowed.reached, TerminalEvidenceCounts::MAX_REACHED);
        assert!(overflowed.overflowed, "loss beyond the cap must be sticky");
        assert!(collector.single_turn().is_none());
        assert!(overflowed.validate());
    }
}
