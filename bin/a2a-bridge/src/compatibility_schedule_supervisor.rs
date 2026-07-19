//! R3d1 deadline and supervision state machine.
//!
//! The mechanism is deliberately default-off until R3d2 supplies authority, admission, and accounting.
//! R3d1 tests it only with local fake controls; every signal-producing transition is journaled first.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::compatibility_process_group::{
    process_group_members, AnchorDropPolicy, AnchoredGroupSignal, AnchoredProcessGroup,
    ProcessIdentityV1,
};
use crate::compatibility_schedule_schema::{
    deadline_derivation_input_sha256, validate_child_artifact_join, validate_deadline_derivation,
    validate_supervisor_record, AnchorLifecycleV1, AnchoredProcessGroupRecordV1,
    ChildArtifactJoinV1, ChildArtifactRefV1, DeadlineContainmentV1, DeadlineDerivationInputV1,
    DeadlineDerivationV1, DeadlinePhaseBudgetsV1, FingerprintV1, OptionalChildArtifactRefV1,
    OptionalElapsedMsV1, OptionalProcessIdentityV1, OptionalSafetyHoldReasonV1, OptionalSha256V1,
    OptionalSupervisorKillCauseV1, OptionalSupervisorOutcomeV1, SafetyHoldReasonV1,
    SupervisorKillCauseV1, SupervisorPhaseV1, SupervisorRecordV1, SupervisorTerminalOutcomeV1,
};
use crate::BoxError;

#[derive(Debug)]
pub(super) struct HardDeadline {
    process_entry: Instant,
    absolute: Instant,
    record: DeadlineDerivationV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeadlinePhase {
    MetadataFetch,
    CheckoutCandidateBuild,
    Preflight,
    ResolutionMaterialization,
    SelectedCase(usize),
    EvidencePublication,
    ColdArchiveHandoff,
    CleanupGrace,
}

#[derive(Debug)]
pub(super) struct VerifiedChildArtifact {
    reference: ChildArtifactRefV1,
}

impl VerifiedChildArtifact {
    const MAX_JOIN_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_AGGREGATE_BYTES: u64 = 16 * 1024 * 1024;

    pub(super) fn load(join_path: &Path, aggregate_path: Option<&Path>) -> Result<Self, BoxError> {
        let join_snapshot = crate::local_file::read_regular_file_bounded(
            join_path,
            "schedule supervisor child artifact join",
            Self::MAX_JOIN_BYTES,
        )?;
        let join: ChildArtifactJoinV1 =
            serde_json::from_slice(&join_snapshot.bytes).map_err(|error| {
                format!("schedule supervisor: invalid child artifact join: {error}")
            })?;
        validate_child_artifact_join(&join)?;
        match (&join.aggregate_sha256, aggregate_path) {
            (OptionalSha256V1::Absent, None) => {}
            (OptionalSha256V1::Sha256 { value }, Some(path)) => {
                let aggregate = crate::local_file::read_regular_file_bounded(
                    path,
                    "schedule supervisor child aggregate",
                    Self::MAX_AGGREGATE_BYTES,
                )?;
                if &aggregate.sha256 != value {
                    return Err(
                        "schedule supervisor: child aggregate byte hash does not match the join"
                            .into(),
                    );
                }
                crate::compatibility::validate_child_aggregate_bytes(&aggregate.bytes)?;
            }
            (OptionalSha256V1::Absent, Some(_)) => {
                return Err(
                    "schedule supervisor: unexpected child aggregate without a joined hash".into(),
                )
            }
            (OptionalSha256V1::Sha256 { .. }, None) => {
                return Err("schedule supervisor: joined child aggregate is missing".into())
            }
        }
        Ok(Self {
            reference: ChildArtifactRefV1 {
                record_id: join.record_id,
                run_id: join.run_id,
                window_id: join.window_id,
                artifact_sha256: join_snapshot.sha256,
                aggregate_sha256: join.aggregate_sha256,
            },
        })
    }

    fn into_reference(self) -> ChildArtifactRefV1 {
        self.reference
    }
}

fn duration_ms_ceil(duration: Duration) -> Result<u64, BoxError> {
    let nanos = duration.as_nanos();
    let millis = nanos
        .checked_add(999_999)
        .ok_or("schedule supervisor: monotonic duration overflows")?
        / 1_000_000;
    u64::try_from(millis)
        .map_err(|_| "schedule supervisor: monotonic duration does not fit u64".into())
}

pub(super) fn schedule_tick_parent(
    process_entry: Instant,
    args: &[String],
) -> Result<(), BoxError> {
    // Capture/validate the monotonic origin before inspecting any future scheduling inputs. R3d1
    // intentionally has no input grammar that could reach credentials or a provider-capable spawn.
    let _entry_elapsed_ms = duration_ms_ceil(process_entry.elapsed())?;
    if !args.is_empty() {
        return Err(
            "compatibility schedule-tick: r3d2_authority_admission_not_implemented; no_effects; arguments are disabled"
                .into(),
        );
    }
    Err("compatibility schedule-tick: r3d2_authority_admission_not_implemented; no_effects".into())
}

fn sum_deadline_budgets(budgets: &DeadlinePhaseBudgetsV1) -> Result<u64, BoxError> {
    let mut total = 0_u64;
    for value in [
        budgets.metadata_fetch_ms,
        budgets.checkout_candidate_build_ms,
        budgets.preflight_ms,
        budgets.resolution_materialization_ms,
        budgets.evidence_publication_ms,
        budgets.cold_archive_handoff_ms,
        budgets.cleanup_grace_ms,
        budgets.fixed_margin_ms,
    ]
    .into_iter()
    .chain(budgets.selected_cases.iter().map(|case| case.timeout_ms))
    {
        total = total
            .checked_add(value)
            .ok_or("schedule supervisor: deadline phase sum overflows")?;
    }
    Ok(total)
}

impl HardDeadline {
    pub(super) fn derive(
        process_entry: Instant,
        run_id: String,
        window_id: String,
        budgets: DeadlinePhaseBudgetsV1,
        containment: DeadlineContainmentV1,
    ) -> Result<Self, BoxError> {
        let total_bound_ms = sum_deadline_budgets(&budgets)?;
        let derivation_now = Instant::now();
        let exact_elapsed = derivation_now
            .checked_duration_since(process_entry)
            .ok_or("schedule supervisor: process-entry origin is in the future")?;
        let process_entry_elapsed_ms = duration_ms_ceil(exact_elapsed)?;
        let remaining_at_derivation_ms = total_bound_ms
            .checked_sub(process_entry_elapsed_ms)
            .ok_or("schedule supervisor: deadline was consumed during derivation")?;
        let input = DeadlineDerivationInputV1 {
            schema_version: 1,
            run_id,
            window_id,
            process_entry_elapsed_ms,
            budgets,
            total_bound_ms,
            remaining_at_derivation_ms,
            containment,
        };
        let record = DeadlineDerivationV1 {
            schema_version: 1,
            derivation: FingerprintV1 {
                schema_version: 1,
                sha256: deadline_derivation_input_sha256(&input)?,
            },
            input,
        };
        validate_deadline_derivation(&record)?;
        // Bind the executable deadline to the same conservatively rounded remaining duration that
        // the containment record validated. Using process_entry + total here would leave up to one
        // unrepresented millisecond outside the admitted schedule/grant/accounting windows.
        let absolute = derivation_now
            .checked_add(Duration::from_millis(remaining_at_derivation_ms))
            .ok_or("schedule supervisor: absolute monotonic deadline overflows")?;
        Ok(Self {
            process_entry,
            absolute,
            record,
        })
    }

    pub(super) fn record(&self) -> &DeadlineDerivationV1 {
        &self.record
    }

    pub(super) fn absolute(&self) -> Instant {
        self.absolute
    }

    pub(super) fn elapsed_ms(&self) -> Result<u64, BoxError> {
        duration_ms_ceil(self.process_entry.elapsed())
    }

    pub(super) fn remaining(&self) -> Duration {
        self.absolute.saturating_duration_since(Instant::now())
    }

    fn phase_budget_and_reserved_after(
        &self,
        phase: DeadlinePhase,
    ) -> Result<(String, u64, u64), BoxError> {
        let budgets = &self.record.input.budgets;
        let mut ordered = vec![
            ("metadata_fetch".to_owned(), budgets.metadata_fetch_ms),
            (
                "checkout_candidate_build".to_owned(),
                budgets.checkout_candidate_build_ms,
            ),
            ("preflight".to_owned(), budgets.preflight_ms),
            (
                "resolution_materialization".to_owned(),
                budgets.resolution_materialization_ms,
            ),
        ];
        ordered.extend(
            budgets
                .selected_cases
                .iter()
                .map(|case| (format!("selected_case:{}", case.case_id), case.timeout_ms)),
        );
        let selected_offset = 4;
        let evidence_index = ordered.len();
        ordered.push((
            "evidence_publication".to_owned(),
            budgets.evidence_publication_ms,
        ));
        let cold_index = ordered.len();
        ordered.push((
            "cold_archive_handoff".to_owned(),
            budgets.cold_archive_handoff_ms,
        ));
        let cleanup_index = ordered.len();
        ordered.push(("cleanup_grace".to_owned(), budgets.cleanup_grace_ms));
        let index = match phase {
            DeadlinePhase::MetadataFetch => 0,
            DeadlinePhase::CheckoutCandidateBuild => 1,
            DeadlinePhase::Preflight => 2,
            DeadlinePhase::ResolutionMaterialization => 3,
            DeadlinePhase::SelectedCase(index) => {
                if index >= budgets.selected_cases.len() {
                    return Err(
                        "schedule supervisor: selected-case deadline index is invalid".into(),
                    );
                }
                selected_offset + index
            }
            DeadlinePhase::EvidencePublication => evidence_index,
            DeadlinePhase::ColdArchiveHandoff => cold_index,
            DeadlinePhase::CleanupGrace => cleanup_index,
        };
        let (label, budget_ms) = ordered[index].clone();
        let reserved_after_ms = ordered[index + 1..].iter().try_fold(
            budgets.fixed_margin_ms,
            |reserved, (_, value)| {
                reserved
                    .checked_add(*value)
                    .ok_or("schedule supervisor: phase reservation sum overflows")
            },
        )?;
        Ok((label, budget_ms, reserved_after_ms))
    }

    pub(super) async fn run_phase<T, F>(
        &self,
        phase: DeadlinePhase,
        future: F,
    ) -> Result<T, BoxError>
    where
        F: std::future::Future<Output = Result<T, BoxError>>,
    {
        let (label, budget_ms, reserved_after_ms) = self.phase_budget_and_reserved_after(phase)?;
        let now = Instant::now();
        let local_deadline = now
            .checked_add(Duration::from_millis(budget_ms))
            .ok_or("schedule supervisor: local phase deadline overflows")?;
        let reserve_deadline = self
            .absolute
            .checked_sub(Duration::from_millis(reserved_after_ms))
            .ok_or("schedule supervisor: phase reservations exceed the absolute deadline")?;
        let phase_deadline = local_deadline.min(reserve_deadline);
        if budget_ms == 0 || now >= phase_deadline {
            return Err(format!("schedule supervisor: phase_deadline_exceeded:{label}").into());
        }

        let timer = tokio::time::sleep_until(tokio::time::Instant::from_std(phase_deadline));
        tokio::pin!(timer);
        tokio::pin!(future);
        tokio::select! {
            biased;
            _ = &mut timer => {
                Err(format!("schedule supervisor: phase_deadline_exceeded:{label}").into())
            }
            result = &mut future => result,
        }
    }
}

/// The live Unix capabilities corresponding to the serializable anchored-group records. R3d2 will
/// wire these into admitted runner spawn; R3d1 keeps creation, signaling, membership proof, and final
/// release in one type so no caller can regress to a bare numeric PGID.
#[cfg(unix)]
pub(super) struct SupervisorAnchorSet {
    groups: BTreeMap<i32, AnchoredProcessGroup>,
}

#[cfg(unix)]
impl SupervisorAnchorSet {
    pub(super) fn new() -> Self {
        Self {
            groups: BTreeMap::new(),
        }
    }

    pub(super) fn create_primary(&mut self) -> Result<AnchoredProcessGroupRecordV1, BoxError> {
        let anchor = AnchoredProcessGroup::start_leader(AnchorDropPolicy::ReleaseOnly)?;
        let record = AnchoredProcessGroupRecordV1 {
            process_group: anchor.process_group(),
            session_id: anchor.anchor_identity().session_id,
            anchor: anchor.anchor_identity().clone(),
            workloads: Vec::new(),
            anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
        };
        if self.groups.insert(record.process_group, anchor).is_some() {
            return Err("schedule supervisor: duplicate primary anchor group".into());
        }
        Ok(record)
    }

