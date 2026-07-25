//! Bounded R2f0a workflow-attempt history domain and storage port.
use crate::ids::{AttemptId, AttemptIdentity, TaskId};

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
    pub degraded: bool,
    pub prompt_acceptance: String,
    pub cleanup_disposition: String,
    pub node_counts: NodeCounts,
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
                self.terminal_evidence_version == "v1" && self.terminal_evidence_source == "adapter"
            }
            _ => false,
        };
        if self.completed_ms <= 0
            || dims.iter().any(|v| !bounded(v, MAX_DIMENSION_LEN))
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
                "not_applicable" | "unsupported" | "v1"
            )
            || !matches!(self.terminal_evidence_version.as_str(), "none" | "v1")
            || !matches!(self.terminal_evidence_source.as_str(), "none" | "adapter")
            || !matches!(
                self.prompt_acceptance.as_str(),
                "not_dispatched" | "dispatch_uncertain" | "unknown"
            )
            || !matches!(
                self.cleanup_disposition.as_str(),
                "complete" | "failed" | "not_needed" | "unknown"
            )
            || !evidence_coherent
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

    pub async fn mark_prompt_dispatch(&mut self) -> Result<(), LedgerError> {
        match self
            .store
            .mark_prompt_acceptance(&self.identity.attempt_id, "dispatch_uncertain")
            .await
        {
            Ok(()) => {
                self.prompt_acceptance = "dispatch_uncertain";
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
            outcome: outcome.into(),
            terminal_reason: if self.prompt_barrier_failed {
                "prompt_barrier_failed".into()
            } else {
                reason.into()
            },
            producer_terminal: "unknown".into(),
            final_message: "unknown".into(),
            process_liveness: "unknown".into(),
            terminal_evidence_capability: "unsupported".into(),
            terminal_evidence_version: "none".into(),
            terminal_evidence_source: "none".into(),
            terminal_evidence_complete: false,
            degraded: degraded || self.prompt_barrier_failed,
            prompt_acceptance: self.prompt_acceptance.into(),
            cleanup_disposition: cleanup_disposition.into(),
            node_counts: NodeCounts::default(),
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
        match self
            .store
            .terminalize(&self.identity.attempt_id, &terminal)
            .await?
        {
            write @ (TerminalWrite::Applied | TerminalWrite::Replayed) => {
                self.terminalized = true;
                Ok((write, terminal))
            }
            TerminalWrite::Conflict => {
                self.terminalized = true;
                Err(LedgerError::new(LedgerUnavailableReason::Collision))
            }
        }
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
}

impl MemoryWorkflowHistoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl WorkflowHistoryStore for MemoryWorkflowHistoryStore {
    async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
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
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| LedgerError::new(LedgerUnavailableReason::Io))?;
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
        let mut changed = 0_u64;
        for (reservation, terminal) in rows.values_mut() {
            if terminal.is_some() || excluded.contains(reservation.identity.attempt_id.as_str()) {
                continue;
            }
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
                degraded: true,
                prompt_acceptance: reservation.prompt_acceptance.clone(),
                cleanup_disposition: "unknown".into(),
                node_counts: NodeCounts::default(),
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
                degraded: false,
                prompt_acceptance: "not_dispatched".into(),
                cleanup_disposition: "complete".into(),
                node_counts: NodeCounts {
                    completed: 1,
                    ..NodeCounts::default()
                },
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
}
