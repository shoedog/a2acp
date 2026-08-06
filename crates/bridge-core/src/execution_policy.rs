//! R2f1a frozen execution-policy and checkout identity primitives.
//!
//! This module is intentionally provider-free. It resolves closed policy declarations and
//! derives durable checkout identities before a registry lookup, checkout, session mint, or
//! provider effect. Graph validation and run-spec assembly live in `bridge-workflow` so
//! `bridge-core` remains below the workflow crate in the dependency graph.

use crate::domain::{
    effective_config, AgentEntry, AgentKind, AgentOverride, EgressPolicy, MountAccess,
    SandboxConfig,
};
use crate::ids::{AgentId, AttemptId};
use crate::mcp::{render_codex_mcp_args, render_kiro_agent_config, McpDelivery, McpServerSpec};
use crate::SessionCwd;
use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest, hmac};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const EXECUTION_POLICY_SCHEMA_V1: u16 = 1;
pub const CHECKOUT_EFFECT_SCHEMA_V1: u16 = 1;
pub const PROFILE_LEGACY_BOUNDED_V1: &str = "legacy_bounded_v1";
pub const PROFILE_REVIEW_HIGH_XHIGH_V1: &str = "review_high_xhigh_v1";
pub const DEFAULT_WORK_CUTOFF_MS: u64 = 7_200_000;
pub const CLEANUP_TAIL_MS: u64 = 60_000;
pub const REPORTING_TAIL_MS: u64 = 10_000;
pub const MAX_QUALIFICATION_REASON_BYTES: usize = 512;
pub const MAX_STATIC_CODE_BYTES: usize = 64;
pub const MAX_DEEPEST_CAUSE_BYTES: usize = 512;
pub const MAX_CONTROL_EVENT_ID_BYTES: usize = 128;
pub const MAX_CLOSED_TOKEN_BYTES: usize = 32;
pub const NODE_TERMINAL_SKELETON_CEILING_BYTES: usize = 352;
pub const POLICY_TRIGGER_SKELETON_CEILING_BYTES: usize = 160;
pub const MAX_NODE_TERMINAL_JSON_BYTES: usize = 2_048;
pub const MAX_POLICY_TRIGGER_JSON_BYTES: usize = 1_024;
pub const MAX_CONTROLS_JSON_BYTES: usize = 4_096;
pub const NODE_PRIMARY_RECORD_SCHEMA_V3: u16 = 3;
pub const NODE_CLEANUP_RECORD_SCHEMA_V2: u16 = 2;
pub const R2F1B_CONTRACT_SCHEMA_V1: u16 = 1;
pub const R2F1B_RESOURCE_CONTRACT_VERSION_V1: u16 = 1;
pub const NODE_PRIMARY_RECORD_SKELETON_CEILING_BYTES: usize = 256;
pub const NODE_CLEANUP_RECORD_SKELETON_CEILING_BYTES: usize = 384;
pub const MAX_NODE_PRIMARY_RECORD_JSON_BYTES: usize = 1_536;
pub const MAX_NODE_CLEANUP_RECORD_JSON_BYTES: usize = 2_048;
const DERIVED_NODE_TERMINAL_WORST_CASE_BYTES: usize = 1_978;
const DERIVED_POLICY_TRIGGER_WORST_CASE_BYTES: usize = 550;
const DERIVED_NODE_PRIMARY_RECORD_WORST_CASE_BYTES: usize = 1_396;
const DERIVED_NODE_CLEANUP_RECORD_WORST_CASE_BYTES: usize = 1_936;
const _: () = assert!(DERIVED_NODE_TERMINAL_WORST_CASE_BYTES <= MAX_NODE_TERMINAL_JSON_BYTES);
const _: () = assert!(DERIVED_POLICY_TRIGGER_WORST_CASE_BYTES <= MAX_POLICY_TRIGGER_JSON_BYTES);
const _: () =
    assert!(DERIVED_NODE_PRIMARY_RECORD_WORST_CASE_BYTES <= MAX_NODE_PRIMARY_RECORD_JSON_BYTES);