    pub(super) fn anchor_descendant_group(
        &mut self,
        process_group: i32,
        expected_session: i32,
        workloads: Vec<ProcessIdentityV1>,
    ) -> Result<AnchoredProcessGroupRecordV1, BoxError> {
        if self.groups.contains_key(&process_group) || workloads.is_empty() {
            return Err("schedule supervisor: descendant group is already anchored".into());
        }
        if workloads.iter().any(|workload| {
            workload.process_group != process_group || workload.session_id != expected_session
        }) {
            return Err("schedule supervisor: descendant workload group/session mismatch".into());
        }
        let anchor = AnchoredProcessGroup::anchor_existing_group(
            process_group,
            expected_session,
            AnchorDropPolicy::ReleaseOnly,
        )?;
        // The new exact anchor now prevents PGID recycling. Do not perform another fallible
        // observation before retaining it: registration revalidates every supplied workload through
        // `SupervisorControl`, where any vanished/recycled identity can be journaled with this exact
        // acquired group in a non-signaling safety hold.
        let record = AnchoredProcessGroupRecordV1 {
            process_group,
            session_id: expected_session,
            anchor: anchor.anchor_identity().clone(),
            workloads,
            anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
        };
        let previous = self.groups.insert(process_group, anchor);
        debug_assert!(previous.is_none(), "exclusive anchor insertion cannot race");
        Ok(record)
    }

    pub(super) fn exact_anchor_live(
        &self,
        group: &AnchoredProcessGroupRecordV1,
    ) -> Result<bool, BoxError> {
        let Some(anchor) = self.groups.get(&group.process_group) else {
            return Ok(false);
        };
        Ok(anchor.anchor_is_retained()
            && anchor.anchor_identity() == &group.anchor
            && anchor.anchor_is_exactly_live()?)
    }

    pub(super) fn signal(
        &mut self,
        group: &AnchoredProcessGroupRecordV1,
        signal: SupervisorSignal,
    ) -> Result<(), BoxError> {
        let anchor = self
            .groups
            .get_mut(&group.process_group)
            .ok_or("schedule supervisor: exact retained anchor is missing")?;
        if anchor.anchor_identity() != &group.anchor {
            return Err("schedule supervisor: retained anchor identity changed".into());
        }
        anchor.signal(match signal {
            SupervisorSignal::Term => AnchoredGroupSignal::Term,
            SupervisorSignal::Kill => AnchoredGroupSignal::Kill,
        })?;
        Ok(())
    }

    pub(super) fn non_anchor_members(
        &self,
        group: &AnchoredProcessGroupRecordV1,
    ) -> Result<Vec<ProcessIdentityV1>, BoxError> {
        let members = process_group_members(group.process_group)?;
        Ok(members
            .into_iter()
            .filter(|member| member != &group.anchor)
            .collect())
    }

    pub(super) async fn release_and_reap(
        &mut self,
        group: &AnchoredProcessGroupRecordV1,
    ) -> Result<(), BoxError> {
        let mut anchor = self
            .groups
            .remove(&group.process_group)
            .ok_or("schedule supervisor: exact retained anchor is missing")?;
        if anchor.anchor_identity() != &group.anchor {
            return Err("schedule supervisor: retained anchor identity changed".into());
        }
        anchor.release_and_reap().await?;
        Ok(())
    }

    pub(super) fn group_absent(&self, process_group: i32) -> Result<bool, BoxError> {
        Ok(process_group_members(process_group)?.is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SupervisorSignal {
    Term,
    Kill,
}

pub(super) trait SupervisorControl {
    fn exact_process_live(&mut self, process: &ProcessIdentityV1) -> Result<bool, BoxError>;

    fn exact_runner_live(&mut self, runner: &ProcessIdentityV1) -> Result<bool, BoxError>;

    fn exact_anchor_live(&mut self, group: &AnchoredProcessGroupRecordV1)
        -> Result<bool, BoxError>;

    /// Signal only through the exact retained anchor capability represented by `group`. Implementations
    /// must refuse without issuing a numeric-group signal when that capability is absent or mismatched;
    /// a fallible liveness observation is not a substitute for the retained capability.
    fn signal_group(
        &mut self,
        group: &AnchoredProcessGroupRecordV1,
        signal: SupervisorSignal,
    ) -> Result<(), BoxError>;

    fn exact_runner_exited(&mut self, runner: &ProcessIdentityV1) -> Result<bool, BoxError>;

    fn non_anchor_members(
        &mut self,
        group: &AnchoredProcessGroupRecordV1,
    ) -> Result<Vec<ProcessIdentityV1>, BoxError>;

    fn release_and_reap_anchor(
        &mut self,
        group: &AnchoredProcessGroupRecordV1,
    ) -> Result<(), BoxError>;

    fn group_absent(&mut self, process_group: i32) -> Result<bool, BoxError>;

    fn reap_exact_containers(&mut self, labels: &[String]) -> Result<(), BoxError>;

    fn exact_containers_absent(&mut self, labels: &[String]) -> Result<bool, BoxError>;
}

pub(super) trait SupervisorJournal {
    fn persist(&mut self, record: &SupervisorRecordV1) -> Result<String, BoxError>;
}

fn field_is_write_once<T: PartialEq>(previous: &T, next: &T, absent: &T) -> bool {
    previous == next || previous == absent
}

fn validate_supervisor_transition(
    previous: &SupervisorRecordV1,
    next: &SupervisorRecordV1,
) -> Result<(), BoxError> {
    validate_supervisor_record(previous)?;
    validate_supervisor_record(next)?;
    if next.generation
        != previous
            .generation
            .checked_add(1)
            .ok_or("schedule supervisor: prior journal generation overflows")?
        || next.recorded_at_ms <= previous.recorded_at_ms
    {
        return Err("schedule supervisor: journal generation/time did not advance".into());
    }
    if previous.schema_version != next.schema_version
        || previous.supervisor_record_id != next.supervisor_record_id
        || previous.run_id != next.run_id
        || previous.window_id != next.window_id
        || previous.trigger != next.trigger
        || previous.deadline_derivation_sha256 != next.deadline_derivation_sha256
        || previous.scheduler != next.scheduler
        || previous.container_run_labels != next.container_run_labels
    {
        return Err("schedule supervisor: immutable journal identity changed".into());
    }
    if !field_is_write_once(
        &previous.runner,
        &next.runner,
        &OptionalProcessIdentityV1::Absent,
    ) || !field_is_write_once(
        &previous.term_journal_elapsed_ms,
        &next.term_journal_elapsed_ms,
        &OptionalElapsedMsV1::Absent,
    ) || !field_is_write_once(
        &previous.kill_journal_elapsed_ms,
        &next.kill_journal_elapsed_ms,
        &OptionalElapsedMsV1::Absent,
    ) || !field_is_write_once(
        &previous.kill_cause,
        &next.kill_cause,
        &OptionalSupervisorKillCauseV1::Absent,
    ) || !field_is_write_once(
        &previous.outcome,
        &next.outcome,
        &OptionalSupervisorOutcomeV1::Absent,
    ) || !field_is_write_once(
        &previous.safety_hold,
        &next.safety_hold,
        &OptionalSafetyHoldReasonV1::Absent,
    ) || !field_is_write_once(
        &previous.child_artifact,
        &next.child_artifact,
        &OptionalChildArtifactRefV1::Absent,
    ) || (!previous.later_group_signal_permitted && next.later_group_signal_permitted)
    {
        return Err("schedule supervisor: write-once journal state changed or regressed".into());
    }

    let phase_allowed = matches!(
        (previous.phase, next.phase),
        (SupervisorPhaseV1::Prepared, SupervisorPhaseV1::Running)
            | (SupervisorPhaseV1::Prepared, SupervisorPhaseV1::SafetyHold)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::Running)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::TermGrace)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::Reaping)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::SafetyHold)
            | (
                SupervisorPhaseV1::TermGrace,
                SupervisorPhaseV1::KillJournaled
            )
            | (SupervisorPhaseV1::TermGrace, SupervisorPhaseV1::Reaping)
            | (SupervisorPhaseV1::TermGrace, SupervisorPhaseV1::SafetyHold)
            | (SupervisorPhaseV1::KillJournaled, SupervisorPhaseV1::Reaping)
            | (
                SupervisorPhaseV1::KillJournaled,
                SupervisorPhaseV1::SafetyHold
            )
            | (SupervisorPhaseV1::Reaping, SupervisorPhaseV1::Reaping)
            | (SupervisorPhaseV1::Reaping, SupervisorPhaseV1::Complete)
            | (SupervisorPhaseV1::Reaping, SupervisorPhaseV1::SafetyHold)
    );
    if !phase_allowed {
        return Err("schedule supervisor: journal phase transition is not monotonic".into());
    }

    let additions_allowed = matches!(
        (previous.phase, next.phase),
        (SupervisorPhaseV1::Prepared, SupervisorPhaseV1::Running)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::Running)
            | (SupervisorPhaseV1::Running, SupervisorPhaseV1::SafetyHold)
    );
    for previous_group in &previous.groups {
        let Some(next_group) = next
            .groups
            .iter()
            .find(|group| group.process_group == previous_group.process_group)
        else {
            return Err("schedule supervisor: anchored group disappeared from journal".into());
        };
        let workloads_are_safe = if matches!(
            (previous.phase, next.phase),
            (SupervisorPhaseV1::Prepared, SupervisorPhaseV1::Running)
        ) {
            previous_group
                .workloads
                .iter()
                .all(|workload| next_group.workloads.contains(workload))
        } else {
            previous_group.workloads == next_group.workloads
        };
        let lifecycle_is_safe = match (previous_group.anchor_lifecycle, next_group.anchor_lifecycle)
        {
            (AnchorLifecycleV1::RetainedLive, AnchorLifecycleV1::RetainedLive)
            | (AnchorLifecycleV1::ReleasedReaped, AnchorLifecycleV1::ReleasedReaped)
            | (AnchorLifecycleV1::Ambiguous, AnchorLifecycleV1::Ambiguous) => true,
            (AnchorLifecycleV1::RetainedLive, AnchorLifecycleV1::ReleasedReaped) => {
                next.phase == SupervisorPhaseV1::Reaping && !next.later_group_signal_permitted
            }
            (AnchorLifecycleV1::RetainedLive, AnchorLifecycleV1::Ambiguous) => {
                next.phase == SupervisorPhaseV1::SafetyHold && !next.later_group_signal_permitted
            }
            _ => false,
        };
        if previous_group.session_id != next_group.session_id
            || previous_group.anchor != next_group.anchor
            || !workloads_are_safe
            || !lifecycle_is_safe
        {
            return Err(
                "schedule supervisor: anchored group identity/lifecycle changed unsafely".into(),
            );
        }
    }
    if next.groups.len() < previous.groups.len()
        || (next.groups.len() > previous.groups.len() && !additions_allowed)
        || next.groups.iter().any(|group| {
            previous
                .groups
                .iter()
                .all(|prior| prior.process_group != group.process_group)
                && group.anchor_lifecycle != AnchorLifecycleV1::RetainedLive
                && !(next.phase == SupervisorPhaseV1::SafetyHold
                    && group.anchor_lifecycle == AnchorLifecycleV1::Ambiguous)
        })
    {
        return Err("schedule supervisor: anchored group inventory changed unsafely".into());
    }
    Ok(())
}

#[cfg(unix)]
pub(super) struct FileSupervisorJournal {
    directory: PathBuf,
    directory_handle: std::fs::File,
    record_id: String,
    next_generation: u64,
    previous_sha256: Option<String>,
}

#[cfg(unix)]
impl FileSupervisorJournal {
    const MAX_JOURNAL_RECORD_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_JOURNAL_GENERATIONS: usize = 10_000;

