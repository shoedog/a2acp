//! Bounded R2f0a workflow-attempt history domain and storage port.
use crate::ids::{AttemptId, AttemptIdentity, NodeId, TaskId};

pub const RETENTION_DAYS: i64 = 180;
pub const MAX_TERMINAL_ROWS: u64 = 100_000;
pub const MAX_CHARGED_BYTES: u64 = 128 * 1024 * 1024;
/// Fixed retained summary charge, including its pre-reserved terminal payload.
pub const RESERVED_ROW_CHARGE: u64 = 16 * 1024;
pub const RETAINED_SUMMARY_CHARGE: u64 = RESERVED_ROW_CHARGE;
/// Fixed charge for the reclaimable history attachment. Permanent identity rows are uncharged.
pub const PERMANENT_IDENTITY_CHARGE: u64 = 1024;
pub const HISTORY_ATTACHMENT_CHARGE: u64 = PERMANENT_IDENTITY_CHARGE;
pub const CONFIGURED_SLOT_CHARGE: u64 = RETAINED_SUMMARY_CHARGE + HISTORY_ATTACHMENT_CHARGE;
pub const MAX_DIMENSION_LEN: usize = 64;
pub const MAX_FINGERPRINT_LEN: usize = 128;
pub const MAX_PHASES: usize = 32;
pub const MAX_RESERVATION_JSON_BYTES: usize = 4 * 1024;
pub const MAX_TERMINAL_JSON_BYTES: usize = 8 * 1024;
pub const WORKFLOW_HISTORY_EVIDENCE_SCHEMA_V1: u16 = 1;

/// Hash a canonical, prompt-free configured workload shape into a bounded
/// partition dimension. The caller owns canonicalization; only the digest is
/// persisted so model names and graph topology cannot inflate the ledger.
pub fn fingerprint_workload_shape(canonical: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = ring::digest::digest(&ring::digest::SHA256, canonical);
    let mut value = String::with_capacity(6 + digest.as_ref().len() * 2);
    value.push_str("shape-");
    for byte in digest.as_ref() {
        let _ = write!(&mut value, "{byte:02x}");
    }
    value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerUnavailableReason {
    Open,
    Permission,
    ReadOnlyDatabase,
    ReadOnlyLock,
    ReadOnlyParent,
    AdvisoryLockUnsupported,
    AdvisoryLockIo,
    Locked,
    Migration,
    Schema,
    Corruption,
    Io,
    CapacityProtected,
    Collision,
}
impl LedgerUnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Permission => "permission",
            Self::ReadOnlyDatabase => "read_only_database",
            Self::ReadOnlyLock => "read_only_lock",
            Self::ReadOnlyParent => "read_only_parent",
            Self::AdvisoryLockUnsupported => "advisory_lock_unsupported",
            Self::AdvisoryLockIo => "advisory_lock_io",
            Self::Locked => "locked",
            Self::Migration => "migration",
            Self::Schema => "schema",
            Self::Corruption => "corruption",
            Self::Io => "io",
            Self::CapacityProtected => "capacity_protected",
            Self::Collision => "collision",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, thiserror::Error)]
#[error("telemetry_unavailable{{reason={reason:?}}}")]
pub struct LedgerError {
    pub reason: LedgerUnavailableReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlite_primary_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqlite_extended_code: Option<i32>,
}
impl LedgerError {
    pub fn new(reason: LedgerUnavailableReason) -> Self {
        Self {
            reason,
            sqlite_primary_code: None,
            sqlite_extended_code: None,
        }
    }

