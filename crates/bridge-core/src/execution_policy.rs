//! R2f1a frozen execution-policy and checkout identity primitives.
//!
//! This module is intentionally provider-free. It resolves closed policy declarations and
//! derives durable checkout identities before a registry lookup, checkout, session mint, or
//! provider effect. Graph validation and run-spec assembly live in `bridge-workflow` so
//! `bridge-core` remains below the workflow crate in the dependency graph.

use crate::ids::AttemptId;
use crate::SessionCwd;
use ring::digest;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const EXECUTION_POLICY_SCHEMA_V1: u16 = 1;
pub const CHECKOUT_EFFECT_SCHEMA_V1: u16 = 1;
pub const PROFILE_LEGACY_BOUNDED_V1: &str = "legacy_bounded_v1";
pub const PROFILE_REVIEW_HIGH_XHIGH_V1: &str = "review_high_xhigh_v1";
pub const DEFAULT_WORK_CUTOFF_MS: u64 = 7_200_000;
pub const CLEANUP_TAIL_MS: u64 = 60_000;
pub const REPORTING_TAIL_MS: u64 = 10_000;
pub const MAX_QUALIFICATION_REASON_BYTES: usize = 512;
const MAX_WORKTREE_OWNER_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessProfileIdV1 {
    LegacyBoundedV1,
    ReviewHighXhighV1,
}