    fn validate_record_id(record_id: &str) -> Result<(), BoxError> {
        let mut bytes = record_id.bytes();
        if record_id.is_empty()
            || record_id.len() > 128
            || !matches!(bytes.next(), Some(b'a'..=b'z') | Some(b'0'..=b'9'))
            || !bytes.all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err("schedule supervisor: journal record id is not a stable id".into());
        }
        Ok(())
    }

    fn open_directory(directory: &Path) -> Result<(PathBuf, std::fs::File), BoxError> {
        use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

        let canonical = std::fs::canonicalize(directory).map_err(|error| {
            format!(
                "schedule supervisor: cannot resolve journal directory {}: {error}",
                directory.display()
            )
        })?;
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&canonical)
            .map_err(|error| {
                format!(
                    "schedule supervisor: cannot open journal directory {}: {error}",
                    canonical.display()
                )
            })?;
        let metadata = handle.metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(
                "schedule supervisor: journal directory must be owner-owned and owner-private"
                    .into(),
            );
        }
        Ok((canonical, handle))
    }

    fn generation_name(record_id: &str, generation: u64) -> String {
        format!("{record_id}.{generation:020}.json")
    }

    pub(super) fn create(directory: &Path, record_id: &str) -> Result<Self, BoxError> {
        Self::validate_record_id(record_id)?;
        let (directory, directory_handle) = Self::open_directory(directory)?;
        let mut journal = Self {
            directory,
            directory_handle,
            record_id: record_id.to_owned(),
            next_generation: 1,
            previous_sha256: None,
        };
        if !journal.generation_entries()?.is_empty() {
            return Err("schedule supervisor: journal already contains this record id".into());
        }
        Ok(journal)
    }

    fn generation_entries(&mut self) -> Result<Vec<(u64, String)>, BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let expected = self.directory_handle.metadata()?;
        let before = std::fs::metadata(&self.directory)?;
        if expected.dev() != before.dev() || expected.ino() != before.ino() {
            return Err("schedule supervisor: journal directory identity changed".into());
        }
        let prefix = format!("{}.", self.record_id);
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(&prefix) || !name.ends_with(".json") {
                continue;
            }
            let raw_generation = &name[prefix.len()..name.len() - ".json".len()];
            if raw_generation.len() != 20
                || !raw_generation.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err("schedule supervisor: malformed journal generation name".into());
            }
            let generation = raw_generation
                .parse::<u64>()
                .map_err(|_| "schedule supervisor: journal generation does not fit u64")?;
            paths.push((generation, name.to_owned()));
        }
        if paths.len() > Self::MAX_JOURNAL_GENERATIONS {
            return Err("schedule supervisor: journal generation count exceeds the bound".into());
        }
        let after = std::fs::metadata(&self.directory)?;
        if expected.dev() != after.dev() || expected.ino() != after.ino() {
            return Err(
                "schedule supervisor: journal directory identity changed during scan".into(),
            );
        }
        paths.sort_by_key(|(generation, _)| *generation);
        Ok(paths)
    }

    fn read_generation(&self, name: &str) -> Result<(Vec<u8>, String), BoxError> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::fs::MetadataExt as _;

        let c_name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "schedule supervisor: journal generation name contains NUL")?;
        // SAFETY: the retained directory descriptor and single-component name are live. O_NOFOLLOW
        // rejects a retargeted final component and O_NONBLOCK prevents a special file from parking
        // the recovery scan before the regular-file check.
        let fd = unsafe {
            libc::openat(
                self.directory_handle.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd == -1 {
            return Err(format!(
                "schedule supervisor: cannot open journal generation {name}: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        // SAFETY: openat returned this descriptor uniquely.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let before = file.metadata()?;
        if !before.is_file()
            || before.nlink() != 1
            || before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o177 != 0
            || before.len() > Self::MAX_JOURNAL_RECORD_BYTES
        {
            return Err(
                "schedule supervisor: journal generation is not a bounded owner-private regular file"
                    .into(),
            );
        }
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut file)
            .take(Self::MAX_JOURNAL_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > Self::MAX_JOURNAL_RECORD_BYTES {
            return Err("schedule supervisor: journal generation exceeds the byte bound".into());
        }
        let after = file.metadata()?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || bytes.len() as u64 != after.len()
        {
            return Err("schedule supervisor: journal generation changed during read".into());
        }
        let sha256 = crate::local_file::sha256_hex(&bytes);
        Ok((bytes, sha256))
    }

    pub(super) fn open_existing(
        directory: &Path,
        record_id: &str,
    ) -> Result<(Self, SupervisorRecordV1, String), BoxError> {
        Self::validate_record_id(record_id)?;
        let (directory, directory_handle) = Self::open_directory(directory)?;
        let mut journal = Self {
            directory,
            directory_handle,
            record_id: record_id.to_owned(),
            next_generation: 1,
            previous_sha256: None,
        };
        let entries = journal.generation_entries()?;
        if entries.is_empty() {
            return Err("schedule supervisor: journal has no generations".into());
        }
        let mut previous_sha256 = None;
        let mut latest = None;
        for (index, (generation, name)) in entries.into_iter().enumerate() {
            let expected_generation = u64::try_from(index + 1)
                .map_err(|_| "schedule supervisor: journal generation index overflows")?;
            if generation != expected_generation {
                return Err("schedule supervisor: journal generations are not contiguous".into());
            }
            let (bytes, sha256) = journal.read_generation(&name)?;
            let record: SupervisorRecordV1 = serde_json::from_slice(&bytes).map_err(|error| {
                format!("schedule supervisor: invalid journal generation: {error}")
            })?;
            validate_supervisor_record(&record)?;
            if record.supervisor_record_id != record_id || record.generation != generation {
                return Err("schedule supervisor: journal generation identity mismatch".into());
            }
            match (&record.previous_record, previous_sha256.as_deref()) {
                (OptionalSha256V1::Absent, None) => {}
                (OptionalSha256V1::Sha256 { value }, Some(previous)) if value == previous => {}
                _ => return Err("schedule supervisor: journal hash chain is broken".into()),
            }
            if let Some(previous) = latest.as_ref() {
                validate_supervisor_transition(previous, &record)?;
            } else if record.phase != SupervisorPhaseV1::Prepared {
                return Err("schedule supervisor: initial journal phase is not prepared".into());
            }
            previous_sha256 = Some(sha256);
            latest = Some(record);
        }
        let latest = latest.expect("non-empty journal has a latest record");
        let latest_sha256 = previous_sha256.expect("non-empty journal has a latest hash");
        journal.next_generation = latest
            .generation
            .checked_add(1)
            .ok_or("schedule supervisor: journal generation overflows")?;
        journal.previous_sha256 = Some(latest_sha256.clone());
        Ok((journal, latest, latest_sha256))
    }
}

#[cfg(unix)]
impl SupervisorJournal for FileSupervisorJournal {
    fn persist(&mut self, record: &SupervisorRecordV1) -> Result<String, BoxError> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::fs::MetadataExt as _;

        validate_supervisor_record(record)?;
        if record.supervisor_record_id != self.record_id
            || record.generation != self.next_generation
        {
            return Err("schedule supervisor: journal append identity/generation mismatch".into());
        }
        match (&record.previous_record, self.previous_sha256.as_deref()) {
            (OptionalSha256V1::Absent, None) => {}
            (OptionalSha256V1::Sha256 { value }, Some(previous)) if value == previous => {}
            _ => return Err("schedule supervisor: journal append hash chain mismatch".into()),
        }
        let mut bytes = serde_json::to_vec_pretty(record)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > Self::MAX_JOURNAL_RECORD_BYTES {
            return Err("schedule supervisor: journal generation exceeds the byte bound".into());
        }
        let name = Self::generation_name(&self.record_id, record.generation);
        let c_name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "schedule supervisor: journal generation name contains NUL")?;
        // SAFETY: the retained directory descriptor and NUL-terminated single-component name are
        // live. O_EXCL/O_NOFOLLOW prevent replacement, and the returned descriptor is uniquely owned.
        let fd = unsafe {
            libc::openat(
                self.directory_handle.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd == -1 {
            return Err(format!(
                "schedule supervisor: cannot create journal generation {name}: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        // SAFETY: `fd` was returned uniquely by openat above.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        // SAFETY: the newly created descriptor is exclusively owned and remains live for both calls.
        if unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) } == -1
            || unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } == -1
        {
            return Err(format!(
                "schedule supervisor: cannot bind journal generation ownership/mode: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o177 != 0
        {
            return Err(
                "schedule supervisor: journal generation is not an owner-private regular file"
                    .into(),
            );
        }
        self.directory_handle.sync_all()?;
        let sha256 = crate::local_file::sha256_hex(&bytes);
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or("schedule supervisor: journal generation overflows")?;
        self.previous_sha256 = Some(sha256.clone());
        Ok(sha256)
    }
}

pub(super) struct ScheduleSupervisor<J> {
    record: SupervisorRecordV1,
    current_record_sha256: String,
    journal: J,
}

impl<J: SupervisorJournal> ScheduleSupervisor<J> {
    pub(super) fn initialize(
        mut record: SupervisorRecordV1,
        mut journal: J,
    ) -> Result<Self, BoxError> {
        if record.generation != 1 || record.phase != SupervisorPhaseV1::Prepared {
            return Err(
                "schedule supervisor: initial generation must be prepared generation 1".into(),
            );
        }
        record.previous_record = OptionalSha256V1::Absent;
        validate_supervisor_record(&record)?;
        let current_record_sha256 = journal.persist(&record)?;
        Ok(Self {
            record,
            current_record_sha256,
            journal,
        })
    }

    /// Resumes the validated tail returned by a journal implementation such as
    /// [`FileSupervisorJournal::open_existing`]. The caller must supply that exact tail hash; the
    /// next transition binds it as `previous_record` before any recovery effect is allowed.
    pub(super) fn resume_existing(
        record: SupervisorRecordV1,
        current_record_sha256: String,
        journal: J,
    ) -> Result<Self, BoxError> {
        validate_supervisor_record(&record)?;
        if current_record_sha256.len() != 64
            || current_record_sha256
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(
                "schedule supervisor: existing record hash is not canonical SHA-256".into(),
            );
        }
        Ok(Self {
            record,
            current_record_sha256,
            journal,
        })
    }

    pub(super) fn record(&self) -> &SupervisorRecordV1 {
        &self.record
    }

    pub(super) fn journal(&self) -> &J {
        &self.journal
    }

    #[cfg(test)]
    fn journal_mut(&mut self) -> &mut J {
        &mut self.journal
    }

    fn transition(&mut self, mut candidate: SupervisorRecordV1) -> Result<(), BoxError> {
        candidate.generation = self
            .record
            .generation
            .checked_add(1)
            .ok_or("schedule supervisor: journal generation overflows")?;
        candidate.previous_record = OptionalSha256V1::Sha256 {
            value: self.current_record_sha256.clone(),
        };
        candidate.recorded_at_ms = candidate.recorded_at_ms.saturating_add(1);
        validate_supervisor_record(&candidate)?;
        validate_supervisor_transition(&self.record, &candidate)?;
        let sha256 = self.journal.persist(&candidate)?;
        self.record = candidate;
        self.current_record_sha256 = sha256;
        Ok(())
    }

    pub(super) fn start_running(
        &mut self,
        runner: crate::compatibility_process_group::ProcessIdentityV1,
        groups: Vec<AnchoredProcessGroupRecordV1>,
    ) -> Result<(), BoxError> {
        if self.record.phase != SupervisorPhaseV1::Prepared {
            return Err("schedule supervisor: runner can start only from prepared".into());
        }
        let mut candidate = self.record.clone();
        candidate.runner = OptionalProcessIdentityV1::Process { value: runner };
        candidate.groups = groups;
        candidate.phase = SupervisorPhaseV1::Running;
        self.transition(candidate)
    }

    pub(super) fn register_descendant_group<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        group: AnchoredProcessGroupRecordV1,
    ) -> Result<(), BoxError> {
        if self.record.phase != SupervisorPhaseV1::Running
            || group.anchor_lifecycle != AnchorLifecycleV1::RetainedLive
            || group.workloads.is_empty()
        {
            return self.enter_hold(SafetyHoldReasonV1::AnchorAcquisitionFailed);
        }
        let runner = match &self.record.runner {
            OptionalProcessIdentityV1::Process { value } => value.clone(),
            OptionalProcessIdentityV1::Absent => {
                return self.enter_hold(SafetyHoldReasonV1::ProcessIdentityUnavailable)
            }
        };
        match control.exact_anchor_live(&group) {
            Ok(true) => {}
            Ok(false) => {
                return self.enter_hold_with_added_group(
                    SafetyHoldReasonV1::AnchorNotLive,
                    group,
                    AnchorLifecycleV1::Ambiguous,
                )
            }
            Err(error) => {
                if let Err(hold_error) = self.enter_hold_with_added_group(
                    SafetyHoldReasonV1::ProcessIdentityUnavailable,
                    group,
                    AnchorLifecycleV1::Ambiguous,
                ) {
                    return Err(format!(
                        "schedule supervisor: anchor observation failed ({error}); durable group hold publication also failed ({hold_error})"
                    )
                    .into());
                }
                return Err(error);
            }
        }
        if group.session_id != runner.session_id
            || group
                .workloads
                .iter()
                .any(|workload| workload.session_id != runner.session_id)
        {
            return self.enter_hold_with_added_group(
                SafetyHoldReasonV1::NewSessionEscape,
                group,
                AnchorLifecycleV1::RetainedLive,
            );
        }
        let mut known_identities = self
            .record
            .groups
            .iter()
            .flat_map(|group| group.workloads.iter().cloned())
            .collect::<Vec<_>>();
        known_identities.push(runner.clone());
        known_identities.sort_by_key(|identity| identity.pid);
        known_identities.dedup();
        for identity in known_identities.iter().chain(group.workloads.iter()) {
            match control.exact_process_live(identity) {
                Ok(true) => {}
                Ok(false) => {
                    return self.enter_hold_with_added_group(
                        SafetyHoldReasonV1::ProcessIdentityUnavailable,
                        group,
                        AnchorLifecycleV1::RetainedLive,
                    )
                }
                Err(error) => {
                    if let Err(hold_error) = self.enter_hold_with_added_group(
                        SafetyHoldReasonV1::ProcessIdentityUnavailable,
                        group,
                        AnchorLifecycleV1::RetainedLive,
                    ) {
                        return Err(format!(
                            "schedule supervisor: process observation failed ({error}); durable group hold publication also failed ({hold_error})"
                        )
                        .into());
                    }
                    return Err(error);
                }
            }
        }
        let mut known = known_identities
            .iter()
            .map(|identity| identity.pid)
            .collect::<BTreeSet<_>>();
        let mut pending = group.workloads.iter().collect::<Vec<_>>();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|workload| {
                if known.contains(&workload.parent_pid) {
                    known.insert(workload.pid);
                    false
                } else {
                    true
                }
            });
            if pending.len() == before {
                return self.enter_hold_with_added_group(
                    SafetyHoldReasonV1::ProcessIdentityUnavailable,
                    group,
                    AnchorLifecycleV1::RetainedLive,
                );
            }
        }
        let mut candidate = self.record.clone();
        candidate.groups.push(group);
        self.transition(candidate)
    }

    pub(super) fn anchor_acquisition_failed(&mut self) -> Result<(), BoxError> {
        self.enter_hold(SafetyHoldReasonV1::AnchorAcquisitionFailed)
    }

    fn enter_hold(&mut self, reason: SafetyHoldReasonV1) -> Result<(), BoxError> {
        self.enter_hold_with_group_lifecycle(reason, None)
    }

    fn enter_hold_with_added_group(
        &mut self,
        reason: SafetyHoldReasonV1,
        mut group: AnchoredProcessGroupRecordV1,
        lifecycle: AnchorLifecycleV1,
    ) -> Result<(), BoxError> {
        if self
            .record
            .groups
            .iter()
            .any(|existing| existing.process_group == group.process_group)
        {
            return self.enter_hold(reason);
        }
        group.anchor_lifecycle = lifecycle;
        let mut candidate = self.record.clone();
        candidate.groups.push(group);
        candidate.phase = SupervisorPhaseV1::SafetyHold;
        candidate.later_group_signal_permitted = false;
        candidate.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::SafetyHold,
        };
        candidate.safety_hold = OptionalSafetyHoldReasonV1::Reason { value: reason };
        self.transition(candidate)
    }

    fn enter_hold_with_group_lifecycle(
        &mut self,
        reason: SafetyHoldReasonV1,
        group_lifecycle: Option<(i32, AnchorLifecycleV1)>,
    ) -> Result<(), BoxError> {
        let mut candidate = self.record.clone();
        if let Some((process_group, lifecycle)) = group_lifecycle {
            let group = candidate
                .groups
                .iter_mut()
                .find(|group| group.process_group == process_group)
                .ok_or("schedule supervisor: hold group is not in the journal inventory")?;
            group.anchor_lifecycle = lifecycle;
        }
        candidate.phase = SupervisorPhaseV1::SafetyHold;
        candidate.later_group_signal_permitted = false;
        candidate.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::SafetyHold,
        };
        candidate.safety_hold = OptionalSafetyHoldReasonV1::Reason { value: reason };
        self.transition(candidate)
    }

    fn mark_group_released(&mut self, process_group: i32) -> Result<(), BoxError> {
        let mut candidate = self.record.clone();
        let group = candidate
            .groups
            .iter_mut()
            .find(|group| group.process_group == process_group)
            .ok_or("schedule supervisor: released group is not in the journal inventory")?;
        group.anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        self.transition(candidate)
    }

    fn control_or_hold<T>(
        &mut self,
        observation: Result<T, BoxError>,
        reason: SafetyHoldReasonV1,
    ) -> Result<T, BoxError> {
        match observation {
            Ok(value) => Ok(value),
            Err(error) => {
                if let Err(hold_error) = self.enter_hold(reason) {
                    return Err(format!(
                        "schedule supervisor: observation failed ({error}); durable hold publication also failed ({hold_error})"
                    )
                    .into());
                }
                Err(error)
            }
        }
    }

    fn begin_term<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        elapsed_ms: u64,
    ) -> Result<(), BoxError> {
        // The retained, unreaped child handle is the exact group capability. A late fallible
        // process observation cannot revoke it or permit PID/PGID reuse. Journal first, then let
        // `signal_group` use that capability and fail closed without a numeric signal if it is
        // actually absent or mismatched.
        let mut candidate = self.record.clone();
        candidate.phase = SupervisorPhaseV1::TermGrace;
        candidate.term_journal_elapsed_ms = OptionalElapsedMsV1::ElapsedMs { value: elapsed_ms };
        self.transition(candidate)?;
        for group in &self.record.groups {
            if let Err(error) = control.signal_group(group, SupervisorSignal::Term) {
                self.enter_hold(SafetyHoldReasonV1::SignalJournalAmbiguous)?;
                return Err(error);
            }
        }
        Ok(())
    }

    fn begin_kill<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        elapsed_ms: u64,
        cause: SupervisorKillCauseV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.record.clone();
        candidate.phase = SupervisorPhaseV1::KillJournaled;
        candidate.kill_journal_elapsed_ms = OptionalElapsedMsV1::ElapsedMs { value: elapsed_ms };
        candidate.kill_cause = OptionalSupervisorKillCauseV1::Cause { value: cause };
        self.transition(candidate)?;
        for group in &self.record.groups {
            if let Err(error) = control.signal_group(group, SupervisorSignal::Kill) {
                self.enter_hold(SafetyHoldReasonV1::SignalJournalAmbiguous)?;
                return Err(error);
            }
        }
        let mut candidate = self.record.clone();
        candidate.phase = SupervisorPhaseV1::Reaping;
        candidate.later_group_signal_permitted = false;
        self.transition(candidate)
    }

    pub(super) fn request_cancel<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        elapsed_ms: u64,
    ) -> Result<(), BoxError> {
        match self.record.phase {
            SupervisorPhaseV1::Running => self.begin_term(control, elapsed_ms),
            SupervisorPhaseV1::TermGrace => self.begin_kill(
                control,
                elapsed_ms,
                SupervisorKillCauseV1::RepeatedCancellation,
            ),
            SupervisorPhaseV1::Prepared
            | SupervisorPhaseV1::KillJournaled
            | SupervisorPhaseV1::Reaping
            | SupervisorPhaseV1::Complete
            | SupervisorPhaseV1::SafetyHold => Ok(()),
        }
    }

    pub(super) fn grace_expired<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        elapsed_ms: u64,
    ) -> Result<(), BoxError> {
        if self.record.phase == SupervisorPhaseV1::TermGrace {
            self.begin_kill(control, elapsed_ms, SupervisorKillCauseV1::Deadline)
        } else {
            Ok(())
        }
    }

    pub(super) fn finish_after_exit<C: SupervisorControl>(
        &mut self,
        control: &mut C,
        child: VerifiedChildArtifact,
    ) -> Result<(), BoxError> {
        if !matches!(
            self.record.phase,
            SupervisorPhaseV1::Running | SupervisorPhaseV1::TermGrace | SupervisorPhaseV1::Reaping
        ) {
            return Err(
                "schedule supervisor: terminal completion is not allowed in this phase".into(),
            );
        }
        let outcome = match (
            &self.record.term_journal_elapsed_ms,
            &self.record.kill_journal_elapsed_ms,
            &self.record.kill_cause,
        ) {
            (
                OptionalElapsedMsV1::Absent,
                OptionalElapsedMsV1::Absent,
                OptionalSupervisorKillCauseV1::Absent,
            ) => SupervisorTerminalOutcomeV1::Completed,
            (
                OptionalElapsedMsV1::ElapsedMs { .. },
                OptionalElapsedMsV1::Absent,
                OptionalSupervisorKillCauseV1::Absent,
            ) => SupervisorTerminalOutcomeV1::Cancelled,
            (
                OptionalElapsedMsV1::ElapsedMs { .. },
                OptionalElapsedMsV1::ElapsedMs { .. },
                OptionalSupervisorKillCauseV1::Cause {
                    value: SupervisorKillCauseV1::Deadline,
                },
            ) => SupervisorTerminalOutcomeV1::KilledAfterDeadline,
            (
                OptionalElapsedMsV1::ElapsedMs { .. },
                OptionalElapsedMsV1::ElapsedMs { .. },
                OptionalSupervisorKillCauseV1::Cause {
                    value: SupervisorKillCauseV1::RepeatedCancellation,
                },
            ) => SupervisorTerminalOutcomeV1::KilledAfterCancellation,
            _ => {
                return Err(
                    "schedule supervisor: terminal outcome cannot be derived from signal state"
                        .into(),
                )
            }
        };
        let child = child.into_reference();
        let runner = match &self.record.runner {
            OptionalProcessIdentityV1::Process { value } => value.clone(),
            OptionalProcessIdentityV1::Absent => {
                return self.enter_hold(SafetyHoldReasonV1::ProcessIdentityUnavailable)
            }
        };
        let runner_exit = control.exact_runner_exited(&runner);
        if !self.control_or_hold(runner_exit, SafetyHoldReasonV1::ExitUnproved)? {
            return self.enter_hold(SafetyHoldReasonV1::ExitUnproved);
        }
        let observed_groups = self.record.groups.clone();
        for group in &observed_groups {
            let members = control.non_anchor_members(group);
            if !self
                .control_or_hold(members, SafetyHoldReasonV1::ProcessIdentityUnavailable)?
                .is_empty()
            {
                return self.enter_hold(SafetyHoldReasonV1::ExitUnproved);
            }
        }

        let mut candidate = self.record.clone();
        candidate.phase = SupervisorPhaseV1::Reaping;
        candidate.later_group_signal_permitted = false;
        candidate.child_artifact = OptionalChildArtifactRefV1::Artifact {
            value: child.clone(),
        };
        self.transition(candidate)?;

        let groups = self.record.groups.clone();
        for group in &groups {
            let anchor_live = control.exact_anchor_live(group);
            if self.control_or_hold(anchor_live, SafetyHoldReasonV1::ProcessIdentityUnavailable)? {
                if let Err(error) = control.release_and_reap_anchor(group) {
                    self.enter_hold_with_group_lifecycle(
                        SafetyHoldReasonV1::AnchorNotLive,
                        Some((group.process_group, AnchorLifecycleV1::Ambiguous)),
                    )?;
                    return Err(error);
                }
                self.mark_group_released(group.process_group)?;
            } else {
                let absent = control.group_absent(group.process_group);
                if !self.control_or_hold(absent, SafetyHoldReasonV1::ProcessIdentityUnavailable)? {
                    return self.enter_hold_with_group_lifecycle(
                        SafetyHoldReasonV1::AnchorNotLive,
                        Some((group.process_group, AnchorLifecycleV1::Ambiguous)),
                    );
                }
                self.mark_group_released(group.process_group)?;
            }
            let absent = control.group_absent(group.process_group);
            if !self.control_or_hold(absent, SafetyHoldReasonV1::ProcessIdentityUnavailable)? {
                return self.enter_hold(SafetyHoldReasonV1::ExitUnproved);
            }
        }
        let labels = self.record.container_run_labels.clone();
        let container_reap = control.reap_exact_containers(&labels);
        self.control_or_hold(
            container_reap,
            SafetyHoldReasonV1::StartupReconciliationIncomplete,
        )?;
        let containers_absent = control.exact_containers_absent(&labels);
        if !self.control_or_hold(
            containers_absent,
            SafetyHoldReasonV1::StartupReconciliationIncomplete,
        )? {
            return self.enter_hold(SafetyHoldReasonV1::ExitUnproved);
        }

        let mut candidate = self.record.clone();
        for group in &mut candidate.groups {
            group.anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        }
        candidate.phase = SupervisorPhaseV1::Complete;
        candidate.outcome = OptionalSupervisorOutcomeV1::Outcome { value: outcome };
        candidate.safety_hold = OptionalSafetyHoldReasonV1::Absent;
        candidate.later_group_signal_permitted = false;
        candidate.child_artifact = OptionalChildArtifactRefV1::Artifact { value: child };
        self.transition(candidate)
    }

    /// Reconciles a journal tail without ever issuing a group signal. Only a provably live,
    /// pre-signal running state may resume. Signal-order ambiguity and missing exact capabilities
    /// are converted into a new durable hold generation.
    pub(super) fn recover<C: SupervisorControl>(
        &mut self,
        control: &mut C,
    ) -> Result<RecoveryDisposition, BoxError> {
        validate_supervisor_record(&self.record)?;
        match self.record.phase {
            SupervisorPhaseV1::Prepared => {
                let groups = self.record.groups.clone();
                for group in &groups {
                    let anchor_live = control.exact_anchor_live(group);
                    if !self.control_or_hold(
                        anchor_live,
                        SafetyHoldReasonV1::StartupReconciliationIncomplete,
                    )? {
                        self.enter_hold(SafetyHoldReasonV1::AnchorNotLive)?;
                        return Ok(RecoveryDisposition::SafetyHold);
                    }
                    let members = control.non_anchor_members(group);
                    if !self
                        .control_or_hold(
                            members,
                            SafetyHoldReasonV1::StartupReconciliationIncomplete,
                        )?
                        .is_empty()
                    {
                        self.enter_hold(SafetyHoldReasonV1::StartupReconciliationIncomplete)?;
                        return Ok(RecoveryDisposition::SafetyHold);
                    }
                }
                Ok(RecoveryDisposition::ResumePreSignal)
            }
            SupervisorPhaseV1::Running => {
                let runner = match &self.record.runner {
                    OptionalProcessIdentityV1::Process { value } => value.clone(),
                    OptionalProcessIdentityV1::Absent => {
                        self.enter_hold(SafetyHoldReasonV1::ProcessIdentityUnavailable)?;
                        return Ok(RecoveryDisposition::SafetyHold);
                    }
                };
                let runner_live = control.exact_runner_live(&runner);
                if !self.control_or_hold(
                    runner_live,
                    SafetyHoldReasonV1::StartupReconciliationIncomplete,
                )? {
                    self.enter_hold(SafetyHoldReasonV1::StartupReconciliationIncomplete)?;
                    return Ok(RecoveryDisposition::SafetyHold);
                }
                let groups = self.record.groups.clone();
                for group in &groups {
                    let anchor_live = control.exact_anchor_live(group);
                    if group.anchor_lifecycle != AnchorLifecycleV1::RetainedLive
                        || !self.control_or_hold(
                            anchor_live,
                            SafetyHoldReasonV1::StartupReconciliationIncomplete,
                        )?
                    {
                        self.enter_hold(SafetyHoldReasonV1::AnchorNotLive)?;
                        return Ok(RecoveryDisposition::SafetyHold);
                    }
                }
                Ok(RecoveryDisposition::ResumeRunning)
            }
            // A journaled TERM/KILL may or may not have reached every group. Recovery persists the
            // ambiguity and never retries a numeric-group signal, even if an anchor still looks live.
            SupervisorPhaseV1::TermGrace | SupervisorPhaseV1::KillJournaled => {
                self.enter_hold(SafetyHoldReasonV1::SignalJournalAmbiguous)?;
                Ok(RecoveryDisposition::SafetyHold)
            }
            // Reaping has already durably forbidden later group signals. The caller may supply the
            // recovered child artifact to `finish_after_exit`, which performs only absence proofs,
            // exact-label container cleanup, and anchor release/reap.
            SupervisorPhaseV1::Reaping => Ok(RecoveryDisposition::ReconcileWithoutSignal),
            SupervisorPhaseV1::Complete => Ok(RecoveryDisposition::Complete),
            SupervisorPhaseV1::SafetyHold => Ok(RecoveryDisposition::SafetyHold),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryDisposition {
    ResumePreSignal,
    ResumeRunning,
    ReconcileWithoutSignal,
    Complete,
    SafetyHold,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_process_group::{
        process_identity, ProcessIdentityV1, ProcessStartMarkerV1,
    };
    use crate::compatibility_schedule::TriggerKindV1;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeSignal {
        Term,
        Kill,
    }

    struct FakeControl {
        anchor_live: bool,
        runner_live: bool,
        fail_process_observation: bool,
        fail_anchor_observation: bool,
        fail_runner_observation: bool,
        stale_process_pids: BTreeSet<i32>,
        fail_signal: bool,
        fail_anchor_release: bool,
        fail_container_reap: bool,
        fail_container_absence: bool,
        recycled: bool,
        signals: Vec<FakeSignal>,
        unrelated_alive: bool,
        term_exits: bool,
        kill_exits: bool,
        runner_exited: bool,
        non_anchor_members: BTreeMap<i32, Vec<ProcessIdentityV1>>,
        released_groups: BTreeSet<i32>,
        containers: BTreeSet<String>,
        unrelated_container_alive: bool,
    }

    impl FakeControl {
        fn recycle_supervised_group_to_unrelated(&mut self) {
            self.anchor_live = false;
            self.recycled = true;
            self.unrelated_alive = true;
        }
    }

    impl SupervisorControl for FakeControl {
        fn exact_process_live(&mut self, process: &ProcessIdentityV1) -> Result<bool, BoxError> {
            if self.fail_process_observation {
                return Err("fake process identity observation failed".into());
            }
            Ok(!self.stale_process_pids.contains(&process.pid))
        }

        fn exact_runner_live(&mut self, _runner: &ProcessIdentityV1) -> Result<bool, BoxError> {
            if self.fail_runner_observation {
                return Err("fake runner identity observation failed".into());
            }
            Ok(self.runner_live && !self.runner_exited)
        }

        fn exact_anchor_live(
            &mut self,
            _group: &AnchoredProcessGroupRecordV1,
        ) -> Result<bool, BoxError> {
            if self.fail_anchor_observation {
                return Err("fake anchor identity observation failed".into());
            }
            Ok(self.anchor_live && !self.recycled)
        }

        fn signal_group(
            &mut self,
            _group: &AnchoredProcessGroupRecordV1,
            signal: SupervisorSignal,
        ) -> Result<(), BoxError> {
            if self.fail_signal {
                return Err("fake group signal failed".into());
            }
            if !self.anchor_live || self.recycled {
                return Err("fake exact retained anchor capability is unavailable".into());
            }
            self.signals.push(match signal {
                SupervisorSignal::Term => FakeSignal::Term,
                SupervisorSignal::Kill => FakeSignal::Kill,
            });
            let exits = match signal {
                SupervisorSignal::Term => self.term_exits,
                SupervisorSignal::Kill => self.kill_exits,
            };
            if exits {
                self.runner_exited = true;
                if let Some(members) = self.non_anchor_members.get_mut(&_group.process_group) {
                    members.clear();
                }
            }
            Ok(())
        }

        fn exact_runner_exited(&mut self, _runner: &ProcessIdentityV1) -> Result<bool, BoxError> {
            Ok(self.runner_exited)
        }

        fn non_anchor_members(
            &mut self,
            group: &AnchoredProcessGroupRecordV1,
        ) -> Result<Vec<ProcessIdentityV1>, BoxError> {
            Ok(self
                .non_anchor_members
                .get(&group.process_group)
                .cloned()
                .unwrap_or_default())
        }

        fn release_and_reap_anchor(
            &mut self,
            group: &AnchoredProcessGroupRecordV1,
        ) -> Result<(), BoxError> {
            if self.fail_anchor_release {
                return Err("fake anchor release failed".into());
            }
            if !self.anchor_live || self.recycled {
                return Err("fake exact anchor is unavailable".into());
            }
            self.released_groups.insert(group.process_group);
            Ok(())
        }

        fn group_absent(&mut self, process_group: i32) -> Result<bool, BoxError> {
            Ok(self.released_groups.contains(&process_group)
                && self
                    .non_anchor_members
                    .get(&process_group)
                    .is_none_or(Vec::is_empty))
        }

        fn reap_exact_containers(&mut self, labels: &[String]) -> Result<(), BoxError> {
            if self.fail_container_reap {
                return Err("fake container reap failed".into());
            }
            for label in labels {
                self.containers.remove(label);
            }
            Ok(())
        }

        fn exact_containers_absent(&mut self, labels: &[String]) -> Result<bool, BoxError> {
            if self.fail_container_absence {
                return Err("fake container absence observation failed".into());
            }
            Ok(labels.iter().all(|label| !self.containers.contains(label)))
        }
    }

    #[derive(Default)]
    struct MemoryJournal {
        records: Vec<SupervisorRecordV1>,
        fail_phase: Option<SupervisorPhaseV1>,
    }

    impl SupervisorJournal for MemoryJournal {
        fn persist(&mut self, record: &SupervisorRecordV1) -> Result<String, BoxError> {
            if self.fail_phase == Some(record.phase) {
                return Err("fake journal publication wedge".into());
            }
            let bytes = serde_json::to_vec(record)?;
            self.records.push(record.clone());
            Ok(crate::local_file::sha256_hex(&bytes))
        }
    }

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn identity_with_parent(pid: i32, parent_pid: i32, group: i32) -> ProcessIdentityV1 {
        ProcessIdentityV1 {
            pid,
            parent_pid,
            process_group: group,
            session_id: 41,
            start: ProcessStartMarkerV1::LinuxBootTicks {
                boot_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
                start_ticks: pid as u64 * 10,
            },
        }
    }

    fn identity(pid: i32, group: i32) -> ProcessIdentityV1 {
        identity_with_parent(pid, 1, group)
    }

    fn prepared_record() -> SupervisorRecordV1 {
        SupervisorRecordV1 {
            schema_version: 1,
            supervisor_record_id: "supervisor-1".into(),
            generation: 1,
            previous_record: OptionalSha256V1::Absent,
            run_id: "run-1".into(),
            window_id: "window-1".into(),
            trigger: TriggerKindV1::Daily,
            deadline_derivation_sha256: digest('a'),
            scheduler: identity(42, 42),
            runner: OptionalProcessIdentityV1::Absent,
            groups: vec![AnchoredProcessGroupRecordV1 {
                process_group: 43,
                session_id: 41,
                anchor: identity(43, 43),
                workloads: Vec::new(),
                anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
            }],
            container_run_labels: vec!["a2a-compat-run-1".into()],
            phase: SupervisorPhaseV1::Prepared,
            term_journal_elapsed_ms: OptionalElapsedMsV1::Absent,
            kill_journal_elapsed_ms: OptionalElapsedMsV1::Absent,
            kill_cause: OptionalSupervisorKillCauseV1::Absent,
            later_group_signal_permitted: true,
            outcome: OptionalSupervisorOutcomeV1::Absent,
            safety_hold: OptionalSafetyHoldReasonV1::Absent,
            child_artifact: OptionalChildArtifactRefV1::Absent,
            recorded_at_ms: 1,
        }
    }

    fn running_fake_supervisor(
        term_exits: bool,
    ) -> (ScheduleSupervisor<MemoryJournal>, FakeControl) {
        let mut supervisor =
            ScheduleSupervisor::initialize(prepared_record(), MemoryJournal::default()).unwrap();
        let runner = identity(44, 43);
        supervisor
            .start_running(
                runner.clone(),
                vec![AnchoredProcessGroupRecordV1 {
                    process_group: 43,
                    session_id: 41,
                    anchor: identity(43, 43),
                    workloads: vec![runner],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                }],
            )
            .unwrap();
        let mut non_anchor_members = BTreeMap::new();
        non_anchor_members.insert(43, vec![identity(44, 43)]);
        (supervisor, fake_control(term_exits, non_anchor_members))
    }

    fn fake_control(
        term_exits: bool,
        non_anchor_members: BTreeMap<i32, Vec<ProcessIdentityV1>>,
    ) -> FakeControl {
        FakeControl {
            anchor_live: true,
            runner_live: true,
            fail_process_observation: false,
            fail_anchor_observation: false,
            fail_runner_observation: false,
            stale_process_pids: BTreeSet::new(),
            fail_signal: false,
            fail_anchor_release: false,
            fail_container_reap: false,
            fail_container_absence: false,
            recycled: false,
            signals: Vec::new(),
            unrelated_alive: true,
            term_exits,
            kill_exits: true,
            runner_exited: false,
            non_anchor_members,
            released_groups: BTreeSet::new(),
            containers: BTreeSet::from(["a2a-compat-run-1".into()]),
            unrelated_container_alive: true,
        }
    }

    fn child_artifact() -> VerifiedChildArtifact {
        child_artifact_for_window("window-1")
    }

    fn child_artifact_for_window(window_id: &str) -> VerifiedChildArtifact {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("child-join.json");
        let join = ChildArtifactJoinV1 {
            schema_version: 1,
            record_id: "aggregate-1".into(),
            run_id: "run-1".into(),
            window_id: window_id.into(),
            aggregate_sha256: OptionalSha256V1::Absent,
        };
        std::fs::write(&path, serde_json::to_vec(&join).unwrap()).unwrap();
        VerifiedChildArtifact::load(&path, None).unwrap()
    }

    fn record_sha256(record: &SupervisorRecordV1) -> String {
        crate::local_file::sha256_hex(&serde_json::to_vec(record).unwrap())
    }

    fn deadline_budgets() -> DeadlinePhaseBudgetsV1 {
        DeadlinePhaseBudgetsV1 {
            metadata_fetch_ms: 10,
            checkout_candidate_build_ms: 20,
            preflight_ms: 30,
            resolution_materialization_ms: 40,
            selected_cases: vec![crate::compatibility_schedule_schema::CaseDeadlineBudgetV1 {
                case_id: "case-1".into(),
                timeout_ms: 50,
            }],
            evidence_publication_ms: 60,
            cold_archive_handoff_ms: 0,
            cleanup_grace_ms: 70,
            fixed_margin_ms: 80,
        }
    }

    #[test]
    fn hard_deadline_is_derived_once_from_process_entry() {
        let process_entry = Instant::now();
        let deadline = HardDeadline::derive(
            process_entry,
            "run-1".into(),
            "window-1".into(),
            deadline_budgets(),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 1_000,
                grant_remaining_ms: 1_000,
                time_budget_remaining_ms: 1_000,
            },
        )
        .unwrap();
        assert_eq!(deadline.record().input.total_bound_ms, 360);
        assert!(deadline.absolute() <= process_entry + Duration::from_millis(360));
        assert!(deadline.remaining() <= Duration::from_millis(360));
        assert!(deadline.elapsed_ms().unwrap() <= 360);

        let short = HardDeadline::derive(
            Instant::now(),
            "run-2".into(),
            "window-2".into(),
            deadline_budgets(),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 1,
                grant_remaining_ms: 1_000,
                time_budget_remaining_ms: 1_000,
            },
        );
        assert!(short.is_err());

        let consumed_entry = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        assert!(HardDeadline::derive(
            consumed_entry,
            "run-3".into(),
            "window-3".into(),
            deadline_budgets(),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 1_000,
                grant_remaining_ms: 1_000,
                time_budget_remaining_ms: 1_000,
            },
        )
        .is_err());
    }

    #[tokio::test]
    async fn publication_wedge_reserves_cleanup_and_margin_under_the_absolute_deadline() {
        let process_entry = Instant::now();
        let deadline = HardDeadline::derive(
            process_entry,
            "run-publication-wedge".into(),
            "window-publication-wedge".into(),
            DeadlinePhaseBudgetsV1 {
                metadata_fetch_ms: 0,
                checkout_candidate_build_ms: 0,
                preflight_ms: 0,
                resolution_materialization_ms: 0,
                selected_cases: Vec::new(),
                evidence_publication_ms: 40,
                cold_archive_handoff_ms: 0,
                cleanup_grace_ms: 20,
                fixed_margin_ms: 20,
            },
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 80,
                grant_remaining_ms: 80,
                time_budget_remaining_ms: 80,
            },
        )
        .unwrap();
        let original_absolute = deadline.absolute();
        let started = Instant::now();

        let error = deadline
            .run_phase(
                DeadlinePhase::EvidencePublication,
                std::future::pending::<Result<(), BoxError>>(),
            )
            .await
            .unwrap_err();

        assert_eq!(deadline.absolute(), original_absolute);
        assert!(started.elapsed() < Duration::from_millis(70));
        assert!(deadline.remaining() > Duration::ZERO);
        assert_eq!(
            error.to_string(),
            "schedule supervisor: phase_deadline_exceeded:evidence_publication"
        );
    }

    #[tokio::test]
    async fn expired_or_zero_budget_phase_never_polls_an_immediately_ready_effect() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let deadline = HardDeadline::derive(
            Instant::now(),
            "run-expired".into(),
            "window-expired".into(),
            DeadlinePhaseBudgetsV1 {
                metadata_fetch_ms: 10,
                checkout_candidate_build_ms: 0,
                preflight_ms: 0,
                resolution_materialization_ms: 0,
                selected_cases: Vec::new(),
                evidence_publication_ms: 0,
                cold_archive_handoff_ms: 0,
                cleanup_grace_ms: 1,
                fixed_margin_ms: 1,
            },
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 12,
                grant_remaining_ms: 12,
                time_budget_remaining_ms: 12,
            },
        )
        .unwrap();
        tokio::time::sleep_until(
            tokio::time::Instant::from_std(deadline.absolute()) + Duration::from_millis(1),
        )
        .await;
        let polled = Arc::new(AtomicBool::new(false));
        let polled_by_effect = Arc::clone(&polled);
        assert!(deadline
            .run_phase(DeadlinePhase::MetadataFetch, async move {
                polled_by_effect.store(true, Ordering::SeqCst);
                Ok::<_, BoxError>(())
            })
            .await
            .is_err());
        assert!(!polled.load(Ordering::SeqCst));

        let zero_deadline = HardDeadline::derive(
            Instant::now(),
            "run-zero".into(),
            "window-zero".into(),
            DeadlinePhaseBudgetsV1 {
                metadata_fetch_ms: 100,
                checkout_candidate_build_ms: 0,
                preflight_ms: 0,
                resolution_materialization_ms: 0,
                selected_cases: Vec::new(),
                evidence_publication_ms: 0,
                cold_archive_handoff_ms: 0,
                cleanup_grace_ms: 10,
                fixed_margin_ms: 10,
            },
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 120,
                grant_remaining_ms: 120,
                time_budget_remaining_ms: 120,
            },
        )
        .unwrap();
        let zero_budget = Arc::new(AtomicBool::new(false));
        let zero_budget_effect = Arc::clone(&zero_budget);
        assert!(zero_deadline
            .run_phase(DeadlinePhase::ColdArchiveHandoff, async move {
                zero_budget_effect.store(true, Ordering::SeqCst);
                Ok::<_, BoxError>(())
            })
            .await
            .is_err());
        assert!(!zero_budget.load(Ordering::SeqCst));
    }

    #[test]
    fn every_phase_uses_its_local_budget_and_reserves_the_complete_tail() {
        let deadline = HardDeadline::derive(
            Instant::now(),
            "run-phase-map".into(),
            "window-phase-map".into(),
            deadline_budgets(),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 360,
                grant_remaining_ms: 360,
                time_budget_remaining_ms: 360,
            },
        )
        .unwrap();
        let expected = [
            (DeadlinePhase::MetadataFetch, 10, 350),
            (DeadlinePhase::CheckoutCandidateBuild, 20, 330),
            (DeadlinePhase::Preflight, 30, 300),
            (DeadlinePhase::ResolutionMaterialization, 40, 260),
            (DeadlinePhase::SelectedCase(0), 50, 210),
            (DeadlinePhase::EvidencePublication, 60, 150),
            (DeadlinePhase::ColdArchiveHandoff, 0, 150),
            (DeadlinePhase::CleanupGrace, 70, 80),
        ];
        for (phase, budget, reserved) in expected {
            let (_, actual_budget, actual_reserved) =
                deadline.phase_budget_and_reserved_after(phase).unwrap();
            assert_eq!((actual_budget, actual_reserved), (budget, reserved));
        }
        assert!(deadline
            .phase_budget_and_reserved_after(DeadlinePhase::SelectedCase(1))
            .is_err());
    }

    #[test]
    fn serialized_remaining_time_never_understates_the_executable_deadline() {
        let process_entry = Instant::now()
            .checked_sub(Duration::from_micros(5_100))
            .unwrap();
        let deadline = HardDeadline::derive(
            process_entry,
            "run-rounded".into(),
            "window-rounded".into(),
            deadline_budgets(),
            DeadlineContainmentV1 {
                schedule_window_remaining_ms: 354,
                grant_remaining_ms: 354,
                time_budget_remaining_ms: 354,
            },
        )
        .unwrap();
        let represented = deadline.record().input.remaining_at_derivation_ms;
        assert!(represented <= 354);
        assert!(deadline.remaining() <= Duration::from_millis(represented));
        assert!(
            deadline.absolute()
                < process_entry + Duration::from_millis(deadline.record().input.total_bound_ms)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_anchor_set_keeps_both_primary_and_descendant_groups_capability_bound() {
        let mut anchors = SupervisorAnchorSet::new();
        let primary = anchors.create_primary().unwrap();
        assert!(anchors.exact_anchor_live(&primary).unwrap());
        assert!(anchors.non_anchor_members(&primary).unwrap().is_empty());
        anchors.signal(&primary, SupervisorSignal::Term).unwrap();
        anchors.release_and_reap(&primary).await.unwrap();
        assert!(anchors.group_absent(primary.process_group).unwrap());

        let mut leader = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let leader_identity = process_identity(leader.id().unwrap() as i32)
            .unwrap()
            .unwrap();
        let descendant = anchors
            .anchor_descendant_group(
                leader_identity.process_group,
                leader_identity.session_id,
                vec![leader_identity.clone()],
            )
            .unwrap();
        assert!(anchors
            .non_anchor_members(&descendant)
            .unwrap()
            .contains(&leader_identity));
        anchors.signal(&descendant, SupervisorSignal::Kill).unwrap();
        let _ = leader.wait().await;
        anchors.release_and_reap(&descendant).await.unwrap();
        assert!(anchors.group_absent(descendant.process_group).unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn descendant_anchor_retains_a_stale_workload_for_a_durable_hold() {
        let mut anchors = SupervisorAnchorSet::new();
        let mut leader = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let leader_identity = process_identity(leader.id().unwrap() as i32)
            .unwrap()
            .unwrap();
        let mut survivor = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(leader_identity.process_group)
            .spawn()
            .unwrap();
        let survivor_identity = process_identity(survivor.id().unwrap() as i32)
            .unwrap()
            .unwrap();
        let mut stale = leader_identity.clone();
        match &mut stale.start {
            ProcessStartMarkerV1::LinuxBootTicks { start_ticks, .. } => {
                *start_ticks = start_ticks.saturating_add(1);
            }
            ProcessStartMarkerV1::MacosEpochMicros { microseconds, .. } => {
                *microseconds = (*microseconds + 1) % 1_000_000;
            }
        }

        let mut initial = prepared_record();
        initial.scheduler.session_id = stale.session_id;
        initial.groups[0].session_id = stale.session_id;
        initial.groups[0].anchor.session_id = stale.session_id;
        let mut running_group = initial.groups[0].clone();
        let mut runner_identity = identity(44, running_group.process_group);
        runner_identity.session_id = stale.session_id;
        running_group.workloads = vec![runner_identity.clone()];
        let mut supervisor =
            ScheduleSupervisor::initialize(initial, MemoryJournal::default()).unwrap();
        supervisor
            .start_running(runner_identity.clone(), vec![running_group])
            .unwrap();
        let mut control = fake_control(
            false,
            BTreeMap::from([(runner_identity.process_group, vec![runner_identity])]),
        );
        control.stale_process_pids.insert(stale.pid);

        let descendant = anchors
            .anchor_descendant_group(
                stale.process_group,
                stale.session_id,
                vec![stale, survivor_identity.clone()],
            )
            .expect("an acquired anchor must remain available for durable supervisor validation");
        assert!(anchors.exact_anchor_live(&descendant).unwrap());
        supervisor
            .register_descendant_group(&mut control, descendant.clone())
            .unwrap();

        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert!(!supervisor.record().later_group_signal_permitted);
        assert_eq!(supervisor.record().groups.len(), 2);
        assert_eq!(
            supervisor.record().groups[1],
            AnchoredProcessGroupRecordV1 {
                anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                ..descendant.clone()
            }
        );
        assert_eq!(
            process_identity(survivor_identity.pid).unwrap().as_ref(),
            Some(&survivor_identity),
            "the surviving workload remains live but durably inventoried in the hold"
        );

        let _ = leader.start_kill();
        let _ = survivor.start_kill();
        let _ = leader.wait().await;
        let _ = survivor.wait().await;
        anchors.release_and_reap(&descendant).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigstopped_fake_child_cannot_drop_the_anchor_or_survive_terminal_kill() {
        use std::os::unix::process::CommandExt as _;

        let mut anchors = SupervisorAnchorSet::new();
        let mut group = anchors.create_primary().unwrap();
        let mut command = tokio::process::Command::new("/bin/cat");
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .process_group(group.process_group);
        // SAFETY: pre_exec runs in the forked child before exec. Ignored dispositions survive
        // exec, making this fake deliberately uncooperative during the TERM grace interval.
        unsafe {
            command.as_std_mut().pre_exec(|| {
                if libc::signal(libc::SIGTERM, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut workload = command.spawn().unwrap();
        let workload_identity = process_identity(workload.id().unwrap() as i32)
            .unwrap()
            .unwrap();
        group.workloads.push(workload_identity.clone());
        // SAFETY: the tokio Child retains this exact live PID; SIGSTOP has no user handler.
        assert_eq!(
            unsafe { libc::kill(workload_identity.pid, libc::SIGSTOP) },
            0
        );
        let mut stop_status = 0;
        loop {
            // SAFETY: this is the exact direct Child PID, and stop_status is writable. WUNTRACED
            // consumes only the stop notification, not the later terminal status/reap.
            let waited =
                unsafe { libc::waitpid(workload_identity.pid, &mut stop_status, libc::WUNTRACED) };
            if waited == workload_identity.pid {
                break;
            }
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EINTR),
                "waitpid failed before the fake child stopped"
            );
        }
        assert!(libc::WIFSTOPPED(stop_status));
        assert_eq!(libc::WSTOPSIG(stop_status), libc::SIGSTOP);

        anchors.signal(&group, SupervisorSignal::Term).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(anchors.exact_anchor_live(&group).unwrap());
        assert_eq!(
            process_identity(workload_identity.pid).unwrap(),
            Some(workload_identity),
            "stopped workload should remain for the journaled terminal KILL"
        );

        anchors.signal(&group, SupervisorSignal::Kill).unwrap();
        let _ = workload.wait().await;
        anchors.release_and_reap(&group).await.unwrap();
        assert!(anchors.group_absent(group.process_group).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn file_journal_reopens_a_contiguous_fsynced_hash_chain() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let journal = FileSupervisorJournal::create(directory.path(), "supervisor-1").unwrap();
        let mut supervisor = ScheduleSupervisor::initialize(prepared_record(), journal).unwrap();
        let runner = identity(44, 43);
        supervisor
            .start_running(
                runner.clone(),
                vec![AnchoredProcessGroupRecordV1 {
                    process_group: 43,
                    session_id: 41,
                    anchor: identity(43, 43),
                    workloads: vec![runner],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                }],
            )
            .unwrap();
        drop(supervisor);

        let (journal, latest, latest_sha256) =
            FileSupervisorJournal::open_existing(directory.path(), "supervisor-1").unwrap();
        assert_eq!(latest.generation, 2);
        assert_eq!(latest.phase, SupervisorPhaseV1::Running);
        assert_eq!(latest_sha256.len(), 64);
        let mut members = BTreeMap::new();
        members.insert(43, vec![identity(44, 43)]);
        let mut control = fake_control(false, members);
        let mut recovered =
            ScheduleSupervisor::resume_existing(latest, latest_sha256, journal).unwrap();
        assert_eq!(
            recovered.recover(&mut control).unwrap(),
            RecoveryDisposition::ResumeRunning
        );
        recovered.request_cancel(&mut control, 10).unwrap();
        drop(recovered);

        let (_journal, latest, _latest_sha256) =
            FileSupervisorJournal::open_existing(directory.path(), "supervisor-1").unwrap();
        assert_eq!(latest.generation, 3);
        assert_eq!(latest.phase, SupervisorPhaseV1::TermGrace);

        std::fs::write(
            directory
                .path()
                .join("supervisor-1.00000000000000000004.json"),
            b"truncated",
        )
        .unwrap();
        assert!(FileSupervisorJournal::open_existing(directory.path(), "supervisor-1").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_journal_rejects_a_valid_hash_chained_phase_rollback() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let journal = FileSupervisorJournal::create(directory.path(), "supervisor-1").unwrap();
        let mut supervisor = ScheduleSupervisor::initialize(prepared_record(), journal).unwrap();
        let runner = identity(44, 43);
        supervisor
            .start_running(
                runner.clone(),
                vec![AnchoredProcessGroupRecordV1 {
                    process_group: 43,
                    session_id: 41,
                    anchor: identity(43, 43),
                    workloads: vec![runner],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                }],
            )
            .unwrap();
        drop(supervisor);

        let (journal, latest, latest_sha256) =
            FileSupervisorJournal::open_existing(directory.path(), "supervisor-1").unwrap();
        drop(journal);
        assert_eq!(latest.phase, SupervisorPhaseV1::Running);
        let mut rollback = prepared_record();
        rollback.generation = 3;
        rollback.previous_record = OptionalSha256V1::Sha256 {
            value: latest_sha256,
        };
        rollback.recorded_at_ms = latest.recorded_at_ms + 1;
        validate_supervisor_record(&rollback).unwrap();
        let mut bytes = serde_json::to_vec_pretty(&rollback).unwrap();
        bytes.push(b'\n');
        let path = directory
            .path()
            .join("supervisor-1.00000000000000000003.json");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert!(FileSupervisorJournal::open_existing(directory.path(), "supervisor-1").is_err());
    }

    #[test]
    fn generation_one_must_be_prepared_even_when_the_record_is_individually_valid() {
        let mut held = prepared_record();
        held.phase = SupervisorPhaseV1::SafetyHold;
        held.later_group_signal_permitted = false;
        held.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::SafetyHold,
        };
        held.safety_hold = OptionalSafetyHoldReasonV1::Reason {
            value: SafetyHoldReasonV1::AnchorAcquisitionFailed,
        };
        validate_supervisor_record(&held).unwrap();

        assert!(ScheduleSupervisor::initialize(held, MemoryJournal::default()).is_err());
    }

    #[test]
    fn start_running_rejects_nonretained_anchor_capabilities() {
        for lifecycle in [
            AnchorLifecycleV1::ReleasedReaped,
            AnchorLifecycleV1::Ambiguous,
        ] {
            let initial = prepared_record();
            let mut running_group = initial.groups[0].clone();
            let runner = identity(44, running_group.process_group);
            running_group.workloads = vec![runner.clone()];
            running_group.anchor_lifecycle = lifecycle;
            let mut supervisor =
                ScheduleSupervisor::initialize(initial, MemoryJournal::default()).unwrap();

            assert!(supervisor
                .start_running(runner, vec![running_group])
                .is_err());
            assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Prepared);
            assert_eq!(supervisor.journal().records.len(), 1);
        }
    }

    #[test]
    fn journal_transition_releases_anchors_only_after_signaling_is_forbidden() {
        let (supervisor, _control) = running_fake_supervisor(false);
        let previous = supervisor.record().clone();
        let successor = |mut record: SupervisorRecordV1| {
            record.generation = previous.generation + 1;
            record.previous_record = OptionalSha256V1::Sha256 {
                value: record_sha256(&previous),
            };
            record.recorded_at_ms = previous.recorded_at_ms + 1;
            record
        };

        let mut released_hold = previous.clone();
        released_hold.phase = SupervisorPhaseV1::SafetyHold;
        released_hold.groups[0].anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        released_hold.later_group_signal_permitted = false;
        released_hold.outcome = OptionalSupervisorOutcomeV1::Outcome {
            value: SupervisorTerminalOutcomeV1::SafetyHold,
        };
        released_hold.safety_hold = OptionalSafetyHoldReasonV1::Reason {
            value: SafetyHoldReasonV1::AnchorNotLive,
        };
        let released_hold = successor(released_hold);
        validate_supervisor_record(&released_hold).unwrap();
        assert!(validate_supervisor_transition(&previous, &released_hold).is_err());

        let mut ambiguous_hold = released_hold.clone();
        ambiguous_hold.groups[0].anchor_lifecycle = AnchorLifecycleV1::Ambiguous;
        validate_supervisor_transition(&previous, &ambiguous_hold).unwrap();

        let mut reaping = previous.clone();
        reaping.phase = SupervisorPhaseV1::Reaping;
        reaping.groups[0].anchor_lifecycle = AnchorLifecycleV1::ReleasedReaped;
        reaping.later_group_signal_permitted = false;
        let reaping = successor(reaping);
        validate_supervisor_transition(&previous, &reaping).unwrap();
    }

    #[test]
    fn failed_anchor_acquisition_enters_hold_before_any_signal() {
        let (mut supervisor, control) = running_fake_supervisor(false);
        supervisor.anchor_acquisition_failed().unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert!(control.signals.is_empty());
    }

    #[test]
    fn anchor_observation_error_does_not_suppress_retained_capability_signals() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        control.fail_anchor_observation = true;

        supervisor.request_cancel(&mut control, 10).unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::TermGrace);
        assert_eq!(control.signals, vec![FakeSignal::Term]);

        supervisor.request_cancel(&mut control, 20).unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Reaping);
        assert_eq!(control.signals, vec![FakeSignal::Term, FakeSignal::Kill]);
    }

    #[test]
    fn signal_effect_failure_is_followed_by_a_durable_ambiguous_hold() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        control.fail_signal = true;

        assert!(supervisor.request_cancel(&mut control, 10).is_err());

        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(
            supervisor.record().safety_hold,
            OptionalSafetyHoldReasonV1::Reason {
                value: SafetyHoldReasonV1::SignalJournalAmbiguous
            }
        );
        assert!(control.signals.is_empty());
    }

    #[test]
    fn ignored_term_escalates_once_after_a_durable_kill_journal() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();
        supervisor.request_cancel(&mut control, 21).unwrap();

        assert_eq!(control.signals, vec![FakeSignal::Term, FakeSignal::Kill]);
        assert!(supervisor
            .journal()
            .records
            .iter()
            .any(|record| record.phase == SupervisorPhaseV1::KillJournaled));
        supervisor
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Complete);
        assert!(control.unrelated_alive);
        assert!(control.unrelated_container_alive);
        assert_eq!(
            supervisor.record().outcome,
            OptionalSupervisorOutcomeV1::Outcome {
                value: SupervisorTerminalOutcomeV1::KilledAfterDeadline
            }
        );
    }

    #[test]
    fn repeated_cancellation_derives_a_killed_after_cancellation_outcome() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.request_cancel(&mut control, 11).unwrap();
        supervisor
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();

        assert_eq!(control.signals, vec![FakeSignal::Term, FakeSignal::Kill]);
        assert_eq!(
            supervisor.record().kill_cause,
            OptionalSupervisorKillCauseV1::Cause {
                value: SupervisorKillCauseV1::RepeatedCancellation
            }
        );
        assert_eq!(
            supervisor.record().outcome,
            OptionalSupervisorOutcomeV1::Outcome {
                value: SupervisorTerminalOutcomeV1::KilledAfterCancellation
            }
        );
    }

    #[test]
    fn graceful_term_completes_without_kill_and_releases_the_anchor() {
        let (mut supervisor, mut control) = running_fake_supervisor(true);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();

        assert_eq!(control.signals, vec![FakeSignal::Term]);
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Complete);
        assert_eq!(
            supervisor.record().groups[0].anchor_lifecycle,
            AnchorLifecycleV1::ReleasedReaped
        );
    }

    #[test]
    fn child_join_is_byte_verified_before_anchor_release() {
        let directory = tempfile::tempdir().unwrap();
        let aggregate_path = directory.path().join("aggregate.json");
        std::fs::write(&aggregate_path, b"{}").unwrap();
        let join_path = directory.path().join("join.json");
        let join = ChildArtifactJoinV1 {
            schema_version: 1,
            record_id: "aggregate-1".into(),
            run_id: "run-1".into(),
            window_id: "window-1".into(),
            aggregate_sha256: OptionalSha256V1::Sha256 { value: digest('c') },
        };
        std::fs::write(&join_path, serde_json::to_vec(&join).unwrap()).unwrap();
        assert_eq!(
            VerifiedChildArtifact::load(&join_path, Some(&aggregate_path))
                .unwrap_err()
                .to_string(),
            "schedule supervisor: child aggregate byte hash does not match the join"
        );

        let (mut supervisor, mut control) = running_fake_supervisor(true);
        supervisor.request_cancel(&mut control, 10).unwrap();
        assert!(supervisor
            .finish_after_exit(&mut control, child_artifact_for_window("other-window"))
            .is_err());
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::TermGrace);
        assert!(control.released_groups.is_empty());
    }

    #[test]
    fn release_and_container_failures_preserve_exact_terminal_hold_state() {
        let (mut release_failure, mut release_control) = running_fake_supervisor(true);
        release_failure
            .request_cancel(&mut release_control, 10)
            .unwrap();
        release_control.fail_anchor_release = true;
        assert!(release_failure
            .finish_after_exit(&mut release_control, child_artifact())
            .is_err());
        assert_eq!(
            release_failure.record().groups[0].anchor_lifecycle,
            AnchorLifecycleV1::Ambiguous
        );
        assert_eq!(
            release_failure.record().phase,
            SupervisorPhaseV1::SafetyHold
        );

        let (mut reap_failure, mut reap_control) = running_fake_supervisor(true);
        reap_failure.request_cancel(&mut reap_control, 10).unwrap();
        reap_control.fail_container_reap = true;
        assert!(reap_failure
            .finish_after_exit(&mut reap_control, child_artifact())
            .is_err());
        assert_eq!(
            reap_failure.record().groups[0].anchor_lifecycle,
            AnchorLifecycleV1::ReleasedReaped
        );
        assert_eq!(reap_failure.record().phase, SupervisorPhaseV1::SafetyHold);

        let (mut absence_failure, mut absence_control) = running_fake_supervisor(true);
        absence_failure
            .request_cancel(&mut absence_control, 10)
            .unwrap();
        absence_control.fail_container_absence = true;
        assert!(absence_failure
            .finish_after_exit(&mut absence_control, child_artifact())
            .is_err());
        assert_eq!(
            absence_failure.record().groups[0].anchor_lifecycle,
            AnchorLifecycleV1::ReleasedReaped
        );
        assert_eq!(
            absence_failure.record().phase,
            SupervisorPhaseV1::SafetyHold
        );
    }

    #[test]
    fn unproved_exit_after_kill_remains_a_safety_hold() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        control.kill_exits = false;
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();

        supervisor
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();

        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert!(!supervisor.record().later_group_signal_permitted);
    }

    #[test]
    fn descendant_created_group_is_anchored_or_a_new_session_holds() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        let descendant = identity_with_parent(55, 44, 53);
        supervisor
            .register_descendant_group(
                &mut control,
                AnchoredProcessGroupRecordV1 {
                    process_group: 53,
                    session_id: 41,
                    anchor: identity(54, 53),
                    workloads: vec![descendant.clone()],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                },
            )
            .unwrap();
        control.non_anchor_members.insert(53, vec![descendant]);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();
        assert_eq!(
            control.signals,
            vec![
                FakeSignal::Term,
                FakeSignal::Term,
                FakeSignal::Kill,
                FakeSignal::Kill,
            ]
        );
        assert!(control.unrelated_alive);
        supervisor
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Complete);
        assert!(control.non_anchor_members.values().all(Vec::is_empty));

        let (mut escaped, mut escaped_control) = running_fake_supervisor(false);
        let mut workload = identity_with_parent(65, 44, 63);
        workload.session_id = 99;
        let mut anchor = identity(64, 63);
        anchor.session_id = 99;
        escaped
            .register_descendant_group(
                &mut escaped_control,
                AnchoredProcessGroupRecordV1 {
                    process_group: 63,
                    session_id: 99,
                    anchor,
                    workloads: vec![workload],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                },
            )
            .unwrap();
        assert_eq!(escaped.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(escaped.record().groups.len(), 2);
        assert_eq!(escaped.record().groups[1].process_group, 63);
    }

    #[test]
    fn recycled_numeric_parent_cannot_authorize_a_descendant_group() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        control.stale_process_pids.insert(44);
        let descendant = identity_with_parent(55, 44, 53);

        supervisor
            .register_descendant_group(
                &mut control,
                AnchoredProcessGroupRecordV1 {
                    process_group: 53,
                    session_id: 41,
                    anchor: identity(54, 53),
                    workloads: vec![descendant],
                    anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
                },
            )
            .unwrap();

        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(supervisor.record().groups.len(), 2);
        assert_eq!(supervisor.record().groups[1].process_group, 53);
        assert!(control.signals.is_empty());
        assert!(control.unrelated_alive);
    }

    #[test]
    fn registration_observation_failures_keep_the_acquired_group_in_the_hold() {
        let group = || AnchoredProcessGroupRecordV1 {
            process_group: 53,
            session_id: 41,
            anchor: identity(54, 53),
            workloads: vec![identity_with_parent(55, 44, 53)],
            anchor_lifecycle: AnchorLifecycleV1::RetainedLive,
        };

        let (mut dead_anchor, mut dead_anchor_control) = running_fake_supervisor(false);
        dead_anchor_control.anchor_live = false;
        dead_anchor
            .register_descendant_group(&mut dead_anchor_control, group())
            .unwrap();
        assert_eq!(dead_anchor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(dead_anchor.record().groups.len(), 2);
        assert_eq!(
            dead_anchor.record().groups[1].anchor_lifecycle,
            AnchorLifecycleV1::Ambiguous
        );
        assert!(dead_anchor_control.signals.is_empty());

        let (mut anchor_error, mut anchor_error_control) = running_fake_supervisor(false);
        anchor_error_control.fail_anchor_observation = true;
        assert!(anchor_error
            .register_descendant_group(&mut anchor_error_control, group())
            .is_err());
        assert_eq!(anchor_error.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(anchor_error.record().groups.len(), 2);
        assert_eq!(
            anchor_error.record().groups[1].anchor_lifecycle,
            AnchorLifecycleV1::Ambiguous
        );
        assert!(anchor_error_control.signals.is_empty());

        let (mut process_error, mut process_error_control) = running_fake_supervisor(false);
        process_error_control.fail_process_observation = true;
        assert!(process_error
            .register_descendant_group(&mut process_error_control, group())
            .is_err());
        assert_eq!(process_error.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(process_error.record().groups.len(), 2);
        assert_eq!(
            process_error.record().groups[1].anchor_lifecycle,
            AnchorLifecycleV1::RetainedLive
        );
        assert!(process_error_control.signals.is_empty());
    }

    #[test]
    fn journal_wedge_before_kill_sends_no_unjournaled_terminal_signal() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.journal_mut().fail_phase = Some(SupervisorPhaseV1::KillJournaled);

        assert!(supervisor.grace_expired(&mut control, 20).is_err());
        assert_eq!(control.signals, vec![FakeSignal::Term]);
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::TermGrace);
        let latest = supervisor.record().clone();
        let latest_sha256 = record_sha256(&latest);
        let mut recovered =
            ScheduleSupervisor::resume_existing(latest, latest_sha256, MemoryJournal::default())
                .unwrap();
        let before = control.signals.len();
        assert_eq!(
            recovered.recover(&mut control).unwrap(),
            RecoveryDisposition::SafetyHold
        );
        assert_eq!(control.signals.len(), before);
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(recovered.record().generation, 4);
    }

    #[test]
    fn ambiguous_recovery_holds_without_retrying_the_numeric_group() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();
        let kill_journaled = supervisor
            .journal()
            .records
            .iter()
            .find(|record| record.phase == SupervisorPhaseV1::KillJournaled)
            .unwrap()
            .clone();
        let before = control.signals.len();

        let generation = kill_journaled.generation;
        let mut recovered = ScheduleSupervisor::resume_existing(
            kill_journaled.clone(),
            record_sha256(&kill_journaled),
            MemoryJournal::default(),
        )
        .unwrap();
        let disposition = recovered.recover(&mut control).unwrap();

        assert_eq!(disposition, RecoveryDisposition::SafetyHold);
        assert_eq!(control.signals.len(), before);
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(recovered.record().generation, generation + 1);
        assert_eq!(recovered.journal().records.len(), 1);
    }

    #[test]
    fn prepared_recovery_resumes_only_when_the_anchored_group_has_no_workload() {
        let prepared = prepared_record();
        let hash = record_sha256(&prepared);
        let mut empty_control = fake_control(false, BTreeMap::from([(43, Vec::new())]));
        let mut empty = ScheduleSupervisor::resume_existing(
            prepared.clone(),
            hash.clone(),
            MemoryJournal::default(),
        )
        .unwrap();
        assert_eq!(
            empty.recover(&mut empty_control).unwrap(),
            RecoveryDisposition::ResumePreSignal
        );

        let mut members = BTreeMap::new();
        members.insert(43, vec![identity(44, 43)]);
        let mut ambiguous_control = fake_control(false, members);
        let mut ambiguous =
            ScheduleSupervisor::resume_existing(prepared, hash, MemoryJournal::default()).unwrap();
        assert_eq!(
            ambiguous.recover(&mut ambiguous_control).unwrap(),
            RecoveryDisposition::SafetyHold
        );
        assert_eq!(ambiguous.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(
            ambiguous.record().safety_hold,
            OptionalSafetyHoldReasonV1::Reason {
                value: SafetyHoldReasonV1::StartupReconciliationIncomplete
            }
        );
        assert!(ambiguous_control.signals.is_empty());
    }

    #[test]
    fn running_recovery_requires_the_exact_runner_and_persists_a_hold() {
        let (supervisor, mut control) = running_fake_supervisor(false);
        let latest = supervisor.record().clone();
        let generation = latest.generation;
        control.runner_live = false;
        let mut recovered = ScheduleSupervisor::resume_existing(
            latest.clone(),
            record_sha256(&latest),
            MemoryJournal::default(),
        )
        .unwrap();

        assert_eq!(
            recovered.recover(&mut control).unwrap(),
            RecoveryDisposition::SafetyHold
        );
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(recovered.record().generation, generation + 1);
        assert!(control.signals.is_empty());
    }

    #[test]
    fn running_recovery_observation_error_persists_a_hold() {
        let (supervisor, mut control) = running_fake_supervisor(false);
        let latest = supervisor.record().clone();
        control.fail_runner_observation = true;
        let mut recovered = ScheduleSupervisor::resume_existing(
            latest.clone(),
            record_sha256(&latest),
            MemoryJournal::default(),
        )
        .unwrap();

        assert!(recovered.recover(&mut control).is_err());
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::SafetyHold);
        assert!(control.signals.is_empty());
    }

    #[test]
    fn reaping_recovery_reconciles_to_complete_without_another_signal() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::Reaping);
        let latest = supervisor.record().clone();
        control.signals.clear();
        let mut recovered = ScheduleSupervisor::resume_existing(
            latest.clone(),
            record_sha256(&latest),
            MemoryJournal::default(),
        )
        .unwrap();

        assert_eq!(
            recovered.recover(&mut control).unwrap(),
            RecoveryDisposition::ReconcileWithoutSignal
        );
        recovered
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();

        assert!(control.signals.is_empty());
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::Complete);
    }

    #[test]
    fn reaping_recovery_accepts_an_already_absent_anchor_and_group() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        supervisor.request_cancel(&mut control, 10).unwrap();
        supervisor.grace_expired(&mut control, 20).unwrap();
        let latest = supervisor.record().clone();
        control.signals.clear();
        control.anchor_live = false;
        control.released_groups.insert(43);
        let mut recovered = ScheduleSupervisor::resume_existing(
            latest.clone(),
            record_sha256(&latest),
            MemoryJournal::default(),
        )
        .unwrap();
        assert_eq!(
            recovered.recover(&mut control).unwrap(),
            RecoveryDisposition::ReconcileWithoutSignal
        );

        recovered
            .finish_after_exit(&mut control, child_artifact())
            .unwrap();

        assert!(control.signals.is_empty());
        assert_eq!(recovered.record().phase, SupervisorPhaseV1::Complete);
    }

    #[test]
    fn recycled_numeric_group_is_never_signaled_after_anchor_loss() {
        let (mut supervisor, mut control) = running_fake_supervisor(false);
        control.recycle_supervised_group_to_unrelated();

        assert!(supervisor.request_cancel(&mut control, 10).is_err());

        assert!(control.signals.is_empty());
        assert!(control.unrelated_alive);
        assert_eq!(supervisor.record().phase, SupervisorPhaseV1::SafetyHold);
        assert_eq!(
            supervisor.record().safety_hold,
            OptionalSafetyHoldReasonV1::Reason {
                value: SafetyHoldReasonV1::SignalJournalAmbiguous
            }
        );
    }
}