    pub fn with_sqlite_codes(
        reason: LedgerUnavailableReason,
        primary_code: i32,
        extended_code: i32,
    ) -> Self {
        Self {
            reason,
            sqlite_primary_code: Some(primary_code),
            sqlite_extended_code: Some(extended_code),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSurface {
    Offline,
    ServedTask,
    DirectUnary,
    Mcp,
    Smoke,
    Other,
}
impl ExecutionSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::ServedTask => "served_task",
            Self::DirectUnary => "direct_unary",
            Self::Mcp => "mcp",
            Self::Smoke => "smoke",
            Self::Other => "other",
        }
    }
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttemptReservation {
    pub identity: AttemptIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub workflow: String,
    pub task_class: String,
    pub surface: ExecutionSurface,
    pub policy: String,
    pub workload_fingerprint: String,
    pub started_ms: i64,
    #[serde(default)]
    pub workload_fingerprint_complete: bool,
    #[serde(default = "default_prompt_acceptance")]
    pub prompt_acceptance: String,
    #[serde(default)]
    pub pinned: bool,
}
fn default_prompt_acceptance() -> String {
    "not_dispatched".to_owned()
}

/// Merge persisted and observed prompt state without ever turning an uncertain
/// dispatch boundary back into the false claim that no dispatch occurred.
pub fn conservative_prompt_acceptance(
    persisted: &str,
    observed: &str,
) -> Result<String, LedgerError> {
    let valid = |value: &str| matches!(value, "not_dispatched" | "dispatch_uncertain" | "unknown");
    if !valid(persisted) || !valid(observed) {
        return Err(LedgerError::new(LedgerUnavailableReason::Schema));
    }
    Ok(
        if persisted == "dispatch_uncertain" || observed == "dispatch_uncertain" {
            "dispatch_uncertain"
        } else if persisted == "unknown" || observed == "unknown" {
            "unknown"
        } else {
            "not_dispatched"
        }
        .to_owned(),
    )
}
impl AttemptReservation {
    pub fn validate(&self) -> Result<(), LedgerError> {
        let linkage = (self.identity.ordinal == 0 && self.identity.parent_attempt_id.is_none())
            || (self.identity.ordinal > 0 && self.identity.parent_attempt_id.is_some());
        if self.started_ms <= 0
            || !linkage
            || !bounded(&self.workflow, MAX_DIMENSION_LEN)
            || self.identity.parent_attempt_id.as_ref() == Some(&self.identity.attempt_id)
            || !bounded(&self.task_class, MAX_DIMENSION_LEN)
            || !bounded(&self.policy, MAX_DIMENSION_LEN)
            || !bounded(&self.workload_fingerprint, MAX_FINGERPRINT_LEN)
            || !matches!(
                self.prompt_acceptance.as_str(),
                "not_dispatched" | "dispatch_uncertain" | "unknown"
            )
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
        if encoded.len() > MAX_RESERVATION_JSON_BYTES {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryNodeReservationV1 {
    pub node: NodeId,
    pub sorted_ordinal: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptReservationV2 {
    pub schema_version: u16,
    pub reservation: AttemptReservation,
    pub controls_json: String,
    pub controls_fingerprint: String,
    pub expected_node_count: u32,
    pub nodes: Vec<HistoryNodeReservationV1>,
}

impl AttemptReservationV2 {
    pub fn validate(&self) -> Result<(), LedgerError> {
        self.reservation.validate()?;
        if self.schema_version != WORKFLOW_HISTORY_EVIDENCE_SCHEMA_V1
            || !bounded(&self.controls_fingerprint, MAX_FINGERPRINT_LEN)
            || self.controls_json.len() > crate::execution_policy::MAX_CONTROLS_JSON_BYTES
            || usize::try_from(self.expected_node_count).ok() != Some(self.nodes.len())
            || self.nodes.is_empty()
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let controls: crate::execution_policy::FrozenWorkflowControlsV1 =
            serde_json::from_str(&self.controls_json)
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
        if controls
            .encode_canonical()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?
            != self.controls_json.as_bytes()
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        for (ordinal, node) in self.nodes.iter().enumerate() {
            let ordinal = u32::try_from(ordinal)
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
            if node.sorted_ordinal != ordinal
                || self
                    .nodes
                    .get(ordinal as usize + 1)
                    .is_some_and(|next| node.node.as_str() >= next.node.as_str())
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Schema));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn node(&self, id: &NodeId) -> Option<&HistoryNodeReservationV1> {
        self.nodes
            .binary_search_by(|candidate| candidate.node.cmp(id))
            .ok()
            .and_then(|index| self.nodes.get(index))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryNodeTerminalV1 {
    pub node: NodeId,
    pub sorted_ordinal: u32,
    pub terminal_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptStructuredEvidenceV1 {
    pub reservation: AttemptReservationV2,
    pub node_terminals: Vec<HistoryNodeTerminalV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_trigger_json: Option<String>,
}

#[must_use]
pub fn node_terminal_placeholder_v1() -> String {
    let payload =
        "x".repeat(crate::execution_policy::MAX_NODE_TERMINAL_JSON_BYTES.saturating_sub(2));
    let encoded = serde_json::to_string(&payload).expect("JSON string encoding is infallible");
    debug_assert_eq!(
        encoded.len(),
        crate::execution_policy::MAX_NODE_TERMINAL_JSON_BYTES
    );
    encoded
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeCounts {
    pub completed: u32,
    pub failed: u32,
    pub canceled: u32,
    pub deadline: u32,
    pub cleanup_partial: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhaseDuration {
    pub phase: String,
    pub duration_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttemptTerminal {
    pub completed_ms: i64,
    pub work_ms: u64,
    pub end_to_end_ms: u64,
    pub queue_ms: u64,
    pub cancellation_ms: u64,
    pub cleanup_ms: u64,
    pub finalization_ms: u64,
    pub outcome: String,
    pub terminal_reason: String,
    pub producer_terminal: String,
    pub final_message: String,
    pub process_liveness: String,
    pub terminal_evidence_capability: String,
    pub terminal_evidence_version: String,
    pub terminal_evidence_source: String,
    pub terminal_evidence_complete: bool,
    #[serde(default)]
    pub terminal_evidence_counts: crate::terminal_evidence::TerminalEvidenceCounts,
    pub degraded: bool,
    pub prompt_acceptance: String,
    pub cleanup_disposition: String,
    pub node_counts: NodeCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_trigger_json: Option<String>,
    pub phase_durations: Vec<PhaseDuration>,
    pub telemetry_complete: bool,
    pub monotonic_clock: bool,
}
impl AttemptTerminal {
    pub fn validate(&self) -> Result<(), LedgerError> {
        let dims = [
            &self.outcome,
            &self.terminal_reason,
            &self.producer_terminal,
            &self.final_message,
            &self.process_liveness,
            &self.terminal_evidence_capability,
            &self.terminal_evidence_version,
            &self.terminal_evidence_source,
            &self.prompt_acceptance,
            &self.cleanup_disposition,
        ];
        let evidence_coherent = match self.terminal_evidence_capability.as_str() {
            "not_applicable" => {
                self.terminal_evidence_version == "none"
                    && self.terminal_evidence_source == "none"
                    && self.terminal_evidence_complete
            }
            "unsupported" => {
                self.terminal_evidence_version == "none"
                    && self.terminal_evidence_source == "none"
                    && !self.terminal_evidence_complete
            }
            "v1" => {
                self.terminal_evidence_version == "v1"
                    && ((self.terminal_evidence_source == "adapter"
                        && self.terminal_evidence_complete)
                        || (self.terminal_evidence_source == "none"
                            && !self.terminal_evidence_complete
                            && self.producer_terminal == "unknown"
                            && self.final_message == "unknown"))
            }
            "unknown" => {
                self.terminal_evidence_version == "none"
                    && self.terminal_evidence_source == "none"
                    && !self.terminal_evidence_complete
                    && self.producer_terminal == "unknown"
                    && self.final_message == "unknown"
                    && self.terminal_evidence_counts.reached > 1
            }
            _ => false,
        };
        let policy_trigger_valid = self.policy_trigger_json.as_deref().is_none_or(|encoded| {
            crate::execution_policy::PolicyTriggerV1::decode_canonical(encoded.as_bytes()).is_ok()
        });
        if self.completed_ms <= 0
            || dims.iter().any(|v| !bounded(v, MAX_DIMENSION_LEN))
            || !matches!(
                self.outcome.as_str(),
                "completed" | "completed_degraded" | "failed" | "canceled" | "interrupted"
            )
            || self.phase_durations.len() > MAX_PHASES
            || self
                .phase_durations
                .iter()
                .any(|p| !bounded(&p.phase, MAX_DIMENSION_LEN))
            || !matches!(
                self.producer_terminal.as_str(),
                "not_started" | "unknown" | "completed" | "interrupted" | "failed"
            )
            || !matches!(
                self.final_message.as_str(),
                "not_started" | "unknown" | "nonempty" | "absent"
            )
            || !matches!(
                self.process_liveness.as_str(),
                "not_started" | "unknown" | "live" | "exited"
            )
            || !matches!(
                self.terminal_evidence_capability.as_str(),
                "not_applicable" | "unsupported" | "unknown" | "v1"
            )
            || !matches!(self.terminal_evidence_version.as_str(), "none" | "v1")
            || !matches!(self.terminal_evidence_source.as_str(), "none" | "adapter")
            || !matches!(
                self.prompt_acceptance.as_str(),
                "not_dispatched" | "dispatch_uncertain" | "unknown"
            )
            || !matches!(
                self.cleanup_disposition.as_str(),
                "pending" | "complete" | "failed" | "not_needed" | "unknown"
            )
            || !self.terminal_evidence_counts.validate()
            || !evidence_coherent
            || !policy_trigger_valid
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
        if encoded.len() > MAX_TERMINAL_JSON_BYTES {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        Ok(())
    }
}

/// Projects the one false-complete timing shape emitted by legacy direct
/// producers into the evidence those rows actually contain.
pub fn compatibility_project_terminal(
    surface: ExecutionSurface,
    terminal: &AttemptTerminal,
) -> std::borrow::Cow<'_, AttemptTerminal> {
    let legacy_direct_timing = matches!(
        surface,
        ExecutionSurface::DirectUnary | ExecutionSurface::Mcp
    ) && terminal.telemetry_complete
        && terminal.work_ms == terminal.end_to_end_ms
        && terminal.queue_ms == 0
        && terminal.cancellation_ms == 0
        && terminal.cleanup_ms == 0
        && terminal.finalization_ms == 0
        && matches!(
            terminal.phase_durations.as_slice(),
            [PhaseDuration { phase, duration_ms }]
                if phase == "work" && *duration_ms == terminal.work_ms
        );
    if !legacy_direct_timing {
        return std::borrow::Cow::Borrowed(terminal);
    }

    let mut projected = terminal.clone();
    projected.work_ms = 0;
    projected.phase_durations.clear();
    projected.telemetry_complete = false;
    std::borrow::Cow::Owned(projected)
}

pub fn compatibility_project_completed(
    row: &CompletedAttempt,
) -> std::borrow::Cow<'_, CompletedAttempt> {
    match compatibility_project_terminal(row.reservation.surface, &row.terminal) {
        std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(row),
        std::borrow::Cow::Owned(terminal) => std::borrow::Cow::Owned(CompletedAttempt {
            reservation: row.reservation.clone(),
            terminal,
        }),
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompletedAttempt {
    pub reservation: AttemptReservation,
    pub terminal: AttemptTerminal,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttemptRecord {
    pub reservation: AttemptReservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<AttemptTerminal>,
}

pub fn compatibility_project_attempt_record(mut row: AttemptRecord) -> AttemptRecord {
    if let Some(terminal) = row.terminal.take() {
        row.terminal =
            Some(compatibility_project_terminal(row.reservation.surface, &terminal).into_owned());
    }
    row
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalWrite {
    Applied,
    Replayed,
    Conflict,
}

#[async_trait::async_trait]
pub trait WorkflowHistoryStore: Send + Sync {
    async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError>;
    /// Admit one structured workflow attempt and its complete node-placeholder
    /// roster atomically before any provider effect. Implementations that do
    /// not own the R2f1a history schema must fail closed.
    async fn reserve_v2(&self, _row: &AttemptReservationV2) -> Result<(), LedgerError> {
        Err(LedgerError::new(LedgerUnavailableReason::Schema))
    }
    /// Replace one admitted placeholder with its canonical terminal. When a
    /// policy trigger is supplied, the selected terminal and attempt-level
    /// trigger must share this one durable transition.
    async fn commit_node_terminal_v2(
        &self,
        _id: &AttemptId,
        _node: &NodeId,
        _terminal_json: &str,
        _policy_trigger_json: Option<&str>,
    ) -> Result<TerminalWrite, LedgerError> {
        Err(LedgerError::new(LedgerUnavailableReason::Schema))
    }
    /// Read the exact structured reservation, committed node terminals, and
    /// trigger without reconstructing missing evidence from legacy fields.
    async fn structured_evidence_v2(
        &self,
        _id: &AttemptId,
    ) -> Result<Option<AttemptStructuredEvidenceV1>, LedgerError> {
        Ok(None)
    }
    /// Persist the conservative prompt-dispatch state before polling a provider prompt.
    /// Values are a closed vocabulary owned by the caller (`not_dispatched` or
    /// `dispatch_uncertain`) and may only advance from the former to the latter.
    async fn mark_prompt_acceptance(
        &self,
        id: &AttemptId,
        acceptance: &str,
    ) -> Result<(), LedgerError>;
    async fn terminalize(
        &self,
        id: &AttemptId,
        terminal: &AttemptTerminal,
    ) -> Result<TerminalWrite, LedgerError>;
    /// Settle the one post-terminal cleanup transition. Only
    /// `pending -> complete|failed` is legal; identical replay is idempotent.
    async fn settle_cleanup(
        &self,
        _id: &AttemptId,
        _disposition: &str,
    ) -> Result<TerminalWrite, LedgerError> {
        Err(LedgerError::new(LedgerUnavailableReason::Schema))
    }
    async fn record_activity_tally(
        &self,
        _id: &AttemptId,
        _tally: &crate::attempt_activity::ActivityTally,
    ) -> Result<(), LedgerError> {
        Ok(())
    }
    async fn activity_tally(
        &self,
        _id: &AttemptId,
    ) -> Result<Option<crate::attempt_activity::ActivityTally>, LedgerError> {
        Ok(None)
    }
    /// Change incident-retention protection for one exact durable attempt.
    /// Returns true only when the requested state changed.
    async fn set_pinned(&self, id: &AttemptId, pinned: bool) -> Result<bool, LedgerError>;
    async fn interrupt_active(&self, completed_ms: i64) -> Result<u64, LedgerError>;
    /// Conservatively terminalize active rows except exact attempts whose
    /// primary task already carries durable marker-only terminal evidence.
    async fn interrupt_active_excluding(
        &self,
        completed_ms: i64,
        excluded: &[AttemptId],
    ) -> Result<u64, LedgerError> {
        if excluded.is_empty() {
            self.interrupt_active(completed_ms).await
        } else {
            // Stores that cannot apply the exclusion atomically must fail
            // closed instead of overwriting authoritative marker-only state.
            Err(LedgerError::new(LedgerUnavailableReason::Schema))
        }
    }
    /// Recover the most recent durable lineage row for a served task.
    async fn latest_reservation_for_task(
        &self,
        task: &TaskId,
    ) -> Result<Option<AttemptReservation>, LedgerError>;
    /// Read one exact durable attempt without substituting the latest task lineage.
    async fn attempt(&self, _id: &AttemptId) -> Result<Option<AttemptRecord>, LedgerError> {
        Ok(None)
    }
    async fn completed_between(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<CompletedAttempt>, LedgerError>;
}

/// Mandatory admission/dispatch/terminal state machine for one direct attempt.
/// Callers supply surface-specific metrics, while this owner keeps the durable
/// transitions and conservative drop terminalization identical.
pub struct DirectAttemptBarrier {
    store: std::sync::Arc<dyn WorkflowHistoryStore>,
    identity: AttemptIdentity,
    started: std::time::Instant,
    scopes: crate::attempt_activity::AttemptScopeOwner,
    terminal_evidence: std::sync::Arc<crate::terminal_evidence::SharedTurnEvidence>,
    multi_provider_legs: std::sync::Arc<crate::terminal_evidence::WorkflowTurnEvidenceCollector>,
    multi_provider: bool,
    prompt_acceptance: &'static str,
    prompt_barrier_failed: bool,
    prepared_terminal: Option<AttemptTerminal>,
    terminalized: bool,
    abort_reason: &'static str,
}

impl DirectAttemptBarrier {
    pub async fn admit(
        store: std::sync::Arc<dyn WorkflowHistoryStore>,
        reservation: AttemptReservation,
        abort_reason: &'static str,
    ) -> Result<Self, LedgerError> {
        if reservation.identity.ordinal != 0 || reservation.identity.parent_attempt_id.is_some() {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        reservation.validate()?;
        store.reserve(&reservation).await?;
        Ok(Self {
            store,
            identity: reservation.identity,
            started: std::time::Instant::now(),
            scopes: crate::attempt_activity::AttemptScopeOwner::new(std::sync::Arc::new(
                crate::attempt_activity::SharedAttemptRecorder::new(
                    crate::attempt_activity::SystemMonotonicClock::start(),
                ),
            )),
            terminal_evidence: std::sync::Arc::new(
                crate::terminal_evidence::SharedTurnEvidence::unsupported(),
            ),
            multi_provider_legs: std::sync::Arc::new(
                crate::terminal_evidence::WorkflowTurnEvidenceCollector::default(),
            ),
            multi_provider: false,
            prompt_acceptance: "not_dispatched",
            prompt_barrier_failed: false,
            prepared_terminal: None,
            terminalized: false,
            abort_reason,
        })
    }

    pub fn identity(&self) -> &AttemptIdentity {
        &self.identity
    }

    pub fn with_recorder(
        mut self,
        recorder: std::sync::Arc<dyn crate::attempt_activity::AttemptRecorder>,
    ) -> Self {
        self.scopes = crate::attempt_activity::AttemptScopeOwner::new(recorder);
        self
    }

    pub fn activity_recorder(
        &self,
    ) -> std::sync::Arc<dyn crate::attempt_activity::AttemptRecorder> {
        self.scopes.recorder()
    }

    /// Switch this attempt's terminal to the bounded multi-provider
    /// projection: exact producer/final evidence projects only when exactly
    /// one provider leg was reached; otherwise the durable row keeps unknown
    /// evidence plus truthful bounded per-leg counts.
    pub fn begin_multi_provider_terminal(&mut self) {
        self.multi_provider = true;
    }

    /// One bounded observation scope plus one registered terminal-evidence
    /// sink for a reached provider leg of a multi-provider attempt.
    pub fn multi_provider_leg(
        &self,
        capability: crate::terminal_evidence::EvidenceCapability,
        binding: Option<crate::terminal_evidence::TurnEvidenceBinding>,
    ) -> (
        std::sync::Arc<dyn crate::attempt_activity::AttemptRecorder>,
        std::sync::Arc<dyn crate::terminal_evidence::TerminalEvidenceSink>,
    ) {
        (
            self.scopes.turn_scope(),
            self.multi_provider_legs.register(capability, binding),
        )
    }

    /// Build one idempotent dispatch-boundary observer for a provider leg.
    /// The collector owns the registered sink, so a response-side transport
    /// failure cannot erase the conservative reached-leg fact.
    pub fn multi_provider_leg_dispatch_observer(
        &self,
        capability: crate::terminal_evidence::EvidenceCapability,
        binding: Option<crate::terminal_evidence::TurnEvidenceBinding>,
    ) -> crate::ports::ProviderDispatchObserver {
        let legs = self.multi_provider_legs.clone();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::sync::Arc::new(move || {
            if !fired.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let _ = legs.register(capability, binding.clone());
            }
        })
    }

    pub fn terminal_evidence_sink(
        &self,
    ) -> std::sync::Arc<dyn crate::terminal_evidence::TerminalEvidenceSink> {
        self.terminal_evidence.clone()
    }

    pub fn prepare_terminal_evidence(
        &self,
        binding: crate::terminal_evidence::TurnEvidenceBinding,
    ) {
        self.terminal_evidence.prepare_binding(binding);
    }

    pub fn configure_terminal_evidence(
        &self,
        binding: crate::terminal_evidence::TurnEvidenceBinding,
    ) {
        self.terminal_evidence.configure_v1(binding);
    }

    pub fn configure_malformed_terminal_evidence(&self) {
        self.terminal_evidence.configure_malformed_advertisement();
    }

    pub fn seal_terminal_evidence(&self) {
        crate::terminal_evidence::TerminalEvidenceSink::close(self.terminal_evidence.as_ref());
    }

    pub fn record_activity(
        &mut self,
        phase: crate::attempt_activity::AttemptPhase,
        reason: crate::attempt_activity::ActivityReason,
        advance: u64,
    ) -> crate::attempt_activity::AttemptActivity {
        self.scopes
            .recorder()
            .record(phase, reason, advance)
            .unwrap_or(crate::attempt_activity::AttemptActivity {
                phase,
                reason,
                kind: crate::attempt_activity::ActivityKind::Activity,
                elapsed_ms: 0,
                advance,
            })
    }

    pub fn seal_child_liveness(&mut self, liveness: crate::terminal_evidence::AcpChildLiveness) {
        crate::terminal_evidence::TerminalEvidenceSink::record_child_liveness(
            self.terminal_evidence.as_ref(),
            liveness,
        );
    }

    pub async fn mark_prompt_dispatch(&mut self) -> Result<(), LedgerError> {
        match self
            .store
            .mark_prompt_acceptance(&self.identity.attempt_id, "dispatch_uncertain")
            .await
        {
            Ok(()) => {
                self.prompt_acceptance = "dispatch_uncertain";
                self.record_activity(
                    crate::attempt_activity::AttemptPhase::Provider,
                    crate::attempt_activity::ActivityReason::PhaseTransition,
                    1,
                );
                Ok(())
            }
            Err(error) => {
                self.prompt_barrier_failed = true;
                self.prompt_acceptance = "unknown";
                Err(error)
            }
        }
    }

    fn terminal(
        &self,
        outcome: &str,
        reason: &str,
        degraded: bool,
        cleanup_disposition: &str,
        _telemetry_complete: bool,
    ) -> AttemptTerminal {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // Multi-provider attempts read their sealed per-leg aggregation; the
        // single-provider path keeps its one attempt-owned sink.
        let mut counts = if self.multi_provider {
            self.multi_provider_legs.close_all();
            self.multi_provider_legs.counts()
        } else {
            crate::terminal_evidence::TerminalEvidenceCounts::default()
        };
        let single_leg = if self.multi_provider {
            self.multi_provider_legs.single_turn()
        } else {
            None
        };
        let (
            capability,
            completeness,
            producer,
            final_presence,
            ordered_notifications_drained,
            deliverable_final_present,
            child_liveness,
        ) = if self.multi_provider {
            match single_leg {
                // Exactly one reached provider leg: its exact evidence
                // projects through the unchanged single-turn contract.
                Some((
                    capability,
                    completeness,
                    producer,
                    final_presence,
                    drained,
                    child_liveness,
                    deliverable_final_present,
                )) => (
                    capability,
                    completeness,
                    producer,
                    final_presence,
                    drained,
                    deliverable_final_present,
                    child_liveness,
                ),
                // Zero or several reached legs: producer/final stay unknown
                // and the failed-outcome resolution behaves exactly like the
                // legacy unsupported single sink.
                None => (
                    crate::terminal_evidence::EvidenceCapability::Unsupported,
                    crate::terminal_evidence::EvidenceCompleteness::Unsupported,
                    crate::terminal_evidence::ProducerTerminal::Unknown,
                    crate::terminal_evidence::FinalPresence::Unknown,
                    false,
                    false,
                    crate::terminal_evidence::AcpChildLiveness::Unknown,
                ),
            }
        } else {
            let capability = crate::terminal_evidence::TerminalEvidenceSink::capability(
                self.terminal_evidence.as_ref(),
            );
            let (completeness, producer, final_presence, ordered_notifications_drained) =
                crate::terminal_evidence::TerminalEvidenceSink::observation(
                    self.terminal_evidence.as_ref(),
                );
            (
                capability,
                completeness,
                producer,
                final_presence,
                ordered_notifications_drained,
                crate::terminal_evidence::TerminalEvidenceSink::deliverable_final_present(
                    self.terminal_evidence.as_ref(),
                ),
                crate::terminal_evidence::TerminalEvidenceSink::child_liveness(
                    self.terminal_evidence.as_ref(),
                ),
            )
        };
        if !self.multi_provider && self.prompt_acceptance == "dispatch_uncertain" {
            counts.reached = 1;
            match completeness {
                crate::terminal_evidence::EvidenceCompleteness::Complete => counts.valid = 1,
                crate::terminal_evidence::EvidenceCompleteness::Missing => counts.missing = 1,
                crate::terminal_evidence::EvidenceCompleteness::Malformed
                | crate::terminal_evidence::EvidenceCompleteness::Mismatched
                | crate::terminal_evidence::EvidenceCompleteness::Late
                | crate::terminal_evidence::EvidenceCompleteness::Conflict => counts.invalid = 1,
                crate::terminal_evidence::EvidenceCompleteness::Unsupported => {}
            }
        }
        let should_resolve = capability == crate::terminal_evidence::EvidenceCapability::V1
            || (outcome == "failed" && self.prompt_acceptance != "not_dispatched");
        let resolution = should_resolve.then(|| {
            crate::terminal_evidence::resolve_terminal(
                crate::terminal_evidence::TerminalObservation {
                    capability,
                    completeness,
                    producer,
                    final_presence,
                    prompt_rpc: if self.prompt_acceptance == "not_dispatched" {
                        crate::terminal_evidence::PromptRpcObservation::RejectedBeforeAcceptance
                    } else if outcome == "failed" {
                        crate::terminal_evidence::PromptRpcObservation::RejectedAcceptedOrUncertain
                    } else {
                        crate::terminal_evidence::PromptRpcObservation::Resolved
                    },
                    ordered_notifications_drained,
                    deliverable_final_present,
                    child_liveness,
                },
            )
        });
        // Attempt-level aggregate rows (multi-provider without exactly one
        // reached leg) cannot claim one adapter's capability: zero reached
        // legs are `not_applicable`; several stay `unknown` with counts.
        let aggregate_legs = self.multi_provider && single_leg.is_none();
        let effective_outcome = resolution
            .as_ref()
            .map_or(outcome, |resolved| resolved.outcome.as_str());
        let effective_reason = if self.prompt_barrier_failed {
            "prompt_barrier_failed"
        } else {
            resolution
                .as_ref()
                .map_or(reason, |resolved| resolved.reason.as_str())
        };
        // Direct owners currently observe only the admission-to-terminal clock.
        // It is valid end-to-end evidence, but it cannot be relabeled as provider
        // work or split into checkout/configure/cleanup/finalization phases. Keep
        // every such row explicitly incomplete until those clocks have real owners.
        AttemptTerminal {
            completed_ms: crate::task_store::system_wall_now_ms(),
            work_ms: 0,
            end_to_end_ms: elapsed,
            queue_ms: 0,
            cancellation_ms: 0,
            cleanup_ms: 0,
            finalization_ms: 0,
            outcome: effective_outcome.into(),
            terminal_reason: effective_reason.into(),
            producer_terminal: match producer {
                crate::terminal_evidence::ProducerTerminal::Unknown => "unknown",
                crate::terminal_evidence::ProducerTerminal::Completed => "completed",
                crate::terminal_evidence::ProducerTerminal::Interrupted => "interrupted",
                crate::terminal_evidence::ProducerTerminal::Failed => "failed",
            }
            .into(),
            final_message: match final_presence {
                crate::terminal_evidence::FinalPresence::Unknown => "unknown",
                crate::terminal_evidence::FinalPresence::Nonempty => "nonempty",
                crate::terminal_evidence::FinalPresence::Absent => "absent",
            }
            .into(),
            process_liveness: match child_liveness {
                crate::terminal_evidence::AcpChildLiveness::Unknown => "unknown",
                crate::terminal_evidence::AcpChildLiveness::Live => "live",
                crate::terminal_evidence::AcpChildLiveness::Exited => "exited",
            }
            .into(),
            terminal_evidence_capability: if aggregate_legs {
                if counts.reached == 0 {
                    "not_applicable"
                } else {
                    "unknown"
                }
            } else {
                match capability {
                    crate::terminal_evidence::EvidenceCapability::Unsupported => "unsupported",
                    crate::terminal_evidence::EvidenceCapability::MalformedAdvertisement
                    | crate::terminal_evidence::EvidenceCapability::V1 => "v1",
                }
            }
            .into(),
            terminal_evidence_version: if aggregate_legs {
                "none"
            } else {
                match capability {
                    crate::terminal_evidence::EvidenceCapability::Unsupported => "none",
                    crate::terminal_evidence::EvidenceCapability::MalformedAdvertisement
                    | crate::terminal_evidence::EvidenceCapability::V1 => "v1",
                }
            }
            .into(),
            terminal_evidence_source: if !aggregate_legs
                && completeness == crate::terminal_evidence::EvidenceCompleteness::Complete
            {
                "adapter"
            } else {
                "none"
            }
            .into(),
            terminal_evidence_complete: if aggregate_legs {
                counts.reached == 0
            } else {
                completeness == crate::terminal_evidence::EvidenceCompleteness::Complete
            },
            terminal_evidence_counts: counts,
            degraded: degraded || self.prompt_barrier_failed || counts.overflowed,
            prompt_acceptance: self.prompt_acceptance.into(),
            cleanup_disposition: cleanup_disposition.into(),
            node_counts: NodeCounts::default(),
            policy_trigger_json: None,
            phase_durations: Vec::new(),
            telemetry_complete: false,
            monotonic_clock: true,
        }
    }

    pub async fn finish(
        &mut self,
        outcome: &str,
        reason: &str,
        degraded: bool,
        cleanup_disposition: &str,
        telemetry_complete: bool,
    ) -> Result<(TerminalWrite, AttemptTerminal), LedgerError> {
        if self.terminalized {
            return Err(LedgerError::new(LedgerUnavailableReason::Collision));
        }
        self.seal_terminal_evidence();
        // The first terminal summary is the retry identity. A store error can be
        // ambiguous (the commit may have succeeded), and Drop is also a retry
        // path, so every later write must use these exact bytes rather than
        // sampling a new timestamp or losing the observed cleanup disposition.
        let terminal = match self.prepared_terminal.as_ref() {
            Some(terminal) => terminal.clone(),
            None => {
                let terminal = self.terminal(
                    outcome,
                    reason,
                    degraded,
                    cleanup_disposition,
                    telemetry_complete,
                );
                self.prepared_terminal = Some(terminal.clone());
                terminal
            }
        };
        self.record_activity(
            crate::attempt_activity::AttemptPhase::TerminalStore,
            crate::attempt_activity::ActivityReason::ProducerTerminal,
            1,
        );
        match self
            .store
            .terminalize(&self.identity.attempt_id, &terminal)
            .await?
        {
            write @ (TerminalWrite::Applied | TerminalWrite::Replayed) => {
                self.terminalized = true;
                // The terminal row is primary truth. Activity is optional
                // enrichment and is attempted only after that truth commits.
                let _ = self
                    .store
                    .record_activity_tally(
                        &self.identity.attempt_id,
                        &self.scopes.recorder().tally().unwrap_or_default(),
                    )
                    .await;
                Ok((write, terminal))
            }
            TerminalWrite::Conflict => {
                self.terminalized = true;
                Err(LedgerError::new(LedgerUnavailableReason::Collision))
            }
        }
    }

    pub fn cleanup_settlement(&self) -> Result<DirectCleanupSettlement, LedgerError> {
        if !self.terminalized
            || self
                .prepared_terminal
                .as_ref()
                .is_none_or(|terminal| terminal.cleanup_disposition != "pending")
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        Ok(DirectCleanupSettlement {
            store: self.store.clone(),
            attempt_id: self.identity.attempt_id.clone(),
        })
    }
}

#[derive(Clone)]
pub struct DirectCleanupSettlement {
    store: std::sync::Arc<dyn WorkflowHistoryStore>,
    attempt_id: AttemptId,
}

impl DirectCleanupSettlement {
    pub async fn settle(&self, disposition: &str) -> Result<TerminalWrite, LedgerError> {
        self.store
            .settle_cleanup(&self.attempt_id, disposition)
            .await
    }
}

impl Drop for DirectAttemptBarrier {
    fn drop(&mut self) {
        if self.terminalized {
            return;
        }
        let store = self.store.clone();
        let attempt_id = self.identity.attempt_id.clone();
        let terminal = self.prepared_terminal.clone().unwrap_or_else(|| {
            self.terminal("interrupted", self.abort_reason, true, "unknown", false)
        });
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = store.terminalize(&attempt_id, &terminal).await;
            });
        }
    }
}

/// Deterministic in-memory implementation used by embedders and test-support
/// constructors. It enforces the same identity, lineage, prompt, and terminal
/// invariants as a durable implementation, but makes no durability claim.
#[derive(Default)]
pub struct MemoryWorkflowHistoryStore {
    rows: std::sync::Mutex<
        std::collections::BTreeMap<String, (AttemptReservation, Option<AttemptTerminal>)>,
    >,
    structured: std::sync::Mutex<std::collections::BTreeMap<String, MemoryStructuredAttemptV1>>,
    activity: std::sync::Mutex<
        std::collections::BTreeMap<String, crate::attempt_activity::ActivityTally>,
    >,
}

#[derive(Clone)]
struct MemoryStructuredAttemptV1 {
    reservation: AttemptReservationV2,
    node_terminals: std::collections::BTreeMap<NodeId, Option<String>>,
    policy_trigger_json: Option<String>,
}

impl MemoryWorkflowHistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn reserve_row_locked(
        rows: &mut std::collections::BTreeMap<
            String,
            (AttemptReservation, Option<AttemptTerminal>),
        >,
        row: &AttemptReservation,
    ) -> Result<(), LedgerError> {
        row.validate()?;
        if matches!(
            row.surface,
            ExecutionSurface::ServedTask
                | ExecutionSurface::DirectUnary
                | ExecutionSurface::Mcp
                | ExecutionSurface::Smoke
        ) && row.task_id.as_ref().map(TaskId::as_str) != Some(row.identity.execution_id.as_str())
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        if rows.contains_key(row.identity.attempt_id.as_str())
            || rows.values().any(|(existing, _)| {
                existing.identity.execution_id == row.identity.execution_id
                    && existing.identity.ordinal == row.identity.ordinal
            })
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Collision));
        }
        match row.identity.parent_attempt_id.as_ref() {
            Some(parent_id) => {
                if rows.values().any(|(existing, _)| {
                    existing.identity.parent_attempt_id.as_ref() == Some(parent_id)
                }) {
                    return Err(LedgerError::new(LedgerUnavailableReason::Collision));
                }
                if let Some((parent, terminal)) = rows.get(parent_id.as_str()) {
                    if terminal.is_none()
                        || parent.identity.execution_id != row.identity.execution_id
                        || parent.identity.ordinal.checked_add(1) != Some(row.identity.ordinal)
                        || parent.task_id != row.task_id
                    {
                        return Err(LedgerError::new(LedgerUnavailableReason::Collision));
                    }
                } else if row.surface != ExecutionSurface::ServedTask {
                    // Served-task state owns the authoritative resume CAS. Its
                    // optional summary may legitimately be absent after a crash
                    // or fail-open reservation, but direct/offline lineage has
                    // no independent authority for such a gap.
                    return Err(LedgerError::new(LedgerUnavailableReason::Collision));
                }
            }
            None => {
                if rows.values().any(|(existing, _)| {
                    existing.identity.execution_id == row.identity.execution_id
                }) {
                    return Err(LedgerError::new(LedgerUnavailableReason::Collision));
                }
            }
        }
        let charged = u64::try_from(rows.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(RESERVED_ROW_CHARGE.saturating_add(PERMANENT_IDENTITY_CHARGE));
        if rows.len() >= MAX_TERMINAL_ROWS as usize
            || charged
                .saturating_add(RESERVED_ROW_CHARGE)
                .saturating_add(PERMANENT_IDENTITY_CHARGE)
                > MAX_CHARGED_BYTES
        {
            return Err(LedgerError::new(LedgerUnavailableReason::CapacityProtected));
        }
        rows.insert(
            row.identity.attempt_id.as_str().to_owned(),
            (row.clone(), None),
        );
        Ok(())
    }

    fn validate_complete_structured_attempt(
        id: &AttemptId,
        structured: &MemoryStructuredAttemptV1,
    ) -> Result<(), LedgerError> {
        use crate::execution_policy::{
            ControlEventIdV1, FanOutPolicyNameV1, FanOutPolicyV1, FrozenWorkflowControlsV1,
            NodePrimaryDispositionV1, NodeTerminalV1, PolicyNodeRefV1, PolicyTriggerV1,
        };

        if structured.node_terminals.values().any(Option::is_none) {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let controls: FrozenWorkflowControlsV1 =
            serde_json::from_str(&structured.reservation.controls_json)
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let mut trigger_mentions = 0_usize;
        let mut qualifying_failure = false;
        let mut selected_terminal_valid = false;
        let trigger = structured
            .policy_trigger_json
            .as_deref()
            .map(|encoded| {
                PolicyTriggerV1::decode_canonical(encoded.as_bytes())
                    .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))
            })
            .transpose()?;
        for admitted in &structured.reservation.nodes {
            let encoded = structured
                .node_terminals
                .get(&admitted.node)
                .and_then(Option::as_deref)
                .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
            let terminal = NodeTerminalV1::decode_canonical(encoded.as_bytes())
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
            qualifying_failure |= matches!(
                terminal.primary,
                NodePrimaryDispositionV1::Failed | NodePrimaryDispositionV1::TimedOut
            );
            if let Some(trigger_id) = terminal.policy_trigger_id.as_ref() {
                trigger_mentions = trigger_mentions.saturating_add(1);
                if let Some(trigger) = trigger.as_ref() {
                    selected_terminal_valid |= trigger_id == &trigger.id
                        && trigger.node
                            == PolicyNodeRefV1::from_node_id(
                                admitted.sorted_ordinal,
                                admitted.node.as_str(),
                            )
                        && matches!(
                            terminal.primary,
                            NodePrimaryDispositionV1::Failed | NodePrimaryDispositionV1::TimedOut
                        );
                }
            }
        }
        match trigger {
            Some(trigger) => {
                let expected_grace = match controls.fan_out {
                    FanOutPolicyV1::FixedGrace { grace_ms } => Some(grace_ms),
                    FanOutPolicyV1::BoundedIndependent | FanOutPolicyV1::FailFast => None,
                };
                if matches!(controls.fan_out, FanOutPolicyV1::BoundedIndependent)
                    || trigger.id != ControlEventIdV1::for_attempt(id, 0)
                    || trigger.policy != FanOutPolicyNameV1::from(&controls.fan_out)
                    || trigger.grace_ms != expected_grace
                    || trigger_mentions != 1
                    || !selected_terminal_valid
                {
                    return Err(LedgerError::new(LedgerUnavailableReason::Schema));
                }
            }
            None => {
                if trigger_mentions != 0
                    || (qualifying_failure
                        && !matches!(controls.fan_out, FanOutPolicyV1::BoundedIndependent))
                {
                    return Err(LedgerError::new(LedgerUnavailableReason::Schema));
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl WorkflowHistoryStore for MemoryWorkflowHistoryStore {
    async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        Self::reserve_row_locked(&mut rows, row)
    }

    async fn reserve_v2(&self, row: &AttemptReservationV2) -> Result<(), LedgerError> {
        row.validate()?;
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let mut structured = self
            .structured
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        if structured.contains_key(row.reservation.identity.attempt_id.as_str()) {
            return Err(LedgerError::new(LedgerUnavailableReason::Collision));
        }
        Self::reserve_row_locked(&mut rows, &row.reservation)?;
        let node_terminals = row
            .nodes
            .iter()
            .map(|node| (node.node.clone(), None))
            .collect();
        structured.insert(
            row.reservation.identity.attempt_id.as_str().to_owned(),
            MemoryStructuredAttemptV1 {
                reservation: row.clone(),
                node_terminals,
                policy_trigger_json: None,
            },
        );
        Ok(())
    }

    async fn commit_node_terminal_v2(
        &self,
        id: &AttemptId,
        node: &NodeId,
        terminal_json: &str,
        policy_trigger_json: Option<&str>,
    ) -> Result<TerminalWrite, LedgerError> {
        use crate::execution_policy::{
            ControlEventIdV1, FanOutPolicyNameV1, FanOutPolicyV1, FrozenWorkflowControlsV1,
            NodePrimaryDispositionV1, NodeTerminalV1, PolicyNodeRefV1, PolicyTriggerV1,
        };

        let terminal = NodeTerminalV1::decode_canonical(terminal_json.as_bytes())
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let trigger = policy_trigger_json
            .map(|encoded| {
                PolicyTriggerV1::decode_canonical(encoded.as_bytes())
                    .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))
            })
            .transpose()?;
        if terminal.policy_trigger_id.as_ref() != trigger.as_ref().map(|value| &value.id) {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }

        let mut structured = self
            .structured
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let attempt = structured
            .get_mut(id.as_str())
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let admitted = attempt
            .reservation
            .node(node)
            .cloned()
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let controls: FrozenWorkflowControlsV1 =
            serde_json::from_str(&attempt.reservation.controls_json)
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;

        if let Some(trigger) = trigger.as_ref() {
            let expected_grace = match controls.fan_out {
                FanOutPolicyV1::FixedGrace { grace_ms } => Some(grace_ms),
                FanOutPolicyV1::BoundedIndependent | FanOutPolicyV1::FailFast => None,
            };
            if matches!(controls.fan_out, FanOutPolicyV1::BoundedIndependent)
                || !matches!(
                    terminal.primary,
                    NodePrimaryDispositionV1::Failed | NodePrimaryDispositionV1::TimedOut
                )
                || trigger.id != ControlEventIdV1::for_attempt(id, 0)
                || trigger.node
                    != PolicyNodeRefV1::from_node_id(admitted.sorted_ordinal, node.as_str())
                || trigger.policy != FanOutPolicyNameV1::from(&controls.fan_out)
                || trigger.grace_ms != expected_grace
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Schema));
            }
        }

        if let Some(existing_trigger) = attempt.policy_trigger_json.as_deref() {
            if policy_trigger_json.is_some_and(|candidate| candidate != existing_trigger) {
                return Ok(TerminalWrite::Conflict);
            }
        }
        let slot = attempt
            .node_terminals
            .get_mut(node)
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        match slot {
            Some(existing) if existing == terminal_json => Ok(TerminalWrite::Replayed),
            Some(_) => Ok(TerminalWrite::Conflict),
            slot @ None => {
                *slot = Some(terminal_json.to_owned());
                if let Some(trigger_json) = policy_trigger_json {
                    attempt.policy_trigger_json = Some(trigger_json.to_owned());
                }
                Ok(TerminalWrite::Applied)
            }
        }
    }

    async fn structured_evidence_v2(
        &self,
        id: &AttemptId,
    ) -> Result<Option<AttemptStructuredEvidenceV1>, LedgerError> {
        let structured = self
            .structured
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        Ok(structured.get(id.as_str()).map(|attempt| {
            let node_terminals = attempt
                .reservation
                .nodes
                .iter()
                .filter_map(|admitted| {
                    attempt
                        .node_terminals
                        .get(&admitted.node)
                        .and_then(Option::as_ref)
                        .map(|terminal_json| HistoryNodeTerminalV1 {
                            node: admitted.node.clone(),
                            sorted_ordinal: admitted.sorted_ordinal,
                            terminal_json: terminal_json.clone(),
                        })
                })
                .collect();
            AttemptStructuredEvidenceV1 {
                reservation: attempt.reservation.clone(),
                node_terminals,
                policy_trigger_json: attempt.policy_trigger_json.clone(),
            }
        }))
    }

    async fn mark_prompt_acceptance(
        &self,
        id: &AttemptId,
        acceptance: &str,
    ) -> Result<(), LedgerError> {
        if acceptance != "not_dispatched" && acceptance != "dispatch_uncertain" {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let (reservation, terminal) = rows
            .get_mut(id.as_str())
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        if terminal.is_some() {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let canonical = conservative_prompt_acceptance(&reservation.prompt_acceptance, acceptance)?;
        if canonical != acceptance {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        reservation.prompt_acceptance = canonical;
        Ok(())
    }

    async fn terminalize(
        &self,
        id: &AttemptId,
        terminal: &AttemptTerminal,
    ) -> Result<TerminalWrite, LedgerError> {
        terminal.validate()?;
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let structured = self
            .structured
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        if let Some(attempt) = structured.get(id.as_str()) {
            Self::validate_complete_structured_attempt(id, attempt)?;
            if terminal.policy_trigger_json != attempt.policy_trigger_json {
                return Err(LedgerError::new(LedgerUnavailableReason::Schema));
            }
        }
        let (reservation, persisted) = rows
            .get_mut(id.as_str())
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let mut canonical = terminal.clone();
        canonical.prompt_acceptance = conservative_prompt_acceptance(
            &reservation.prompt_acceptance,
            &terminal.prompt_acceptance,
        )?;
        canonical.validate()?;
        match persisted {
            Some(existing) if existing == &canonical => Ok(TerminalWrite::Replayed),
            Some(_) => Ok(TerminalWrite::Conflict),
            slot @ None => {
                *slot = Some(canonical);
                Ok(TerminalWrite::Applied)
            }
        }
    }

    async fn settle_cleanup(
        &self,
        id: &AttemptId,
        disposition: &str,
    ) -> Result<TerminalWrite, LedgerError> {
        if !matches!(disposition, "complete" | "failed") {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let (_, terminal) = rows
            .get_mut(id.as_str())
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let terminal = terminal
            .as_mut()
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        if terminal.cleanup_disposition == disposition {
            return Ok(TerminalWrite::Replayed);
        }
        if terminal.cleanup_disposition != "pending" {
            return Ok(TerminalWrite::Conflict);
        }
        terminal.cleanup_disposition = disposition.to_owned();
        terminal.validate()?;
        Ok(TerminalWrite::Applied)
    }

    async fn record_activity_tally(
        &self,
        id: &AttemptId,
        tally: &crate::attempt_activity::ActivityTally,
    ) -> Result<(), LedgerError> {
        if tally.encoded_len() > crate::attempt_activity::MAX_ATTACHMENT_ENCODING_BYTES {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        if !self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?
            .contains_key(id.as_str())
        {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        self.activity
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?
            .insert(id.as_str().to_owned(), *tally);
        Ok(())
    }

    async fn activity_tally(
        &self,
        id: &AttemptId,
    ) -> Result<Option<crate::attempt_activity::ActivityTally>, LedgerError> {
        Ok(self
            .activity
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?
            .get(id.as_str())
            .copied())
    }

    async fn set_pinned(&self, id: &AttemptId, pinned: bool) -> Result<bool, LedgerError> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let (reservation, _) = rows
            .get_mut(id.as_str())
            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
        let changed = reservation.pinned != pinned;
        reservation.pinned = pinned;
        Ok(changed)
    }

    async fn interrupt_active(&self, completed_ms: i64) -> Result<u64, LedgerError> {
        self.interrupt_active_excluding(completed_ms, &[]).await
    }

    async fn interrupt_active_excluding(
        &self,
        completed_ms: i64,
        excluded: &[AttemptId],
    ) -> Result<u64, LedgerError> {
        if completed_ms <= 0 {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let excluded = excluded
            .iter()
            .map(AttemptId::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let mut structured = self
            .structured
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        let mut changed = 0_u64;
        for (reservation, terminal) in rows.values_mut() {
            if terminal.is_some() || excluded.contains(reservation.identity.attempt_id.as_str()) {
                continue;
            }
            let (node_counts, policy_trigger_json) = if let Some(attempt) =
                structured.get_mut(reservation.identity.attempt_id.as_str())
            {
                use crate::execution_policy::{
                    NodeCleanupDispositionV1 as Cleanup, NodeCleanupV1,
                    NodePrimaryDispositionV1 as Primary, NodeTerminalV1,
                    EXECUTION_POLICY_SCHEMA_V1,
                };

                let interrupted = NodeTerminalV1 {
                    schema_version: EXECUTION_POLICY_SCHEMA_V1,
                    primary: Primary::InterruptedLegacy,
                    cleanup: NodeCleanupV1 {
                        disposition: Cleanup::UnknownLegacy,
                        duration_ms: 0,
                    },
                    cause: None,
                    prompt_may_have_been_accepted: reservation.prompt_acceptance
                        != "not_dispatched",
                    degraded_ancestry: false,
                    policy_trigger_id: None,
                };
                let interrupted_json = String::from_utf8(
                    interrupted
                        .encode_canonical()
                        .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?,
                )
                .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
                for slot in attempt.node_terminals.values_mut() {
                    if slot.is_none() {
                        *slot = Some(interrupted_json.clone());
                    }
                }
                let mut counts = NodeCounts::default();
                for encoded in attempt.node_terminals.values().flatten() {
                    let terminal = NodeTerminalV1::decode_canonical(encoded.as_bytes())
                        .map_err(|_| LedgerError::new(LedgerUnavailableReason::Schema))?;
                    let counter = match terminal.primary {
                        Primary::Completed => Some(&mut counts.completed),
                        Primary::Failed | Primary::TimedOut => Some(&mut counts.failed),
                        Primary::CanceledWorkflow
                        | Primary::CanceledPolicy
                        | Primary::CanceledNode => Some(&mut counts.canceled),
                        Primary::SkippedDependency
                        | Primary::NotStartedPolicy
                        | Primary::InterruptedLegacy => None,
                        Primary::Deadline => {
                            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
                        }
                    };
                    if let Some(counter) = counter {
                        *counter = counter
                            .checked_add(1)
                            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
                    }
                    if matches!(
                        terminal.cleanup.disposition,
                        Cleanup::Failed | Cleanup::UnknownLegacy
                    ) {
                        counts.cleanup_partial = counts
                            .cleanup_partial
                            .checked_add(1)
                            .ok_or_else(|| LedgerError::new(LedgerUnavailableReason::Schema))?;
                    }
                }
                (counts, attempt.policy_trigger_json.clone())
            } else {
                (NodeCounts::default(), None)
            };
            *terminal = Some(AttemptTerminal {
                completed_ms,
                work_ms: 0,
                end_to_end_ms: 0,
                queue_ms: 0,
                cancellation_ms: 0,
                cleanup_ms: 0,
                finalization_ms: 0,
                outcome: "interrupted".into(),
                terminal_reason: "process_restart".into(),
                producer_terminal: "unknown".into(),
                final_message: "unknown".into(),
                process_liveness: "exited".into(),
                terminal_evidence_capability: "unsupported".into(),
                terminal_evidence_version: "none".into(),
                terminal_evidence_source: "none".into(),
                terminal_evidence_complete: false,
                terminal_evidence_counts: Default::default(),
                degraded: true,
                prompt_acceptance: reservation.prompt_acceptance.clone(),
                cleanup_disposition: "unknown".into(),
                node_counts,
                policy_trigger_json,
                phase_durations: Vec::new(),
                telemetry_complete: false,
                monotonic_clock: false,
            });
            changed = changed.saturating_add(1);
        }
        Ok(changed)
    }

    async fn latest_reservation_for_task(
        &self,
        task: &TaskId,
    ) -> Result<Option<AttemptReservation>, LedgerError> {
        let rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        Ok(rows
            .values()
            .filter(|(reservation, _)| reservation.task_id.as_ref() == Some(task))
            .max_by_key(|(reservation, _)| reservation.identity.ordinal)
            .map(|(reservation, _)| reservation.clone()))
    }

    async fn attempt(&self, id: &AttemptId) -> Result<Option<AttemptRecord>, LedgerError> {
        let rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        Ok(rows
            .get(id.as_str())
            .map(|(reservation, terminal)| AttemptRecord {
                reservation: reservation.clone(),
                terminal: terminal.clone(),
            }))
    }

    async fn completed_between(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<CompletedAttempt>, LedgerError> {
        if start_ms < 0 || end_ms < start_ms {
            return Err(LedgerError::new(LedgerUnavailableReason::Schema));
        }
        let rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
        Ok(rows
            .values()
            .filter_map(|(reservation, terminal)| {
                terminal
                    .as_ref()
                    .filter(|terminal| {
                        terminal.completed_ms >= start_ms && terminal.completed_ms <= end_ms
                    })
                    .map(|terminal| CompletedAttempt {
                        reservation: reservation.clone(),
                        terminal: terminal.clone(),
                    })
            })
            .collect())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Distribution {
    pub count: usize,
    pub min: f64,
    pub mean: f64,
    pub median: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}
pub fn distribution(values: impl IntoIterator<Item = u64>) -> Option<Distribution> {
    let mut values: Vec<f64> = values.into_iter().map(|v| v as f64).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    Some(Distribution {
        count: values.len(),
        min: values[0],
        mean,
        median: percentile(&values, 0.5),
        p90: percentile(&values, 0.9),
        p95: percentile(&values, 0.95),
        p99: percentile(&values, 0.99),
        max: *values.last().expect("nonempty"),
    })
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PartitionReport {
    pub sample_count: usize,
    pub work_ms: Option<Distribution>,
    pub end_to_end_ms: Option<Distribution>,
    pub sufficient: bool,
    pub recommendation: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StatsReport {
    pub start_ms: i64,
    pub end_ms: i64,
    pub sample_count: usize,
    pub calibration_sample_count: usize,
    pub work_ms: Option<Distribution>,
    pub end_to_end_ms: Option<Distribution>,
    pub partitions: std::collections::BTreeMap<String, PartitionReport>,
    pub excluded: std::collections::BTreeMap<String, usize>,
    pub sufficient: bool,
    pub recommendation: String,
}
pub fn report(start_ms: i64, end_ms: i64, rows: &[CompletedAttempt]) -> StatsReport {
    let projected_rows: Vec<_> = rows.iter().map(compatibility_project_completed).collect();
    let mut partition_rows: std::collections::BTreeMap<String, Vec<&CompletedAttempt>> =
        std::collections::BTreeMap::new();
    let mut excluded = std::collections::BTreeMap::new();
    let healthy: Vec<_> = projected_rows
        .iter()
        .map(std::borrow::Cow::as_ref)
        .filter(|row| {
            let ok = row.terminal.outcome == "completed"
                && !row.terminal.degraded
                && row.terminal.telemetry_complete
                && row.reservation.workload_fingerprint_complete
                && row.terminal.monotonic_clock;
            if ok {
                partition_rows
                    .entry(format!(
                        "{}/{}/{}/{}/{}",
                        row.reservation.workflow,
                        row.reservation.task_class,
                        row.reservation.surface.as_str(),
                        row.reservation.policy,
                        row.reservation.workload_fingerprint
                    ))
                    .or_default()
                    .push(row);
            } else {
                if row.terminal.outcome != "completed" {
                    *excluded.entry(row.terminal.outcome.clone()).or_insert(0) += 1;
                } else if row.terminal.degraded {
                    *excluded
                        .entry("completed_but_degraded".to_owned())
                        .or_insert(0) += 1;
                }
                if !row.terminal.telemetry_complete {
                    *excluded
                        .entry("telemetry_incomplete".to_owned())
                        .or_insert(0) += 1;
                }
                if !row.reservation.workload_fingerprint_complete {
                    *excluded
                        .entry("workload_fingerprint_incomplete".to_owned())
                        .or_insert(0) += 1;
                }
                if !row.terminal.monotonic_clock {
                    *excluded.entry("non_monotonic".to_owned()).or_insert(0) += 1;
                }
            }
            ok
        })
        .collect();
    let partitions = partition_rows
        .into_iter()
        .map(|(key, values)| {
            let sufficient = values.len() >= 30;
            let report = PartitionReport {
                sample_count: values.len(),
                work_ms: distribution(values.iter().map(|row| row.terminal.work_ms)),
                end_to_end_ms: distribution(values.iter().map(|row| row.terminal.end_to_end_ms)),
                sufficient,
                recommendation: if sufficient {
                    "partition sample sufficient for advisory calibration"
                } else {
                    "partition sample insufficient for advisory calibration"
                }
                .to_owned(),
            };
            (key, report)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let sufficient = partitions.values().any(|partition| partition.sufficient);
    StatsReport {
        start_ms,
        end_ms,
        sample_count: rows.len(),
        calibration_sample_count: healthy.len(),
        work_ms: distribution(healthy.iter().map(|r| r.terminal.work_ms)),
        end_to_end_ms: distribution(healthy.iter().map(|r| r.terminal.end_to_end_ms)),
        partitions,
        excluded,
        sufficient,
        recommendation: if sufficient {
            "one or more exact partitions are sufficient; review each before any policy edit"
        } else {
            "no exact partition has a sufficient healthy sample"
        }
        .to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quantiles_cover_empty_singleton_even_odd_and_boundary() {
        assert!(distribution([]).is_none());
        assert_eq!(distribution([7]).unwrap().p99, 7.0);
        assert_eq!(distribution([1, 3]).unwrap().median, 2.0);
        assert_eq!(distribution([9, 1, 5]).unwrap().median, 5.0);
        let d = distribution([0, u64::MAX]).unwrap();
        assert_eq!(d.min, 0.0);
        assert_eq!(d.max, u64::MAX as f64);
    }

    fn served_reservation(
        identity: AttemptIdentity,
        surface: ExecutionSurface,
    ) -> AttemptReservation {
        let task_id = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
        AttemptReservation {
            identity,
            task_id: Some(task_id),
            workflow: "review".into(),
            task_class: "workflow".into(),
            surface,
            policy: "r2f0a".into(),
            workload_fingerprint: "shape-a".into(),
            started_ms: 1_000,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        }
    }

    #[test]
    fn reservation_rejects_self_parent_lineage() {
        let identity = AttemptIdentity::initial().unwrap();
        let mut row = served_reservation(identity, ExecutionSurface::ServedTask);
        row.identity.ordinal = 1;
        row.identity.parent_attempt_id = Some(row.identity.attempt_id.clone());

        assert_eq!(
            row.validate().unwrap_err().reason,
            LedgerUnavailableReason::Schema
        );
    }

    #[tokio::test]
    async fn prompt_acceptance_never_downgrades_unknown_to_not_dispatched() {
        let store = MemoryWorkflowHistoryStore::new();
        let mut row = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::Offline,
        );
        row.prompt_acceptance = "unknown".into();
        let attempt_id = row.identity.attempt_id.clone();
        store.reserve(&row).await.unwrap();

        assert_eq!(
            store
                .mark_prompt_acceptance(&attempt_id, "not_dispatched")
                .await
                .unwrap_err()
                .reason,
            LedgerUnavailableReason::Schema
        );
        assert_eq!(
            store
                .attempt(&attempt_id)
                .await
                .unwrap()
                .unwrap()
                .reservation
                .prompt_acceptance,
            "unknown"
        );
    }

    fn structured_reservation(identity: AttemptIdentity) -> AttemptReservationV2 {
        use crate::execution_policy::{
            resolve_execution_policy_v1, ExecutionPolicyInvocationV1, FanOutPolicyV1,
            PolicyActivationV1, WorkflowControlDefaultsV1,
        };

        let controls = resolve_execution_policy_v1(
            &WorkflowControlDefaultsV1 {
                fan_out: Some(FanOutPolicyV1::FailFast),
                ..WorkflowControlDefaultsV1::default()
            },
            &ExecutionPolicyInvocationV1::default(),
            false,
            PolicyActivationV1::Production,
        )
        .unwrap();
        AttemptReservationV2 {
            schema_version: WORKFLOW_HISTORY_EVIDENCE_SCHEMA_V1,
            reservation: served_reservation(identity, ExecutionSurface::Offline),
            controls_json: String::from_utf8(controls.encode_canonical().unwrap()).unwrap(),
            controls_fingerprint: "controls-a".into(),
            expected_node_count: 2,
            nodes: vec![
                HistoryNodeReservationV1 {
                    node: NodeId::parse("root").unwrap(),
                    sorted_ordinal: 0,
                },
                HistoryNodeReservationV1 {
                    node: NodeId::parse("synth").unwrap(),
                    sorted_ordinal: 1,
                },
            ],
        }
    }

    #[tokio::test]
    async fn memory_v2_history_reserves_exact_roster_and_commits_trigger_atomically() {
        use crate::execution_policy::{
            ControlEventIdV1, FanOutPolicyNameV1, NodeCleanupDispositionV1, NodeCleanupV1,
            NodePrimaryDispositionV1, NodeTerminalV1, PolicyNodeRefV1, PolicyTriggerV1,
            EXECUTION_POLICY_SCHEMA_V1,
        };

        let store = MemoryWorkflowHistoryStore::new();
        let identity = AttemptIdentity::initial().unwrap();
        let reservation = structured_reservation(identity.clone());
        store.reserve_v2(&reservation).await.unwrap();

        let empty = store
            .structured_evidence_v2(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(empty.reservation, reservation);
        assert!(empty.node_terminals.is_empty());
        assert!(empty.policy_trigger_json.is_none());
        assert_eq!(
            node_terminal_placeholder_v1().len(),
            crate::execution_policy::MAX_NODE_TERMINAL_JSON_BYTES
        );

        let root = NodeId::parse("root").unwrap();
        let trigger = PolicyTriggerV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            id: ControlEventIdV1::for_attempt(&identity.attempt_id, 0),
            node: PolicyNodeRefV1::from_node_id(0, root.as_str()),
            policy: FanOutPolicyNameV1::FailFast,
            grace_ms: None,
        };
        let root_terminal = NodeTerminalV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            primary: NodePrimaryDispositionV1::Failed,
            cleanup: NodeCleanupV1 {
                disposition: NodeCleanupDispositionV1::Complete,
                duration_ms: 1,
            },
            cause: None,
            prompt_may_have_been_accepted: true,
            degraded_ancestry: false,
            policy_trigger_id: Some(trigger.id.clone()),
        };
        let terminal_json = String::from_utf8(root_terminal.encode_canonical().unwrap()).unwrap();
        let trigger_json = String::from_utf8(trigger.encode_canonical().unwrap()).unwrap();
        assert_eq!(
            store
                .commit_node_terminal_v2(
                    &identity.attempt_id,
                    &root,
                    &terminal_json,
                    Some(&trigger_json),
                )
                .await
                .unwrap(),
            TerminalWrite::Applied
        );
        assert_eq!(
            store
                .commit_node_terminal_v2(
                    &identity.attempt_id,
                    &root,
                    &terminal_json,
                    Some(&trigger_json),
                )
                .await
                .unwrap(),
            TerminalWrite::Replayed
        );
        let mut conflicting_root_terminal = root_terminal.clone();
        conflicting_root_terminal.cleanup.duration_ms = 2;
        let conflicting_terminal_json =
            String::from_utf8(conflicting_root_terminal.encode_canonical().unwrap()).unwrap();
        assert_eq!(
            store
                .commit_node_terminal_v2(
                    &identity.attempt_id,
                    &root,
                    &conflicting_terminal_json,
                    Some(&trigger_json),
                )
                .await
                .unwrap(),
            TerminalWrite::Conflict
        );

        let evidence = store
            .structured_evidence_v2(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            evidence.policy_trigger_json.as_deref(),
            Some(trigger_json.as_str())
        );
        assert_eq!(
            evidence.node_terminals,
            vec![HistoryNodeTerminalV1 {
                node: root,
                sorted_ordinal: 0,
                terminal_json,
            }]
        );

        let mut summary = completed("shape-a", true).terminal;
        summary.outcome = "failed".into();
        summary.terminal_reason = "failed".into();
        summary.prompt_acceptance = "dispatch_uncertain".into();
        summary.node_counts = NodeCounts {
            failed: 1,
            ..NodeCounts::default()
        };
        assert_eq!(
            store
                .terminalize(&identity.attempt_id, &summary)
                .await
                .unwrap_err()
                .reason,
            LedgerUnavailableReason::Schema,
            "terminalization must not outrun the missing synth terminal"
        );

        let synth = NodeId::parse("synth").unwrap();
        let synth_terminal = NodeTerminalV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            primary: NodePrimaryDispositionV1::Completed,
            cleanup: NodeCleanupV1 {
                disposition: NodeCleanupDispositionV1::Complete,
                duration_ms: 1,
            },
            cause: None,
            prompt_may_have_been_accepted: true,
            degraded_ancestry: true,
            policy_trigger_id: None,
        };
        let synth_terminal_json =
            String::from_utf8(synth_terminal.encode_canonical().unwrap()).unwrap();
        assert_eq!(
            store
                .commit_node_terminal_v2(&identity.attempt_id, &synth, &synth_terminal_json, None,)
                .await
                .unwrap(),
            TerminalWrite::Applied
        );
        summary.node_counts.completed = 1;
        summary.policy_trigger_json = Some(trigger_json);
        assert_eq!(
            store
                .terminalize(&identity.attempt_id, &summary)
                .await
                .unwrap(),
            TerminalWrite::Applied
        );
    }

    #[tokio::test]
    async fn memory_boot_reconciliation_completes_structured_placeholders() {
        use crate::execution_policy::{
            NodeCleanupDispositionV1, NodePrimaryDispositionV1, NodeTerminalV1,
        };

        let store = MemoryWorkflowHistoryStore::new();
        let identity = AttemptIdentity::initial().unwrap();
        let reservation = structured_reservation(identity.clone());
        store.reserve_v2(&reservation).await.unwrap();
        assert_eq!(store.interrupt_active(3_000).await.unwrap(), 1);

        let evidence = store
            .structured_evidence_v2(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(evidence.node_terminals.len(), 2);
        for persisted in &evidence.node_terminals {
            let terminal =
                NodeTerminalV1::decode_canonical(persisted.terminal_json.as_bytes()).unwrap();
            assert_eq!(
                terminal.primary,
                NodePrimaryDispositionV1::InterruptedLegacy
            );
            assert_eq!(
                terminal.cleanup.disposition,
                NodeCleanupDispositionV1::UnknownLegacy
            );
        }
        let terminal = store
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(terminal.outcome, "interrupted");
        assert!(terminal.policy_trigger_json.is_none());
    }

    #[tokio::test]
    async fn served_summary_gap_accepts_one_child_but_other_surfaces_fail_closed() {
        let parent = AttemptIdentity::initial().unwrap();
        let child = served_reservation(parent.resume().unwrap(), ExecutionSurface::ServedTask);
        let fork = served_reservation(parent.resume().unwrap(), ExecutionSurface::ServedTask);
        let store = MemoryWorkflowHistoryStore::new();
        store.reserve(&child).await.unwrap();
        assert_eq!(
            store.reserve(&fork).await.unwrap_err().reason,
            LedgerUnavailableReason::Collision
        );

        let direct_store = MemoryWorkflowHistoryStore::new();
        let direct = served_reservation(parent.resume().unwrap(), ExecutionSurface::DirectUnary);
        assert_eq!(
            direct_store.reserve(&direct).await.unwrap_err().reason,
            LedgerUnavailableReason::Collision,
            "only a served task has an independent durable resume CAS"
        );
    }

    fn completed(fingerprint: &str, fingerprint_complete: bool) -> CompletedAttempt {
        CompletedAttempt {
            reservation: AttemptReservation {
                identity: AttemptIdentity::initial().unwrap(),
                task_id: None,
                workflow: "review".into(),
                task_class: "workflow".into(),
                surface: ExecutionSurface::Offline,
                policy: "r2f0a".into(),
                workload_fingerprint: fingerprint.into(),
                started_ms: 1_000,
                workload_fingerprint_complete: fingerprint_complete,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            },
            terminal: AttemptTerminal {
                completed_ms: 2_000,
                work_ms: 100,
                end_to_end_ms: 120,
                queue_ms: 0,
                cancellation_ms: 0,
                cleanup_ms: 0,
                finalization_ms: 0,
                outcome: "completed".into(),
                terminal_reason: "completed".into(),
                producer_terminal: "unknown".into(),
                final_message: "unknown".into(),
                process_liveness: "unknown".into(),
                terminal_evidence_capability: "unsupported".into(),
                terminal_evidence_version: "none".into(),
                terminal_evidence_source: "none".into(),
                terminal_evidence_complete: false,
                terminal_evidence_counts: Default::default(),
                degraded: false,
                prompt_acceptance: "not_dispatched".into(),
                cleanup_disposition: "complete".into(),
                node_counts: NodeCounts {
                    completed: 1,
                    ..NodeCounts::default()
                },
                policy_trigger_json: None,
                phase_durations: Vec::new(),
                telemetry_complete: true,
                monotonic_clock: true,
            },
        }
    }

    #[test]
    fn calibration_sufficiency_never_combines_heterogeneous_workloads() {
        let mut rows = Vec::new();
        rows.extend((0..20).map(|_| completed("shape-a", true)));
        rows.extend((0..20).map(|_| completed("shape-b", true)));
        let mixed = report(0, 3_000, &rows);
        assert_eq!(mixed.calibration_sample_count, 40);
        assert!(!mixed.sufficient);
        assert!(mixed
            .partitions
            .values()
            .all(|partition| !partition.sufficient));

        rows.extend((0..10).map(|_| completed("shape-a", true)));
        let partitioned = report(0, 3_000, &rows);
        assert!(partitioned.sufficient);
        assert_eq!(
            partitioned
                .partitions
                .iter()
                .find(|(key, _)| key.ends_with("shape-a"))
                .unwrap()
                .1
                .sample_count,
            30
        );
    }

    #[test]
    fn incomplete_workload_identity_is_excluded_from_calibration() {
        let rows = vec![completed("shape-unknown", false)];
        let value = report(0, 3_000, &rows);
        assert_eq!(value.calibration_sample_count, 0);
        assert_eq!(value.excluded["workload_fingerprint_incomplete"], 1);
        assert!(value.partitions.is_empty());
    }

    #[test]
    fn legacy_direct_timing_shape_is_excluded_after_upgrade() {
        let mut legacy = completed("legacy-direct", true);
        legacy.terminal.work_ms = legacy.terminal.end_to_end_ms;
        legacy.terminal.phase_durations = vec![PhaseDuration {
            phase: "work".into(),
            duration_ms: legacy.terminal.work_ms,
        }];

        for surface in [ExecutionSurface::DirectUnary, ExecutionSurface::Mcp] {
            legacy.reservation.surface = surface;
            let projected = compatibility_project_completed(&legacy).into_owned();
            assert_eq!(
                projected.terminal.end_to_end_ms,
                legacy.terminal.end_to_end_ms
            );
            assert_eq!(projected.terminal.work_ms, 0);
            assert!(projected.terminal.phase_durations.is_empty());
            assert!(!projected.terminal.telemetry_complete);
            assert_eq!(
                compatibility_project_completed(&projected).into_owned(),
                projected,
                "the compatibility projection must be idempotent"
            );

            let value = report(0, 3_000, std::slice::from_ref(&legacy));
            assert_eq!(value.calibration_sample_count, 0);
            assert_eq!(value.excluded["telemetry_incomplete"], 1);
            assert!(value.work_ms.is_none());
            assert!(value.end_to_end_ms.is_none());
        }
    }

    #[test]
    fn timing_compatibility_projection_preserves_distinguishable_evidence() {
        let mut offline = completed("offline", true);
        offline.terminal.work_ms = offline.terminal.end_to_end_ms;
        offline.terminal.phase_durations = vec![PhaseDuration {
            phase: "work".into(),
            duration_ms: offline.terminal.work_ms,
        }];
        assert_eq!(compatibility_project_completed(&offline).as_ref(), &offline);
        assert_eq!(report(0, 3_000, &[offline]).calibration_sample_count, 1);

        let mut distinct = completed("distinct-direct", true);
        distinct.reservation.surface = ExecutionSurface::DirectUnary;
        distinct.terminal.work_ms = distinct.terminal.end_to_end_ms;
        distinct.terminal.phase_durations = vec![PhaseDuration {
            phase: "provider_work".into(),
            duration_ms: distinct.terminal.work_ms,
        }];
        assert_eq!(
            compatibility_project_completed(&distinct).as_ref(),
            &distinct
        );

        let mut incomplete = completed("incomplete-direct", true);
        incomplete.reservation.surface = ExecutionSurface::DirectUnary;
        incomplete.terminal.telemetry_complete = false;
        incomplete.terminal.work_ms = incomplete.terminal.end_to_end_ms;
        incomplete.terminal.phase_durations = vec![PhaseDuration {
            phase: "work".into(),
            duration_ms: incomplete.terminal.work_ms,
        }];
        assert_eq!(
            compatibility_project_completed(&incomplete).as_ref(),
            &incomplete
        );
    }

    #[test]
    fn terminal_evidence_fields_reject_incoherent_combinations() {
        let mut value = completed("shape-a", true).terminal;
        assert!(value.validate().is_ok());
        value.terminal_evidence_complete = true;
        assert_eq!(
            value.validate().unwrap_err().reason,
            LedgerUnavailableReason::Schema
        );
        value.terminal_evidence_capability = "v1".into();
        value.terminal_evidence_version = "v1".into();
        value.terminal_evidence_source = "adapter".into();
        assert!(value.validate().is_ok());
    }
    #[derive(Default)]
    struct OneShotFaultStore {
        inner: MemoryWorkflowHistoryStore,
        fail_reserve: std::sync::atomic::AtomicBool,
        fail_prompt: std::sync::atomic::AtomicBool,
        fail_terminal: std::sync::atomic::AtomicBool,
        fail_activity: std::sync::atomic::AtomicBool,
        commit_then_fail_terminal: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl WorkflowHistoryStore for OneShotFaultStore {
        async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
            if self
                .fail_reserve
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            self.inner.reserve(row).await
        }

        async fn mark_prompt_acceptance(
            &self,
            id: &AttemptId,
            acceptance: &str,
        ) -> Result<(), LedgerError> {
            if self
                .fail_prompt
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            self.inner.mark_prompt_acceptance(id, acceptance).await
        }

        async fn terminalize(
            &self,
            id: &AttemptId,
            terminal: &AttemptTerminal,
        ) -> Result<TerminalWrite, LedgerError> {
            if self
                .commit_then_fail_terminal
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.inner.terminalize(id, terminal).await?;
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            if self
                .fail_terminal
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            self.inner.terminalize(id, terminal).await
        }

        async fn record_activity_tally(
            &self,
            id: &AttemptId,
            tally: &crate::attempt_activity::ActivityTally,
        ) -> Result<(), LedgerError> {
            if self
                .fail_activity
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            self.inner.record_activity_tally(id, tally).await
        }

        async fn set_pinned(&self, id: &AttemptId, pinned: bool) -> Result<bool, LedgerError> {
            self.inner.set_pinned(id, pinned).await
        }

        async fn interrupt_active(&self, completed_ms: i64) -> Result<u64, LedgerError> {
            self.inner.interrupt_active(completed_ms).await
        }

        async fn latest_reservation_for_task(
            &self,
            task: &TaskId,
        ) -> Result<Option<AttemptReservation>, LedgerError> {
            self.inner.latest_reservation_for_task(task).await
        }

        async fn attempt(&self, id: &AttemptId) -> Result<Option<AttemptRecord>, LedgerError> {
            self.inner.attempt(id).await
        }

        async fn completed_between(
            &self,
            start_ms: i64,
            end_ms: i64,
        ) -> Result<Vec<CompletedAttempt>, LedgerError> {
            self.inner.completed_between(start_ms, end_ms).await
        }
    }

    #[tokio::test]
    async fn direct_barrier_one_shot_fault_matrix_is_shared_by_all_direct_surfaces() {
        for surface in [
            ExecutionSurface::DirectUnary,
            ExecutionSurface::Mcp,
            ExecutionSurface::Smoke,
        ] {
            let admission_store = std::sync::Arc::new(OneShotFaultStore::default());
            admission_store
                .fail_reserve
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
            let error = DirectAttemptBarrier::admit(admission_store, reservation, "caller_aborted")
                .await
                .err()
                .expect("admission fault must refuse");
            assert_eq!(error.reason, LedgerUnavailableReason::Io);

            let prompt_store = std::sync::Arc::new(OneShotFaultStore::default());
            let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
            let attempt_id = reservation.identity.attempt_id.clone();
            let mut barrier =
                DirectAttemptBarrier::admit(prompt_store.clone(), reservation, "caller_aborted")
                    .await
                    .unwrap();
            prompt_store
                .fail_prompt
                .store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(barrier.mark_prompt_dispatch().await.is_err());
            barrier
                .finish("failed", "prompt_barrier_failed", true, "unknown", true)
                .await
                .unwrap();
            let terminal = prompt_store
                .attempt(&attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap();
            assert!(terminal.degraded);
            assert!(!terminal.telemetry_complete);
            assert_eq!(terminal.prompt_acceptance, "unknown");

            let activity_store = std::sync::Arc::new(OneShotFaultStore::default());
            let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
            let attempt_id = reservation.identity.attempt_id.clone();
            let mut barrier =
                DirectAttemptBarrier::admit(activity_store.clone(), reservation, "caller_aborted")
                    .await
                    .unwrap();
            barrier.record_activity(
                crate::attempt_activity::AttemptPhase::Provider,
                crate::attempt_activity::ActivityReason::MessageDelta,
                1,
            );
            activity_store
                .fail_activity
                .store(true, std::sync::atomic::Ordering::SeqCst);
            barrier
                .finish("completed", "completed", false, "complete", true)
                .await
                .unwrap();
            let terminal = activity_store
                .attempt(&attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap();
            assert_eq!(terminal.outcome, "completed");
            assert_eq!(terminal.terminal_reason, "completed");

            let terminal_store = std::sync::Arc::new(OneShotFaultStore::default());
            let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
            let attempt_id = reservation.identity.attempt_id.clone();
            let mut barrier =
                DirectAttemptBarrier::admit(terminal_store.clone(), reservation, "caller_aborted")
                    .await
                    .unwrap();
            terminal_store
                .fail_terminal
                .store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(barrier
                .finish("failed", "terminal_retry", true, "failed", false)
                .await
                .is_err());
            barrier
                .finish("completed", "changed_retry", false, "complete", true)
                .await
                .unwrap();
            let terminal = terminal_store
                .attempt(&attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap();
            assert_eq!(terminal.outcome, "failed");
            assert_eq!(terminal.terminal_reason, "terminal_retry");
            assert_eq!(terminal.cleanup_disposition, "failed");
            assert!(!terminal.telemetry_complete);
        }
    }

    #[tokio::test]
    async fn ambiguous_terminal_commit_replays_the_exact_first_summary() {
        let store = std::sync::Arc::new(OneShotFaultStore::default());
        let reservation = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::DirectUnary,
        );
        let attempt_id = reservation.identity.attempt_id.clone();
        let mut barrier = DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
            .await
            .unwrap();
        store
            .commit_then_fail_terminal
            .store(true, std::sync::atomic::Ordering::SeqCst);

        assert!(barrier
            .finish("failed", "cleanup_failed", true, "failed", false)
            .await
            .is_err());
        let (write, returned) = barrier
            .finish("completed", "changed_retry", false, "complete", true)
            .await
            .unwrap();
        assert_eq!(write, TerminalWrite::Replayed);

        let persisted = store
            .attempt(&attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(returned, persisted);
        assert_eq!(persisted.outcome, "failed");
        assert_eq!(persisted.terminal_reason, "cleanup_failed");
        assert_eq!(persisted.cleanup_disposition, "failed");
        assert!(!persisted.telemetry_complete);
    }

    #[tokio::test]
    async fn drop_retries_the_prepared_terminal_without_losing_cleanup_evidence() {
        let store = std::sync::Arc::new(OneShotFaultStore::default());
        let reservation = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::DirectUnary,
        );
        let attempt_id = reservation.identity.attempt_id.clone();
        let mut barrier = DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
            .await
            .unwrap();
        store
            .fail_terminal
            .store(true, std::sync::atomic::Ordering::SeqCst);

        assert!(barrier
            .finish("failed", "cleanup_failed", true, "failed", false)
            .await
            .is_err());
        drop(barrier);

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Some(terminal) = store
                    .attempt(&attempt_id)
                    .await
                    .unwrap()
                    .and_then(|record| record.terminal)
                {
                    break terminal;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop fallback must settle the prepared terminal");
        assert_eq!(terminal.outcome, "failed");
        assert_eq!(terminal.terminal_reason, "cleanup_failed");
        assert_eq!(terminal.cleanup_disposition, "failed");
        assert!(!terminal.telemetry_complete);
    }

    #[tokio::test]
    async fn direct_barrier_persists_truthful_cleanup_vocabulary() {
        for disposition in ["complete", "failed", "not_needed", "unknown"] {
            let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
            let reservation = served_reservation(
                AttemptIdentity::initial().unwrap(),
                ExecutionSurface::DirectUnary,
            );
            let attempt_id = reservation.identity.attempt_id.clone();
            let mut barrier =
                DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
                    .await
                    .unwrap();
            barrier
                .finish(
                    "completed",
                    "completed",
                    disposition != "complete",
                    disposition,
                    disposition != "unknown",
                )
                .await
                .unwrap();
            assert_eq!(
                store
                    .attempt(&attempt_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .terminal
                    .unwrap()
                    .cleanup_disposition,
                disposition
            );
        }
    }

    #[tokio::test]
    async fn direct_barrier_does_not_publish_collapsed_elapsed_as_complete_work_timing() {
        for surface in [ExecutionSurface::DirectUnary, ExecutionSurface::Mcp] {
            let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
            let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
            let attempt_id = reservation.identity.attempt_id.clone();
            let mut barrier =
                DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
                    .await
                    .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            barrier
                .finish("completed", "completed", false, "complete", true)
                .await
                .unwrap();

            let terminal = store
                .attempt(&attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap();
            assert!(terminal.end_to_end_ms > 0);
            assert_eq!(terminal.work_ms, 0);
            assert!(terminal.phase_durations.is_empty());
            assert!(!terminal.telemetry_complete);

            let completed = store.completed_between(0, i64::MAX).await.unwrap();
            let report = report(0, i64::MAX, &completed);
            assert_eq!(report.sample_count, 1);
            assert_eq!(report.calibration_sample_count, 0);
            assert_eq!(report.excluded.get("telemetry_incomplete"), Some(&1));
        }
    }

    #[tokio::test]
    async fn prompt_failure_cleanup_matrix_is_identical_for_every_direct_surface() {
        for surface in [
            ExecutionSurface::DirectUnary,
            ExecutionSurface::Mcp,
            ExecutionSurface::Smoke,
        ] {
            for disposition in ["complete", "failed", "not_needed", "unknown"] {
                let store = std::sync::Arc::new(OneShotFaultStore::default());
                let reservation = served_reservation(AttemptIdentity::initial().unwrap(), surface);
                let attempt_id = reservation.identity.attempt_id.clone();
                let mut barrier =
                    DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
                        .await
                        .unwrap();
                store
                    .fail_prompt
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let original = barrier
                    .mark_prompt_dispatch()
                    .await
                    .expect_err("the injected barrier error must remain primary");
                assert_eq!(original.reason, LedgerUnavailableReason::Io);
                barrier
                    .finish("failed", "later_cleanup_reason", false, disposition, true)
                    .await
                    .unwrap();
                let terminal = store
                    .attempt(&attempt_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .terminal
                    .unwrap();
                assert_eq!(terminal.terminal_reason, "prompt_barrier_failed");
                assert_eq!(terminal.cleanup_disposition, disposition);
                assert_eq!(terminal.prompt_acceptance, "unknown");
                assert!(terminal.degraded);
                assert!(!terminal.telemetry_complete);
            }
        }
    }
    #[test]
    fn excluded_populations_preserve_outcome_and_quality_dimensions() {
        let mut failed = completed("failed", true);
        failed.terminal.outcome = "failed".into();
        failed.terminal.degraded = true;
        let mut canceled = completed("canceled", true);
        canceled.terminal.outcome = "canceled".into();
        canceled.terminal.degraded = true;
        let mut interrupted = completed("interrupted", true);
        interrupted.terminal.outcome = "interrupted".into();
        interrupted.terminal.degraded = true;
        let mut degraded = completed("degraded", true);
        degraded.terminal.degraded = true;
        let mut telemetry = completed("telemetry", true);
        telemetry.terminal.telemetry_complete = false;
        let fingerprint = completed("fingerprint", false);
        let mut non_monotonic = completed("clock", true);
        non_monotonic.terminal.monotonic_clock = false;

        let value = report(
            0,
            3_000,
            &[
                failed,
                canceled,
                interrupted,
                degraded,
                telemetry,
                fingerprint,
                non_monotonic,
            ],
        );
        assert_eq!(value.excluded["failed"], 1);
        assert_eq!(value.excluded["canceled"], 1);
        assert_eq!(value.excluded["interrupted"], 1);
        assert_eq!(value.excluded["completed_but_degraded"], 1);
        assert_eq!(value.excluded["telemetry_incomplete"], 1);
        assert_eq!(value.excluded["workload_fingerprint_incomplete"], 1);
        assert_eq!(value.excluded["non_monotonic"], 1);
    }

    fn multi_provider_binding(attempt_id: &str) -> crate::terminal_evidence::TurnEvidenceBinding {
        crate::terminal_evidence::TurnEvidenceBinding {
            generation: 1,
            session_id: "bridge-session".into(),
            turn_id: "turn-leg-1".into(),
            attempt_id: attempt_id.into(),
            marker_nonce: "00112233445566778899aabbccddeeff".into(),
        }
    }

    #[tokio::test]
    async fn r2f0b_plain_unary_counts_only_a_reached_provider_turn() {
        for dispatched in [false, true] {
            let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
            let reservation = served_reservation(
                AttemptIdentity::initial().unwrap(),
                ExecutionSurface::DirectUnary,
            );
            let mut barrier = DirectAttemptBarrier::admit(store, reservation, "caller_aborted")
                .await
                .unwrap();
            if dispatched {
                barrier.mark_prompt_dispatch().await.unwrap();
            }

            let (_, terminal) = barrier
                .finish("completed", "completed", false, "not_needed", true)
                .await
                .unwrap();
            assert_eq!(
                terminal.terminal_evidence_counts.reached,
                u32::from(dispatched),
                "plain unary provider reachability must follow the durable dispatch boundary"
            );
            assert_eq!(terminal.terminal_evidence_counts.valid, 0);
            assert_eq!(terminal.terminal_evidence_counts.missing, 0);
            assert_eq!(terminal.terminal_evidence_counts.invalid, 0);
        }
    }

    #[tokio::test]
    async fn r2f0b_multi_provider_zero_reached_legs_terminal_is_not_applicable() {
        let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
        let reservation = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::DirectUnary,
        );
        let mut barrier = DirectAttemptBarrier::admit(store, reservation, "caller_aborted")
            .await
            .unwrap();
        barrier.mark_prompt_dispatch().await.unwrap();
        barrier.begin_multi_provider_terminal();

        let (_, terminal) = barrier
            .finish("failed", "prompt_failed", true, "unknown", true)
            .await
            .unwrap();
        assert_eq!(terminal.terminal_evidence_capability, "not_applicable");
        assert_eq!(terminal.terminal_evidence_version, "none");
        assert_eq!(terminal.terminal_evidence_source, "none");
        assert!(terminal.terminal_evidence_complete);
        assert_eq!(terminal.terminal_evidence_counts.reached, 0);
        assert_eq!(terminal.producer_terminal, "unknown");
        assert_eq!(terminal.final_message, "unknown");
        assert_eq!(terminal.outcome, "failed");
        assert_eq!(
            terminal.terminal_reason, "protocol_terminal_unknown",
            "the accepted-failed resolution is unchanged from the legacy single sink"
        );
    }

    #[tokio::test]
    async fn r2f0b_multi_provider_two_reached_legs_keep_unknown_with_bounded_counts() {
        let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
        let reservation = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::DirectUnary,
        );
        let attempt_id = reservation.identity.attempt_id.clone();
        let mut barrier = DirectAttemptBarrier::admit(store, reservation, "caller_aborted")
            .await
            .unwrap();
        barrier.mark_prompt_dispatch().await.unwrap();
        barrier.begin_multi_provider_terminal();
        let (_scope_a, _leg_a) = barrier.multi_provider_leg(
            crate::terminal_evidence::EvidenceCapability::V1,
            Some(multi_provider_binding(attempt_id.as_str())),
        );
        let (_scope_b, _leg_b) = barrier.multi_provider_leg(
            crate::terminal_evidence::EvidenceCapability::Unsupported,
            None,
        );

        let (_, terminal) = barrier
            .finish("completed", "completed", false, "unknown", true)
            .await
            .unwrap();
        assert_eq!(terminal.outcome, "completed", "fan-out policy unchanged");
        assert_eq!(terminal.terminal_evidence_capability, "unknown");
        assert_eq!(terminal.terminal_evidence_version, "none");
        assert_eq!(terminal.terminal_evidence_source, "none");
        assert!(!terminal.terminal_evidence_complete);
        assert_eq!(terminal.terminal_evidence_counts.reached, 2);
        assert_eq!(terminal.terminal_evidence_counts.missing, 1);
        assert_eq!(terminal.terminal_evidence_counts.valid, 0);
        assert_eq!(terminal.terminal_evidence_counts.invalid, 0);
        assert_eq!(terminal.producer_terminal, "unknown");
        assert_eq!(terminal.final_message, "unknown");
    }

    #[tokio::test]
    async fn r2f0b_multi_provider_single_reached_leg_projects_exact_evidence_and_scope() {
        let store = std::sync::Arc::new(MemoryWorkflowHistoryStore::new());
        let reservation = served_reservation(
            AttemptIdentity::initial().unwrap(),
            ExecutionSurface::DirectUnary,
        );
        let attempt_id = reservation.identity.attempt_id.clone();
        let mut barrier = DirectAttemptBarrier::admit(store.clone(), reservation, "caller_aborted")
            .await
            .unwrap();
        barrier.mark_prompt_dispatch().await.unwrap();
        barrier.begin_multi_provider_terminal();
        let binding = multi_provider_binding(attempt_id.as_str());
        let (scope, leg) = barrier.multi_provider_leg(
            crate::terminal_evidence::EvidenceCapability::V1,
            Some(binding.clone()),
        );
        let _ = scope.record(
            crate::attempt_activity::AttemptPhase::Provider,
            crate::attempt_activity::ActivityReason::MessageDelta,
            3,
        );
        assert_eq!(
            leg.accept(crate::terminal_evidence::TurnEvidenceEnvelope {
                version: crate::terminal_evidence::TURN_EVIDENCE_VERSION.into(),
                generation: binding.generation,
                session_id: binding.session_id,
                turn_id: binding.turn_id,
                attempt_id: binding.attempt_id,
                marker_nonce: binding.marker_nonce,
                native_turn_id: "native-leg".into(),
                sequence: 1,
                producer: crate::terminal_evidence::ProducerTerminal::Completed,
                final_presence: crate::terminal_evidence::FinalPresence::Nonempty,
                ordered_notifications_drained: true,
                complete: true,
            }),
            crate::terminal_evidence::EvidenceAcceptance::Accepted,
        );
        leg.record_deliverable_final();

        let (_, terminal) = barrier
            .finish("completed", "completed", false, "unknown", true)
            .await
            .unwrap();
        assert_eq!(terminal.outcome, "completed");
        assert_eq!(terminal.terminal_reason, "completed_final");
        assert_eq!(terminal.terminal_evidence_capability, "v1");
        assert_eq!(terminal.terminal_evidence_version, "v1");
        assert_eq!(terminal.terminal_evidence_source, "adapter");
        assert!(terminal.terminal_evidence_complete);
        assert_eq!(terminal.terminal_evidence_counts.reached, 1);
        assert_eq!(terminal.terminal_evidence_counts.valid, 1);
        assert_eq!(terminal.producer_terminal, "completed");
        assert_eq!(terminal.final_message, "nonempty");

        let tally = store
            .activity_tally(&attempt_id)
            .await
            .unwrap()
            .expect("attempt tally recorded");
        assert!(
            tally.max_advance >= 3,
            "the leg scope feeds the shared attempt tally: {tally:?}"
        );
    }
}