impl LivenessProfileIdV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacyBoundedV1 => PROFILE_LEGACY_BOUNDED_V1,
            Self::ReviewHighXhighV1 => PROFILE_REVIEW_HIGH_XHIGH_V1,
        }
    }

    #[must_use]
    pub const fn task_class(self) -> TaskClassV1 {
        match self {
            Self::LegacyBoundedV1 => TaskClassV1::Other,
            Self::ReviewHighXhighV1 => TaskClassV1::ReviewHighXhigh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClassV1 {
    Other,
    ReviewHighXhigh,
}

impl TaskClassV1 {
    #[must_use]
    pub const fn profile(self) -> LivenessProfileIdV1 {
        match self {
            Self::Other => LivenessProfileIdV1::LegacyBoundedV1,
            Self::ReviewHighXhigh => LivenessProfileIdV1::ReviewHighXhighV1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceCutoffV1 {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivenessProfileV1 {
    pub schema_version: u16,
    pub id: LivenessProfileIdV1,
    pub queue_wait_ms: u64,
    pub control_observable_ms: u64,
    pub no_progress_snapshot_ms: u64,
    pub silence_cutoff: SilenceCutoffV1,
    pub work_cutoff_ms: u64,
    pub cancel_observable_ms: u64,
    pub cleanup_tail_ms: u64,
    pub reporting_tail_ms: u64,
    pub terminal_bound_ms: u64,
}

#[must_use]
pub const fn liveness_profile_v1(id: LivenessProfileIdV1) -> LivenessProfileV1 {
    LivenessProfileV1 {
        schema_version: EXECUTION_POLICY_SCHEMA_V1,
        id,
        queue_wait_ms: 1_800_000,
        control_observable_ms: 31_000,
        no_progress_snapshot_ms: 1_800_000,
        silence_cutoff: SilenceCutoffV1::None,
        work_cutoff_ms: DEFAULT_WORK_CUTOFF_MS,
        cancel_observable_ms: 6_000,
        cleanup_tail_ms: CLEANUP_TAIL_MS,
        reporting_tail_ms: REPORTING_TAIL_MS,
        terminal_bound_ms: 7_270_000,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedReasonV1(String);

impl BoundedReasonV1 {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExecutionPolicyError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_QUALIFICATION_REASON_BYTES
            || trimmed.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(ExecutionPolicyError::InvalidMaxReason);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BoundedReasonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaxQualificationV1 {
    pub work_cutoff_ms: u64,
    pub reason: BoundedReasonV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FanOutPolicyV1 {
    BoundedIndependent,
    FailFast,
    FixedGrace { grace_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthesisModeV1 {
    Strict,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSelectionSourceV1 {
    Invocation,
    WorkflowProfile,
    WorkflowTaskClass,
    CompatibilityOmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineActivationV1 {
    ManualOnlyR2f1a,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyActivationV1 {
    Production,
    ManualTest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenWorkflowControlsV1 {
    pub schema_version: u16,
    pub task_class: TaskClassV1,
    pub profile: LivenessProfileV1,
    pub profile_source: ProfileSelectionSourceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_qualification: Option<MaxQualificationV1>,
    pub fan_out: FanOutPolicyV1,
    pub synthesis: SynthesisModeV1,
    pub deadline_activation: DeadlineActivationV1,
}

impl FrozenWorkflowControlsV1 {
    #[must_use]
    pub fn effective_work_cutoff_ms(&self) -> u64 {
        self.max_qualification
            .as_ref()
            .map_or(self.profile.work_cutoff_ms, |qualification| {
                qualification.work_cutoff_ms
            })
    }

    pub fn effective_terminal_bound_ms(&self) -> Result<u64, ExecutionPolicyError> {
        self.effective_work_cutoff_ms()
            .checked_add(self.profile.cleanup_tail_ms)
            .and_then(|value| value.checked_add(self.profile.reporting_tail_ms))
            .ok_or(ExecutionPolicyError::ArithmeticOverflow)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowControlDefaultsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_class: Option<TaskClassV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_profile: Option<LivenessProfileIdV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_out: Option<FanOutPolicyV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesis: Option<SynthesisModeV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_work_cutoff_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPolicyInvocationV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_profile: Option<LivenessProfileIdV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_work_cutoff_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionPolicyError {
    #[error("workflow liveness profile and task class disagree")]
    ProfileTaskClassMismatch,
    #[error("max cutoff and reason must be supplied together")]
    PartialMaxQualification,
    #[error("max qualification is required for max effort")]
    MissingMaxQualification,
    #[error("max qualification is unused without max effort")]
    UnusedMaxQualification,
    #[error("max work cutoff must be greater than the named profile cutoff")]
    InvalidMaxCutoff,
    #[error("max qualification reason is invalid")]
    InvalidMaxReason,
    #[error("fixed grace is inactive in production during R2f1a")]
    FixedGraceInactive,
    #[error("fixed grace must fit inside the effective work cutoff")]
    InvalidFixedGrace,
    #[error("execution-policy arithmetic overflow")]
    ArithmeticOverflow,
    #[error("worktree owner must be a bounded portable identifier")]
    InvalidWorktreeOwner,
    #[error("derived worktree target escaped its configured root")]
    WorktreeTargetOutsideRoot,
    #[error("invalid SHA-256 hex value")]
    InvalidSha256,
}

fn max_qualification(
    cutoff: Option<u64>,
    reason: Option<&str>,
) -> Result<Option<MaxQualificationV1>, ExecutionPolicyError> {
    match (cutoff, reason) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(ExecutionPolicyError::PartialMaxQualification),
        (Some(work_cutoff_ms), Some(reason)) => {
            if work_cutoff_ms <= DEFAULT_WORK_CUTOFF_MS {
                return Err(ExecutionPolicyError::InvalidMaxCutoff);
            }
            Ok(Some(MaxQualificationV1 {
                work_cutoff_ms,
                reason: BoundedReasonV1::parse(reason)?,
            }))
        }
    }
}

pub fn resolve_execution_policy_v1(
    workflow: &WorkflowControlDefaultsV1,
    invocation: &ExecutionPolicyInvocationV1,
    any_max_effort: bool,
    activation: PolicyActivationV1,
) -> Result<FrozenWorkflowControlsV1, ExecutionPolicyError> {
    if workflow
        .task_class
        .zip(workflow.liveness_profile)
        .is_some_and(|(task_class, profile)| task_class.profile() != profile)
    {
        return Err(ExecutionPolicyError::ProfileTaskClassMismatch);
    }

    let workflow_max =
        max_qualification(workflow.max_work_cutoff_ms, workflow.max_reason.as_deref())?;
    let invocation_max = max_qualification(
        invocation.max_work_cutoff_ms,
        invocation.max_reason.as_deref(),
    )?;
    let qualification = invocation_max.or(workflow_max);
    match (any_max_effort, qualification.is_some()) {
        (true, false) => return Err(ExecutionPolicyError::MissingMaxQualification),
        (false, true) => return Err(ExecutionPolicyError::UnusedMaxQualification),
        _ => {}
    }

    let (profile_id, task_class, profile_source) =
        if let Some(profile) = invocation.liveness_profile {
            (
                profile,
                profile.task_class(),
                ProfileSelectionSourceV1::Invocation,
            )
        } else if let Some(profile) = workflow.liveness_profile {
            (
                profile,
                profile.task_class(),
                ProfileSelectionSourceV1::WorkflowProfile,
            )
        } else if let Some(task_class) = workflow.task_class {
            (
                task_class.profile(),
                task_class,
                ProfileSelectionSourceV1::WorkflowTaskClass,
            )
        } else {
            (
                LivenessProfileIdV1::LegacyBoundedV1,
                TaskClassV1::Other,
                ProfileSelectionSourceV1::CompatibilityOmission,
            )
        };

    let profile = liveness_profile_v1(profile_id);
    let fan_out = workflow
        .fan_out
        .clone()
        .unwrap_or(FanOutPolicyV1::BoundedIndependent);
    let controls = FrozenWorkflowControlsV1 {
        schema_version: EXECUTION_POLICY_SCHEMA_V1,
        task_class,
        profile,
        profile_source,
        max_qualification: qualification,
        fan_out,
        synthesis: workflow.synthesis.unwrap_or(SynthesisModeV1::Degraded),
        deadline_activation: DeadlineActivationV1::ManualOnlyR2f1a,
    };

    if let FanOutPolicyV1::FixedGrace { grace_ms } = controls.fan_out {
        if matches!(activation, PolicyActivationV1::Production) {
            return Err(ExecutionPolicyError::FixedGraceInactive);
        }
        if grace_ms == 0 || grace_ms > controls.effective_work_cutoff_ms() {
            return Err(ExecutionPolicyError::InvalidFixedGrace);
        }
    }
    let _ = controls.effective_terminal_bound_ms()?;
    Ok(controls)
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Sha256HexV1(String);

impl std::fmt::Debug for Sha256HexV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("Sha256HexV1").field(&self.0).finish()
    }
}

impl Sha256HexV1 {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExecutionPolicyError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ExecutionPolicyError::InvalidSha256);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        let digest = digest::digest(&digest::SHA256, bytes);
        let mut value = String::with_capacity(64);
        for byte in digest.as_ref() {
            use std::fmt::Write as _;
            let _ = write!(&mut value, "{byte:02x}");
        }
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256HexV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyNodeRefV1 {
    pub sorted_ordinal: u32,
    pub id_sha256: Sha256HexV1,
}

impl PolicyNodeRefV1 {
    #[must_use]
    pub fn from_node_id(sorted_ordinal: u32, node_id: &str) -> Self {
        Self {
            sorted_ordinal,
            id_sha256: Sha256HexV1::digest(node_id.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrozenProviderLogicalSessionV1 {
    Preflight { candidate_ordinal: u16 },
    Execute { candidate_ordinal: u16 },
}

impl FrozenProviderLogicalSessionV1 {
    fn tag(self) -> (&'static str, u16) {
        match self {
            Self::Preflight { candidate_ordinal } => ("preflight", candidate_ordinal),
            Self::Execute { candidate_ordinal } => ("execute", candidate_ordinal),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrozenCheckoutEffectV1 {
    Direct {
        source_cwd: SessionCwd,
        effective_cwd: SessionCwd,
    },
    Worktree {
        source_cwd: SessionCwd,
        canonical_source_cwd: SessionCwd,
        canonical_worktree_root: SessionCwd,
        worktree_owner: String,
        target_cwd: SessionCwd,
        checkout_digest: Sha256HexV1,
    },
}

impl FrozenCheckoutEffectV1 {
    #[must_use]
    pub fn effective_cwd(&self) -> &SessionCwd {
        match self {
            Self::Direct { effective_cwd, .. } => effective_cwd,
            Self::Worktree { target_cwd, .. } => target_cwd,
        }
    }

    #[must_use]
    pub fn source_cwd(&self) -> &SessionCwd {
        match self {
            Self::Direct { source_cwd, .. } | Self::Worktree { source_cwd, .. } => source_cwd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCheckoutInputV1 {
    pub attempt_id: AttemptId,
    pub node: PolicyNodeRefV1,
    pub logical_session: FrozenProviderLogicalSessionV1,
    pub source_cwd: SessionCwd,
    pub canonical_source_cwd: SessionCwd,
    pub canonical_worktree_root: SessionCwd,
    pub worktree_owner: String,
}

fn push_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn checkout_preimage(input: &WorktreeCheckoutInputV1) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_bytes(&mut bytes, b"a2a-bridge/frozen-worktree-target/v1");
    bytes.extend_from_slice(&CHECKOUT_EFFECT_SCHEMA_V1.to_be_bytes());
    push_bytes(&mut bytes, input.attempt_id.as_str().as_bytes());
    bytes.extend_from_slice(&input.node.sorted_ordinal.to_be_bytes());
    push_bytes(&mut bytes, input.node.id_sha256.as_str().as_bytes());
    let (kind, candidate_ordinal) = input.logical_session.tag();
    push_bytes(&mut bytes, kind.as_bytes());
    bytes.extend_from_slice(&candidate_ordinal.to_be_bytes());
    push_bytes(&mut bytes, input.source_cwd.as_str().as_bytes());
    push_bytes(&mut bytes, input.canonical_source_cwd.as_str().as_bytes());
    push_bytes(
        &mut bytes,
        input.canonical_worktree_root.as_str().as_bytes(),
    );
    push_bytes(&mut bytes, input.worktree_owner.as_bytes());
    bytes
}

pub fn freeze_worktree_checkout_v1(
    input: &WorktreeCheckoutInputV1,
) -> Result<FrozenCheckoutEffectV1, ExecutionPolicyError> {
    if input.worktree_owner.is_empty()
        || input.worktree_owner.len() > MAX_WORKTREE_OWNER_BYTES
        || !input.worktree_owner.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(ExecutionPolicyError::InvalidWorktreeOwner);
    }

    let preimage = checkout_preimage(input);
    let target_key = Sha256HexV1::digest(&preimage);
    let name = format!(
        "{}-r2f1a-{}",
        input.worktree_owner,
        &target_key.as_str()[..32]
    );
    let target_path = Path::new(input.canonical_worktree_root.as_str()).join(name);
    let target_cwd = SessionCwd::parse(&target_path.to_string_lossy())
        .map_err(|_| ExecutionPolicyError::WorktreeTargetOutsideRoot)?;
    if !target_cwd.is_under(&input.canonical_worktree_root) {
        return Err(ExecutionPolicyError::WorktreeTargetOutsideRoot);
    }

    let mut committed = preimage;
    push_bytes(&mut committed, target_cwd.as_str().as_bytes());
    let checkout_digest = Sha256HexV1::digest(&committed);
    Ok(FrozenCheckoutEffectV1::Worktree {
        source_cwd: input.source_cwd.clone(),
        canonical_source_cwd: input.canonical_source_cwd.clone(),
        canonical_worktree_root: input.canonical_worktree_root.clone(),
        worktree_owner: input.worktree_owner.clone(),
        target_cwd,
        checkout_digest,
    })
}

#[must_use]
pub fn freeze_direct_checkout_v1(source_cwd: SessionCwd) -> FrozenCheckoutEffectV1 {
    FrozenCheckoutEffectV1::Direct {
        effective_cwd: source_cwd.clone(),
        source_cwd,
    }
}