const _: () =
    assert!(DERIVED_NODE_CLEANUP_RECORD_WORST_CASE_BYTES <= MAX_NODE_CLEANUP_RECORD_JSON_BYTES);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineActivationV2 {
    ManualOnlyR2f1a,
    AutomaticR2f1b,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct WorktreeCustodyIdV1(String);

impl WorktreeCustodyIdV1 {
    pub const PREFIX: &'static str = "custody-";
    pub const ENCODED_LEN: usize = Self::PREFIX.len() + 64;

    pub fn mint() -> Result<Self, crate::error::BridgeError> {
        let mut bytes = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| crate::error::BridgeError::IdentityUnavailable)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(crate::error::BridgeError::IdentityUnavailable);
        }
        let mut value = String::with_capacity(Self::ENCODED_LEN);
        value.push_str(Self::PREFIX);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(&mut value, "{byte:02x}");
        }
        Ok(Self(value))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, ExecutionPolicyError> {
        let value = value.into();
        let suffix = value
            .strip_prefix(Self::PREFIX)
            .ok_or(ExecutionPolicyError::InvalidStructuredEvidence)?;
        if value.len() != Self::ENCODED_LEN
            || suffix.bytes().all(|byte| byte == b'0')
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for WorktreeCustodyIdV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeObjectIdentityV1 {
    pub canonical_path: String,
    pub directory_identity: crate::fs_custody::DirectoryIdentityV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenWorktreeCustodyPlanV1 {
    pub custody_id: WorktreeCustodyIdV1,
    pub checkout_fingerprint: Sha256HexV1,
    pub target_cwd: SessionCwd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenR2f1bContractV1 {
    pub schema_version: u16,
    pub activation: DeadlineActivationV2,
    pub custody_plans: Vec<FrozenWorktreeCustodyPlanV1>,
    pub resource_contract_version: u16,
    pub contract_fingerprint: Sha256HexV1,
}

impl FrozenR2f1bContractV1 {
    pub fn validate(&self) -> Result<(), ExecutionPolicyError> {
        if self.schema_version != R2F1B_CONTRACT_SCHEMA_V1
            || self.resource_contract_version != R2F1B_RESOURCE_CONTRACT_VERSION_V1
        {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        let mut plans = self.custody_plans.clone();
        plans.sort_by(|left, right| {
            left.checkout_fingerprint
                .cmp(&right.checkout_fingerprint)
                .then_with(|| left.custody_id.cmp(&right.custody_id))
        });
        if plans != self.custody_plans
            || plans.windows(2).any(|pair| {
                pair[0].checkout_fingerprint == pair[1].checkout_fingerprint
                    || pair[0].custody_id == pair[1].custody_id
            })
        {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        let mut clone = self.clone();
        clone.contract_fingerprint = Sha256HexV1::digest(b"r2f1b-contract-fingerprint-placeholder");
        let encoded = canonical_json(&clone)?;
        let expected = Sha256HexV1::digest(&encoded);
        if self.contract_fingerprint != expected {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(())
    }

    pub fn with_computed_fingerprint(
        activation: DeadlineActivationV2,
        mut custody_plans: Vec<FrozenWorktreeCustodyPlanV1>,
    ) -> Result<Self, ExecutionPolicyError> {
        custody_plans.sort_by(|left, right| {
            left.checkout_fingerprint
                .cmp(&right.checkout_fingerprint)
                .then_with(|| left.custody_id.cmp(&right.custody_id))
        });
        let mut value = Self {
            schema_version: R2F1B_CONTRACT_SCHEMA_V1,
            activation,
            custody_plans,
            resource_contract_version: R2F1B_RESOURCE_CONTRACT_VERSION_V1,
            contract_fingerprint: Sha256HexV1::digest(b"r2f1b-contract-fingerprint-placeholder"),
        };
        value.contract_fingerprint = Sha256HexV1::digest(&canonical_json(&value)?);
        value.validate()?;
        Ok(value)
    }
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

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ExecutionPolicyError> {
        let encoded = serde_json::to_vec(self)
            .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
        if encoded.len() > MAX_CONTROLS_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        Ok(encoded)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ControlEventIdV1(String);

impl std::fmt::Debug for ControlEventIdV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ControlEventIdV1")
            .field(&self.0)
            .finish()
    }
}

impl ControlEventIdV1 {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExecutionPolicyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CONTROL_EVENT_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(ExecutionPolicyError::InvalidControlEventId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn for_attempt(attempt_id: &AttemptId, ordinal: u32) -> Self {
        Self(format!("{}:policy:{ordinal}", attempt_id.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ControlEventIdV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePrimaryDispositionV1 {
    Completed,
    Failed,
    TimedOut,
    CanceledWorkflow,
    CanceledPolicy,
    CanceledNode,
    SkippedDependency,
    NotStartedPolicy,
    InterruptedLegacy,
    Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeCleanupDispositionV1 {
    Complete,
    Failed,
    NotNeeded,
    UnknownLegacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeCleanupV1 {
    pub disposition: NodeCleanupDispositionV1,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySetRefV1 {
    pub count: u32,
    pub sorted_node_refs_sha256: Sha256HexV1,
}

pub type NodeFailureClassV1 = crate::diagnostics::DiagnosticFailureClass;
pub type StaticBoundedCodeV1 = crate::diagnostics::DiagnosticCode;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeCauseV1 {
    pub failure_class: NodeFailureClassV1,
    pub code: StaticBoundedCodeV1,
    pub deepest_cause: Option<String>,
    pub cause_truncated: bool,
    pub evidence_overflow: bool,
    pub dependency_set: Option<DependencySetRefV1>,
}

impl NodeCauseV1 {
    #[must_use]
    pub fn skipped_dependency(dependency_set: DependencySetRefV1) -> Self {
        Self {
            failure_class: crate::diagnostics::DiagnosticFailureClass::Unknown,
            code: crate::diagnostics::DiagnosticCode::build(
                "workflow.skipped_dependency",
                &crate::diagnostics::DiagnosticRedactor::default(),
            )
            .expect("bridge-owned node terminal codes are static and bounded"),
            deepest_cause: None,
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: Some(dependency_set),
        }
    }

    /// Convert one bridge-owned failure into the bounded evidence carried by a
    /// V2 node terminal. Structured agent diagnostics retain their exact closed
    /// class/code and deepest sanitized cause. Every legacy bridge error maps
    /// exhaustively to a static code and a client-safe cause; neither `Debug`
    /// text nor provider/process detail enters durable evidence.
    #[must_use]
    pub fn from_bridge_error(error: &crate::error::BridgeError) -> Self {
        use crate::diagnostics::{DiagnosticCode, DiagnosticFailureClass as Class};
        use crate::error::BridgeError;

        if let BridgeError::AgentFailure { diagnostic } = error {
            return Self {
                failure_class: diagnostic.class(),
                code: diagnostic.code().clone(),
                deepest_cause: diagnostic
                    .causes()
                    .last()
                    .cloned()
                    .or_else(|| Some(diagnostic.summary().to_owned())),
                cause_truncated: false,
                evidence_overflow: false,
                dependency_set: None,
            };
        }

        let (failure_class, code) = match error {
            BridgeError::A2aVersionMismatch => (Class::Protocol, "bridge.a2a_version_mismatch"),
            BridgeError::InvalidRequest { .. } => (Class::Config, "bridge.invalid_request"),
            BridgeError::IdentityUnavailable => (Class::Persistence, "bridge.identity_unavailable"),
            BridgeError::DurableEvidenceUnavailable { .. } => {
                (Class::Persistence, "bridge.durable_evidence_unavailable")
            }
            BridgeError::TaskNotFound => (Class::Config, "bridge.task_not_found"),
            BridgeError::SessionNotFound => (Class::Unknown, "bridge.session_not_found"),
            BridgeError::ConfigMismatch { .. } => (Class::Config, "bridge.config_mismatch"),
            BridgeError::ConfigReseedRequired { .. } => {
                (Class::Config, "bridge.config_reseed_required")
            }
            BridgeError::BoundSessionUnsupported => {
                (Class::Config, "bridge.bound_session_unsupported")
            }
            BridgeError::BindUnsupported => (Class::Config, "bridge.bind_unsupported"),
            BridgeError::SessionExpired => (Class::Unknown, "bridge.session_expired"),
            BridgeError::HandleBusy => (Class::Unknown, "bridge.handle_busy"),
            BridgeError::AuthRequired { .. } => (Class::Authentication, "bridge.auth_required"),
            BridgeError::PermissionRequired { .. } => (Class::Config, "bridge.permission_required"),
            BridgeError::PermissionDenied => (Class::Config, "bridge.permission_denied"),
            BridgeError::AgentNotAuthenticated => {
                (Class::Authentication, "bridge.agent_not_authenticated")
            }
            BridgeError::ModelNotAvailable => (Class::Model, "bridge.model_not_available"),
            BridgeError::CancelTimeout => (Class::Timeout, "bridge.cancel_timeout"),
            BridgeError::AgentTimedOut => (Class::Timeout, "bridge.agent_timed_out"),
            BridgeError::FrameError => (Class::Transport, "bridge.frame_error"),
            BridgeError::MissingTerminal => (Class::Protocol, "bridge.missing_terminal"),
            BridgeError::MessageTooLarge => (Class::Protocol, "bridge.message_too_large"),
            BridgeError::EmptyFinal => (Class::Protocol, "bridge.empty_final"),
            BridgeError::AgentCrashed { .. } => (Class::AgentProcess, "bridge.agent_crashed"),
            BridgeError::AgentFailure { .. } => unreachable!("handled above"),
            BridgeError::AgentOverloaded => (Class::Overloaded, "bridge.agent_overloaded"),
            BridgeError::UpstreamA2aError => (Class::Transport, "bridge.upstream_a2a_error"),
            BridgeError::StoreFailure => (Class::Persistence, "bridge.store_failure"),
            BridgeError::HarvestAuditPersistFailed { .. } => {
                (Class::Persistence, "bridge.harvest_audit_persist_failed")
            }
            BridgeError::InvalidStateTransition => {
                (Class::Protocol, "bridge.invalid_state_transition")
            }
            BridgeError::UnknownAgent { .. } => (Class::Config, "bridge.unknown_agent"),
            BridgeError::ConfigInvalid { .. } => (Class::Config, "bridge.config_invalid"),
            BridgeError::TaskSpecInvalid { .. } => (Class::Config, "bridge.task_spec_invalid"),
        };
        let deepest_cause = error.client_message();
        Self {
            failure_class,
            code: DiagnosticCode::build(code, &crate::diagnostics::DiagnosticRedactor::default())
                .expect("bridge-owned node terminal codes are static and bounded"),
            deepest_cause: (!deepest_cause.is_empty()).then_some(deepest_cause),
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTerminalV1 {
    pub schema_version: u16,
    pub primary: NodePrimaryDispositionV1,
    pub cleanup: NodeCleanupV1,
    pub cause: Option<NodeCauseV1>,
    pub prompt_may_have_been_accepted: bool,
    pub degraded_ancestry: bool,
    pub policy_trigger_id: Option<ControlEventIdV1>,
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ExecutionPolicyError> {
    serde_json::to_vec(value).map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)
}

fn remove_first_scalar(value: &mut String) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    value.drain(..first.len_utf8());
    true
}

fn shorten_terminal_cause(value: &mut NodeTerminalV1) -> bool {
    let Some(cause) = value.cause.as_mut() else {
        return false;
    };
    let Some(deepest) = cause.deepest_cause.as_mut() else {
        return false;
    };
    if remove_first_scalar(deepest) {
        true
    } else {
        cause.deepest_cause = None;
        true
    }
}

fn normalize_bounded_cause(cause: &mut BoundedCauseV1) {
    let original = cause.deepest_cause.clone();
    cause.deepest_cause = original.as_deref().map(|value| {
        crate::diagnostics::DiagnosticRedactor::default()
            .sanitize_stderr_line(value, MAX_DEEPEST_CAUSE_BYTES)
    });
    if cause.deepest_cause != original {
        cause.cause_truncated = true;
    }
}

fn shorten_bounded_cause(cause: &mut BoundedCauseV1) -> bool {
    let Some(deepest) = cause.deepest_cause.as_mut() else {
        return false;
    };
    if remove_first_scalar(deepest) {
        true
    } else {
        cause.deepest_cause = None;
        true
    }
}

impl NodeTerminalV1 {
    fn normalize(mut self) -> Result<Self, ExecutionPolicyError> {
        if self.schema_version != EXECUTION_POLICY_SCHEMA_V1 {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        if let Some(cause) = self.cause.as_mut() {
            normalize_bounded_cause(cause);
        }
        Ok(self)
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ExecutionPolicyError> {
        let mut value = self.clone().normalize()?;
        let mut encoded = canonical_json(&value)?;
        if encoded.len() <= MAX_NODE_TERMINAL_JSON_BYTES {
            return Ok(encoded);
        }

        if let Some(cause) = value.cause.as_mut() {
            cause.cause_truncated = true;
        }
        while encoded.len() > MAX_NODE_TERMINAL_JSON_BYTES && shorten_terminal_cause(&mut value) {
            encoded = canonical_json(&value)?;
        }
        if encoded.len() <= MAX_NODE_TERMINAL_JSON_BYTES {
            return Ok(encoded);
        }

        if let Some(cause) = value.cause.as_mut() {
            cause.evidence_overflow = true;
            cause.dependency_set = None;
        }
        encoded = canonical_json(&value)?;
        while encoded.len() > MAX_NODE_TERMINAL_JSON_BYTES && shorten_terminal_cause(&mut value) {
            encoded = canonical_json(&value)?;
        }
        if encoded.len() > MAX_NODE_TERMINAL_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        Ok(encoded)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ExecutionPolicyError> {
        if bytes.len() > MAX_NODE_TERMINAL_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
        if value.encode_canonical()? != bytes {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(value)
    }
}

pub type BoundedCauseV1 = NodeCauseV1;
pub type PolicyTriggerIdV1 = ControlEventIdV1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodePrimaryRecordV3 {
    pub schema_version: u16,
    pub primary: NodePrimaryDispositionV1,
    pub cause: Option<BoundedCauseV1>,
    pub prompt_may_have_been_accepted: bool,
    pub degraded_ancestry: bool,
    pub policy_trigger_id: Option<PolicyTriggerIdV1>,
}

impl NodePrimaryRecordV3 {
    fn normalize(mut self) -> Result<Self, ExecutionPolicyError> {
        if self.schema_version != NODE_PRIMARY_RECORD_SCHEMA_V3 {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        if let Some(cause) = self.cause.as_mut() {
            normalize_bounded_cause(cause);
        }
        Ok(self)
    }

    pub fn placeholder() -> Self {
        Self {
            schema_version: NODE_PRIMARY_RECORD_SCHEMA_V3,
            primary: NodePrimaryDispositionV1::NotStartedPolicy,
            cause: None,
            prompt_may_have_been_accepted: false,
            degraded_ancestry: false,
            policy_trigger_id: None,
        }
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ExecutionPolicyError> {
        let mut value = self.clone().normalize()?;
        let mut encoded = canonical_json(&value)?;
        if encoded.len() <= MAX_NODE_PRIMARY_RECORD_JSON_BYTES {
            return Ok(encoded);
        }
        if let Some(cause) = value.cause.as_mut() {
            cause.cause_truncated = true;
        }
        let mut terminal_like = NodeTerminalV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            primary: value.primary,
            cleanup: NodeCleanupV1 {
                disposition: NodeCleanupDispositionV1::NotNeeded,
                duration_ms: 0,
            },
            cause: value.cause.clone(),
            prompt_may_have_been_accepted: value.prompt_may_have_been_accepted,
            degraded_ancestry: value.degraded_ancestry,
            policy_trigger_id: value.policy_trigger_id.clone(),
        };
        while encoded.len() > MAX_NODE_PRIMARY_RECORD_JSON_BYTES
            && shorten_terminal_cause(&mut terminal_like)
        {
            value.cause = terminal_like.cause.clone();
            encoded = canonical_json(&value)?;
        }
        if encoded.len() > MAX_NODE_PRIMARY_RECORD_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        Ok(encoded)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ExecutionPolicyError> {
        if bytes.len() > MAX_NODE_PRIMARY_RECORD_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
        if value.encode_canonical()? != bytes {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreePreservationDispositionV1 {
    Pending,
    Preserved,
    Removed,
    NotNeeded,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreePreservationResultV1 {
    pub disposition: WorktreePreservationDispositionV1,
    pub custody_id: Option<WorktreeCustodyIdV1>,
    pub claim_digest: Option<Sha256HexV1>,
}

impl WorktreePreservationResultV1 {
    pub fn pending() -> Self {
        Self {
            disposition: WorktreePreservationDispositionV1::Pending,
            custody_id: None,
            claim_digest: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollateralDispositionV1 {
    None,
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollateralResultV1 {
    pub disposition: CollateralDispositionV1,
    pub affected_owner_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeCleanupV2 {
    Pending {
        resource_flight_id: crate::resource_flight::ResourceFlightIdV1,
    },
    Complete {
        duration_ms: u64,
    },
    Partial {
        duration_ms: u64,
        recovery_owner: crate::resource_flight::RecoveryOwnerV1,
    },
    Failed {
        duration_ms: u64,
        cause: BoundedCauseV1,
    },
    NotNeeded,
    Unknown {
        duration_ms: u64,
        recovery_owner: Option<crate::resource_flight::RecoveryOwnerV1>,
    },
}

impl NodeCleanupV2 {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeCleanupRecordV2 {
    pub schema_version: u16,
    pub cleanup: NodeCleanupV2,
    pub preservation: WorktreePreservationResultV1,
    pub collateral: Option<CollateralResultV1>,
}

impl NodeCleanupRecordV2 {
    pub fn pending(resource_flight_id: crate::resource_flight::ResourceFlightIdV1) -> Self {
        Self {
            schema_version: NODE_CLEANUP_RECORD_SCHEMA_V2,
            cleanup: NodeCleanupV2::Pending { resource_flight_id },
            preservation: WorktreePreservationResultV1::pending(),
            collateral: None,
        }
    }

    /// Validate the closed cleanup/preservation/collateral product table.
    ///
    /// Pending is the pre-effect projection: it owns the admitted flight, keeps
    /// preservation Pending, and cannot describe collateral. NotNeeded is the
    /// no-resource projection. Every other cleanup state is final: preservation
    /// is settled (or explicitly Unknown), and collateral is either absent or a
    /// non-empty disposition. This mirrors the preservation-first flow in the
    /// focused R2f1b boundary: collateral is recorded only after flight
    /// admission closes, while a Pending row exists before any effect.
    ///
    /// Accepted product table:
    ///
    /// Pending + Pending + None; NotNeeded + (Removed|NotNeeded|Unknown)
    /// + None; or any other final cleanup with non-Pending preservation,
    ///   where None collateral has zero owners and every other disposition has
    ///   at least one affected owner.
    pub fn validate_coherence(&self) -> Result<(), ExecutionPolicyError> {
        let final_cleanup = !self.cleanup.is_pending();
        match (
            &self.cleanup,
            &self.preservation.disposition,
            &self.collateral,
        ) {
            (NodeCleanupV2::Pending { .. }, WorktreePreservationDispositionV1::Pending, None) => {}
            (NodeCleanupV2::NotNeeded, disposition, None)
                if !matches!(
                    disposition,
                    WorktreePreservationDispositionV1::Preserved
                        | WorktreePreservationDispositionV1::Pending
                ) => {}
            (_, WorktreePreservationDispositionV1::Pending, _) if final_cleanup => {
                return Err(ExecutionPolicyError::InvalidStructuredEvidence);
            }
            (_, _, Some(collateral))
                if matches!(collateral.disposition, CollateralDispositionV1::None)
                    && collateral.affected_owner_count != 0 =>
            {
                return Err(ExecutionPolicyError::InvalidStructuredEvidence);
            }
            (_, _, Some(collateral))
                if !matches!(collateral.disposition, CollateralDispositionV1::None)
                    && collateral.affected_owner_count == 0 =>
            {
                return Err(ExecutionPolicyError::InvalidStructuredEvidence);
            }
            (NodeCleanupV2::Pending { .. }, _, _) | (NodeCleanupV2::NotNeeded, _, _) => {
                return Err(ExecutionPolicyError::InvalidStructuredEvidence);
            }
            _ => {}
        }
        Ok(())
    }

    /// Bind recovery authority to the reservation that admitted this node.
    pub fn validate_for_attempt(
        &self,
        attempt_id: &crate::ids::AttemptId,
        resource_flight_id: &crate::resource_flight::ResourceFlightIdV1,
    ) -> Result<(), ExecutionPolicyError> {
        self.validate_coherence()?;
        match &self.cleanup {
            NodeCleanupV2::Pending {
                resource_flight_id: persisted,
            } if persisted != resource_flight_id => {
                Err(ExecutionPolicyError::InvalidStructuredEvidence)
            }
            NodeCleanupV2::Partial { recovery_owner, .. }
            | NodeCleanupV2::Unknown {
                recovery_owner: Some(recovery_owner),
                ..
            } if recovery_owner.attempt_id != *attempt_id
                || recovery_owner.resource_flight_id != *resource_flight_id =>
            {
                Err(ExecutionPolicyError::InvalidStructuredEvidence)
            }
            _ => Ok(()),
        }
    }

    fn cleanup_cause_mut(&mut self) -> Option<&mut BoundedCauseV1> {
        match &mut self.cleanup {
            NodeCleanupV2::Failed { cause, .. } => Some(cause),
            NodeCleanupV2::Pending { .. }
            | NodeCleanupV2::Complete { .. }
            | NodeCleanupV2::Partial { .. }
            | NodeCleanupV2::NotNeeded
            | NodeCleanupV2::Unknown { .. } => None,
        }
    }

    fn normalize(mut self) -> Result<Self, ExecutionPolicyError> {
        if self.schema_version != NODE_CLEANUP_RECORD_SCHEMA_V2 {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        self.validate_coherence()?;
        match &self.cleanup {
            NodeCleanupV2::Pending { resource_flight_id } => {
                crate::resource_flight::ResourceFlightIdV1::parse(resource_flight_id.as_str())
                    .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
            }
            NodeCleanupV2::Partial { recovery_owner, .. } => {
                crate::resource_flight::ResourceFlightIdV1::parse(
                    recovery_owner.resource_flight_id.as_str(),
                )
                .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
            }
            NodeCleanupV2::Unknown {
                recovery_owner: Some(recovery_owner),
                ..
            } => {
                crate::resource_flight::ResourceFlightIdV1::parse(
                    recovery_owner.resource_flight_id.as_str(),
                )
                .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
            }
            NodeCleanupV2::Complete { .. }
            | NodeCleanupV2::Failed { .. }
            | NodeCleanupV2::NotNeeded
            | NodeCleanupV2::Unknown {
                recovery_owner: None,
                ..
            } => {}
        }
        if let Some(cause) = self.cleanup_cause_mut() {
            normalize_bounded_cause(cause);
        }
        Ok(self)
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ExecutionPolicyError> {
        let mut value = self.clone().normalize()?;
        let mut encoded = canonical_json(&value)?;
        if encoded.len() <= MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            return Ok(encoded);
        }
        if let Some(cause) = value.cleanup_cause_mut() {
            cause.cause_truncated = true;
        }
        while encoded.len() > MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            let Some(cause) = value.cleanup_cause_mut() else {
                break;
            };
            if !shorten_bounded_cause(cause) {
                break;
            }
            encoded = canonical_json(&value)?;
        }
        if encoded.len() <= MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            return Ok(encoded);
        }
        if let Some(cause) = value.cleanup_cause_mut() {
            cause.evidence_overflow = true;
            cause.dependency_set = None;
        }
        encoded = canonical_json(&value)?;
        while encoded.len() > MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            let Some(cause) = value.cleanup_cause_mut() else {
                break;
            };
            if !shorten_bounded_cause(cause) {
                break;
            }
            encoded = canonical_json(&value)?;
        }
        if encoded.len() > MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        Ok(encoded)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ExecutionPolicyError> {
        if bytes.len() > MAX_NODE_CLEANUP_RECORD_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
        if value.encode_canonical()? != bytes {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanOutPolicyNameV1 {
    BoundedIndependent,
    FailFast,
    FixedGrace,
}

impl From<&FanOutPolicyV1> for FanOutPolicyNameV1 {
    fn from(value: &FanOutPolicyV1) -> Self {
        match value {
            FanOutPolicyV1::BoundedIndependent => Self::BoundedIndependent,
            FanOutPolicyV1::FailFast => Self::FailFast,
            FanOutPolicyV1::FixedGrace { .. } => Self::FixedGrace,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyTriggerV1 {
    pub schema_version: u16,
    pub id: ControlEventIdV1,
    pub node: PolicyNodeRefV1,
    pub policy: FanOutPolicyNameV1,
    pub grace_ms: Option<u64>,
}

impl PolicyTriggerV1 {
    fn validate(&self) -> Result<(), ExecutionPolicyError> {
        let grace_valid = match (self.policy, self.grace_ms) {
            (FanOutPolicyNameV1::FixedGrace, Some(value)) => value > 0,
            (FanOutPolicyNameV1::FixedGrace, None) => false,
            (_, None) => true,
            (_, Some(_)) => false,
        };
        if self.schema_version != EXECUTION_POLICY_SCHEMA_V1 || !grace_valid {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, ExecutionPolicyError> {
        self.validate()?;
        let encoded = canonical_json(self)?;
        if encoded.len() > MAX_POLICY_TRIGGER_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        Ok(encoded)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ExecutionPolicyError> {
        if bytes.len() > MAX_POLICY_TRIGGER_JSON_BYTES {
            return Err(ExecutionPolicyError::StructuredEvidenceOverBound);
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionPolicyError::InvalidStructuredEvidence)?;
        if value.encode_canonical()? != bytes {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRuntimeOutcomeV1 {
    Completed,
    CompletedDegraded,
    Failed,
    Canceled,
}

impl WorkflowRuntimeOutcomeV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletedDegraded => "completed_degraded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    #[must_use]
    pub const fn durable(self) -> WorkflowDurableOutcomeV1 {
        match self {
            Self::Completed => WorkflowDurableOutcomeV1::Completed,
            Self::CompletedDegraded => WorkflowDurableOutcomeV1::CompletedDegraded,
            Self::Failed => WorkflowDurableOutcomeV1::Failed,
            Self::Canceled => WorkflowDurableOutcomeV1::Canceled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDurableOutcomeV1 {
    Completed,
    CompletedDegraded,
    Failed,
    Canceled,
    Interrupted,
}

impl WorkflowDurableOutcomeV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletedDegraded => "completed_degraded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Interrupted => "interrupted",
        }
    }

    #[must_use]
    pub fn parse_closed(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "completed_degraded" => Some(Self::CompletedDegraded),
            "failed" => Some(Self::Failed),
            "canceled" => Some(Self::Canceled),
            "interrupted" => Some(Self::Interrupted),
            _ => None,
        }
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
    #[error("provider-effect key is required for MCP environment values")]
    MissingProviderEffectKey,
    #[error("provider candidate is outside the frozen ordered set")]
    ProviderSelectionOutOfSet,
    #[error("provider-attempt identity matrix is incomplete or non-canonical")]
    ProviderAttemptMatrixInvalid,
    #[error("control event id is invalid")]
    InvalidControlEventId,
    #[error("canonical structured evidence exceeded its encoded bound")]
    StructuredEvidenceOverBound,
    #[error("canonical structured evidence is invalid")]
    InvalidStructuredEvidence,
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

impl DependencySetRefV1 {
    pub fn from_node_refs(mut refs: Vec<PolicyNodeRefV1>) -> Result<Self, ExecutionPolicyError> {
        refs.sort_by(|left, right| {
            left.sorted_ordinal
                .cmp(&right.sorted_ordinal)
                .then_with(|| left.id_sha256.as_str().cmp(right.id_sha256.as_str()))
        });
        if refs
            .windows(2)
            .any(|pair| pair[0].sorted_ordinal == pair[1].sorted_ordinal)
        {
            return Err(ExecutionPolicyError::InvalidStructuredEvidence);
        }
        let count =
            u32::try_from(refs.len()).map_err(|_| ExecutionPolicyError::ArithmeticOverflow)?;
        let mut canonical = Vec::with_capacity(refs.len().saturating_mul(68).saturating_add(4));
        canonical.extend_from_slice(&count.to_be_bytes());
        for node_ref in refs {
            canonical.extend_from_slice(&node_ref.sorted_ordinal.to_be_bytes());
            canonical.extend_from_slice(node_ref.id_sha256.as_str().as_bytes());
        }
        Ok(Self {
            count,
            sorted_node_refs_sha256: Sha256HexV1::digest(&canonical),
        })
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

/// Separately held provider-effect commitment key. It is intentionally neither serializable nor
/// printable; persisted identity carries only [`ProviderEffectKeyV1::key_id`].
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderEffectKeyV1([u8; 32]);

impl std::fmt::Debug for ProviderEffectKeyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderEffectKeyV1([REDACTED])")
    }
}

impl ProviderEffectKeyV1 {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn key_id(&self) -> Sha256HexV1 {
        self.mac(b"a2a-bridge/provider-effect-key-id/v1", &[])
    }

    fn mac(&self, domain: &[u8], value: &[u8]) -> Sha256HexV1 {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.0);
        let mut bytes = Vec::with_capacity(domain.len() + 16 + value.len());
        push_bytes(&mut bytes, domain);
        push_bytes(&mut bytes, value);
        let tag = hmac::sign(&key, &bytes);
        let mut encoded = String::with_capacity(64);
        for byte in tag.as_ref() {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "{byte:02x}");
        }
        Sha256HexV1(encoded)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum BoundMcpDeliveryPayloadV1 {
    Acp(Vec<McpServerSpec>),
    CodexNative(Vec<String>),
    KiroNative { agent_name: String, json: String },
}

impl std::fmt::Debug for BoundMcpDeliveryPayloadV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Acp(servers) => formatter
                .debug_struct("Acp")
                .field("server_count", &servers.len())
                .finish(),
            Self::CodexNative(args) => formatter
                .debug_struct("CodexNative")
                .field("arg_count", &args.len())
                .finish(),
            Self::KiroNative { json, .. } => formatter
                .debug_struct("KiroNative")
                .field("json_bytes", &json.len())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BoundMcpDeliveryV1 {
    payload: BoundMcpDeliveryPayloadV1,
    digest: Sha256HexV1,
}

impl std::fmt::Debug for BoundMcpDeliveryV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundMcpDeliveryV1")
            .field("payload", &self.payload)
            .field("digest", &self.digest)
            .finish()
    }
}

impl BoundMcpDeliveryV1 {
    #[must_use]
    pub fn payload(&self) -> &BoundMcpDeliveryPayloadV1 {
        &self.payload
    }

    #[must_use]
    pub fn digest(&self) -> &Sha256HexV1 {
        &self.digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenProviderSelectionV1 {
    pub agent: AgentId,
    pub preflight: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_model: Option<String>,
    pub ordered_fallback_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_effort: Option<crate::domain::Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_mode: Option<String>,
    pub selection_digest: Sha256HexV1,
}

impl FrozenProviderSelectionV1 {
    #[must_use]
    pub fn candidates(&self) -> Vec<Option<String>> {
        let mut candidates = vec![self.effective_model.clone()];
        if self.preflight {
            candidates.extend(self.ordered_fallback_models.iter().cloned().map(Some));
        }
        candidates
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenProviderEffectV1 {
    pub agent: AgentId,
    pub effective_session_cwd: SessionCwd,
    pub mcp_delivery_digest: Sha256HexV1,
    pub effect_digest: Sha256HexV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_commitment_key_id: Option<Sha256HexV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenProviderAttemptIdentityV1 {
    pub logical_session: FrozenProviderLogicalSessionV1,
    pub checkout: FrozenCheckoutEffectV1,
    pub effect: FrozenProviderEffectV1,
    pub attempt_fingerprint: Sha256HexV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenNodeExecutionIdentityV1 {
    pub node: PolicyNodeRefV1,
    pub selection: FrozenProviderSelectionV1,
    /// Canonically ordered by candidate ordinal, with `Preflight` before `Execute`.
    /// Current retries reuse the one `Execute` row for their selected candidate.
    pub provider_attempts: Vec<FrozenProviderAttemptIdentityV1>,
    pub identity_fingerprint: Sha256HexV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAllocationKindV1 {
    Configured,
    Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundedLedgerReasonV1 {
    Open,
    Permission,
    ReadOnlyDatabase,
    ReadOnlyLock,
    ReadOnlyParent,
    AdvisoryLockUnsupported,
    AdvisoryLockIo,
    Locked,
    Migration,
    UnsupportedConfiguration,
    Schema,
    Corruption,
    Io,
    CapacityProtected,
    Collision,
}

impl From<crate::workflow_history::LedgerUnavailableReason> for BoundedLedgerReasonV1 {
    fn from(value: crate::workflow_history::LedgerUnavailableReason) -> Self {
        use crate::workflow_history::LedgerUnavailableReason as Source;
        match value {
            Source::Open => Self::Open,
            Source::Permission => Self::Permission,
            Source::ReadOnlyDatabase => Self::ReadOnlyDatabase,
            Source::ReadOnlyLock => Self::ReadOnlyLock,
            Source::ReadOnlyParent => Self::ReadOnlyParent,
            Source::AdvisoryLockUnsupported => Self::AdvisoryLockUnsupported,
            Source::AdvisoryLockIo => Self::AdvisoryLockIo,
            Source::Locked => Self::Locked,
            Source::Migration => Self::Migration,
            Source::UnsupportedConfiguration => Self::UnsupportedConfiguration,
            Source::Schema => Self::Schema,
            Source::Corruption => Self::Corruption,
            Source::Io => Self::Io,
            Source::CapacityProtected => Self::CapacityProtected,
            Source::Collision => Self::Collision,
        }
    }
}

impl From<BoundedLedgerReasonV1> for crate::workflow_history::LedgerUnavailableReason {
    fn from(value: BoundedLedgerReasonV1) -> Self {
        use crate::workflow_history::LedgerUnavailableReason as Target;
        match value {
            BoundedLedgerReasonV1::Open => Target::Open,
            BoundedLedgerReasonV1::Permission => Target::Permission,
            BoundedLedgerReasonV1::ReadOnlyDatabase => Target::ReadOnlyDatabase,
            BoundedLedgerReasonV1::ReadOnlyLock => Target::ReadOnlyLock,
            BoundedLedgerReasonV1::ReadOnlyParent => Target::ReadOnlyParent,
            BoundedLedgerReasonV1::AdvisoryLockUnsupported => Target::AdvisoryLockUnsupported,
            BoundedLedgerReasonV1::AdvisoryLockIo => Target::AdvisoryLockIo,
            BoundedLedgerReasonV1::Locked => Target::Locked,
            BoundedLedgerReasonV1::Migration => Target::Migration,
            BoundedLedgerReasonV1::UnsupportedConfiguration => Target::UnsupportedConfiguration,
            BoundedLedgerReasonV1::Schema => Target::Schema,
            BoundedLedgerReasonV1::Corruption => Target::Corruption,
            BoundedLedgerReasonV1::Io => Target::Io,
            BoundedLedgerReasonV1::CapacityProtected => Target::CapacityProtected,
            BoundedLedgerReasonV1::Collision => Target::Collision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "admission", rename_all = "snake_case")]
pub enum LedgerAdmissionV1 {
    DurablePrimaryTaskStore,
    HistoryLedgerAdmitted { kind: HistoryAllocationKindV1 },
    HistoryLedgerUnavailable { reason: BoundedLedgerReasonV1 },
}

#[derive(Clone)]
pub struct BoundProviderEffectV1 {
    frozen: FrozenProviderAttemptIdentityV1,
    delivery: Arc<BoundMcpDeliveryV1>,
}

impl std::fmt::Debug for BoundProviderEffectV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundProviderEffectV1")
            .field("frozen", &self.frozen)
            .field("delivery", &self.delivery)
            .finish()
    }
}

impl BoundProviderEffectV1 {
    #[must_use]
    pub fn frozen(&self) -> &FrozenProviderAttemptIdentityV1 {
        &self.frozen
    }

    #[must_use]
    pub fn delivery(&self) -> &BoundMcpDeliveryV1 {
        &self.delivery
    }
}

#[derive(Clone)]
pub struct BoundSessionSpecV1 {
    pub session: crate::domain::SessionSpec,
    pub provider_effect: Arc<BoundProviderEffectV1>,
}

impl std::fmt::Debug for BoundSessionSpecV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundSessionSpecV1")
            .field("session", &self.session)
            .field("provider_effect", &self.provider_effect)
            .finish()
    }
}

impl BoundSessionSpecV1 {
    #[must_use]
    pub fn new(
        config: crate::domain::EffectiveConfig,
        provider_effect: Arc<BoundProviderEffectV1>,
    ) -> Self {
        let cwd = provider_effect
            .frozen()
            .effect
            .effective_session_cwd
            .clone();
        Self {
            session: crate::domain::SessionSpec {
                config,
                cwd: Some(cwd),
            },
            provider_effect,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrozenProviderAttemptBundleV1 {
    pub selection: FrozenProviderSelectionV1,
    pub frozen: FrozenProviderAttemptIdentityV1,
    pub bound: BoundProviderEffectV1,
}

pub struct ProviderFreezeInputV1<'a> {
    pub entry: &'a AgentEntry,
    pub overrides: Option<&'a AgentOverride>,
    pub node: PolicyNodeRefV1,
    pub logical_session: FrozenProviderLogicalSessionV1,
    pub checkout: FrozenCheckoutEffectV1,
    pub provider_effect_key: Option<&'a ProviderEffectKeyV1>,
}

fn push_option(target: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            target.push(1);
            push_bytes(target, value.as_bytes());
        }
        None => target.push(0),
    }
}

fn push_bool(target: &mut Vec<u8>, value: bool) {
    target.push(u8::from(value));
}

fn effort_token(effort: crate::domain::Effort) -> &'static str {
    match effort {
        crate::domain::Effort::Minimal => "minimal",
        crate::domain::Effort::Low => "low",
        crate::domain::Effort::Medium => "medium",
        crate::domain::Effort::High => "high",
        crate::domain::Effort::Xhigh => "xhigh",
        crate::domain::Effort::Max => "max",
    }
}

fn freeze_selection(
    entry: &AgentEntry,
    overrides: Option<&AgentOverride>,
) -> FrozenProviderSelectionV1 {
    let effective = effective_config(entry, overrides);
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, b"a2a-bridge/provider-selection/v1");
    push_bytes(&mut canonical, entry.id.as_str().as_bytes());
    push_bool(&mut canonical, entry.preflight);
    push_option(&mut canonical, effective.model.as_deref());
    canonical.extend_from_slice(&(entry.fallback_models.len() as u64).to_be_bytes());
    for fallback in &entry.fallback_models {
        push_bytes(&mut canonical, fallback.as_bytes());
    }
    push_option(&mut canonical, effective.effort.map(effort_token));
    push_option(&mut canonical, effective.mode.as_deref());
    FrozenProviderSelectionV1 {
        agent: entry.id.clone(),
        preflight: entry.preflight,
        effective_model: effective.model,
        ordered_fallback_models: entry.fallback_models.clone(),
        effective_effort: effective.effort,
        effective_mode: effective.mode,
        selection_digest: Sha256HexV1::digest(&canonical),
    }
}

fn encode_servers(servers: &[McpServerSpec]) -> Vec<u8> {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(&(servers.len() as u64).to_be_bytes());
    for server in servers {
        push_bytes(&mut canonical, server.name.as_bytes());
        push_bytes(&mut canonical, server.command.as_bytes());
        canonical.extend_from_slice(&(server.args.len() as u64).to_be_bytes());
        for arg in &server.args {
            push_bytes(&mut canonical, arg.as_bytes());
        }
        canonical.extend_from_slice(&(server.env.len() as u64).to_be_bytes());
        for (name, value) in &server.env {
            push_bytes(&mut canonical, name.as_bytes());
            push_bytes(&mut canonical, value.source_kind().as_bytes());
            push_option(&mut canonical, value.source_name());
            push_bytes(&mut canonical, value.resolved_value().as_bytes());
        }
    }
    canonical
}

fn secret_silent_digest(
    key: Option<&ProviderEffectKeyV1>,
    secret_bearing: bool,
    domain: &'static [u8],
    value: &[u8],
) -> Result<Sha256HexV1, ExecutionPolicyError> {
    if secret_bearing {
        return key
            .map(|key| key.mac(domain, value))
            .ok_or(ExecutionPolicyError::MissingProviderEffectKey);
    }
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, domain);
    push_bytes(&mut canonical, value);
    Ok(Sha256HexV1::digest(&canonical))
}

fn freeze_delivery(
    entry: &AgentEntry,
    cwd: &SessionCwd,
    key: Option<&ProviderEffectKeyV1>,
) -> Result<(BoundMcpDeliveryV1, bool, Option<Sha256HexV1>, Sha256HexV1), ExecutionPolicyError> {
    let secret_bearing = entry.mcp.iter().any(|server| !server.env.is_empty());
    if secret_bearing && key.is_none() {
        return Err(ExecutionPolicyError::MissingProviderEffectKey);
    }
    let source_digest = secret_silent_digest(
        key,
        secret_bearing,
        b"a2a-bridge/mcp-source/v1",
        &encode_servers(&entry.mcp),
    )?;
    let payload = match entry.mcp_delivery {
        McpDelivery::Acp => BoundMcpDeliveryPayloadV1::Acp(
            entry
                .mcp
                .iter()
                .map(|server| server.substituted_for_managed_agent(cwd.as_str()))
                .collect(),
        ),
        McpDelivery::CodexNative => {
            BoundMcpDeliveryPayloadV1::CodexNative(render_codex_mcp_args(&entry.mcp, cwd.as_str()))
        }
        McpDelivery::KiroNative => {
            let delivered: Vec<McpServerSpec> = entry
                .mcp
                .iter()
                .map(|server| server.substituted_for_managed_agent(cwd.as_str()))
                .collect();
            let content_key = secret_silent_digest(
                key,
                secret_bearing,
                b"a2a-bridge/kiro-mcp-content/v1",
                &encode_servers(&delivered),
            )?;
            let agent_name = format!("a2a-mcp-v2-{}", content_key.as_str());
            let json = render_kiro_agent_config(&entry.mcp, cwd.as_str(), &agent_name);
            BoundMcpDeliveryPayloadV1::KiroNative { agent_name, json }
        }
    };

    let mut delivered_bytes = Vec::new();
    match &payload {
        BoundMcpDeliveryPayloadV1::Acp(servers) => {
            push_bytes(&mut delivered_bytes, b"acp");
            push_bytes(&mut delivered_bytes, &encode_servers(servers));
        }
        BoundMcpDeliveryPayloadV1::CodexNative(args) => {
            push_bytes(&mut delivered_bytes, b"codex_native");
            delivered_bytes.extend_from_slice(&(args.len() as u64).to_be_bytes());
            for arg in args {
                push_bytes(&mut delivered_bytes, arg.as_bytes());
            }
        }
        BoundMcpDeliveryPayloadV1::KiroNative { agent_name, json } => {
            push_bytes(&mut delivered_bytes, b"kiro_native");
            push_bytes(&mut delivered_bytes, agent_name.as_bytes());
            push_bytes(&mut delivered_bytes, json.as_bytes());
        }
    }
    let delivery_digest = secret_silent_digest(
        key,
        secret_bearing,
        b"a2a-bridge/mcp-delivery/v1",
        &delivered_bytes,
    )?;
    Ok((
        BoundMcpDeliveryV1 {
            payload,
            digest: delivery_digest.clone(),
        },
        secret_bearing,
        secret_bearing.then(|| key.expect("checked above").key_id()),
        source_digest,
    ))
}

fn agent_kind_token(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Acp => "acp",
        AgentKind::Api => "api",
        AgentKind::ContainerRw => "container_rw",
    }
}

fn encode_sandbox(target: &mut Vec<u8>, sandbox: Option<&SandboxConfig>) {
    let Some(sandbox) = sandbox else {
        target.push(0);
        return;
    };
    target.push(1);
    push_option(target, sandbox.runtime.as_deref());
    push_bytes(target, sandbox.image.as_bytes());
    push_bytes(target, sandbox.mount.as_bytes());
    push_bytes(
        target,
        match sandbox.access {
            MountAccess::Ro => b"ro",
            MountAccess::Rw => b"rw",
        },
    );
    match &sandbox.egress {
        EgressPolicy::Locked {
            network,
            proxy,
            no_proxy,
        } => {
            push_bytes(target, b"locked");
            push_bytes(target, network.as_bytes());
            push_bytes(target, proxy.as_bytes());
            push_option(target, no_proxy.as_deref());
        }
        EgressPolicy::Open => push_bytes(target, b"open"),
    }
    target.extend_from_slice(&(sandbox.volumes.len() as u64).to_be_bytes());
    for volume in &sandbox.volumes {
        push_bytes(target, volume.as_bytes());
    }
}

fn encode_checkout(target: &mut Vec<u8>, checkout: &FrozenCheckoutEffectV1) {
    match checkout {
        FrozenCheckoutEffectV1::Direct {
            source_cwd,
            effective_cwd,
        } => {
            push_bytes(target, b"direct");
            push_bytes(target, source_cwd.as_str().as_bytes());
            push_bytes(target, effective_cwd.as_str().as_bytes());
        }
        FrozenCheckoutEffectV1::Worktree {
            source_cwd,
            canonical_source_cwd,
            canonical_worktree_root,
            worktree_owner,
            target_cwd,
            checkout_digest,
        } => {
            push_bytes(target, b"worktree");
            push_bytes(target, source_cwd.as_str().as_bytes());
            push_bytes(target, canonical_source_cwd.as_str().as_bytes());
            push_bytes(target, canonical_worktree_root.as_str().as_bytes());
            push_bytes(target, worktree_owner.as_bytes());
            push_bytes(target, target_cwd.as_str().as_bytes());
            push_bytes(target, checkout_digest.as_str().as_bytes());
        }
    }
}

fn freeze_effect(
    entry: &AgentEntry,
    checkout: &FrozenCheckoutEffectV1,
    delivery_digest: Sha256HexV1,
    source_digest: &Sha256HexV1,
    secret_bearing: bool,
    key: Option<&ProviderEffectKeyV1>,
) -> Result<FrozenProviderEffectV1, ExecutionPolicyError> {
    let AgentEntry {
        id,
        cmd,
        base_url,
        api_key_env,
        args,
        kind,
        model_provider: _,
        model: _,
        effort: _,
        mode: _,
        preflight: _,
        fallback_models: _,
        cwd,
        session_cwd,
        sandbox,
        watchdog,
        mcp: _,
        mcp_delivery,
        auth_method,
        pre_authenticated,
        host_fallback_eligible: _,
        name: _,
        description: _,
        tags: _,
        version: _,
        extensions: _,
    } = entry;
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, b"a2a-bridge/provider-effect/v1");
    push_bytes(&mut canonical, id.as_str().as_bytes());
    push_bytes(&mut canonical, agent_kind_token(*kind).as_bytes());
    push_option(&mut canonical, cmd.as_deref());
    push_option(&mut canonical, base_url.as_deref());
    push_option(&mut canonical, api_key_env.as_deref());
    canonical.extend_from_slice(&(args.len() as u64).to_be_bytes());
    for arg in args {
        push_bytes(&mut canonical, arg.as_bytes());
    }
    push_option(&mut canonical, cwd.as_deref());
    push_option(&mut canonical, session_cwd.as_deref());
    encode_sandbox(&mut canonical, sandbox.as_ref());
    match watchdog {
        Some(watchdog) => {
            canonical.push(1);
            canonical.extend_from_slice(&watchdog.idle_timeout.as_millis().to_be_bytes());
            canonical.extend_from_slice(&watchdog.hard_wall_clock.as_millis().to_be_bytes());
        }
        None => canonical.push(0),
    }
    push_option(&mut canonical, auth_method.as_deref());
    push_bool(&mut canonical, *pre_authenticated);
    encode_checkout(&mut canonical, checkout);
    push_bytes(&mut canonical, checkout.effective_cwd().as_str().as_bytes());
    push_bytes(
        &mut canonical,
        match mcp_delivery {
            McpDelivery::Acp => b"acp",
            McpDelivery::CodexNative => b"codex_native",
            McpDelivery::KiroNative => b"kiro_native",
        },
    );
    push_bytes(&mut canonical, source_digest.as_str().as_bytes());
    push_bytes(&mut canonical, delivery_digest.as_str().as_bytes());
    let effect_digest = secret_silent_digest(
        key,
        secret_bearing,
        b"a2a-bridge/provider-effect-final/v1",
        &canonical,
    )?;
    Ok(FrozenProviderEffectV1 {
        agent: id.clone(),
        effective_session_cwd: checkout.effective_cwd().clone(),
        mcp_delivery_digest: delivery_digest,
        effect_digest,
        secret_commitment_key_id: secret_bearing.then(|| key.expect("checked above").key_id()),
    })
}

fn attempt_fingerprint(
    node: &PolicyNodeRefV1,
    logical_session: FrozenProviderLogicalSessionV1,
    checkout: &FrozenCheckoutEffectV1,
    effect: &FrozenProviderEffectV1,
    selection: &FrozenProviderSelectionV1,
) -> Sha256HexV1 {
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, b"a2a-bridge/provider-attempt/v1");
    canonical.extend_from_slice(&node.sorted_ordinal.to_be_bytes());
    push_bytes(&mut canonical, node.id_sha256.as_str().as_bytes());
    let (kind, ordinal) = logical_session.tag();
    push_bytes(&mut canonical, kind.as_bytes());
    canonical.extend_from_slice(&ordinal.to_be_bytes());
    encode_checkout(&mut canonical, checkout);
    push_bytes(&mut canonical, effect.effect_digest.as_str().as_bytes());
    push_bytes(
        &mut canonical,
        selection.selection_digest.as_str().as_bytes(),
    );
    Sha256HexV1::digest(&canonical)
}

pub fn freeze_provider_attempt_v1(
    input: &ProviderFreezeInputV1<'_>,
) -> Result<FrozenProviderAttemptBundleV1, ExecutionPolicyError> {
    let selection = freeze_selection(input.entry, input.overrides);
    let candidate_ordinal = match input.logical_session {
        FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal } => {
            if !selection.preflight {
                return Err(ExecutionPolicyError::ProviderSelectionOutOfSet);
            }
            candidate_ordinal
        }
        FrozenProviderLogicalSessionV1::Execute { candidate_ordinal } => candidate_ordinal,
    };
    if usize::from(candidate_ordinal) >= selection.candidates().len() {
        return Err(ExecutionPolicyError::ProviderSelectionOutOfSet);
    }

    let (delivery, secret_bearing, key_id, source_digest) = freeze_delivery(
        input.entry,
        input.checkout.effective_cwd(),
        input.provider_effect_key,
    )?;
    let effect = freeze_effect(
        input.entry,
        &input.checkout,
        delivery.digest().clone(),
        &source_digest,
        secret_bearing,
        input.provider_effect_key,
    )?;
    debug_assert_eq!(effect.secret_commitment_key_id, key_id);
    let frozen = FrozenProviderAttemptIdentityV1 {
        logical_session: input.logical_session,
        checkout: input.checkout.clone(),
        attempt_fingerprint: attempt_fingerprint(
            &input.node,
            input.logical_session,
            &input.checkout,
            &effect,
            &selection,
        ),
        effect,
    };
    let bound = BoundProviderEffectV1 {
        frozen: frozen.clone(),
        delivery: Arc::new(delivery),
    };
    Ok(FrozenProviderAttemptBundleV1 {
        selection,
        frozen,
        bound,
    })
}

fn expected_logical_sessions(
    selection: &FrozenProviderSelectionV1,
) -> Result<Vec<FrozenProviderLogicalSessionV1>, ExecutionPolicyError> {
    if !selection.preflight {
        return Ok(vec![FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }]);
    }
    let candidates = selection.candidates();
    let mut sessions = Vec::with_capacity(
        candidates
            .len()
            .checked_mul(2)
            .ok_or(ExecutionPolicyError::ArithmeticOverflow)?,
    );
    for ordinal in 0..candidates.len() {
        let candidate_ordinal = u16::try_from(ordinal)
            .map_err(|_| ExecutionPolicyError::ProviderAttemptMatrixInvalid)?;
        sessions.push(FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal });
        sessions.push(FrozenProviderLogicalSessionV1::Execute { candidate_ordinal });
    }
    Ok(sessions)
}

fn node_identity_fingerprint(
    node: &PolicyNodeRefV1,
    selection: &FrozenProviderSelectionV1,
    attempts: &[FrozenProviderAttemptIdentityV1],
) -> Sha256HexV1 {
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, b"a2a-bridge/node-execution-identity/v1");
    canonical.extend_from_slice(&node.sorted_ordinal.to_be_bytes());
    push_bytes(&mut canonical, node.id_sha256.as_str().as_bytes());
    push_bytes(
        &mut canonical,
        selection.selection_digest.as_str().as_bytes(),
    );
    canonical.extend_from_slice(&(attempts.len() as u64).to_be_bytes());
    for attempt in attempts {
        push_bytes(
            &mut canonical,
            attempt.attempt_fingerprint.as_str().as_bytes(),
        );
    }
    Sha256HexV1::digest(&canonical)
}

fn validate_provider_attempt_matrix(
    selection: &FrozenProviderSelectionV1,
    attempts: &[FrozenProviderAttemptIdentityV1],
) -> Result<(), ExecutionPolicyError> {
    let expected = expected_logical_sessions(selection)?;
    if attempts.len() != expected.len() {
        return Err(ExecutionPolicyError::ProviderAttemptMatrixInvalid);
    }
    for (attempt, logical_session) in attempts.iter().zip(expected) {
        if attempt.logical_session != logical_session
            || attempt.effect.agent != selection.agent
            || attempt.effect.effective_session_cwd != *attempt.checkout.effective_cwd()
        {
            return Err(ExecutionPolicyError::ProviderAttemptMatrixInvalid);
        }
    }
    Ok(())
}

pub fn freeze_node_execution_identity_v1(
    node: PolicyNodeRefV1,
    bundles: Vec<FrozenProviderAttemptBundleV1>,
) -> Result<FrozenNodeExecutionIdentityV1, ExecutionPolicyError> {
    let selection = bundles
        .first()
        .map(|bundle| bundle.selection.clone())
        .ok_or(ExecutionPolicyError::ProviderAttemptMatrixInvalid)?;
    if bundles.iter().any(|bundle| {
        bundle.selection != selection
            || bundle.frozen != *bundle.bound.frozen()
            || bundle.frozen.effect.mcp_delivery_digest != *bundle.bound.delivery().digest()
    }) {
        return Err(ExecutionPolicyError::ProviderAttemptMatrixInvalid);
    }
    let attempts: Vec<_> = bundles.into_iter().map(|bundle| bundle.frozen).collect();
    validate_provider_attempt_matrix(&selection, &attempts)?;
    let identity_fingerprint = node_identity_fingerprint(&node, &selection, &attempts);
    Ok(FrozenNodeExecutionIdentityV1 {
        node,
        selection,
        provider_attempts: attempts,
        identity_fingerprint,
    })
}

pub fn validate_node_execution_identity_v1(
    identity: &FrozenNodeExecutionIdentityV1,
) -> Result<(), ExecutionPolicyError> {
    validate_provider_attempt_matrix(&identity.selection, &identity.provider_attempts)?;
    if identity.identity_fingerprint
        != node_identity_fingerprint(
            &identity.node,
            &identity.selection,
            &identity.provider_attempts,
        )
    {
        return Err(ExecutionPolicyError::ProviderAttemptMatrixInvalid);
    }
    Ok(())
}
